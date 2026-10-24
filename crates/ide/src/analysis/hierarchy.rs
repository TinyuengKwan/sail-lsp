//! Hierarchy continuations retain a declaration and an include context.
//! Call-site ranges always belong to the item whose outgoing calls were asked
//! for; scattered clauses in other source files are separate hierarchy items.
use super::*;
use hir::callgraph::WorkspaceFile as _;
use hir::source_order::bindings::Namespace;
use hir::source_order::{self, Declaration};
use ide_db::{
    defs::Definition,
    ide_types::{CallItem, TypeHierarchyItem},
};
use salsa::Database as _;
use syntax::SyntaxKind as SK;

fn is_source_callable(node: &syntax::SyntaxNode) -> bool {
    let has = |kind| node.children_with_tokens().any(|t| t.kind() == kind);
    match node.kind() {
        SK::CALLABLE_SPEC | SK::CALLABLE_DEF | SK::OUTCOME_DEF => true,
        SK::SCATTERED_DEF | SK::SCATTERED_CLAUSE_DEF => has(SK::KW_FUNCTION) || has(SK::KW_MAPPING),
        SK::NAMED_DEF => has(SK::KW_OVERLOAD),
        _ => false,
    }
}

impl Analysis {
    fn call_origin(&self, file: FileId, offset: usize, name: &str) -> Option<Declaration> {
        let sf = self.file_by_id(file)?;
        // A local function parameter shadows a top-level callable too.
        let classified = Definition::classify_at(&sf, sf.position_at(offset));
        if classified.as_ref().is_some_and(Definition::is_local) {
            return None;
        }
        let (_, ft) = sf.file_text()?;
        if source_order::declarations_for(&self.db, ft).is_some() {
            let at = source_order::at_offset(&self.db, ft, offset)?;
            let selected = source_order::bindings::resolve(
                &self.db,
                at,
                hir::Name::new(name),
                Namespace::Callable,
            );
            if selected.is_some()
                || !matches!(
                    source_order::program(&self.db, ft),
                    Some(source_order::Program::Inline(_))
                )
                || source_order::bindings::contains_name(&self.db, at, &hir::Name::new(name))
            {
                return selected;
            }
        }
        if let Some(loc) = classified.and_then(|d| d.def_location()) {
            let (tree, item) = loc.item(&self.db)?;
            return source_order::at_offset(&self.db, loc.file_text, item.name_span(tree).start);
        }
        // Unconfigured buffers have no compiler expansion order. Preserve
        // their existing cross-file lookup, but retain the selected identity.
        for (_, candidate) in self.visible_salsa_files(file) {
            let (_, ft) = candidate.file_text()?;
            if !hir::visibility::symbol_visible_from(&self.db, file, ft, name) {
                continue;
            }
            let Some(tree) = candidate.item_tree() else { continue };
            for item in tree.top_level_items() {
                if item.name(tree).as_str() == name {
                    let declaration =
                        source_order::at_offset(&self.db, ft, item.name_span(tree).start)?;
                    if self.call_item_named(declaration, name).is_some() {
                        return Some(declaration);
                    }
                }
            }
        }
        None
    }

    fn call_item(&self, declaration: Declaration) -> Option<CallItem> {
        let node = source_order::node(&self.db, declaration)?;
        if !is_source_callable(&node) {
            return None;
        }
        self.call_item_named(declaration, &syntax::definition_name(&node)?)
    }

