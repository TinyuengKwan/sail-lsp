//! Binding identity at an expanded declaration. Later signatures
//! replace the binding before privacy is checked; an inaccessible replacement
//! must not reveal an older signature of the same name.
use super::{Declaration, FileText, Program, WorkspaceFiles};
use crate::Name;
use std::collections::HashMap;
use syntax::SyntaxKind as SK;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Namespace {
    Callable,
    Type,
    Value,
    /// Instantiation selects an outcome independently of same-named values.
    Outcome,
}

/// Resolve implicit assignment declarations against the value namespace before
/// checking accessibility: a hidden global cannot become a new mutable local.
pub fn body_scopes(
    db: &dyn salsa::Database,
    file: FileText,
    body: &crate::Body,
    offset: usize,
) -> crate::expr_store::scope::ExprScopes {
    if super::program(db, file).is_some() {
        return super::at_offset(db, file, offset).map_or_else(
            || crate::expr_store::scope::ExprScopes::new(body),
            |at| body_scopes_at(db, at, body),
        );
    }
    crate::expr_store::scope::ExprScopes::new_with_globals(body, |name| {
        let candidates = WorkspaceFiles::try_get(db)
            .map(|ws| ws.file_texts(db).clone())
            .unwrap_or_else(|| vec![file]);
        candidates.into_iter().any(|candidate| {
            file_events(db, candidate)
                .iter()
                .flatten()
                .any(|event| event.namespace == Namespace::Value && &event.name == name)
        })
    })
}

/// Use a stable declaration identity when a cached body's offsets are rebased.
pub fn body_scopes_at(
    db: &dyn salsa::Database,
    at: Declaration,
    body: &crate::Body,
) -> crate::expr_store::scope::ExprScopes {
    crate::expr_store::scope::ExprScopes::new_with_globals(body, |name| {
        exists_at(db, at, name.clone(), Namespace::Value)
    })
}

