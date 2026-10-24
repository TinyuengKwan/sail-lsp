//! Names introduced by Sail declarations. Both definition indexing and type
//! inference use these descriptions; source positions remain on the owner.
use super::*;
use crate::type_ref::type_ref_from_text;
use syntax::{SyntaxKind as SK, SyntaxNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    EnumNumber,
    EnumFunction,
    EnumVector,
    /// Identity belongs to the head; availability and type are computed at end.
    ScatteredEnumVector,
    MappingFunction,
    BitfieldFunction,
    BitfieldOverload,
}

impl Kind {
    /// Initial_check creates enum callable specs with public annotations.
    /// The enum vector is checked in the owning declaration's visibility.
    pub fn visibility(
        self,
        owner: crate::visibility::RawVisibility,
    ) -> crate::visibility::RawVisibility {
        match self {
            Self::EnumNumber | Self::EnumFunction | Self::ScatteredEnumVector => {
                crate::visibility::RawVisibility::Public
            }
            _ => owner,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Origin {
    pub owner: DefItem,
    pub kind: Kind,
    /// Names explicitly chosen in attributes do not change with the owner.
    pub derived_name: bool,
}

pub struct Binding {
    pub name: String,
    /// A type scheme, without the `val name :` prefix; empty for overloads.
    pub type_text: String,
    pub kind: Kind,
    pub derived_name: bool,
    /// Members of a synthesized overload, empty for ordinary bindings.
    pub overload_members: Vec<String>,
}

/// Sail suppresses an enum's default/custom numeric conversion when an
/// explicit signature of that name occurs in the same preprocessed file.
pub fn explicit_val_names(
    nodes: impl Iterator<Item = SyntaxNode>,
) -> std::collections::HashSet<String> {
    nodes
        .filter(|n| n.kind() == SK::CALLABLE_SPEC)
        .filter_map(|n| syntax::definition_name(&n))
        .collect()
}

/// Attributes are separate siblings in the parser's lossless tree. Stop at
/// any intervening declaration/directive rather than borrowing a distant one.
fn attributes(node: &SyntaxNode) -> Vec<Vec<String>> {
    let mut result = Vec::new();
    let mut previous = node.prev_sibling();
    while let Some(attr) = previous.filter(|n| n.kind() == SK::ATTRIBUTE) {
        result.push(
            attr.descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .filter(|t| !t.kind().is_trivia())
                .map(|t| t.text().to_owned())
                .collect(),
        );
        previous = attr.prev_sibling();
    }
    result
}

fn attribute<'a>(attrs: &'a [Vec<String>], name: &str) -> Option<&'a [String]> {
    attrs.iter().find_map(|tokens| {
        // `$`, `[`, attribute name, payload..., `]`.
        (tokens.get(2).is_some_and(|s| s == name)).then(|| &tokens[3..tokens.len() - 1])
    })
}

fn identifier(value: &str) -> Option<String> {
    let value = value.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(value);
    (!value.is_empty()
        && value.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '#' | '?')))
    .then(|| value.to_owned())
}

pub fn enum_vector_name(node: &SyntaxNode) -> Option<String> {
    let attrs = attributes(node);
    let [value] = attribute(&attrs, "enum_vector")? else { return None };
    identifier(value)
}