    fn call_item_named(&self, declaration: Declaration, name: &str) -> Option<CallItem> {
        let node = source_order::node(&self.db, declaration)?;
        let generated_signature =
            hir::def_query::file_item_tree(&self.db, declaration.file).as_ref().and_then(|tree| {
                tree.top_level_items()
                    .iter()
                    .find(|item| {
                        tree.generated.contains_key(item)
                            && matches!(
                                item.def_kind(tree),
                                hir::DefKind::ValSpec | hir::DefKind::Overload
                            )
                            && item.name(tree).as_str() == name
                            && node
                                .text_range()
                                .contains((item.name_span(tree).start as u32).into())
                    })
                    .map(|item| item.signature(tree).to_owned())
            });
        if generated_signature.is_none() {
            if syntax::definition_name(&node).as_deref() != Some(name) {
                return None;
            }
            if !is_source_callable(&node) {
                return None;
            }
        }
        let span = syntax::definition_name_span(&node)?;
        let view = declaration.file.file_id(&self.db);
        let source = base_db::preprocess::physical_file(&self.db, view);
        let mut data = serde_json::json!({"kind": "call", "name": name,
            "source": source.index(), "context": view.index(), "offset": span.start});
        if generated_signature.is_some() {
            data["generated"] = true.into();
        }
        Some(CallItem {
            name: name.to_owned(),
            kind: ide_db::ide_types::SymbolKind::Function,
            detail: generated_signature,
            url: self.url_for_file_id(source)?.clone(),
            range: node.text_range(),
            selection_range: base_db::text_range(span.start, span.end),
            data: Some(data),
        })
    }

