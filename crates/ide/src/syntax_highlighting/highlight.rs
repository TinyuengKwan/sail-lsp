//! Core highlighting algorithm.

use super::tags::token_tag;
use super::*;
use hir::DefKind;

/// Build a name → tag map from ItemTree for type-driven classification.
///
/// This lets us classify identifier references (not just declarations) correctly,
/// distinguishing structs from type aliases, enum members from enum types,
/// registers from local variables, etc.
pub(super) fn build_name_classification(
    file: &dyn FileDb,
) -> std::collections::HashMap<String, HlTag> {
    let mut map = std::collections::HashMap::new();
    let Some(item_tree) = file.item_tree() else {
        return map;
    };
    for &id in item_tree.top_level_items() {
        let kind = id.def_kind(item_tree);
        let tag = match kind {
            DefKind::Function | DefKind::Mapping | DefKind::ValSpec | DefKind::MappingSpec => {
                HlTag::Function
            }
            DefKind::TypeAlias | DefKind::Newtype => HlTag::Type,
            DefKind::Struct => HlTag::Struct,
            DefKind::Union => HlTag::Type,
            DefKind::Bitfield => HlTag::Struct,
            DefKind::Enum => HlTag::Enum,
            DefKind::Register => HlTag::Register,
            DefKind::Let => HlTag::Variable,
            DefKind::Var => HlTag::Variable,
            _ => continue,
        };
        map.insert(id.name(item_tree).as_str().to_string(), tag);
    }
    // Also classify enum members as enumMember.
    // ItemTree entries record enum type names; we need to look at
    // the signature text for member names.
    for &id in item_tree.top_level_items() {
        if id.def_kind(item_tree) == DefKind::Enum {
            // Parse enum members from signature: "enum Foo = { A, B, C }"
            let sig = id.signature(item_tree);
            if let Some(brace_start) = sig.find('{') {
                if let Some(brace_end) = sig.rfind('}') {
                    let inner = &sig[brace_start + 1..brace_end];
                    for member in inner.split(',') {
                        let name = member.trim();
                        if !name.is_empty() {
                            map.insert(name.to_string(), HlTag::EnumMember);
                        }
                    }
                }
            }
        }
    }
    map
}

pub(super) fn compute_semantic_tokens_filtered(
    file: &dyn FileDb,
    range: Option<&TextRange>,
) -> HlRanges {
    let mut result = Vec::<HlRange>::new();

    // The lexer is shared by all include instances, but inactive regions must
    // not contribute semantic classifications to the displayed document.
    let tokens = file
        .file_text()
        .map(|(db, input)| syntax::parse_query::semantic_parse_file(db, input).tokens().as_slice())
        .or_else(|| file.tokens());
    let Some(tokens) = tokens else {
        return HlRanges { data: result };
    };

    // Build name classification from ItemTree for type-driven semantic tokens
    let name_classes = build_name_classification(file);

    // Which name spans declare something, and which of those define it.
    //
    // From the item tree, which lowered the definitions and recorded each
    // one's `name_span`. It used to come from the flat `symbol_occurrences`
    // scrape, which is the same fact derived a second time — and derived
    // only for the constructs the scrape happens to walk.
    let (declaring_spans, defining_spans) = {
        let mut declaring: std::collections::HashSet<usize> = Default::default();
        let mut defining: std::collections::HashSet<usize> = Default::default();
        if let Some(tree) = file.item_tree() {
            for &id in tree.top_level_items() {
                let start = id.name_span(tree).start;
                declaring.insert(start);
                // A `val` or a bare `mapping` signature declares without
                // defining; a scattered head opens a definition its clauses
                // complete. Everything else with a name defines it.
                if !matches!(
                    id.def_kind(tree),
                    DefKind::ValSpec | DefKind::MappingSpec | DefKind::ScatteredHead
                ) {
                    defining.insert(start);
                }
            }
        }
        (declaring, defining)
    };

    let mut prev_token: Option<&parser::Token> = None;
    for (token, span) in tokens {
        // Use ItemTree-driven classification for identifiers at reference positions
        let tag = match token {
            // What the item tree says the name is, at its declaration and
            // at every use alike; else the syntactic classification.
            parser::Token::Id(name) => name_classes.get(name.as_str()).copied(),
            _ => None,
        }
        .or_else(|| token_tag(token, prev_token));
        let Some(tag) = tag else {
            prev_token = Some(token);
            continue;
        };
        let start_lc = file.position_at(span.start);
        let end_lc = file.position_at(span.end);
        if start_lc.line != end_lc.line || end_lc.col <= start_lc.col {
            prev_token = Some(token);
            continue;
        }
        if let Some(r) = range {
            // Use byte offset comparison for range filtering
            if span.start < base_db::range_start(*r) || span.start > base_db::range_end(*r) {
                prev_token = Some(token);
                continue;
            }
        }

        // Falls back to the preceding keyword for names the item tree has
        // no entry for — a local binding, a parameter.
        let is_ident = matches!(token, parser::Token::Id(_));
        let is_decl = (is_ident && declaring_spans.contains(&span.start))
            || matches!(
                prev_token,
                Some(parser::Token::KwFunction)
                    | Some(parser::Token::KwVal)
                    | Some(parser::Token::KwType)
                    | Some(parser::Token::KwEnum)
                    | Some(parser::Token::KwStruct)
                    | Some(parser::Token::KwUnion)
                    | Some(parser::Token::KwRegister)
                    | Some(parser::Token::KwLet)
                    | Some(parser::Token::KwVar)
                    | Some(parser::Token::KwMapping)
                    | Some(parser::Token::KwBitfield)
                    | Some(parser::Token::KwOverload)
                    | Some(parser::Token::KwNewtype)
                    | Some(parser::Token::KwScattered)
            );
        let is_control_flow = matches!(
            token,
            parser::Token::KwIf
                | parser::Token::KwThen
                | parser::Token::KwElse
                | parser::Token::KwMatch
                | parser::Token::KwForeach
                | parser::Token::KwWhile
                | parser::Token::KwRepeat
                | parser::Token::KwReturn
                | parser::Token::KwThrow
                | parser::Token::KwTry
                | parser::Token::KwCatch
                | parser::Token::KwExit
        );
        let mut highlight = Highlight::new(tag);
        if is_decl && is_ident {
            highlight |= HlMod::Declaration;
        }
        if is_ident && defining_spans.contains(&span.start) {
            highlight |= HlMod::Definition;
        }
        if matches!(prev_token, Some(parser::Token::KwLet)) {
            highlight |= HlMod::Readonly;
        }
        if matches!(prev_token, Some(parser::Token::KwVar)) {
            highlight |= HlMod::Modification;
        }
        if tag == HlTag::Register {
            highlight |= HlMod::Static;
        }
        if is_control_flow {
            highlight |= HlMod::ControlFlow;
        }

        result.push(HlRange {
            line: start_lc.line,
            col: start_lc.col,
            length: end_lc.col - start_lc.col,
            highlight,
        });
        prev_token = Some(token);
    }

    HlRanges { data: result }
}
