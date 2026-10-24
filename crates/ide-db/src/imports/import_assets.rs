//! Shared auto-include candidates and validated edits. Search is cheap and
//! completion resolves an edit only for the selected candidate, as in RA.
use base_db::{preprocess::WorkspaceSourceProject, FileId, FileText};
use hir::source_order::bindings::{self, Namespace};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Component, Path};
use syntax::{SyntaxKind as SK, SyntaxNode};

use crate::{defs::CompletionItemKind, text_edit::TextEdit};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportRequest {
    pub source: FileId,
    pub context: FileId,
    pub offset: usize,
    pub target: FileId,
    pub declaration: usize,
    pub name: String,
    pub in_type: bool,
    pub source_hash: u64,
    pub target_hash: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocatedImport {
    pub request: ImportRequest,
    pub include_path: String,
    pub kind: CompletionItemKind,
    pub signature: String,
    pub documentation: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Symbol {
    name: String,
    namespace: Namespace,
    callable: bool,
    private: bool,
    global: bool,
    kind: CompletionItemKind,
    signature: String,
    documentation: Option<String>,
    enum_conversion: bool,
    explicit_val: bool,
    deferred_vector: bool,
}

impl Symbol {
    fn matches_namespace(&self, namespace: Namespace) -> bool {
        self.namespace == namespace || (namespace == Namespace::Callable && self.callable)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Decl {
    start: usize,
    end: usize,
    region: usize,
    symbols: Vec<Symbol>,
    hazard: bool,
    semantic: bool,
    /// Bodies and types cannot be defined twice, even under different modules.
    /// Prototypes may legitimately replace a previous signature and its owner.
    unique: bool,
    scattered: Option<hir::source_order::scattered::Event>,
}

#[salsa::tracked(returns(ref))]
fn outline(db: &dyn salsa::Database, file: FileText) -> Vec<Decl> {
    syntax::parse_query::parse_file(db, file)
        .syntax_node()
        .map(|root| describe(&root))
        .unwrap_or_default()
}

fn describe(root: &SyntaxNode) -> Vec<Decl> {
    // Raw lowering retains both conditional branches. The preview, not the
    // catalog, decides which branch a new include actually activates.
    let tree = hir::ItemTree::build_from_cst(root);
    let mut region = 0;
    root.children()
        .map(|node| {
            let start = usize::from(node.text_range().start());
            let end = usize::from(node.text_range().end());
            let mut symbols = Vec::new();
            let mut unique = false;
            for &item in tree.top_level_items().iter().filter(|item| {
                let at = item.name_span(&tree).start;
                start <= at && at < end
            }) {
                unique |= !item.is_clause(&tree)
                    && matches!(
                        item.def_kind(&tree),
                        hir::DefKind::Function
                            | hir::DefKind::Mapping
                            | hir::DefKind::Struct
                            | hir::DefKind::Union
                            | hir::DefKind::Enum
                            | hir::DefKind::TypeAlias
                            | hir::DefKind::Bitfield
                            | hir::DefKind::Newtype
                    );
                let member_only = item.def_kind(&tree) == hir::DefKind::ScatteredClause
                    && item.scattered_type_kind(&tree).is_some();
                let (in_type, kind) = match item.def_kind(&tree) {
                    hir::DefKind::ScatteredHead if item.scattered_type_kind(&tree).is_some() => {
                        (true, CompletionItemKind::Enum)
                    }
                    hir::DefKind::ScatteredClause if member_only => {
                        (false, CompletionItemKind::EnumMember)
                    }
                    hir::DefKind::Function
                    | hir::DefKind::ValSpec
                    | hir::DefKind::Mapping
                    | hir::DefKind::MappingSpec
                    | hir::DefKind::Overload => (false, CompletionItemKind::Function),
                    hir::DefKind::Struct
                    | hir::DefKind::Union
                    | hir::DefKind::Enum
                    | hir::DefKind::TypeAlias
                    | hir::DefKind::Bitfield
                    | hir::DefKind::Newtype => (true, CompletionItemKind::Struct),
                    hir::DefKind::Let | hir::DefKind::Var | hir::DefKind::Register => {
                        (false, CompletionItemKind::Variable)
                    }
                    _ => continue,
                };
                let private = item.visibility(&tree) == hir::visibility::RawVisibility::Private;
                if !member_only {
                    symbols.push(Symbol {
                        name: item.name(&tree).to_string(),
                        namespace: if node.kind() == SK::OUTCOME_DEF {
                            Namespace::Outcome
                        } else if in_type {
                            Namespace::Type
                        } else {
                            Namespace::Value
                        },
                        callable: matches!(
                            item.def_kind(&tree),
                            hir::DefKind::Function
                                | hir::DefKind::ValSpec
                                | hir::DefKind::Mapping
                                | hir::DefKind::MappingSpec
                        ),
                        private,
                        global: item.def_kind(&tree) == hir::DefKind::Overload
                            || tree.generated.get(&item).is_some_and(|origin| {
                                origin.kind == hir::generated::Kind::ScatteredEnumVector
                            }),
                        kind,
                        signature: item.signature(&tree).to_owned(),
                        documentation: item.doc(&tree).map(str::to_owned),
                        enum_conversion: tree
                            .generated
                            .get(&item)
                            .is_some_and(|origin| origin.kind == hir::generated::Kind::EnumNumber),
                        explicit_val: node.kind() == SK::CALLABLE_SPEC
                            && !tree.generated.contains_key(&item),
                        deferred_vector: tree.generated.get(&item).is_some_and(|origin| {
                            origin.kind == hir::generated::Kind::ScatteredEnumVector
                        }),
                    });
                }
                if matches!(
                    item.def_kind(&tree),
                    hir::DefKind::Enum | hir::DefKind::Union | hir::DefKind::Newtype
                ) || member_only
                {
                    for member in item.fields(&tree) {
                        symbols.push(Symbol {
                            name: member.name.to_string(),
                            namespace: Namespace::Value,
                            callable: false,
                            // Enum clauses inherit the head's privacy. Its
                            // actual owner may be in another file; shared
                            // preview validation checks access to that head.
                            private: if member_only
                                && item.scattered_type_kind(&tree) == Some(hir::TypeDefShape::Enum)
                            {
                                false
                            } else {
                                private
                                    || member.visibility == hir::visibility::RawVisibility::Private
                            },
                            global: false,
                            kind: CompletionItemKind::EnumMember,
                            signature: member.type_text.clone(),
                            documentation: None,
                            enum_conversion: false,
                            explicit_val: false,
                            deferred_vector: false,
                        });
                    }
                }
            }
            let event = syntax::preprocess::Event::from_node(&node);
            let hazard = node.kind() == SK::DIRECTIVE_DEF && {
                let raw = node.text().to_string();
                raw.trim_start().starts_with("$include_error")
                    || raw.trim_start().starts_with("$option")
            };
            let result = Decl {
                start,
                end,
                region,
                symbols,
                hazard,
                semantic: node.kind() != SK::DIRECTIVE_DEF,
                unique,
                scattered: hir::source_order::scattered::event(&node),
            };
            if event.is_some() {
                region += 1;
            }
            result
        })
        .collect()
}

fn fingerprint(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Recognize a reference, not a field selector, definition, string or comment.
/// Completion may supply a different, fully spelled candidate for this token.
pub fn reference_at(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
) -> Option<(String, bool)> {
    reference(db, file, offset).map(|(name, namespace)| (name, namespace == Namespace::Type))
}

fn reference(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
) -> Option<(String, Namespace)> {
    if !file.text(db).is_char_boundary(offset) {
        return None;
    }
    let root = syntax::parse_query::parse_file(db, file).syntax_node()?;
    let token = root.token_at_offset((offset as u32).into()).find(|t| t.kind() == SK::IDENT)?;
    let parent = token.parent()?;
    if parent.kind() == SK::DIRECTIVE_DEF || parent.ancestors().any(|n| n.kind() == SK::ATTRIBUTE) {
        return None;
    }
    let top =
        parent.ancestors().find(|n| n.parent().is_some_and(|p| p.kind() == SK::SOURCE_FILE))?;
    if top.kind() == SK::INSTANTIATION_DEF {
        let namespace =
            match syntax::instantiation_reference(&top, usize::from(token.text_range().start()))? {
                syntax::InstantiationReference::Outcome => Namespace::Outcome,
                syntax::InstantiationReference::Type => Namespace::Type,
                syntax::InstantiationReference::Callable => Namespace::Callable,
            };
        return Some((token.text().to_owned(), namespace));
    }
    if syntax::definition_name_span(&top)
        .is_some_and(|s| s.start == usize::from(token.text_range().start()))
    {
        // The grammar represents this target as NAME, but it references an
        // outcome; it does not declare a new outcome or reference a function.
        if top.kind() == SK::INSTANTIATION_DEF {
            return Some((token.text().to_owned(), Namespace::Outcome));
        }
        return None;
    }
    if parent.kind() == SK::FIELD_ACCESS_EXPR {
        return None;
    }
    let in_type = parent.kind().is_type_node();
    if !in_type && parent.kind() != SK::IDENT_EXPR {
        return None;
    }
    Some((token.text().to_owned(), if in_type { Namespace::Type } else { Namespace::Value }))
}

fn views_at(db: &dyn salsa::Database, source: FileText, offset: usize) -> Vec<FileText> {
    let Some(input) = base_db::preprocess::WorkspacePreprocessing::try_get(db) else {
        return vec![];
    };
    let Some(workspace) = base_db::WorkspaceFiles::try_get(db) else { return vec![] };
    let id = source.file_id(db);
    let region =
        outline(db, source).iter().find(|d| d.start <= offset && offset <= d.end).map(|d| d.region);
    let ids = input.instances(db).get(&id);
    workspace
        .file_texts(db)
        .iter()
        .copied()
        .filter(|ft| {
            let view = ft.file_id(db);
            ids.is_some_and(|ids| ids.contains(&view))
                && region.is_some_and(|region| {
                    input
                        .regions(db)
                        .get(&view)
                        .and_then(|regions| regions.get(region))
                        .copied()
                        .unwrap_or(false)
                })
        })
        .collect()
}

fn already_resolved(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    name: &str,
    namespace: Namespace,
) -> bool {
    let offset = syntax::parse_query::parse_file(db, file)
        .syntax_node()
        .and_then(|root| {
            root.token_at_offset((offset as u32).into()).find(|t| t.kind() == SK::IDENT)
        })
        .map_or(offset, |token| usize::from(token.text_range().start()));
    if namespace == Namespace::Type
        && (hir::is_builtin_type_name(name) || hir::is_builtin_type_level_fn(name))
    {
        return true;
    }
    let Some(at) = hir::source_order::at_offset(db, file, offset) else {
        return false;
    };
    bindings::resolve(db, at, hir::Name::new(name), namespace).is_some()
        || (namespace == Namespace::Value
            && matches!(
                hir::Semantics::new(db).resolve_path(db, file, offset, name),
                Some(hir::PathResolution::Local(_))
            ))
}

/// Search the loaded project catalog, including inactive include candidates.
/// No filesystem IO or speculative database mutation occurs here.
pub fn search(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    prefix: &str,
    exact: bool,
) -> Vec<LocatedImport> {
    let Some(input) = WorkspaceSourceProject::try_get(db) else { return vec![] };
    let source_id = base_db::preprocess::physical_file(db, file.file_id(db));
    let Some(&source) = input.files(db).get(&source_id) else { return vec![] };
    let Some((_, namespace)) = reference(db, source, offset) else { return vec![] };
    let in_type = namespace == Namespace::Type;
    let views = views_at(db, source, offset);
    if views.is_empty() {
        return vec![];
    }
    let context = views
        .iter()
        .copied()
        .find(|view| view.file_id(db) == file.file_id(db))
        .unwrap_or(views[0])
        .file_id(db);
    let project = input.project(db);
    let Some((source_path, _)) = project.paths.iter().find(|(_, id)| *id == source_id) else {
        return vec![];
    };
    let prefix_lower = prefix.to_lowercase();
    let source_hash = fingerprint(source.text(db));
    let mut result = Vec::new();
    for (path, id) in &project.paths {
        db.unwind_if_revision_cancelled();
        if *id == source_id {
            continue;
        }
        let Some(&target) = input.files(db).get(id) else { continue };
        let Some(include_path) = relative_path(source_path, path) else { continue };
        let mut found = HashSet::new();
        let declarations = outline(db, target);
        let public_names: HashSet<_> = declarations
            .iter()
            .flat_map(|decl| &decl.symbols)
            .filter(|symbol| !symbol.private)
            .map(|symbol| (&symbol.name, symbol.namespace))
            .collect();
        for (declaration, decl) in declarations.iter().enumerate().rev() {
            for symbol in &decl.symbols {
                if !symbol.matches_namespace(namespace)
                    || if exact {
                        symbol.name != prefix
                    } else {
                        !symbol.name.to_lowercase().starts_with(&prefix_lower)
                    }
                {
                    continue;
                }
                if !found.insert(symbol.name.clone()) {
                    continue;
                }
                // Includes inherit the caller's module, but are not a way to
                // import another module's private declarations under a new owner.
                // A raw catalog representative may come from an inactive
                // branch. Defer mixed public/private names to the preview,
                // which checks the declaration actually selected at the use.
                if symbol.private
                    && !public_names.contains(&(&symbol.name, symbol.namespace))
                    && !private_import_allowed(db, input, &views, *id)
                {
                    continue;
                }
                if views
                    .iter()
                    .any(|&view| already_resolved(db, view, offset, &symbol.name, namespace))
                {
                    continue;
                }
                result.push(LocatedImport {
                    request: ImportRequest {
                        source: source_id,
                        context,
                        offset,
                        target: *id,
                        declaration,
                        name: symbol.name.clone(),
                        in_type,
                        source_hash,
                        target_hash: fingerprint(target.text(db)),
                    },
                    include_path: include_path.clone(),
                    kind: symbol.kind,
                    signature: symbol.signature.clone(),
                    documentation: symbol.documentation.clone(),
                });
            }
        }
    }
    result.sort_by(|a, b| {
        a.request.name.cmp(&b.request.name).then(a.include_path.cmp(&b.include_path))
    });
    result
}

fn private_import_allowed(
    db: &dyn salsa::Database,
    input: WorkspaceSourceProject,
    views: &[FileText],
    target: FileId,
) -> bool {
    let project = input.project(db);
    let plan = hir::preprocess::workspace_plan(db, input);
    views.iter().all(|view| {
        let owners = project
            .modules
            .modules
            .iter()
            .enumerate()
            .filter(|(_, module)| module.files.contains(&target))
            .map(|(owner, _)| owner)
            .chain(
                plan.instances
                    .iter()
                    .filter(|instance| instance.source == target && instance.active)
                    .filter_map(|instance| instance.key.module),
            );
        owners.into_iter().all(|owner| {
            project.modules.modules[owner]
                .files
                .iter()
                .any(|&file| base_db::module_graph::same_module(db, view.file_id(db), file))
        })
    })
}

fn relative_path(source: &Path, target: &Path) -> Option<String> {
    let from: Vec<_> = source.parent()?.components().collect();
    let to: Vec<_> = target.components().collect();
    if from.first() != to.first() {
        return None;
    }
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec!["..".to_owned(); from.len() - shared];
    for c in &to[shared..] {
        let Component::Normal(c) = c else { return None };
        parts.push(c.to_str()?.to_owned());
    }
    let path = parts.join("/");
    // Sail's include directive does not decode escaped path strings.
    (!path.is_empty() && !path.contains(['"', '\n', '\r'])).then_some(path)
}

type Identity = (FileId, usize, Option<usize>);
struct Visit<'a> {
    instance: usize,
    identity: Identity,
    decl: &'a Decl,
}

fn expanded<'a>(
    plan: &hir::preprocess::Plan,
    outlines: &'a HashMap<FileId, Vec<Decl>>,
) -> Vec<Visit<'a>> {
    let instances: HashMap<_, _> = plan
        .instances
        .iter()
        .enumerate()
        .map(|(index, instance)| (&instance.key, (index, instance)))
        .collect();
    let mut visits = Vec::new();
    for (key, region) in &plan.order {
        let Some(&(instance_index, instance)) = instances.get(key) else { continue };
        if !instance.regions.get(*region).copied().unwrap_or(false) {
            continue;
        }
        let Some(decls) = outlines.get(&instance.source) else { continue };
        for (index, decl) in decls.iter().enumerate().filter(|(_, d)| d.region == *region) {
            visits.push(Visit {
                instance: instance_index,
                identity: (instance.source, index, key.module),
                decl,
            });
        }
    }
    visits
}

fn explicit_vals<'a>(visits: &[Visit<'a>]) -> HashSet<(usize, &'a str)> {
    visits
        .iter()
        .flat_map(|visit| {
            visit
                .decl
                .symbols
                .iter()
                .filter(|symbol| symbol.explicit_val)
                .map(|symbol| (visit.instance, symbol.name.as_str()))
        })
        .collect()
}