    pub(super) fn prepare_call_item(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Option<CallItem> {
        let sf = self.file(url)?;
        let (token, span) = sf.token_at(position)?;
        let name = ide_db::token_symbol_key(token)?;
        let view = self.file_id(url)?;
        let (_, ft) = sf.file_text()?;
        let at = source_order::at_offset(&self.db, ft, span.start)?;
        if let Some(item) = self.call_item(at) {
            if item.selection_range == base_db::text_range(span.start, span.end) {
                return Some(item);
            }
        }
        self.call_item_named(self.call_origin(view, span.start, &name)?, &name)
    }

    fn call_item_declaration(&self, data: &serde_json::Value) -> Option<Declaration> {
        self.hierarchy_declaration(data, "call")
    }

    fn hierarchy_declaration(&self, data: &serde_json::Value, kind: &str) -> Option<Declaration> {
        if data["kind"] != kind {
            return None;
        }
        let source_id = u32::try_from(data["source"].as_u64()?).ok()?;
        let source = self
            .url_map
            .all_url_file_ids()
            .find_map(|(_, id)| (id.index() == source_id).then_some(id))?;
        let view_id = u32::try_from(data["context"].as_u64()?).ok()?;
        let view = base_db::preprocess::file_instances(&self.db, source)
            .into_iter()
            .find(|id| id.index() == view_id)?;
        let offset = usize::try_from(data["offset"].as_u64()?).ok()?;
        let declaration = source_order::at_offset(&self.db, self.file_text(view)?, offset)?;
        if kind == "call" {
            let item = self.call_item_named(declaration, data["name"].as_str()?)?;
            return (base_db::range_start(item.selection_range) == offset).then_some(declaration);
        }
        let node = source_order::node(&self.db, declaration)?;
        let span = syntax::definition_name_span(&node)?;
        (span.start == offset && data["name"].as_str() == syntax::definition_name(&node).as_deref())
            .then_some(declaration)
    }

    pub(super) fn incoming_call_items(
        &self,
        data: &serde_json::Value,
    ) -> Vec<(CallItem, Vec<base_db::TextRange>)> {
        let Some(target) = self.call_item_declaration(data) else { return Vec::new() };
        let Some(name) = data["name"].as_str() else { return Vec::new() };
        let Some(item) = self.call_item_named(target, name) else { return Vec::new() };
        let generated = item.data.as_ref().is_some_and(|data| data["generated"] == true);
        let Some(origin) = (if generated {
            Some(target)
        } else {
            self.call_origin(
                target.file.file_id(&self.db),
                base_db::range_start(item.selection_range),
                &item.name,
            )
        }) else {
            return Vec::new();
        };
        let mut grouped: Vec<(Declaration, CallItem, Vec<base_db::TextRange>)> = Vec::new();
        for (_, sf) in self.all_salsa_files() {
            self.db.unwind_if_revision_cancelled();
            let Some((_, ft)) = sf.file_text() else { continue };
            let Some(graph) = sf.callgraph() else { continue };
            for site in graph.call_sites_to(&item.name) {
                if self.call_origin(ft.file_id(&self.db), site.callee_span.start, &site.callee)
                    != Some(origin)
                {
                    continue;
                }
                let Some(caller) = source_order::at_offset(&self.db, ft, site.callee_span.start)
                else {
                    continue;
                };
                let Some(caller_item) = self.call_item(caller) else { continue };
                let range = base_db::text_range(site.callee_span.start, site.callee_span.end);
                if let Some((_, _, ranges)) = grouped.iter_mut().find(|(d, _, _)| *d == caller) {
                    if !ranges.contains(&range) {
                        ranges.push(range);
                    }
                } else {
                    grouped.push((caller, caller_item, vec![range]));
                }
            }
        }
        grouped.into_iter().map(|(_, item, ranges)| (item, ranges)).collect()
    }

    pub(super) fn outgoing_call_items(
        &self,
        data: &serde_json::Value,
    ) -> Vec<(CallItem, Vec<base_db::TextRange>)> {
        let Some(source) = self.call_item_declaration(data) else { return Vec::new() };
        let Some(name) = data["name"].as_str() else { return Vec::new() };
        let Some(item) = self.call_item_named(source, name) else { return Vec::new() };
        // Generated declarations have no source callable body. Do not borrow
        // calls from their owning type/mapping as if they belonged to a helper.
        if item.data.as_ref().is_some_and(|data| data["generated"] == true) {
            return Vec::new();
        }
        let view = source.file.file_id(&self.db);
        let Some(origin) =
            self.call_origin(view, base_db::range_start(item.selection_range), &item.name)
        else {
            return Vec::new();
        };
        let Some(sf) = self.file_by_id(view) else { return Vec::new() };
        let Some(graph) = sf.callgraph() else { return Vec::new() };
        let mut grouped: Vec<(Declaration, CallItem, Vec<base_db::TextRange>)> = Vec::new();
        let is_body = source_order::node(&self.db, source)
            .is_some_and(|n| matches!(n.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF));
        for site in graph.call_sites_in(&item.name) {
            let Some(caller) =
                source_order::at_offset(&self.db, source.file, site.callee_span.start)
            else {
                continue;
            };
            if is_body && caller != source {
                continue;
            }
            let Some(caller_node) = source_order::node(&self.db, caller) else { continue };
            let Some(caller_span) = syntax::definition_name_span(&caller_node) else { continue };
            if self.call_origin(view, caller_span.start, &site.caller) != Some(origin) {
                continue;
            }
            let Some(callee) = self.call_origin(view, site.callee_span.start, &site.callee) else {
                continue;
            };
            let Some(callee_item) = self.call_item_named(callee, &site.callee) else { continue };
            let range = base_db::text_range(site.callee_span.start, site.callee_span.end);
            if let Some((_, _, ranges)) = grouped
                .iter_mut()
                .find(|(d, item, _)| *d == callee && item.name == callee_item.name)
            {
                if !ranges.contains(&range) {
                    ranges.push(range);
                }
            } else {
                grouped.push((callee, callee_item, vec![range]));
            }
        }
        grouped.into_iter().map(|(_, item, ranges)| (item, ranges)).collect()
    }

    fn type_item(&self, declaration: Declaration) -> Option<TypeHierarchyItem> {
        let node = source_order::node(&self.db, declaration)?;
        let has = |kind| node.children_with_tokens().any(|t| t.kind() == kind);
        let kind = match node.kind() {
            SK::TYPE_ALIAS_DEF => ide_db::ide_types::SymbolKind::Struct,
            SK::NAMED_DEF | SK::SCATTERED_DEF if has(SK::KW_ENUM) => {
                ide_db::ide_types::SymbolKind::Enum
            }
            SK::NAMED_DEF | SK::SCATTERED_DEF
                if [SK::KW_STRUCT, SK::KW_UNION, SK::KW_BITFIELD, SK::KW_NEWTYPE]
                    .into_iter()
                    .any(has) =>
            {
                ide_db::ide_types::SymbolKind::Struct
            }
            _ => return None,
        };
        let name = syntax::definition_name(&node)?;
        let span = syntax::definition_name_span(&node)?;
        let view = declaration.file.file_id(&self.db);
        let source = base_db::preprocess::physical_file(&self.db, view);
        Some(TypeHierarchyItem {
            name: name.clone(),
            kind,
            detail: Some("type".into()),
            url: self.url_for_file_id(source)?.clone(),
            range: node.text_range(),
            selection_range: base_db::text_range(span.start, span.end),
            data: Some(serde_json::json!({"kind": "type", "name": name,
                "source": source.index(), "context": view.index(), "offset": span.start})),
        })
    }

    fn type_origin(&self, at: Declaration, name: &str) -> Option<Declaration> {
        if source_order::declarations_for(&self.db, at.file).is_some() {
            let selected = source_order::bindings::resolve(
                &self.db,
                at,
                hir::Name::new(name),
                Namespace::Type,
            );
            if selected.is_some()
                || !matches!(
                    source_order::program(&self.db, at.file),
                    Some(source_order::Program::Inline(_))
                )
                || source_order::bindings::contains_name(&self.db, at, &hir::Name::new(name))
            {
                return selected;
            }
        }
        let from = at.file.file_id(&self.db);
        for (_, sf) in self.visible_salsa_files(from) {
            let (_, ft) = sf.file_text()?;
            if !hir::visibility::symbol_visible_from(&self.db, from, ft, name) {
                continue;
            }
            let Some(tree) = sf.item_tree() else { continue };
            for item in tree.top_level_items() {
                if item.name(tree).as_str() != name {
                    continue;
                }
                let declaration =
                    source_order::at_offset(&self.db, ft, item.name_span(tree).start)?;
                if self.type_item(declaration).is_some() {
                    return Some(declaration);
                }
            }
        }
        None
    }

    pub(super) fn prepare_type_item(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Option<TypeHierarchyItem> {
        let sf = self.file(url)?;
        let (token, span) = sf.token_at(position)?;
        let name = ide_db::token_symbol_key(token)?;
        let (_, ft) = sf.file_text()?;
        let at = source_order::at_offset(&self.db, ft, span.start)?;
        if let Some(item) = self.type_item(at) {
            if item.selection_range == base_db::text_range(span.start, span.end) {
                return Some(item);
            }
        }
        if Definition::classify_at(&sf, position).is_some_and(|d| d.is_local()) {
            return None;
        }
        self.type_item(self.type_origin(at, &name)?)
    }

    fn alias_parent(&self, declaration: Declaration) -> Option<Declaration> {
        let node = source_order::node(&self.db, declaration)?;
        let newtype = node.kind() == SK::NAMED_DEF
            && node.children_with_tokens().any(|t| t.kind() == SK::KW_NEWTYPE);
        if node.kind() != SK::TYPE_ALIAS_DEF && !newtype {
            return None;
        }
        let mut tokens = node
            .descendants_with_tokens()
            .filter_map(|t| t.into_token())
            .filter(|t| !t.kind().is_trivia());
        if !tokens.any(|t| t.kind() == SK::EQ) {
            return None;
        }
        if newtype && !tokens.any(|t| t.kind() == SK::COLON) {
            return None;
        }
        let token = tokens.next()?;
        if token.kind() != SK::IDENT {
            return None;
        }
        self.type_origin(declaration, token.text())
    }

    pub(super) fn supertype_items(&self, data: &serde_json::Value) -> Vec<TypeHierarchyItem> {
        self.hierarchy_declaration(data, "type")
            .and_then(|d| self.alias_parent(d))
            .and_then(|d| self.type_item(d))
            .into_iter()
            .collect()
    }

    pub(super) fn subtype_items(&self, data: &serde_json::Value) -> Vec<TypeHierarchyItem> {
        let Some(target) = self.hierarchy_declaration(data, "type") else { return Vec::new() };
        if self.type_item(target).is_none() {
            return Vec::new();
        }
        let mut result = Vec::new();
        for (_, sf) in self.all_salsa_files() {
            self.db.unwind_if_revision_cancelled();
            let Some((_, ft)) = sf.file_text() else { continue };
            let Some(root) = syntax::parse_query::semantic_parse_file(&self.db, ft).syntax_node()
            else {
                continue;
            };
            for (index, _) in root.children().enumerate() {
                let declaration = Declaration { file: ft, index };
                if self.alias_parent(declaration) == Some(target) {
                    if let Some(item) = self.type_item(declaration) {
                        result.push(item);
                    }
                }
            }
        }
        result
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use base_db::{
        module_graph::{ModuleData, ModuleGraph},
        preprocess::SourceProject,
    };
    use ide_db::apply_change::Change;

    pub(crate) fn setup(
        sources: &[(&str, &str)],
        roots: &[usize],
        modules: Vec<ModuleData>,
    ) -> (AnalysisHost, UrlMap, Vec<url::Url>) {
        let urls: Vec<_> = sources
            .iter()
            .map(|(name, _)| url::Url::parse(&format!("file:///project/{name}")).unwrap())
            .collect();
        let ids: Vec<_> = (0..sources.len()).map(|i| FileId::from_raw(i as u32)).collect();
        let map = UrlMap::new(
            urls.iter().cloned().zip(ids.iter().copied()).collect(),
            ids.iter().copied().zip(urls.iter().cloned()).collect(),
        );
        let mut host = AnalysisHost::new(None);
        let mut change = Change::new();
        for (&id, (_, text)) in ids.iter().zip(sources) {
            change.change_file(id, *text);
        }
        change.set_source_project(SourceProject {
            roots: roots.iter().map(|&i| ids[i]).collect(),
            paths: sources
                .iter()
                .zip(&ids)
                .map(|((name, _), &id)| (format!("/project/{name}").into(), id))
                .collect(),
            modules: ModuleGraph { modules, globals: vec![] },
            ..Default::default()
        });
        host.apply_change(change);
        (host, map, urls)
    }
    pub(crate) fn snapshot(host: &AnalysisHost, map: &UrlMap) -> Analysis {
        host.analysis_with_url_map(host.raw_database().files(), map.clone())
    }
    fn prepare(a: &Analysis, url: &url::Url, needle: &str) -> CallItem {
        let f = a.file(url).unwrap();
        let offset = f.text().find(needle).unwrap();
        a.prepare_call_hierarchy(url, f.position_at(offset)).unwrap().unwrap()
    }

    #[test]
    fn repeated_signatures_select_distinct_call_origins() {
        let (host, map, urls) = setup(&[("main.sail",
            "val f : unit -> int\nval a : unit -> int\nfunction a() = f()\nval f : unit -> bool\nval b : unit -> bool\nfunction b() = f()\n")], &[0], vec![]);
        let a = snapshot(&host, &map);
        let first = prepare(&a, &urls[0], "f : unit -> int");
        let second = prepare(&a, &urls[0], "f : unit -> bool");
        let callers = |item: &CallItem| {
            a.incoming_calls(item.data.as_ref().unwrap())
                .unwrap()
                .into_iter()
                .map(|(i, _)| i.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(callers(&first), ["a"]);
        assert_eq!(callers(&second), ["b"]);
        let before = prepare(&a, &urls[0], "a()");
        let after = prepare(&a, &urls[0], "b()");
        for (caller, target) in [(before, first), (after, second)] {
            let edges = a.outgoing_calls(caller.data.as_ref().unwrap()).unwrap();
            assert_eq!(edges.len(), 1);
            assert_eq!(edges[0].0.selection_range, target.selection_range);
        }
    }

    #[test]
    fn generated_callables_keep_distinct_hierarchy_identities_and_validate_continuations() {
        for (defs, body, names) in [
            (
                "default Order dec\nbitfield B : bits(8) = { part : 3 .. 0 }\n",
                "function use() -> bits(8) = _get_B_bits(Mk_B(0x00))\n",
                ["_get_B_bits", "Mk_B"],
            ),
            (
                "enum E = { A, B }\n",
                "function use() -> int = num_of_E(E_of_num(0))\n",
                ["num_of_E", "E_of_num"],
            ),
            (
                "val m : bool <-> int\n",
                "function use() -> int = m_forwards(m_backwards(1))\n",
                ["m_forwards", "m_backwards"],
            ),
        ] {
            let (mut host, map, urls) =
                setup(&[("defs.sail", defs), ("main.sail", body)], &[0, 1], vec![]);
            let a = snapshot(&host, &map);
            let caller = prepare(&a, &urls[1], "use()");
            let outgoing = a.outgoing_calls(caller.data.as_ref().unwrap()).unwrap();
            assert_eq!(outgoing.len(), 2, "{defs}{outgoing:?}");
            let mut saved = Vec::new();
            for name in names {
                let target = prepare(&a, &urls[1], name);
                assert_eq!(target.name, name);
                assert_eq!(target.url, urls[0]);
                assert!(
                    outgoing.iter().any(|(item, ranges)| item.name == name
                        && ranges
                            .iter()
                            .any(|range| base_db::range_start(*range) == body.find(name).unwrap())),
                    "{outgoing:?}"
                );
                let data = target.data.unwrap();
                let incoming = a.incoming_calls(&data).unwrap();
                assert_eq!(incoming.len(), 1, "{name}: {incoming:?}");
                assert_eq!(incoming[0].0.name, "use");
                assert_eq!(incoming[0].1.len(), 1);
                assert_eq!(base_db::range_start(incoming[0].1[0]), body.find(name).unwrap());
                assert!(a.outgoing_calls(&data).unwrap().is_empty());
                saved.push(data);
            }
            drop(a);
            let mut change = Change::new();
            change.change_file(FileId::from_raw(0), "\n");
            host.apply_change(change);
            let a = snapshot(&host, &map);
            for data in saved {
                assert!(a.incoming_calls(&data).unwrap().is_empty());
                assert!(a.outgoing_calls(&data).unwrap().is_empty());
            }
        }
    }

    #[test]
    fn call_before_signature_is_unresolved_and_later_private_binding_hides_old_one() {
        let modules = vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
        ];
        let (host, map, urls) = setup(
            &[
                (
                    "a.sail",
                    "val before : unit -> int\nfunction before() = f()\nval f : unit -> int\n",
                ),
                ("b.sail", "private val f : unit -> int\n"),
                ("c.sail", "val after : unit -> int\nfunction after() = f()\n"),
            ],
            &[0, 1, 2],
            modules,
        );
        let a = snapshot(&host, &map);
        for (url, needle) in [(&urls[0], "before()"), (&urls[2], "after()")] {
            let item = prepare(&a, url, needle);
            assert!(a.outgoing_calls(item.data.as_ref().unwrap()).unwrap().is_empty());
            let sf = a.file(url).unwrap();
            assert!(a
                .prepare_call_hierarchy(url, sf.position_at(sf.text().find("f()").unwrap()))
                .unwrap()
                .is_none());
        }
    }

    #[test]
    fn continuation_keeps_include_context_and_rejects_removed_instance() {
        let modules = (0..2)
            .map(|i| ModuleData { files: vec![FileId::from_raw(i)], requires: vec![] })
            .collect();
        let (mut host, map, urls) = setup(
            &[
                ("a.sail", "private val helper : unit -> int\n$include \"shared.sail\"\n"),
                ("b.sail", "private val helper : unit -> int\n$include \"shared.sail\"\n"),
                ("shared.sail", "val caller : unit -> int\nfunction caller() = helper()\n"),
            ],
            &[0, 1],
            modules,
        );
        let a = snapshot(&host, &map);
        let mut contexts = Vec::new();
        for url in &urls[..2] {
            let helper = prepare(&a, url, "helper :");
            let callers = a.incoming_calls(helper.data.as_ref().unwrap()).unwrap();
            assert_eq!(callers.len(), 1);
            assert_eq!(callers[0].0.url, urls[2]);
            let caller = &callers[0].0;
            let callees = a.outgoing_calls(caller.data.as_ref().unwrap()).unwrap();
            assert_eq!(callees.len(), 1);
            assert_eq!(&callees[0].0.url, url);
            contexts.push(caller.data.clone().unwrap());
        }
        assert_ne!(contexts[0]["context"], contexts[1]["context"]);
        drop(a);
        let mut change = Change::new();
        change.change_file(FileId::from_raw(1), "private val helper : unit -> int\n");
        host.apply_change(change);
        let a = snapshot(&host, &map);
        assert!(a.outgoing_calls(&contexts[1]).unwrap().is_empty());
        assert_eq!(a.outgoing_calls(&contexts[0]).unwrap().len(), 1);
    }

    #[test]
    fn type_edges_keep_module_privacy_and_include_context() {
        let modules = (0..2)
            .map(|i| ModuleData { files: vec![FileId::from_raw(i)], requires: vec![] })
            .collect();
        let (host, map, urls) = setup(&[
            ("a.sail", "private struct BaseA = { x : int }\n$include \"shared.sail\"\n"),
            ("b.sail", "private struct BaseB = { x : bool }\n$define SECOND\n$include \"shared.sail\"\ntype Hidden = BaseA\n"),
            ("shared.sail", "$ifdef SECOND\ntype AliasB = BaseB\n$else\ntype AliasA = BaseA\n$endif\n"),
        ], &[0, 1], modules);
        let a = snapshot(&host, &map);
        let prepare_type = |url: &url::Url, needle: &str| {
            let sf = a.file(url).unwrap();
            a.prepare_type_hierarchy(url, sf.position_at(sf.text().find(needle).unwrap()))
                .unwrap()
                .unwrap()
        };
        let mut contexts = Vec::new();
        for (i, alias, base) in [(0, "AliasA", "BaseA"), (1, "AliasB", "BaseB")] {
            let alias = prepare_type(&urls[2], alias);
            let supers = a.type_supertypes(alias.data.as_ref().unwrap()).unwrap();
            assert_eq!(supers.len(), 1);
            assert_eq!(supers[0].url, urls[i]);
            assert_eq!(supers[0].name, base);
            let subs = a.type_subtypes(supers[0].data.as_ref().unwrap()).unwrap();
            assert_eq!(subs.len(), 1);
            assert_eq!(subs[0].data, alias.data);
            contexts.push(alias.data.unwrap());
        }
        assert_ne!(contexts[0]["context"], contexts[1]["context"]);
        let hidden = prepare_type(&urls[1], "Hidden");
        assert!(a.type_supertypes(hidden.data.as_ref().unwrap()).unwrap().is_empty());
        assert!(a
            .type_subtypes(
                &serde_json::json!({"kind": "type", "context": u64::MAX, "source": u64::MAX})
            )
            .unwrap()
            .is_empty());
    }

    #[test]
    fn hierarchy_works_in_unconfigured_buffers() {
        let (a, _) = crate::fixture::analysis("//- /lib.sail\nval helper : unit -> int\ntype Base = int\n//- /main.sail\nval run : unit -> int\nfunction run() = helper()\ntype Alias = Base\n");
        let main = url::Url::parse("file:///main.sail").unwrap();
        let run = prepare(&a, &main, "run()");
        let outgoing = a.outgoing_calls(run.data.as_ref().unwrap()).unwrap();
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].0.name, "helper");
        let incoming = a.incoming_calls(outgoing[0].0.data.as_ref().unwrap()).unwrap();
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].0.name, "run");
        let file = a.file(&main).unwrap();
        let alias = a
            .prepare_type_hierarchy(&main, file.position_at(file.text().find("Alias").unwrap()))
            .unwrap()
            .unwrap();
        let supers = a.type_supertypes(alias.data.as_ref().unwrap()).unwrap();
        assert_eq!(supers.len(), 1);
        assert_eq!(supers[0].name, "Base");
    }
}