/// Existence is independent of visibility. Keep this query name-specific so
/// unrelated declaration edits do not invalidate a body's binding decisions.
#[salsa::tracked]
pub fn exists_at(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
    namespace: Namespace,
) -> bool {
    select_binding(db, at, name, namespace, true).is_some()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Event {
    name: Name,
    namespace: Namespace,
    replaces: bool,
    requires_signature: bool,
    private: bool,
    recursive: bool,
    global: bool,
    enum_parent: Option<Name>,
    visibility_owner: Option<Declaration>,
}

#[salsa::tracked(returns(ref))]
fn file_events(db: &dyn salsa::Database, file: FileText) -> Vec<Vec<Event>> {
    let Some(root) = syntax::parse_query::semantic_parse_file(db, file).syntax_node() else {
        return Vec::new();
    };
    let tree = crate::def_query::file_item_tree(db, file);
    let explicit_vals = crate::item_tree::generated::explicit_val_names(root.children());
    let mut items: Vec<_> = tree
        .iter()
        .flat_map(|tree| {
            tree.top_level_items().iter().map(|&item| (item.name_span(tree).start, item))
        })
        .collect();
    items.sort_by_key(|(start, _)| *start);
    root.children()
        .map(|node| {
            let has = |kind| node.children_with_tokens().any(|t| t.kind() == kind);
            let (namespace, replaces) = match node.kind() {
                SK::CALLABLE_SPEC | SK::OUTCOME_DEF => (Namespace::Callable, true),
                SK::NAMED_DEF if has(SK::KW_OVERLOAD) => (Namespace::Callable, true),
                SK::CALLABLE_DEF => (Namespace::Callable, false),
                SK::SCATTERED_DEF | SK::SCATTERED_CLAUSE_DEF
                    if has(SK::KW_FUNCTION) || has(SK::KW_MAPPING) =>
                {
                    (Namespace::Callable, false)
                }
                SK::TYPE_ALIAS_DEF => (Namespace::Type, true),
                SK::NAMED_DEF | SK::SCATTERED_DEF
                    if [
                        SK::KW_STRUCT,
                        SK::KW_ENUM,
                        SK::KW_UNION,
                        SK::KW_BITFIELD,
                        SK::KW_NEWTYPE,
                    ]
                    .into_iter()
                    .any(has) =>
                {
                    (Namespace::Type, true)
                }
                SK::NAMED_DEF if [SK::KW_LET, SK::KW_VAR, SK::KW_REGISTER].into_iter().any(has) => {
                    (Namespace::Value, true)
                }
                _ => (Namespace::Value, false),
            };
            let private = node.children().any(|c| c.kind() == SK::VISIBILITY);
            let mut events = Vec::new();
            if namespace != Namespace::Value || replaces {
                if let Some(name) = syntax::definition_name(&node) {
                    events.push(Event {
                        name: Name::new(name.trim_start_matches('\'')),
                        namespace,
                        replaces,
                        requires_signature: matches!(
                            node.kind(),
                            SK::SCATTERED_DEF | SK::SCATTERED_CLAUSE_DEF
                        ) && namespace == Namespace::Callable,
                        private,
                        recursive: namespace == Namespace::Callable,
                        global: node.kind() == SK::NAMED_DEF && has(SK::KW_OVERLOAD),
                        enum_parent: None,
                        visibility_owner: None,
                    });
                }
            }
            if namespace == Namespace::Callable {
                if let Some(event) = events.first().cloned() {
                    if node.kind() == SK::OUTCOME_DEF {
                        events.push(Event {
                            namespace: Namespace::Outcome,
                            recursive: false,
                            ..event.clone()
                        });
                    }
                    events.push(Event { namespace: Namespace::Value, ..event });
                }
            }
            // Constructors are values introduced by their containing type or
            // scattered clause. Project only names/ownership, never payload
            // types or byte offsets, into the workspace binding index.
            if let Some(tree) = tree {
                let start = usize::from(node.text_range().start());
                let end = usize::from(node.text_range().end());
                let lo = items.partition_point(|(at, _)| *at < start);
                let hi = items.partition_point(|(at, _)| *at < end);
                for &(_, item) in &items[lo..hi] {
                    if tree.generated.get(&item).is_some_and(|origin| {
                        origin.kind == crate::item_tree::generated::Kind::ScatteredEnumVector
                    }) {
                        continue; // Introduced by the program's end, not this head.
                    }
                    if tree.generated.get(&item).is_some_and(|origin| {
                        origin.kind == crate::item_tree::generated::Kind::EnumNumber
                            && explicit_vals.contains(item.name(tree).as_str())
                    }) {
                        continue;
                    }
                    let generated = tree.generated.contains_key(&item);
                    if generated {
                        let namespaces = if item.def_kind(tree) == crate::DefKind::Let {
                            &[Namespace::Value][..]
                        } else {
                            &[Namespace::Callable, Namespace::Value][..]
                        };
                        for &namespace in namespaces {
                            events.push(Event {
                                name: item.name(tree).clone(),
                                namespace,
                                replaces: true,
                                requires_signature: false,
                                private: item.visibility(tree)
                                    == crate::visibility::RawVisibility::Private,
                                recursive: false,
                                global: item.def_kind(tree) == crate::DefKind::Overload,
                                enum_parent: None,
                                visibility_owner: None,
                            });
                        }
                    }
                    if matches!(
                        item.def_kind(tree),
                        crate::DefKind::Enum | crate::DefKind::Union | crate::DefKind::Newtype
                    ) {
                        events.extend(item.fields(tree).iter().map(|field| Event {
                            name: field.name.clone(),
                            namespace: Namespace::Value,
                            replaces: true,
                            requires_signature: false,
                            private: private
                                || field.visibility == crate::visibility::RawVisibility::Private,
                            recursive: false,
                            global: false,
                            enum_parent: None,
                            visibility_owner: None,
                        }));
                    }
                    if let Some((name, _)) = item.scattered_member(tree) {
                        let member_private = item.fields(tree).iter().any(|field| {
                            field.name.as_str() == name
                                && field.visibility == crate::visibility::RawVisibility::Private
                        });
                        events.push(Event {
                            name: Name::new(name),
                            namespace: Namespace::Value,
                            replaces: true,
                            requires_signature: false,
                            private: private || member_private,
                            recursive: false,
                            global: false,
                            enum_parent: has(SK::KW_ENUM).then(|| item.name(tree).clone()),
                            visibility_owner: None,
                        });
                    }
                }
            }
            events
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Index {
    positions: HashMap<Declaration, usize>,
    bindings: HashMap<(Name, Namespace), Vec<(usize, Declaration, Event)>>,
}

#[salsa::tracked(returns(ref))]
fn index(db: &dyn salsa::Database, _files: WorkspaceFiles, program: Program) -> Index {
    let mut result = Index { positions: HashMap::new(), bindings: HashMap::new() };
    let vectors: HashMap<_, _> =
        super::scattered::enum_vectors(db, program).iter().map(|v| (v.end, v)).collect();
    for (position, &declaration) in program.declarations(db).iter().enumerate() {
        db.unwind_if_revision_cancelled();
        result.positions.insert(declaration, position);
        if let Some(vector) = vectors.get(&declaration) {
            let event = Event {
                name: Name::new(&vector.name),
                namespace: Namespace::Value,
                replaces: true,
                requires_signature: false,
                private: false,
                recursive: false,
                global: true,
                enum_parent: None,
                visibility_owner: None,
            };
            result.bindings.entry((event.name.clone(), event.namespace)).or_default().push((
                position,
                vector.head,
                event,
            ));
        }
        for event in file_events(db, declaration.file).get(declaration.index).into_iter().flatten()
        {
            let mut event = event.clone();
            if let Some(parent) = &event.enum_parent {
                if let Some((_, head, head_event)) =
                    result.bindings.get(&(parent.clone(), Namespace::Type)).and_then(|xs| xs.last())
                {
                    // Enum membership is stored on the head's environment
                    // item. Its source identity and availability remain at
                    // the clause, even when the clause is in another module.
                    event.private = head_event.private;
                    event.visibility_owner = Some(head_event.visibility_owner.unwrap_or(*head));
                }
            }
            result.bindings.entry((event.name.clone(), event.namespace)).or_default().push((
                position,
                declaration,
                event,
            ));
        }
    }
    result
}

/// Resolve a binding at a declaration. Callables include their own signature
/// for recursion; a type's RHS sees only preceding types (Sail forbids recursive
/// types). Available only for configured, ordered source projects.
#[salsa::tracked]
pub fn resolve(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
    namespace: Namespace,
) -> Option<Declaration> {
    resolve_at(db, at, name, namespace, true)
}

/// Declaration checks run before introducing the new binding. In particular,
/// an overload cannot use itself to satisfy its own member list.
#[salsa::tracked]
pub fn resolve_before(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
    namespace: Namespace,
) -> Option<Declaration> {
    resolve_at(db, at, name, namespace, false)
}

fn resolve_at(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
    namespace: Namespace,
    include_recursive: bool,
) -> Option<Declaration> {
    let (declaration, event) = select_binding(db, at, name, namespace, include_recursive)?;
    let visible = super::inline::declaration_visible(
        db,
        at,
        event.visibility_owner.unwrap_or(declaration),
        event.private,
        event.global,
    );
    visible.then_some(declaration)
}

/// Binding privacy can belong to a different declaration than its source
/// identity, notably a scattered enum member added in another module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingVisibility {
    pub owner: Declaration,
    pub private: bool,
    pub global: bool,
}

#[salsa::tracked]
pub fn declaration_visibility(
    db: &dyn salsa::Database,
    declaration: Declaration,
    name: Name,
    namespace: Namespace,
) -> Option<BindingVisibility> {
    let program = super::program(db, declaration.file)?;
    let files = WorkspaceFiles::try_get(db)?;
    let (_, _, event) = index(db, files, program)
        .bindings
        .get(&(name, namespace))?
        .iter()
        .rev()
        .find(|(_, origin, _)| *origin == declaration)?;
    Some(BindingVisibility {
        owner: event.visibility_owner.unwrap_or(declaration),
        private: event.private,
        global: event.global,
    })
}

fn select_binding(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
    namespace: Namespace,
    include_recursive: bool,
) -> Option<(Declaration, Event)> {
    let index = index(db, WorkspaceFiles::try_get(db)?, super::program(db, at.file)?);
    let &position = index.positions.get(&at)?;
    let events = index.bindings.get(&(name, namespace))?;
    let mut binding = None;
    for (declared, declaration, event) in events {
        if *declared > position
            || (*declared == position && !(event.recursive && include_recursive))
        {
            break;
        }
        if (event.replaces || binding.is_none()) && (!event.requires_signature || binding.is_some())
        {
            binding = Some((*declaration, event.clone()));
        }
    }
    binding
}

/// Declaration existence without scope filtering, for Sail's overload
/// construction rules. This is deliberately not reference resolution: the
/// compiler permits an overload to list a previously declared hidden member,
/// and checks that member's visibility only when the group is used.
#[salsa::tracked]
pub fn declared_before(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
    namespace: Namespace,
) -> Option<Declaration> {
    select_binding(db, at, name, namespace, false).map(|(declaration, _)| declaration)
}

/// Completion shares the same binding selection as individual references.
/// The names-only result backdates when bodies or signature types change.
#[salsa::tracked(returns(ref))]
pub fn names_at(db: &dyn salsa::Database, file: FileText, ordinal: usize) -> Vec<Name> {
    let at = Declaration { file, index: ordinal };
    let Some(program) = super::program(db, file) else { return Vec::new() };
    let Some(files) = WorkspaceFiles::try_get(db) else { return Vec::new() };
    let mut names: Vec<_> = index(db, files, program)
        .bindings
        .keys()
        .filter(|(name, ns)| resolve(db, at, name.clone(), *ns).is_some())
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Name existence in this program, including later or inaccessible declarations.
/// Such a name must not fall back to an unrelated workspace declaration.
pub fn contains_name(db: &dyn salsa::Database, at: Declaration, name: &Name) -> bool {
    let Some(files) = WorkspaceFiles::try_get(db) else { return false };
    super::program(db, at.file).is_some_and(|program| {
        let index = index(db, files, program);
        [Namespace::Callable, Namespace::Type, Namespace::Value, Namespace::Outcome]
            .into_iter()
            .any(|ns| index.bindings.contains_key(&(name.clone(), ns)))
    })
}

/// Candidate declarations in the use site's program for identity-based
/// implementation searches, including inline programs.
pub fn declarations_named_at(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
    namespace: Namespace,
) -> Vec<Declaration> {
    let Some(program) = super::program(db, at.file) else { return Vec::new() };
    let Some(files) = WorkspaceFiles::try_get(db) else { return Vec::new() };
    index(db, files, program)
        .bindings
        .get(&(name, namespace))
        .into_iter()
        .flatten()
        .map(|(_, decl, _)| *decl)
        .collect()
}

/// Overload declarations extend their preceding group. Select the current
/// binding first (including privacy), then collect preceding extensions and
/// filter their members at the use site, regardless of the extension module. The name projection backdates unrelated body edits.
#[salsa::tracked(returns(ref))]
pub fn overload_members(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
) -> Option<Vec<Name>> {
    let selected = resolve(db, at, name.clone(), Namespace::Callable)?;
    declaration_overload_members(db, selected, &name)?;
    let index = index(db, WorkspaceFiles::try_get(db)?, super::program(db, at.file)?);
    let position = *index.positions.get(&at)?;
    let mut members = Vec::new();
    for (declared, declaration, _) in index.bindings.get(&(name.clone(), Namespace::Callable))? {
        if *declared > position {
            break;
        }
        for member in declaration_overload_members(db, *declaration, &name).into_iter().flatten() {
            if !members.contains(&member)
                && resolve(db, at, member.clone(), Namespace::Value).is_some()
            {
                members.push(member);
            }
        }
    }
    Some(members)
}

/// Callable alternatives after Sail's `get_overloads_recursive`: expand one
/// indirect group level. A remaining group is not a function signature and
/// makes the call invalid, even if another alternative is callable. Returning
/// an empty set keeps inference and IDE consumers from accepting deeper trees.
pub fn overload_leaf_members(
    db: &dyn salsa::Database,
    at: Declaration,
    name: Name,
) -> Option<Vec<Name>> {
    let mut seen = std::collections::HashSet::new();
    let mut leaves = Vec::new();
    for member in overload_members(db, at, name).as_ref()? {
        if let Some(children) = overload_members(db, at, member.clone()) {
            for child in children {
                if overload_members(db, at, child.clone()).is_some() {
                    return Some(Vec::new());
                }
                if seen.insert(child.clone()) {
                    leaves.push(child.clone());
                }
            }
        } else if seen.insert(member.clone()) {
            leaves.push(member.clone());
        }
    }
    Some(leaves)
}

fn declaration_overload_members(
    db: &dyn salsa::Database,
    declaration: Declaration,
    name: &Name,
) -> Option<Vec<Name>> {
    let node = super::node(db, declaration)?;
    let tree = crate::def_query::file_item_tree(db, declaration.file).as_ref()?;
    let item = tree.top_level_items().iter().find(|item| {
        item.def_kind(tree) == crate::DefKind::Overload
            && item.name(tree) == name
            && node.text_range().contains((item.name_span(tree).start as u32).into())
    })?;
    // Explicit overloads retain their member list in the CST; generated ones
    // carry it in the virtual declaration, whose source node is the bitfield.
    let signature = if tree.generated.contains_key(item) {
        item.signature(tree).to_owned()
    } else {
        node.text().to_string()
    };
    let mut after_equals = false;
    Some(
        parser::tokenize(&signature)
            .into_iter()
            .filter_map(|(token, span)| {
                if SK::from_token(&token) == SK::EQ {
                    after_equals = true;
                    None
                } else if after_equals && SK::from_token(&token) == SK::IDENT {
                    Some(Name::new(&signature[span.start..span.end]))
                } else {
                    None
                }
            })
            .collect(),
    )
}
