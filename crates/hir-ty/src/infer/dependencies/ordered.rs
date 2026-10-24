//! Declaration contributions for a configured, sequential Sail program.
//! The directory contains names and stable ordinals only. Type/signature
//! projections backdate independently of bodies and unrelated declarations.
use super::*;
use hir_def::source_order::{self, Declaration};
use parser::SyntaxKind as SK;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Kind {
    Binding,
    Type,
    Members,
    Closed,
    Overload,
    Field,
    Fact,
    Order,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Piece {
    kind: Kind,
    value: Arc<Symbol<TopLevelEnv>>,
    private: bool,
    fallback: bool,
    enum_parent: Option<Name>,
    global: bool,
}

type Contributions = HashMap<Name, Vec<Piece>>;

#[salsa::tracked(returns(ref))]
fn file_contributions(db: &dyn salsa::Database, file: FileText) -> Vec<Contributions> {
    let Some(root) = syntax::parse_query::semantic_parse_file(db, file).syntax_node() else {
        return Vec::new();
    };
    let children: Vec<_> = root.children().collect();
    let explicit_vals = hir_def::item_tree::generated::explicit_val_names(root.children());
    children.iter().enumerate().map(|(index, node)| {
        db.unwind_if_revision_cancelled();
        // Reuse the existing type reader and signature metadata on one
        // declaration. Its root is rebased, so no offsets enter the result.
        // Attributes are siblings of their declaration in the CST. Include
        // them in that declaration's projection so generated enum bindings
        // appear at the enum, rather than disappearing from the ordered view.
        let mut start = index;
        if node.kind() != SK::ATTRIBUTE {
            while start > 0 && children[start - 1].kind() == SK::ATTRIBUTE { start -= 1; }
        }
        let green = syntax::GreenNode::new(rowan::SyntaxKind(SK::SOURCE_FILE as u16),
            children[start..=index].iter().map(|node| rowan::NodeOrToken::Node(node.green().into_owned())));
        let root = syntax::SyntaxNode::new_root(green);
        let text = root.text().to_string();
        let (mut env, constants) = TopLevelEnv::from_cst(&root);
        if let Some(name) = syntax::definition_name(node) {
            if let Some(members) = env.enums.get(&name) {
                for binding in hir_def::item_tree::generated::enum_bindings(node, &name, members.len()) {
                    if binding.kind == hir_def::item_tree::generated::Kind::EnumNumber
                        && explicit_vals.contains(&binding.name) {
                        env.functions.remove(&binding.name);
                    }
                }
            }
        }
        let parsed = syntax::cst_lower::parsed_file_from_cst(&root, &text);
        super::super::env::apply_callable_signature_metadata(&parsed, &text, &mut env);
        let private = node.children().any(|c| c.kind() == SK::VISIBILITY);
        let generated_visibility: HashMap<_, _> = syntax::definition_name(node)
            .and_then(|name| env.enums.get(&name).map(|members| (name, members.len())))
            .into_iter()
            .flat_map(|(name, count)| hir_def::item_tree::generated::enum_bindings(node, &name, count))
            .map(|binding| (binding.name, binding.kind.visibility(if private { hir_def::visibility::RawVisibility::Private } else { hir_def::visibility::RawVisibility::Public }) == hir_def::visibility::RawVisibility::Private))
            .collect();
        let fallback = matches!(node.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF);
        let head = node.kind() == SK::SCATTERED_DEF;
        let clause = node.kind() == SK::SCATTERED_CLAUSE_DEF;
        let enum_parent = (clause && node.children_with_tokens().any(|t| t.kind() == SK::KW_ENUM))
            .then(|| syntax::definition_name(node).map(|name| Name::new(&name))).flatten();
        if head || clause {
            if let Some(name) = syntax::definition_name(&node) {
                if node.children_with_tokens().any(|t| t.kind() == SK::KW_UNION) {
                    env.unions.entry(name.clone()).or_default();
                    env.scattered_open_types.insert(name);
                } else if node.children_with_tokens().any(|t| t.kind() == SK::KW_ENUM) {
                    env.enums.entry(name.clone()).or_default();
                    env.scattered_open_types.insert(name);
                }
            }
        }
        let facts = std::mem::take(&mut env.global_constraints);
        let order = env.vector_order;
        let has_members = node.children_with_tokens().any(|t| matches!(t.kind(), SK::KW_UNION | SK::KW_ENUM | SK::KW_NEWTYPE));
        let private_members: HashSet<_> = (if has_members { hir_def::item_tree::extract_field_decls(node) } else { Vec::new() })
            .into_iter()
            .filter(|field| field.visibility == hir_def::visibility::RawVisibility::Private)
            .map(|field| field.name.to_string())
            .collect();
        let mut output = Contributions::new();
        for (name, symbol) in env.symbols() {
            if name.is_empty() { continue; }
            let mut data = symbol.data;
            let mut add = |kind, fragment: TopLevelEnv, present: bool| {
                if present {
                    output.entry(Name::new(&name)).or_default().push(Piece {
                        kind, private: if kind == Kind::Binding { generated_visibility.get(&name).copied().unwrap_or(private || private_members.contains(&name)) } else { private }, fallback,
                        enum_parent: enum_parent.clone(), global: false,
                        value: Arc::new(Symbol {
                            data: fragment, references: symbol.references.clone(),
                            pattern_constant: kind == Kind::Binding && constants.contains(&name),
                        }),
                    });
                }
            };
            macro_rules! part { ($kind:ident; $($field:ident),+) => {{
                let mut fragment = TopLevelEnv::default();
                let mut present = false;
                $(present |= !data.$field.is_empty(); fragment.$field = std::mem::take(&mut data.$field);)+
                add(Kind::$kind, fragment, present);
            }}; }
            part!(Binding; functions, mappings, values, registers, constructors);
            part!(Type; type_aliases, alias_schemes, constraint_synonyms, records, bitfields);
            part!(Members; enums, unions, scattered_open_types);
            part!(Overload; overloads);
            part!(Field; known_field_names);
            // Reverse field/member edges can contain references without data.
            if name.starts_with('\0') || !output.contains_key(&Name::new(&name)) {
                output.entry(Name::new(&name)).or_default().push(Piece {
                    kind: Kind::Field, private, fallback: false,
                    enum_parent: enum_parent.clone(), global: false,
                    value: Arc::new(Symbol { data: TopLevelEnv::default(), references: symbol.references, pattern_constant: false }),
                });
            }
        }
        for fact in facts {
            let mut references = names_in(&fact.to_text());
            references.retain(|n| n.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '\''));
            if references.is_empty() { references.insert(Name::new("")); }
            for name in &references {
                let mut data = TopLevelEnv::default();
                data.global_constraints.push(fact.clone());
                output.entry(name.clone()).or_default().push(Piece {
                    kind: Kind::Fact, private: false, fallback: false,
                    enum_parent: None, global: false,
                    value: Arc::new(Symbol { data, references: references.clone(), pattern_constant: false }),
                });
            }
        }
        if node.kind() == SK::DEFAULT_DEF {
            let mut data = TopLevelEnv::default();
            data.vector_order = order;
            output.entry(Name::new("")).or_default().push(Piece {
                kind: Kind::Order, private: false, fallback: false,
                enum_parent: None, global: false,
                value: Arc::new(Symbol { data, ..Default::default() }),
            });
        }
        if node.kind() == SK::END_DEF {
            if let Some(name) = syntax::definition_name(node) {
                let mut data = TopLevelEnv::default();
                data.scattered_open_types.insert(name.clone());
                output.entry(Name::new(&name)).or_default().push(Piece {
                    kind: Kind::Closed, private: false, fallback: false,
                    enum_parent: None, global: false,
                    value: Arc::new(Symbol { data, ..Default::default() }),
                });
            }
        }
        output
    }).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Directory {
    positions: HashMap<Declaration, usize>,
    names: HashMap<Name, Vec<(usize, Declaration)>>,
}

/// The workspace directory depends on declaration names, not their types.
/// A signature edit backdates this projection instead of rebuilding the
/// directory for every file before checking the three affected bodies.
#[salsa::tracked(returns(ref))]
fn file_names(db: &dyn salsa::Database, file: FileText) -> Vec<Vec<Name>> {
    file_contributions(db, file)
        .iter()
        .map(|declaration| {
            let mut names: Vec<_> = declaration.keys().cloned().collect();
            names.sort();
            names
        })
        .collect()
}

#[salsa::tracked(returns(ref))]
fn directory(
    db: &dyn salsa::Database,
    _ws: base_db::WorkspaceFiles,
    program: source_order::Program,
) -> Directory {
    let mut result = Directory { positions: HashMap::new(), names: HashMap::new() };
    let vectors: HashMap<_, _> =
        source_order::scattered::enum_vectors(db, program).iter().map(|v| (v.end, v)).collect();
    for (position, &declaration) in program.declarations(db).iter().enumerate() {
        result.positions.insert(declaration, position);
        if let Some(vector) = vectors.get(&declaration) {
            result.names.entry(Name::new(&vector.name)).or_default().push((position, declaration));
        }
        if let Some(names) = file_names(db, declaration.file).get(declaration.index) {
            for name in names {
                result.names.entry(name.clone()).or_default().push((position, declaration));
            }
        }
    }
    result
}

#[salsa::tracked(returns(ref))]
fn contribution(db: &dyn salsa::Database, declaration: Declaration, name: Name) -> Vec<Piece> {
    if let Some(program) = source_order::program(db, declaration.file) {
        if let Some(vector) = source_order::scattered::enum_vectors(db, program)
            .iter()
            .find(|v| v.end == declaration && v.name == name.as_str())
        {
            let mut data = TopLevelEnv::default();
            data.values.insert(
                vector.name.clone(),
                super::super::env::vector_ty(vector.members, Ty::named(vector.enum_name.clone())),
            );
            return vec![Piece {
                kind: Kind::Binding,
                private: false,
                fallback: false,
                enum_parent: None,
                global: true,
                value: Arc::new(Symbol {
                    data,
                    references: [Name::new(&vector.enum_name)].into(),
                    pattern_constant: false,
                }),
            }];
        }
    }
    file_contributions(db, declaration.file)
        .get(declaration.index)
        .and_then(|c| c.get(&name))
        .cloned()
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Selected {
    declaration: Declaration,
    piece: Piece,
}

fn select_at(db: &dyn salsa::Database, at: Declaration, name: Name, support: bool) -> &[Selected] {
    let Some(program) = source_order::program(db, at.file) else { return &[] };
    let Some(ws) = base_db::WorkspaceFiles::try_get(db) else { return &[] };
    let directory = directory(db, ws, program);
    let Some(&position) = directory.positions.get(&at) else { return &[] };
    let Some(locations) = directory.names.get(&name) else { return &[] };
    let mut prefix = locations.partition_point(|(declared, _)| *declared <= position);
    // Sail checks a top-level initializer before inserting its value/register
    // binding. Callable definitions still include themselves for recursion.
    if !support
        && prefix > 0
        && locations[prefix - 1].1 == at
        && contribution(db, at, name.clone()).iter().any(|piece| {
            piece.kind == Kind::Binding
                && (!piece.value.data.values.is_empty() || !piece.value.data.registers.is_empty())
        })
    {
        prefix -= 1;
    }
    let from = source_order::inline::scope(db, at);
    // Bodies between two declarations of this name share the same selection.
    // In global scope this is shared across physical files as well.
    select_prefix(db, program, from, name, prefix, support)
}

#[salsa::tracked(returns(ref))]
fn select_prefix(
    db: &dyn salsa::Database,
    program: source_order::Program,
    from: source_order::inline::Scope,
    name: Name,
    prefix: usize,
    support: bool,
) -> Vec<Selected> {
    let Some(ws) = base_db::WorkspaceFiles::try_get(db) else { return Vec::new() };
    let directory = directory(db, ws, program);
    let Some(locations) = directory.names.get(&name) else { return Vec::new() };
    let mut result: Vec<Selected> = Vec::new();
    for &(_, declaration) in locations.iter().take(prefix) {
        for piece in contribution(db, declaration, name.clone()) {
            if support
                && !matches!(piece.kind, Kind::Type | Kind::Members | Kind::Closed | Kind::Fact)
            {
                continue;
            }
            if matches!(piece.kind, Kind::Binding | Kind::Type | Kind::Order) {
                if piece.kind == Kind::Binding
                    && piece.fallback
                    && result.iter().any(|p| p.piece.kind == Kind::Binding)
                {
                    continue;
                }
                result.retain(|p| p.piece.kind != piece.kind);
            }
            result.push(Selected { declaration, piece: piece.clone() });
        }
    }
    // Overload groups are global. Their most recent extension determines
    // group privacy; members retain their own visibility at the use site.
    let overload_visible = result
        .iter()
        .rev()
        .find(|s| s.piece.kind == Kind::Overload)
        .is_none_or(|s| !s.piece.private || from.is_global());
    // Filtering follows replacement: a newer private val must hide an older
    // public signature, even in a module which cannot use the replacement.
    result.retain(|s| {
        if s.piece.kind == Kind::Overload {
            return overload_visible;
        }
        let inherited = s.piece.enum_parent.as_ref().and_then(|parent| {
            use source_order::bindings::{self, Namespace};
            let head =
                bindings::declared_before(db, s.declaration, parent.clone(), Namespace::Type)?;
            bindings::declaration_visibility(db, head, parent.clone(), Namespace::Type)
        });
        let owner = inherited.map_or(s.declaration, |v| v.owner);
        let private = inherited.map_or(s.piece.private, |v| v.private);
        s.piece.kind == Kind::Order
            || s.piece.global
            || from.inline_visible(db, owner, private, false).unwrap_or_else(|| match from {
                source_order::inline::Scope::File(from) => {
                    let owner = owner.file.file_id(db);
                    base_db::module_graph::can_reference_file(db, from, owner)
                        && (!private || base_db::module_graph::same_module(db, from, owner))
                }
                _ => true,
            })
    });
    result
}

fn merge(data: &mut TopLevelEnvData, selected: &Selected) {
    if selected.piece.kind == Kind::Closed {
        return;
    }
    let symbol = &selected.piece.value;
    let source = &symbol.data;
    // These collections grow at scattered clauses and overload extensions.
    if !matches!(selected.piece.kind, Kind::Members | Kind::Overload) {
        data.env.extend_symbols(source);
    }
    for (incoming, dest) in [
        (&source.enums, &mut data.env.enums),
        (&source.unions, &mut data.env.unions),
        (&source.overloads, &mut data.env.overloads),
    ] {
        for (name, entries) in incoming {
            let combined = dest.entry(name.clone()).or_default();
            for entry in entries {
                if !combined.contains(entry) {
                    combined.push(entry.clone());
                }
            }
        }
    }
    data.env.scattered_open_types.extend(source.scattered_open_types.iter().cloned());
    for fact in &source.global_constraints {
        if !data.env.global_constraints.contains(fact) {
            data.env.global_constraints.push(fact.clone());
        }
    }
    if selected.piece.kind == Kind::Order {
        data.env.vector_order = source.vector_order;
    }
    if symbol.pattern_constant {
        data.pattern_constants
            .extend(source.values.keys().chain(source.constructors.keys()).cloned());
    }
}

/// Infer an unannotated top-level value at its own declaration. Keeping the
/// result behind a type projection avoids invalidating users when an edit
/// changes its initializer without changing the exported type.
#[salsa::tracked]
fn inferred_value(db: &dyn salsa::Database, declaration: Declaration, name: Name) -> Option<Ty> {
    let id = hir_def::def_query::file_def_with_body_ids(db, declaration.file)
        .iter()
        .find(|&&id| {
            id.name(db) == name.as_str() && source_order::for_body(db, id) == Some(declaration)
        })
        .copied()?;
    let body = hir_def::def_query::callable_body(db, id).as_ref()?;
    crate::query::infer(db, id).0.type_of_expr.get(body.body.body.root()).cloned()
}

#[salsa::tracked(returns(ref))]
pub(crate) fn environment(
    db: &dyn salsa::Database,
    at: Declaration,
    mut names: Names,
) -> ArcTopLevelEnv {
    names.insert(Name::new(""));
    let mut pending: Vec<_> = names.into_iter().map(|name| (at, name, false)).collect();
    let mut visited = HashSet::new();
    let mut direct = Vec::new();
    let mut support_data = Vec::new();
    let mut body_metadata = HashSet::new();
    let inline =
        matches!(source_order::program(db, at.file), Some(source_order::Program::Inline(_)));
    let mut external_names = Names::new();
    while let Some((point, name, support)) = pending.pop() {
        if !visited.insert((point, name.clone(), support)) {
            continue;
        }
        db.unwind_if_revision_cancelled();
        if inline
            && base_db::WorkspaceFiles::try_get(db).is_some_and(|ws| {
                !directory(db, ws, source_order::Program::Inline(at.file)).names.contains_key(&name)
            })
        {
            external_names.insert(name.clone());
        }
        let mut derived = Names::new();
        mapping_base(&name, &mut derived);
        pending.extend(derived.into_iter().map(|name| (point, name, support)));
        // A signature keeps its definition-site type meaning, but its caller
        // also knows constraints and scattered members introduced before the
        // body. A later alias must not reinterpret an earlier signature, and
        // metadata after the body remains unavailable.
        if point != at && support && body_metadata.insert(name.clone()) {
            for selected in select_at(db, at, name.clone(), true).iter().filter(|selected| {
                matches!(selected.piece.kind, Kind::Fact | Kind::Members | Kind::Closed)
            }) {
                pending.extend(
                    selected
                        .piece
                        .value
                        .references
                        .iter()
                        .cloned()
                        .map(|name| (selected.declaration, name, true)),
                );
                support_data.push(selected.clone());
            }
        }
        for original in select_at(db, point, name, support) {
            let mut selected = original.clone();
            if !support && selected.declaration != at && selected.piece.kind == Kind::Binding {
                let missing: Vec<_> = selected
                    .piece
                    .value
                    .data
                    .values
                    .iter()
                    .filter(|(_, ty)| ty.is_error())
                    .map(|(name, _)| name.clone())
                    .collect();
                for name in missing {
                    if let Some(ty) = inferred_value(db, selected.declaration, Name::new(&name)) {
                        let symbol = Arc::make_mut(&mut selected.piece.value);
                        ty.references(&mut symbol.references);
                        symbol.data.values.insert(name, ty);
                    }
                }
            }
            let types = support || matches!(selected.piece.kind, Kind::Binding | Kind::Type);
            let origin = if types { selected.declaration } else { at };
            pending.extend(
                selected.piece.value.references.iter().cloned().map(|name| (origin, name, types)),
            );
            if support {
                support_data.push(selected);
            } else {
                direct.push(selected);
            }
        }
    }
    let mut data =
        TopLevelEnvData { env: TopLevelEnv::default(), pattern_constants: HashSet::new() };
    // Hidden type support is for interpreting exported signatures. Visible
    // declarations take precedence over this structural compatibility data.
    for selected in support_data.iter().chain(&direct) {
        merge(&mut data, selected);
    }
    // Hidden type support can contain the original open head. An end visible
    // at the body closes it even if that old head was merged again later.
    for selected in support_data.iter().chain(&direct).filter(|s| s.piece.kind == Kind::Closed) {
        for name in &selected.piece.value.data.scattered_open_types {
            data.env.scattered_open_types.remove(name);
        }
    }
    if inline {
        if let Some(ws) = base_db::WorkspaceFiles::try_get(db) {
            let local = directory(db, ws, source_order::Program::Inline(at.file));
            let external = super::index::select_workspace_excluding(
                db,
                ws,
                external_names,
                at.file.file_id(db),
                Some(at.file),
            );
            let mut context = super::super::WorkspaceContext::default();
            for (name, symbol) in &external.symbols {
                if !local.names.contains_key(name) {
                    context.extend_symbols(&symbol.data);
                }
            }
            context.apply_to(&mut data.env, &mut data.pattern_constants);
        }
    }
    // An unavailable alternative is not a missing signature. Leaving it in
    // the group made inference treat a complete visible set as partial and
    // silently suppress failed overload selection. Resolve nested groups here
    // through the same declaration identities used by IDE lookup.
    let groups: Vec<_> = data.env.overloads.keys().cloned().collect();
    for name in groups {
        let key = Name::new(&name);
        if source_order::bindings::contains_name(db, at, &key) {
            match source_order::bindings::overload_leaf_members(db, at, key) {
                Some(members) => {
                    data.env
                        .overloads
                        .insert(name, members.iter().map(ToString::to_string).collect());
                }
                None => {
                    data.env.overloads.remove(&name);
                }
            }
        }
    }
    data.env.has_workspace_context = true;
    data.env.report_unknown_calls = true;
    ArcTopLevelEnv(Arc::new(data))
}
