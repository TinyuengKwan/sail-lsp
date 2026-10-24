//! Core highlighting algorithm.

use super::tags::token_type_index;
use super::*;
use hir::DefKind;

/// Build a name → token type index map from ItemTree for type-driven classification.
///
/// resolves identifiers to `SymbolKind` for accurate coloring.
///
/// This lets us classify identifier references (not just declarations) correctly,
/// distinguishing structs from type aliases, enum members from enum types,
/// registers from local variables, etc.
pub(super) fn build_name_classification(
    file: &dyn FileDb,
) -> std::collections::HashMap<String, u32> {
    let mut map = std::collections::HashMap::new();
    let Some(item_tree) = file.item_tree() else {
        return map;
    };
    for &id in item_tree.top_level_items() {
        let kind = id.def_kind(item_tree);
        let idx = match kind {
            DefKind::Function | DefKind::Mapping | DefKind::ValSpec | DefKind::MappingSpec => 1, // function
            DefKind::TypeAlias | DefKind::Newtype => 2, // type
            DefKind::Struct => 9,                       // struct (new)
            DefKind::Union => 2,                        // union → type
            DefKind::Bitfield => 9,                     // bitfield → struct (visually similar)
            DefKind::Enum => 3,                         // enum type name
            DefKind::Register => 13,                    // register → macro (distinct color)
            DefKind::Let => 4,                          // let → variable
            DefKind::Var => 4,                          // var → variable
            _ => continue,
        };
        map.insert(id.name(item_tree).as_str().to_string(), idx);
    }
    // Also classify enum members as enumMember (index 10).
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
                            map.insert(name.to_string(), 10); // enumMember
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
    let mut prev_line = 0_u32;
    let mut prev_start = 0_u32;
    let mut first = true;

    // The lexer is shared by all include instances, but inactive regions must
    // not contribute semantic classifications to the displayed document.
    let tokens = file
        .file_text()
        .map(|(db, input)| syntax::parse_query::semantic_parse_file(db, input).tokens.as_slice())
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
        let tt = if let parser::Token::Id(name) = token {
            if let Some(&cls) = name_classes.get(name.as_str()) {
                // At declaration site, use the declaration-context logic
                let decl_context = token_type_index(token, prev_token);
                decl_context.unwrap_or(cls) // prefer context if available, else use ItemTree
            } else {
                // Unknown name — fall back to syntactic classification
                match token_type_index(token, prev_token) {
                    Some(t) => t,
                    None => {
                        prev_token = Some(token);
                        continue;
                    }
                }
            }
        } else {
            match token_type_index(token, prev_token) {
                Some(t) => t,
                None => {
                    prev_token = Some(token);
                    continue;
                }
            }
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

        let delta_line = if first { start_lc.line } else { start_lc.line - prev_line };
        let delta_start = if first {
            start_lc.col
        } else if delta_line == 0 {
            start_lc.col - prev_start
        } else {
            start_lc.col
        };

        // Use ParsedFile occurrence roles for accurate modifiers.
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
        let is_definition = is_ident && defining_spans.contains(&span.start);
        let is_mutable = matches!(prev_token, Some(parser::Token::KwVar));
        let is_readonly = matches!(prev_token, Some(parser::Token::KwLet));
        let is_register = tt == 13; // register type index
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
        let mut modifier = 0u32;
        if is_decl && matches!(token, parser::Token::Id(_)) {
            modifier |= 1;
        } // bit 0: declaration
        if is_definition {
            modifier |= 2;
        } // bit 1: definition
        if is_readonly {
            modifier |= 4;
        } // bit 2: readonly
        if is_mutable {
            modifier |= 8;
        } // bit 3: modification
        if is_register {
            modifier |= 32;
        } // bit 5: static (for registers)
        if is_control_flow {
            modifier |= 64;
        } // bit 6: controlFlow

        result.push(HlRange {
            delta_line,
            delta_start,
            length: end_lc.col - start_lc.col,
            token_type: tt,
            token_modifiers_bitset: modifier,
        });
        first = false;
        prev_line = start_lc.line;
        prev_start = start_lc.col;
        prev_token = Some(token);
    }

    HlRanges { data: result }
}
