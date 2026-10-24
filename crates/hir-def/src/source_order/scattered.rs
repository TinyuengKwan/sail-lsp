//! Scattered state is global and sequential; payload types retain the union
//! head's global environment with the clause's module scope. File-local
//! presence sets cannot express either rule.
use super::Program;
use base_db::{FileId, FileText, TextRange, WorkspaceFiles};
use std::collections::HashMap;
use syntax::{SyntaxKind as SK, SyntaxNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Function,
    Mapping,
    Union,
    Enum,
}
impl Kind {
    fn text(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Mapping => "mapping",
            Self::Union => "union",
            Self::Enum => "enum",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tag {
    Head(Kind),
    Clause(Kind),
    End,
    Type,
    Signature,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    tag: Tag,
    name: String,
    range: TextRange,
    private: bool,
    payload_types: Vec<String>,
    member_kind: Option<Kind>,
    enum_vector: Option<String>,
    members: Vec<(String, TextRange)>,
}
pub fn event(node: &SyntaxNode) -> Option<Event> {
    let has = |kind| node.children_with_tokens().any(|t| t.kind() == kind);
    let kind = if has(SK::KW_UNION) {
        Some(Kind::Union)
    } else if has(SK::KW_ENUM) {
        Some(Kind::Enum)
    } else if has(SK::KW_FUNCTION) {
        Some(Kind::Function)
    } else if has(SK::KW_MAPPING) {
        Some(Kind::Mapping)
    } else {
        None
    };
    let tag = match node.kind() {
        SK::SCATTERED_DEF => Tag::Head(kind?),
        SK::SCATTERED_CLAUSE_DEF => Tag::Clause(kind?),
        SK::CALLABLE_DEF if has(SK::KW_CLAUSE) => Tag::Clause(kind?),
        SK::END_DEF => Tag::End,
        SK::CALLABLE_SPEC => Tag::Signature,
        SK::TYPE_ALIAS_DEF => Tag::Type,
        SK::NAMED_DEF
            if [SK::KW_STRUCT, SK::KW_UNION, SK::KW_ENUM, SK::KW_BITFIELD, SK::KW_NEWTYPE]
                .into_iter()
                .any(has) =>
        {
            Tag::Type
        }
        _ => return None,
    };
    let name = syntax::definition_name(node)?;
    let span = syntax::definition_name_span(node)?;
    let range = base_db::text_range(span.start, span.end);
    let payload_types = if tag == Tag::Clause(Kind::Union) {
        node.descendants()
            .filter(|n| n.kind().is_type_node())
            .flat_map(|n| {
                n.descendants_with_tokens()
                    .filter_map(|t| t.into_token())
                    .filter(|t| t.kind() == SK::IDENT)
                    .map(|t| t.text().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        Vec::new()
    };
    let private = node.children().any(|child| child.kind() == SK::VISIBILITY);
    let member_kind = kind
        .filter(|kind| matches!(kind, Kind::Union | Kind::Enum))
        .or_else(|| has(SK::KW_NEWTYPE).then_some(Kind::Union));
    let members = if member_kind.is_some() {
        crate::item_tree::extract_field_decls(node)
            .into_iter()
            .map(|field| {
                (
                    field.name.to_string(),
                    base_db::text_range(field.name_span.start, field.name_span.end),
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let enum_vector = (tag == Tag::Head(Kind::Enum))
        .then(|| crate::item_tree::generated::enum_vector_name(node))
        .flatten();
    Some(Event { tag, name, range, private, payload_types, member_kind, members, enum_vector })
}
#[salsa::tracked(returns(ref))]
fn file_events(db: &dyn salsa::Database, file: FileText) -> Vec<Option<Event>> {
    syntax::parse_query::semantic_parse_file(db, file)
        .syntax_node()
        .into_iter()
        .flat_map(|root| root.children().map(|node| event(&node)).collect::<Vec<_>>())
        .collect()
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub context: usize,
    pub file: FileId,
    pub range: TextRange,
    pub code: &'static str,
    pub message: String,
    pub related: Option<(FileId, TextRange, String)>,
}
#[derive(Clone)]
struct Owner {
    context: usize,
    file: FileId,
    range: TextRange,
    private: bool,
}
struct Group {
    kind: Kind,
    owner: Owner,
    position: usize,
    clauses: usize,
    ended: Option<Owner>,
}

/// A declaration in an expansion, with an opaque scope/identity supplied by
/// the caller. Preview checks use visit IDs; database checks use ordinals.
#[derive(Clone, Debug)]
pub struct EventAt {
    pub file: FileId,
    pub context: usize,
    pub event: Event,
}

/// A generated value has a stable identity at the attribute's head, but only
/// enters the local environment when that group's end is checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumVector {
    pub head: super::Declaration,
    pub end: super::Declaration,
    pub name: String,
    pub enum_name: String,
    pub members: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnumVectorAt {
    pub head: usize,
    pub end: usize,
    pub name: String,
    pub enum_name: String,
    pub members: usize,
}

/// Shared by actual programs and include previews. Context IDs are opaque;
/// the caller supplies module ownership in its own expansion.
pub fn completed_enum_vectors(
    events: &[EventAt],
    visible: impl Fn(usize, usize, bool) -> bool,
) -> Vec<EnumVectorAt> {
    let mut groups = HashMap::new();
    let mut result = Vec::new();
    for input in events {
        let at = input.context;
        let event = &input.event;
        match event.tag {
            Tag::Head(Kind::Enum) => {
                groups.entry(event.name.clone()).or_insert((
                    at,
                    event.private,
                    event.enum_vector.clone(),
                    std::collections::HashSet::<String>::new(),
                    false,
                ));
            }
            Tag::Clause(Kind::Enum) => {
                if let Some((head, private, _, members, ended)) = groups.get_mut(&event.name) {
                    if !*ended && visible(*head, at, *private) {
                        members.extend(event.members.iter().map(|(name, _)| name.clone()));
                    }
                }
            }
            Tag::End => {
                if let Some((head, private, name, members, ended)) = groups.get_mut(&event.name) {
                    if !*ended && !members.is_empty() && visible(*head, at, *private) {
                        if let Some(name) = name {
                            result.push(EnumVectorAt {
                                head: *head,
                                end: at,
                                name: name.clone(),
                                enum_name: event.name.clone(),
                                members: members.len(),
                            });
                        }
                    }
                    *ended = true;
                }
            }
            _ => {}
        }
    }
    result
}

pub fn enum_vectors(db: &dyn salsa::Database, program: Program) -> &[EnumVector] {
    let Some(files) = WorkspaceFiles::try_get(db) else { return &[] };
    enum_vectors_impl(db, files, program)
}

#[salsa::tracked(returns(ref))]
fn enum_vectors_impl(
    db: &dyn salsa::Database,
    _files: WorkspaceFiles,
    program: Program,
) -> Vec<EnumVector> {
    let declarations = program.declarations(db);
    let events: Vec<_> = declarations
        .iter()
        .enumerate()
        .filter_map(|(context, at)| {
            db.unwind_if_revision_cancelled();
            Some(EventAt {
                context,
                file: at.file.file_id(db),
                event: file_events(db, at.file).get(at.index)?.clone()?,
            })
        })
        .collect();
    completed_enum_vectors(&events, |head, from, private| {
        super::inline::declaration_visible(
            db,
            declarations[from],
            declarations[head],
            private,
            false,
        )
    })
    .into_iter()
    .map(|v| EnumVector {
        head: declarations[v.head],
        end: declarations[v.end],
        name: v.name,
        enum_name: v.enum_name,
        members: v.members,
    })
    .collect()
}

/// An `end` refers to the group that was opened earlier, including implicit
/// callable groups. It does not introduce another declaration of that name.
pub fn end_origin(db: &dyn salsa::Database, at: super::Declaration) -> Option<super::Declaration> {
    end_origin_impl(db, WorkspaceFiles::try_get(db)?, at)
}

/// A callable head is a group marker, not a value binding. It can precede
/// its first signature. Associate the marker with that signature for identity
/// operations without exposing the signature to earlier expressions.
pub fn callable_head_signature(
    db: &dyn salsa::Database,
    head: super::Declaration,
) -> Option<super::Declaration> {
    callable_head_signature_impl(db, WorkspaceFiles::try_get(db)?, head)
}

#[salsa::tracked]
fn callable_head_signature_impl(
    db: &dyn salsa::Database,
    _files: WorkspaceFiles,
    head: super::Declaration,
) -> Option<super::Declaration> {
    let event = file_events(db, head.file).get(head.index)?.as_ref()?;
    if !matches!(event.tag, Tag::Head(Kind::Function | Kind::Mapping)) {
        return None;
    }
    let mut signature = None;
    let mut after_head = false;
    for &at in super::program(db, head.file)?.declarations(db) {
        db.unwind_if_revision_cancelled();
        if at == head {
            if signature.is_some() {
                return signature;
            }
            after_head = true;
            continue;
        }
        let Some(Some(candidate)) = file_events(db, at.file).get(at.index) else { continue };
        if candidate.name != event.name {
            continue;
        }
        if candidate.tag == Tag::Signature {
            if after_head {
                return Some(at);
            }
            signature = Some(at);
        } else if after_head && matches!(candidate.tag, Tag::Clause(_) | Tag::End) {
            return None;
        }
    }
    None
}

#[salsa::tracked]
fn end_origin_impl(
    db: &dyn salsa::Database,
    _files: WorkspaceFiles,
    at: super::Declaration,
) -> Option<super::Declaration> {
    let program = super::program(db, at.file)?;
    let mut groups = HashMap::new();
    let mut signatures = HashMap::new();
    for &declaration in program.declarations(db) {
        db.unwind_if_revision_cancelled();
        let Some(Some(event)) = file_events(db, declaration.file).get(declaration.index) else {
            if declaration == at {
                return None;
            }
            continue;
        };
        if declaration == at {
            if event.tag != Tag::End {
                return None;
            }
            let &(kind, owner, private) = groups.get(&event.name)?;
            return (kind != Kind::Enum
                || super::inline::declaration_visible(db, at, owner, private, false))
            .then_some(owner);
        }
        match event.tag {
            Tag::Head(kind) => {
                groups.entry(event.name.clone()).or_insert((kind, declaration, event.private));
            }
            Tag::Signature => {
                signatures.insert(event.name.clone(), (declaration, event.private));
            }
            Tag::Clause(kind @ (Kind::Function | Kind::Mapping)) => {
                if let Some(&(signature, private)) = signatures.get(&event.name) {
                    groups.entry(event.name.clone()).or_insert((kind, signature, private));
                }
            }
            _ => {}
        }
    }
    None
}

/// The same state machine checks real programs and hypothetical include edits.
/// Visibility is supplied by the caller's actual expansion/module context.
pub fn validate(events: &[EventAt], visible: impl Fn(usize, usize, bool) -> bool) -> Vec<Issue> {
    let mut result = Vec::new();
    let mut groups: HashMap<String, Group> = HashMap::new();
    let mut types: HashMap<String, (usize, Owner)> = HashMap::new();
    let mut signatures: HashMap<String, Owner> = HashMap::new();
    // Union constructors share the value namespace; enum duplicate checks are
    // per enumeration, as in Sail's add_enum_clause.
    let mut members: HashMap<(Option<String>, String), Owner> = HashMap::new();
    // Retain later type declarations too: a payload may precede its type,
    // which a whole-workspace name-presence check would otherwise accept.
    let future_types: HashMap<_, _> = events
        .iter()
        .filter_map(|input| {
            let event = &input.event;
            matches!(event.tag, Tag::Type | Tag::Head(Kind::Union | Kind::Enum)).then(|| {
                (
                    event.name.clone(),
                    Owner {
                        context: input.context,
                        file: input.file,
                        range: event.range,
                        private: event.private,
                    },
                )
            })
        })
        .collect();
    for (position, input) in events.iter().enumerate() {
        let event = &input.event;
        let file = input.file;
        let context = input.context;
        let owner = Owner { context, file, range: event.range, private: event.private };
        let mut report = |range, code, message, related: Option<&Owner>| {
            result.push(Issue {
                context,
                file,
                range,
                code,
                message,
                related: related
                    .map(|p| (p.file, p.range, "previous declaration or end here".into())),
            })
        };
        match event.tag {
            Tag::Type => {
                types.insert(event.name.clone(), (position, owner.clone()));
                if let Some(kind) = event.member_kind {
                    for (name, range) in &event.members {
                        let group = (kind == Kind::Enum).then(|| event.name.clone());
                        members
                            .entry((group, name.clone()))
                            .or_insert_with(|| Owner { range: *range, ..owner.clone() });
                    }
                }
            }
            Tag::Signature => {
                signatures.insert(event.name.clone(), owner);
            }
            Tag::Head(kind) => {
                if matches!(kind, Kind::Union | Kind::Enum) {
                    if let Some((_, previous)) = types.get(&event.name) {
                        report(
                            event.range,
                            "duplicate-scattered-head",
                            format!(
                                "Cannot create {} type {}, name is already bound",
                                if kind == Kind::Union { "scattered union" } else { "enum" },
                                event.name
                            ),
                            Some(previous),
                        );
                        continue;
                    }
                    types.insert(event.name.clone(), (position, owner.clone()));
                }
                // A repeated head never reopens an ended scattered ID.
                groups.entry(event.name.clone()).or_insert(Group {
                    kind,
                    owner,
                    position,
                    clauses: 0,
                    ended: None,
                });
            }
            Tag::End => {
                if let Some(group) = groups.get_mut(&event.name) {
                    if let Some(end) = &group.ended {
                        report(
                            event.range,
                            "scattered-double-end",
                            format!(
                                "Cannot end scattered definition {} as it has already been ended",
                                event.name
                            ),
                            Some(end),
                        );
                    } else {
                        // Sail's end_scattered_id checks visibility only when
                        // retrieving enum members; union/function ends are global.
                        if group.kind == Kind::Enum
                            && !visible(group.owner.context, context, group.owner.private)
                        {
                            report(
                                event.range,
                                "scattered-not-visible",
                                format!("Enumeration {} is not in scope", event.name),
                                Some(&group.owner),
                            );
                        }
                        group.ended = Some(owner);
                    }
                } else {
                    report(
                        event.range,
                        "invalid-scattered-end",
                        format!(
                            "{} is not a scattered definition, so it cannot be ended",
                            event.name
                        ),
                        None,
                    );
                }
            }
            Tag::Clause(kind) => {
                if let Some(end) = groups.get(&event.name).and_then(|g| g.ended.as_ref()) {
                    report(
                        event.range,
                        "scattered-clause-after-end",
                        format!(
                            "Cannot add clause to scattered {} {} as it has already been ended",
                            kind.text(),
                            event.name
                        ),
                        Some(end),
                    );
                    continue;
                }
                if matches!(kind, Kind::Function | Kind::Mapping) {
                    // A callable clause may open a group implicitly, but its
                    // signature must already exist and be visible here.
                    let Some(signature) = signatures.get(&event.name) else {
                        report(
                            event.range,
                            "scattered-missing-signature",
                            format!(
                                "No preceding signature for scattered {} {}",
                                kind.text(),
                                event.name
                            ),
                            None,
                        );
                        continue;
                    };
                    if !visible(signature.context, context, signature.private) {
                        report(
                            event.range,
                            "scattered-not-visible",
                            format!("{} is not in scope", event.name),
                            Some(signature),
                        );
                        continue;
                    }
                    groups
                        .entry(event.name.clone())
                        .or_insert(Group {
                            kind,
                            owner: signature.clone(),
                            position,
                            clauses: 0,
                            ended: None,
                        })
                        .clauses += 1;
                } else {
                    let Some(group) = groups.get_mut(&event.name).filter(|g| g.kind == kind) else {
                        report(
                            event.range,
                            "scattered-missing-head",
                            format!(
                                "{} has no preceding scattered {} declaration",
                                event.name,
                                kind.text()
                            ),
                            None,
                        );
                        continue;
                    };
                    if !visible(group.owner.context, context, group.owner.private) {
                        report(
                            event.range,
                            "scattered-not-visible",
                            format!("{} {} is not in scope", kind.text(), event.name),
                            Some(&group.owner),
                        );
                        continue;
                    }
                    group.clauses += 1;
                    for (name, range) in &event.members {
                        let key = ((kind == Kind::Enum).then(|| event.name.clone()), name.clone());
                        if let Some(previous) = members.get(&key) {
                            let message = if kind == Kind::Enum {
                                format!("Enumeration {} already has a member {name}", event.name)
                            } else {
                                format!("Cannot create union constructor {name}, name is already bound to a union constructor")
                            };
                            report(
                                if kind == Kind::Enum { event.range } else { *range },
                                "duplicate-scattered-member",
                                message,
                                Some(previous),
                            );
                        } else {
                            members.insert(key, Owner { range: *range, ..owner.clone() });
                        }
                    }
                    for name in &event.payload_types {
                        if let Some((declared, ty)) = types.get(name) {
                            // get_scattered_variant_env restores only the global
                            // bindings, retaining the clause's current module scope.
                            if *declared >= group.position
                                || !visible(ty.context, context, ty.private)
                            {
                                report(event.range, "scattered-forward-type", format!("Type {name} is not visible in the environment of scattered union {}", event.name), Some(ty));
                                break;
                            }
                        } else if let Some(ty) = future_types.get(name) {
                            report(event.range, "scattered-forward-type", format!("Type {name} is declared after the environment of scattered union {}", event.name), Some(ty));
                            break;
                        }
                    }
                }
            }
        }
    }
    for (name, group) in groups {
        if matches!(group.kind, Kind::Union | Kind::Enum) && group.clauses == 0 {
            result.push(Issue {
                context: group.owner.context,
                file: group.owner.file,
                range: group.owner.range,
                code: "empty-scattered-definition",
                message: format!(
                    "No clauses found for scattered {} type {name}",
                    group.kind.text()
                ),
                related: None,
            });
        }
    }
    result.sort_by_key(|i| (i.file, i.range.start(), i.range.end(), i.code));
    result
}

/// Declaration-only projection; body edits can backdate the validation result.
#[salsa::tracked(returns(ref))]
pub fn issues(
    db: &dyn salsa::Database,
    _workspace: WorkspaceFiles,
    program: Program,
) -> Vec<Issue> {
    let declarations = program.declarations(db);
    let mut events = Vec::new();
    for (context, declaration) in declarations.iter().enumerate() {
        db.unwind_if_revision_cancelled();
        if let Some(Some(event)) = file_events(db, declaration.file).get(declaration.index) {
            events.push(EventAt {
                file: declaration.file.file_id(db),
                context,
                event: event.clone(),
            });
        }
    }
    validate(&events, |owner, from, private| {
        super::inline::declaration_visible(
            db,
            declarations[from],
            declarations[owner],
            private,
            false,
        )
    })
}