/// Resolve against the current inputs and preview all affected include visits.
/// Stale completion requests cannot publish an edit at an old byte offset.
pub fn resolve(
    db: &dyn salsa::Database,
    request: &ImportRequest,
) -> Option<(LocatedImport, TextEdit)> {
    let input = WorkspaceSourceProject::try_get(db)?;
    let &source = input.files(db).get(&request.source)?;
    let &target = input.files(db).get(&request.target)?;
    if fingerprint(source.text(db)) != request.source_hash
        || fingerprint(target.text(db)) != request.target_hash
    {
        return None;
    }
    let (_, namespace) = reference(db, source, request.offset)?;
    let context = base_db::WorkspaceFiles::try_get(db)?
        .file_texts(db)
        .iter()
        .copied()
        .find(|file| file.file_id(db) == request.context)?;
    let mut candidate = search(db, context, request.offset, &request.name, true)
        .into_iter()
        .find(|c| c.request == *request)?;
    let root = syntax::parse_query::parse_file(db, source).syntax_node()?;
    let mut node = root.children().find(|node| {
        let r = node.text_range();
        usize::from(r.start()) <= request.offset && request.offset <= usize::from(r.end())
    })?;
    // Keep documentation and attributes attached to the original declaration.
    while let Some(previous) = node.prev_sibling().filter(|n| n.kind() == SK::ATTRIBUTE) {
        node = previous;
    }
    let mut start = usize::from(node.text_range().start());
    let mut previous = node.first_token().and_then(|t| t.prev_token());
    while let Some(token) = previous {
        if token.kind().is_trivia()
            || token.text().starts_with("///")
            || token.text().starts_with("/**")
        {
            if token.text().trim().is_empty() {
                previous = token.prev_token();
                continue;
            }
            if token.text().starts_with("///") || token.text().starts_with("/**") {
                start = usize::from(token.text_range().start());
                previous = token.prev_token();
                continue;
            }
        }
        break;
    }
    let line_start = source.text(db)[..start].rfind('\n').map_or(0, |i| i + 1);
    let insert =
        if source.text(db)[line_start..start].trim().is_empty() { line_start } else { start };
    let newline = if source.text(db).contains("\r\n") { "\r\n" } else { "\n" };
    let prefix =
        if insert > 0 && !source.text(db)[..insert].ends_with('\n') { newline } else { "" };
    let edit = TextEdit {
        range: base_db::text_range(insert, insert),
        new_text: format!("{prefix}$include \"{}\"{newline}", candidate.include_path),
    };
    let mut edited = source.text(db).to_string();
    edited.insert_str(insert, &edit.new_text);
    let (new_root, _) = if let Some(fixities) = base_db::WorkspaceFixities::try_get(db) {
        syntax::parse_text_with_fixities(&edited, fixities.fixities(db).clone())
    } else {
        syntax::parse_text(&edited)
    };
    let before = hir::preprocess::workspace_plan(db, input);
    let after = hir::preprocess::preview_file(db, input, request.source, &new_root);
    if after.cycles.iter().any(|file| !before.cycles.contains(file))
        || after.missing.iter().any(|(file, _, path)| {
            !before.missing.iter().any(|(old, _, p)| old == file && p == path)
        })
    {
        return None;
    }
    let old_outlines: HashMap<_, _> =
        input.files(db).iter().map(|(&id, &file)| (id, outline(db, file).clone())).collect();
    let mut new_outlines = old_outlines.clone();
    new_outlines.insert(request.source, describe(&new_root));
    let old_visits = expanded(before, &old_outlines);
    let new_visits = expanded(&after, &new_outlines);
    let old_explicit = explicit_vals(&old_visits);
    let new_explicit = explicit_vals(&new_visits);
    let originals = &old_outlines[&request.source];
    let map_identity = |visit: &Visit<'_>| -> Option<Identity> {
        if visit.identity.0 != request.source {
            return Some(visit.identity);
        }
        if (insert..insert + edit.new_text.len()).contains(&visit.decl.start) {
            return None;
        }
        let start =
            visit.decl.start - if visit.decl.start >= insert { edit.new_text.len() } else { 0 };
        let index = originals.iter().position(|d| d.start == start)?;
        Some((request.source, index, visit.identity.2))
    };
    // Validate the same head/clause/end state used by real diagnostics. The
    // preview already supplies compiler expansion order and include ownership;
    // no temporary database or second scattered semantics is needed.
    let scattered_issues = |visits: &[Visit<'_>]| {
        let events: Vec<_> = visits
            .iter()
            .enumerate()
            .filter_map(|(context, visit)| {
                visit.decl.scattered.clone().map(|event| hir::source_order::scattered::EventAt {
                    file: visit.identity.0,
                    context,
                    event,
                })
            })
            .collect();
        hir::source_order::scattered::validate(&events, |owner, from, private| {
            let owner = visits[owner].identity.2;
            let from = visits[from].identity.2;
            owner == from
                || (!private
                    && (owner.is_none()
                        || from.is_some_and(|from| {
                            input.project(db).modules.modules.get(from).is_some_and(|module| {
                                owner.is_some_and(|owner| module.requires.contains(&owner))
                            })
                        })))
        })
    };
    let old_scattered: HashSet<_> = scattered_issues(&old_visits)
        .into_iter()
        .map(|issue| (old_visits[issue.context].identity, issue.code, issue.message))
        .collect();
    for issue in scattered_issues(&new_visits) {
        let identity = map_identity(&new_visits[issue.context])?;
        if !old_scattered.contains(&(identity, issue.code, issue.message)) {
            return None;
        }
    }
    let count = |visits: &[Visit<'_>], overlay: bool| {
        let mut counts = HashMap::new();
        for visit in visits {
            // Include/preprocessor directives are expected to change execution
            // counts when guards avoid a later visit. Definitions must survive.
            if !visit.decl.semantic && !visit.decl.hazard {
                continue;
            }
            if let Some(id) = if overlay { map_identity(visit) } else { Some(visit.identity) } {
                *counts.entry(id).or_insert(0usize) += 1;
            }
        }
        counts
    };
    let old_counts = count(&old_visits, false);
    let new_counts = count(&new_visits, true);
    if old_counts.iter().any(|(id, n)| new_counts.get(id) != Some(n))
        || new_counts.iter().any(|(id, n)| *n > old_counts.get(id).copied().unwrap_or(1))
    {
        return None;
    }
    // Module-aware counts above preserve existing ownership, but a new owner
    // gives the same physical declaration a different Identity. Check actual
    // bodies/types across owners too, including multiple entirely new visits.
    let unique_counts = |visits: &[Visit<'_>], overlay: bool| {
        let mut counts = HashMap::new();
        for visit in visits.iter().filter(|visit| visit.decl.unique) {
            if let Some((file, declaration, _)) =
                if overlay { map_identity(visit) } else { Some(visit.identity) }
            {
                *counts.entry((file, declaration)).or_insert(0usize) += 1;
            }
        }
        counts
    };
    let old_unique = unique_counts(&old_visits, false);
    if unique_counts(&new_visits, true)
        .iter()
        .any(|(id, n)| *n > old_unique.get(id).copied().unwrap_or(1))
    {
        return None;
    }
    for visit in &new_visits {
        let Some(id) = map_identity(visit) else { continue };
        if visit.decl.hazard && !old_counts.contains_key(&id) {
            return None;
        }
        if old_counts.contains_key(&id) {
            continue;
        }
        // Reject new declarations colliding with a different existing origin.
        // A prototype and its implementation are intentionally the same binding.
        for symbol in &visit.decl.symbols {
            if symbol.enum_conversion
                && new_explicit.contains(&(visit.instance, symbol.name.as_str()))
            {
                continue;
            }
            if old_visits.iter().any(|old| {
                old.identity.0 != id.0
                    && old.decl.symbols.iter().any(|s| {
                        s.name == symbol.name
                            && s.namespace == symbol.namespace
                            // Public field overloads from distinct bitfields
                            // extend one global group. Changing its privacy
                            // would instead hide existing members.
                            && !(s.global && symbol.global && !s.deferred_vector && !symbol.deferred_vector && s.private == symbol.private)
                            && !(s.enum_conversion
                                && old_explicit.contains(&(old.instance, s.name.as_str())))
                    })
            }) {
                return None;
            }
        }
    }
    // Resolve the requested name immediately before each affected declaration.
    // Includes inherit module ownership. Later replacements are filtered only
    // after selection, matching the production ordered resolver.
    let use_decl =
        originals.iter().position(|d| d.start <= request.offset && request.offset <= d.end)?;
    let visible_from = |owner: usize, from: usize, private: bool| {
        let owner = new_visits[owner].identity.2;
        let from = new_visits[from].identity.2;
        owner == from
            || (!private
                && (owner.is_none()
                    || from.is_some_and(|from| {
                        input.project(db).modules.modules.get(from).is_some_and(|module| {
                            owner.is_some_and(|owner| module.requires.contains(&owner))
                        })
                    })))
    };
    let vector_events: Vec<_> = new_visits
        .iter()
        .enumerate()
        .filter_map(|(context, visit)| {
            Some(hir::source_order::scattered::EventAt {
                context,
                file: visit.identity.0,
                event: visit.decl.scattered.clone()?,
            })
        })
        .collect();
    let vectors =
        hir::source_order::scattered::completed_enum_vectors(&vector_events, visible_from);
    let vectors: HashMap<_, _> = vectors.iter().map(|v| (v.end, v)).collect();
    let mut binding: Option<(&Symbol, Identity)> = None;
    let mut vector_owner = None;
    let mut vector_signature = None;
    let mut uses = 0;
    let mut selected: Option<&Symbol> = None;
    let mut selected_vector_signature = None;
    for (context, visit) in new_visits.iter().enumerate() {
        let Some(id) = map_identity(visit) else { continue };
        if id.0 == request.source && id.1 == use_decl {
            let (symbol, origin) = binding?;
            if let Some((owner, private)) = vector_owner {
                if !visible_from(owner, context, private) {
                    return None;
                }
            }
            let same = origin.2 == id.2;
            let visible = if symbol.global {
                !symbol.private || id.2.is_none()
            } else {
                same || (!symbol.private
                    && (origin.2.is_none()
                        || id.2.is_some_and(|m| {
                            input.project(db).modules.modules.get(m).is_some_and(|module| {
                                origin.2.is_some_and(|owner| module.requires.contains(&owner))
                            })
                        })))
            };
            if origin.0 != request.target || !visible {
                return None;
            }
            if selected.is_some_and(|previous| previous != symbol) {
                return None;
            }
            selected = Some(symbol);
            selected_vector_signature = vector_signature.clone();
            uses += 1;
        }
        for symbol in &visit.decl.symbols {
            if symbol.deferred_vector {
                continue;
            }
            if symbol.enum_conversion
                && new_explicit.contains(&(visit.instance, symbol.name.as_str()))
            {
                continue;
            }
            if symbol.name == request.name && symbol.matches_namespace(namespace) {
                binding = Some((symbol, id));
                vector_owner = None;
                vector_signature = None;
            }
        }
        if let Some(vector) = vectors
            .get(&context)
            .filter(|v| v.name == request.name && namespace == Namespace::Value)
        {
            let owner = &new_visits[vector.head];
            let symbol =
                owner.decl.symbols.iter().find(|s| s.deferred_vector && s.name == vector.name)?;
            let ty = owner
                .decl
                .symbols
                .iter()
                .find(|s| s.namespace == Namespace::Type && s.name == vector.enum_name)?;
            binding = Some((symbol, map_identity(owner)?));
            vector_owner = Some((vector.head, ty.private));
            vector_signature = Some(format!(
                "let {} : vector({}, {})",
                vector.name, vector.members, vector.enum_name
            ));
        }
    }
    let symbol = selected?;
    if symbol.private
        && !private_import_allowed(db, input, &views_at(db, source, request.offset), request.target)
    {
        return None;
    }
    candidate.kind = symbol.kind;
    candidate.signature = selected_vector_signature.unwrap_or_else(|| symbol.signature.clone());
    candidate.documentation = symbol.documentation.clone();
    (uses > 0).then_some((candidate, edit))
}