/// Enum conversions and attribute bindings. Numeric conversions carry the
/// actual finite range rather than the coarse `int`/`nat` recovery types.
pub fn enum_bindings(node: &SyntaxNode, name: &str, members: usize) -> Vec<Binding> {
    let attrs = attributes(node);
    let mut result = Vec::new();
    if attribute(&attrs, "no_enum_number_conversions").is_none() {
        let names = match attribute(&attrs, "enum_number_conversions") {
            None => Some((format!("{name}_of_num"), format!("num_of_{name}"))),
            Some(payload) => {
                let field = |key| {
                    payload.windows(3).find_map(|ts| {
                        (ts[0] == key && ts[1] == "=").then(|| identifier(&ts[2])).flatten()
                    })
                };
                field("to_enum").zip(field("from_enum"))
            }
        };
        if let Some((to, from)) = names {
            let upper = members as i64 - 1;
            result.push(Binding {
                name: to,
                type_text: format!("forall ('e : Int), 0 <= 'e & 'e <= {upper}. int('e) -> {name}"),
                kind: Kind::EnumNumber,
                derived_name: attribute(&attrs, "enum_number_conversions").is_none(),
                overload_members: Vec::new(),
            });
            result.push(Binding {
                name: from,
                type_text: format!("{name} -> range(0, {upper})"),
                kind: Kind::EnumNumber,
                derived_name: attribute(&attrs, "enum_number_conversions").is_none(),
                overload_members: Vec::new(),
            });
        }
    }
    if let Some(payload) = attribute(&attrs, "enum_vector") {
        if let [value] = payload {
            if let Some(vector) = identifier(value) {
                result.push(Binding {
                    name: vector,
                    type_text: format!("vector({members}, {name})"),
                    kind: Kind::EnumVector,
                    derived_name: false,
                    overload_members: Vec::new(),
                });
            }
        }
    }
    // Keep return-type nesting when splitting the `with f -> T, ...` list.
    let tokens: Vec<_> = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    if let Some(start) = tokens.iter().position(|t| t.kind() == SK::KW_WITH) {
        let mut depth = 0usize;
        let mut begin = start + 1;
        for i in begin..tokens.len() {
            let kind = tokens[i].kind();
            if depth == 0 && matches!(kind, SK::COMMA | SK::EQ) {
                let clause = &tokens[begin..i];
                if clause.len() > 2 && clause[1].text() == "->" {
                    let lo = clause[2].text_range().start();
                    let hi = clause.last().unwrap().text_range().end();
                    let ty = node
                        .text()
                        .slice(lo - node.text_range().start()..hi - node.text_range().start())
                        .to_string();
                    result.push(Binding {
                        name: format!("{name}_{}", clause[0].text()),
                        type_text: format!("{name} -> {ty}"),
                        kind: Kind::EnumFunction,
                        derived_name: true,
                        overload_members: Vec::new(),
                    });
                }
                if kind == SK::EQ {
                    break;
                }
                begin = i + 1;
            }
            match kind {
                SK::L_PAREN | SK::L_CURLY | SK::L_BRACK => depth += 1,
                SK::R_PAREN | SK::R_CURLY | SK::R_BRACK => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    result
}

fn mapping_bindings(node: &SyntaxNode, name: &str) -> Vec<Binding> {
    let Some(bidir) = node.descendants().find(|n| {
        n.children_with_tokens().any(|t| t.as_token().is_some_and(|t| t.text() == "<->"))
    }) else {
        return vec![];
    };
    let sides: Vec<_> = bidir.children().filter(|n| n.kind().is_type_node()).collect();
    let [lhs, rhs] = sides.as_slice() else { return vec![] };
    let prefix = node
        .descendants()
        .find(|n| n.kind().is_type_node())
        .map(|root| {
            let start = root.text_range().start();
            let end = bidir.text_range().start();
            root.text().slice(..end - start).to_string()
        })
        .unwrap_or_default();
    [
        ("forwards", lhs, rhs.text().to_string()),
        ("backwards", rhs, lhs.text().to_string()),
        ("forwards_matches", lhs, "bool".into()),
        ("backwards_matches", rhs, "bool".into()),
    ]
    .into_iter()
    .map(|(suffix, arg, ret)| Binding {
        name: format!("{name}_{suffix}"),
        type_text: format!("{prefix}({}) -> {ret}", arg.text()),
        kind: Kind::MappingFunction,
        derived_name: true,
        overload_members: Vec::new(),
    })
    .collect()
}

pub fn bitfield_bindings(node: &SyntaxNode, name: &str) -> Vec<Binding> {
    let Some(underlying) = node.children().find(|n| n.kind().is_type_node()) else {
        return Vec::new();
    };
    let underlying = underlying.text().to_string();
    let function = |name, type_text| Binding {
        name,
        type_text,
        kind: Kind::BitfieldFunction,
        derived_name: true,
        overload_members: Vec::new(),
    };
    let mut result = vec![function(format!("Mk_{name}"), format!("{underlying} -> {name}"))];
    let fields = std::iter::once(("bits".to_owned(), underlying)).chain(
        crate::bitfield::field_widths(node)
            .into_iter()
            .filter(|(field, _)| field != "bits")
            .map(|(field, width)| (field, format!("bits({width})"))),
    );
    for (field, ty) in fields {
        let (_, get, update, set) = crate::bitfield::accessor_names(name, &field);
        result.push(function(get.clone(), format!("{name} -> {ty}")));
        result.push(function(update.clone(), format!("({name}, {ty}) -> {name}")));
        result.push(function(set.clone(), format!("(register({name}), {ty}) -> unit")));
        for (name, members) in
            [(format!("_mod_{field}"), vec![get, set]), (format!("update_{field}"), vec![update])]
        {
            result.push(Binding {
                name,
                type_text: String::new(),
                kind: Kind::BitfieldOverload,
                derived_name: false,
                overload_members: members,
            });
        }
    }
    result
}

pub(super) fn expand(root: &SyntaxNode, tree: &mut ItemTree, suppress_explicit: bool) {
    let originals = tree.top_level.clone();
    let nodes: std::collections::HashMap<_, _> =
        root.children().map(|node| (usize::from(node.text_range().start()), node)).collect();
    let explicit: std::collections::HashSet<_> = originals
        .iter()
        .filter(|item| {
            nodes.get(&item.span(tree).start).is_some_and(|node| node.kind() == SK::CALLABLE_SPEC)
        })
        .map(|item| item.name(tree).to_string())
        .collect();
    for owner in originals {
        let kind = owner.def_kind(tree);
        if !matches!(
            kind,
            DefKind::Enum
                | DefKind::ValSpec
                | DefKind::Mapping
                | DefKind::MappingSpec
                | DefKind::Bitfield
                | DefKind::ScatteredHead
        ) {
            continue;
        }
        let Some(node) = nodes.get(&owner.span(tree).start) else {
            continue;
        };
        let name = owner.name(tree).to_string();
        let bindings = if kind == DefKind::ScatteredHead {
            if owner.scattered_type_kind(tree) != Some(TypeDefKind::Enum) {
                continue;
            }
            enum_vector_name(node)
                .into_iter()
                .map(|name| Binding {
                    name,
                    type_text: String::new(),
                    kind: Kind::ScatteredEnumVector,
                    derived_name: false,
                    overload_members: Vec::new(),
                })
                .collect()
        } else if kind == DefKind::Enum {
            enum_bindings(&node, &name, owner.fields(tree).len())
        } else if kind == DefKind::Bitfield {
            bitfield_bindings(&node, &name)
        } else {
            mapping_bindings(&node, &name)
        };
        for binding in bindings {
            if suppress_explicit
                && binding.kind == Kind::EnumNumber
                && explicit.contains(&binding.name)
            {
                continue;
            }
            let span = owner.span(tree);
            let name_span = owner.name_span(tree);
            let visibility = binding.kind.visibility(owner.visibility(tree));
            let doc = owner.doc(tree).map(str::to_owned);
            let item = if binding.kind == Kind::BitfieldOverload {
                DefItem::Overload(tree.overloads.alloc(Overload {
                    name: Name::new(&binding.name),
                    signature: format!(
                        "overload {} = {{{}}}",
                        binding.name,
                        binding.overload_members.join(", ")
                    ),
                    type_ref: None,
                    span,
                    name_span,
                    visibility,
                }))
            } else if matches!(binding.kind, Kind::EnumVector | Kind::ScatteredEnumVector) {
                let signature = if binding.type_text.is_empty() {
                    format!("let {}", binding.name)
                } else {
                    format!("let {} : {}", binding.name, binding.type_text)
                };
                DefItem::Let(
                    tree.lets.alloc(LetDef {
                        name: Name::new(&binding.name),
                        type_ref: (!binding.type_text.is_empty())
                            .then(|| type_ref_from_text(&binding.type_text)),
                        signature,
                        span,
                        name_span,
                        is_var: false,
                        doc,
                        visibility,
                    }),
                )
            } else {
                let signature = format!("val {} : {}", binding.name, binding.type_text);
                DefItem::ValSpec(tree.val_specs.alloc(ValSpec {
                    name: Name::new(&binding.name),
                    type_ref: Some(type_ref_from_text(&binding.type_text)),
                    signature,
                    span,
                    name_span,
                    doc,
                    visibility,
                    signature_hash: 0,
                }))
            };
            tree.top_level.push(item);
            tree.generated.insert(
                item,
                Origin { owner, kind: binding.kind, derived_name: binding.derived_name },
            );
        }
    }
}
