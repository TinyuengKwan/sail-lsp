//! Computes the highlight of each token.
//!
//! rust-analyzer's `syntax_highlighting/highlight.rs`: a name is classified
//! by what it resolves to (`IdentClass` → `Definition`), through the same
//! `Semantics` that goto-definition and find-references use, and only a
//! name nothing resolves falls back to what the keyword before it suggests.
//! This used to build a name → kind map from the current file's top-level
//! items, so a function defined in an included file was a variable, and a
//! local shadowing a top-level name took the global's colour.

use std::collections::{HashMap, HashSet};

use super::tags::token_tag;
use super::*;
use hir::DefKind;
use ide_db::defs::{Definition, IdentClass, NameClass, NameRefClass};

pub(super) fn compute_semantic_tokens_filtered(
    file: &dyn FileDb,
    range: Option<&TextRange>,
) -> HlRanges {
    let mut result = Vec::<HlRange>::new();

    // The lexer is shared by all include instances, but inactive regions must
    // not contribute semantic classifications to the displayed document.
    let parsed = file.file_text().map(|(db, input)| {
        let parsed = syntax::parse_query::semantic_parse_file(db, input);
        (parsed.tokens().as_slice(), parsed.syntax_node())
    });
    let (tokens, root) = match parsed {
        Some((tokens, root)) => (Some(tokens), root),
        None => (file.tokens(), file.syntax()),
    };
    let Some(tokens) = tokens else {
        return HlRanges { data: result };
    };
    let in_range = |start: usize| {
        range.is_none_or(|r| start >= base_db::range_start(*r) && start <= base_db::range_end(*r))
    };

    let items = Items::collect(file);
    let locals = Locals::collect(file);

    // Comments and attributes are not lexer tokens; the tree has them.
    let mut attributes: Vec<parser::Span> = Vec::new();
    if let Some(root) = &root {
        for element in root.descendants_with_tokens() {
            let text_range = element.text_range();
            let span = parser::Span::new(text_range.start().into(), text_range.end().into());
            let highlight = match &element {
                rowan::NodeOrToken::Node(node) if node.kind() == syntax::SyntaxKind::ATTRIBUTE => {
                    attributes.push(span);
                    HlTag::Attribute.into()
                }
                rowan::NodeOrToken::Token(token) => match token.kind() {
                    syntax::SyntaxKind::LINE_COMMENT | syntax::SyntaxKind::BLOCK_COMMENT => {
                        HlTag::Comment.into()
                    }
                    syntax::SyntaxKind::DOC_COMMENT => HlTag::Comment | HlMod::Documentation,
                    _ => continue,
                },
                _ => continue,
            };
            if in_range(span.start) {
                push_lines(file, &mut result, span, highlight);
            }
        }
    }

    let mut prev_token: Option<&parser::Token> = None;
    for (token, span) in tokens {
        let previous = prev_token.replace(token);
        if !in_range(span.start) {
            continue;
        }
        // An attribute is one token; what it is made of is not coloured
        // piecemeal.
        if attributes.iter().any(|a| a.start <= span.start && span.end <= a.end) {
            continue;
        }
        let highlight = match token {
            parser::Token::Id(_) => name(file, token, span, previous, &items, &locals),
            parser::Token::String(_) => {
                string_literal(file, &mut result, *span);
                continue;
            }
            _ => token_tag(token, previous).map(|tag| {
                let mut highlight = Highlight::new(tag);
                if is_control_flow(token) {
                    highlight |= HlMod::ControlFlow;
                }
                highlight
            }),
        };
        let Some(highlight) = highlight else { continue };
        push_lines(file, &mut result, *span, highlight);
    }

    result.sort_by_key(|r| (r.line, r.col));
    HlRanges { data: result }
}

/// Push `span` as one range per line it covers. A semantic token cannot
/// cross a line, and a block comment or an attribute can.
fn push_lines(file: &dyn FileDb, out: &mut Vec<HlRange>, span: parser::Span, highlight: Highlight) {
    let Some(slice) = file.text().get(span.start..span.end) else { return };
    let mut start = span.start;
    for piece in slice.split_inclusive('\n') {
        let end = start + piece.trim_end_matches(['\n', '\r']).len();
        if end > start {
            let lc = file.position_at(start);
            out.push(HlRange {
                line: lc.line,
                col: lc.col,
                length: (end - start) as u32,
                highlight,
            });
        }
        start += piece.len();
    }
}

/// A string literal, with its escape sequences as tokens of their own.
///
/// The pieces between escapes stay strings: semantic tokens cannot nest,
/// so the literal is cut around each escape rather than overlaid.
fn string_literal(file: &dyn FileDb, out: &mut Vec<HlRange>, span: parser::Span) {
    let Some(text) = file.text().get(span.start..span.end) else { return };
    let mut piece_start = span.start;
    for (offset, len) in escape_sequences(text) {
        let start = span.start + offset;
        if start > piece_start {
            push_lines(
                file,
                out,
                parser::Span::new(piece_start, start),
                HlTag::StringLiteral.into(),
            );
        }
        push_lines(file, out, parser::Span::new(start, start + len), HlTag::EscapeSequence.into());
        piece_start = start + len;
    }
    if span.end > piece_start {
        push_lines(
            file,
            out,
            parser::Span::new(piece_start, span.end),
            HlTag::StringLiteral.into(),
        );
    }
}

/// `(offset, length)` of each escape in a string literal's text, as the
/// lexer reads them (`hand_lexer.rs`): `\xHH`, `\DDD`, or `\` and one
/// character.
fn escape_sequences(text: &str) -> Vec<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let digits = |i: &mut usize, n: usize, ok: fn(&u8) -> bool| {
            for _ in 0..n {
                if *i < bytes.len() && ok(&bytes[*i]) {
                    *i += 1;
                }
            }
        };
        match bytes[i] {
            b'x' => {
                i += 1;
                digits(&mut i, 2, u8::is_ascii_hexdigit);
            }
            b'0'..=b'9' => {
                i += 1;
                digits(&mut i, 2, u8::is_ascii_digit);
            }
            _ => i += text[i..].chars().next().map_or(1, char::len_utf8),
        }
        out.push((start, i - start));
    }
    out
}

/// The file's top-level declarations, by the start of their name span.
///
/// A name at one of these is the item it declares; the item tree already
/// says what kind, so the resolver need not be asked.
#[derive(Default)]
struct Items {
    declared: HashMap<usize, DefKind>,
}

impl Items {
    fn collect(file: &dyn FileDb) -> Self {
        let mut items = Self::default();
        if let Some(tree) = file.item_tree() {
            for id in tree.top_level_items() {
                // A generated item — an enum's `E_of_num` — shares the span
                // of the name it is generated from; the span is that name's.
                let span = id.name_span(tree);
                if file.text().get(span.start..span.end) == Some(id.name(tree).as_str()) {
                    items.declared.insert(span.start, id.def_kind(tree));
                }
            }
        }
        items
    }

    /// Whether the declaration at `start` also defines what it declares: a
    /// `val` or a bare `mapping` signature does not, and a scattered head
    /// opens a definition its clauses complete.
    fn defines(&self, start: usize) -> bool {
        self.declared.get(&start).is_some_and(|kind| {
            !matches!(kind, DefKind::ValSpec | DefKind::MappingSpec | DefKind::ScatteredHead)
        })
    }
}

/// Every mention of a local binding in the file, by the start of its span.
///
/// Resolved once per body through its scope tree, as `IdentClass` resolves
/// one cursor — asking the classifier for each mention would walk the body
/// once per name in it. Each binding pattern is still classified by
/// `IdentClass`, so a pattern that names an enum member or a union
/// constructor is left to it, as are the uses that resolve to that pattern.
#[derive(Default)]
struct Locals {
    mentions: HashMap<usize, Mention>,
    /// Start offsets of the binding patterns of `var` locals.
    mutable_bindings: HashSet<usize>,
    /// Start offsets of names that are the target of an assignment — the
    /// `R` of `R = ..`, `R.f = ..`, `R[i] = ..`.
    written: HashSet<usize>,
}

struct Mention {
    local: hir::Local,
    is_binding: bool,
}

impl Locals {
    fn collect(file: &dyn FileDb) -> Self {
        let mut locals = Self::default();
        let Some(bodies) = file.bodies() else { return locals };
        for entry in bodies.entries() {
            let scopes = file.body_scopes(entry);
            for (scope, _) in scopes.all_scopes() {
                for binding in scopes.entries(scope).iter().filter(|e| e.is_mutable()) {
                    if let Some(span) = entry.source_map.pat_syntax(binding.pat()) {
                        locals.mutable_bindings.insert(span.start);
                    }
                }
            }
            for (_, expr) in entry.body.iter_exprs() {
                let hir::Expr::Assign { target, .. } = expr else { continue };
                let mut target = *target;
                loop {
                    match entry.body.expr(target) {
                        Some(hir::Expr::Field { expr, .. }) => target = *expr,
                        Some(hir::Expr::Index { base, .. })
                        | Some(hir::Expr::Subrange { base, .. }) => target = *base,
                        Some(hir::Expr::Ident(_)) => {
                            if let Some(span) = entry.source_map.expr_syntax(target) {
                                locals.written.insert(span.start);
                            }
                            break;
                        }
                        _ => break,
                    }
                }
            }
            let mut by_pat: HashMap<hir::PatId, hir::Local> = HashMap::new();
            for (pat_id, pat) in entry.body.iter_pats() {
                let (hir::Pat::Bind(name) | hir::Pat::As { binding: name, .. }) = pat else {
                    continue;
                };
                if !scopes.binds_pattern(pat_id) {
                    continue;
                }
                let Some(span) = entry.source_map.pat_syntax(pat_id) else { continue };
                let token = parser::Token::Id(name.clone());
                if let Some(IdentClass::NameClass(NameClass::Definition(Definition::Local(
                    local,
                )))) = IdentClass::classify_token(file, &token, &span)
                {
                    locals
                        .mentions
                        .insert(span.start, Mention { local: local.clone(), is_binding: true });
                    by_pat.insert(pat_id, local);
                }
            }
            if by_pat.is_empty() {
                continue;
            }
            for (expr_id, expr) in entry.body.iter_exprs() {
                let hir::Expr::Ident(name) = expr else { continue };
                let Some(span) = entry.source_map.expr_syntax(expr_id) else { continue };
                let Some(local) = scopes
                    .scope_for(expr_id)
                    .and_then(|scope| scopes.resolve_name_in_scope(scope, &hir::Name::new(name)))
                    .and_then(|entry| by_pat.get(&entry.pat()))
                else {
                    continue;
                };
                locals
                    .mentions
                    .entry(span.start)
                    .or_insert_with(|| Mention { local: local.clone(), is_binding: false });
            }
        }
        locals
    }
}

/// The highlight of a name: what it resolves to, and whether this is where
/// it is declared.
fn name(
    file: &dyn FileDb,
    token: &parser::Token,
    span: &parser::Span,
    previous: Option<&parser::Token>,
    items: &Items,
    locals: &Locals,
) -> Option<Highlight> {
    if let Some(mention) = locals.mentions.get(&span.start) {
        let mut highlight =
            Highlight::new(if mention.local.is_param { HlTag::Parameter } else { HlTag::Variable });
        if mention.is_binding {
            highlight |= HlMod::Declaration;
            highlight |= HlMod::Definition;
        }
        if locals.mutable_bindings.contains(&mention.local.span.start) {
            highlight |= HlMod::Mutable;
        }
        return Some(highlight);
    }
    let (def, declares) = match items.declared.get(&span.start) {
        Some(kind) => match item_highlight(*kind) {
            Some(mut highlight) => {
                highlight |= HlMod::Declaration;
                if items.defines(span.start) {
                    highlight |= HlMod::Definition;
                }
                if file.file_text().is_some_and(|(db, ft)| file.is_library(ft.file_id(db))) {
                    highlight |= HlMod::DefaultLibrary;
                }
                return Some(highlight);
            }
            None => return Some(lexical(token, previous)),
        },
        None => match IdentClass::classify_token(file, token, span) {
            Some(IdentClass::NameClass(NameClass::Definition(def))) => (def, true),
            Some(IdentClass::NameClass(NameClass::ConstReference(def)))
            | Some(IdentClass::NameRefClass(NameRefClass::Definition(def))) => (def, false),
            None => return Some(lexical(token, previous)),
        },
    };
    let db = file.file_text().map(|(db, _)| db);
    let mut highlight = match &def {
        Definition::Function(_)
        | Definition::Mapping(_)
        | Definition::ValSpec(_)
        | Definition::Overload(_)
        | Definition::TerminationMeasure(_)
        | Definition::Outcome(_)
        | Definition::Instantiation(_) => Highlight::new(HlTag::Function),
        // `scattered union`/`scattered enum` and the clauses and `end`
        // that complete them name a type; the rest name a function.
        Definition::ScatteredHead(loc)
        | Definition::ScatteredClause(loc)
        | Definition::EndMarker(loc) => {
            let kind = db
                .and_then(|db| loc.item(db))
                .and_then(|(tree, item)| item.scattered_type_kind(tree));
            Highlight::new(match kind {
                Some(hir::TypeDefShape::Enum) => HlTag::Enum,
                Some(_) => HlTag::Type,
                None => HlTag::Function,
            })
        }
        Definition::TypeDef(t) => Highlight::new(match t.kind() {
            hir::TypeDefKind::Struct | hir::TypeDefKind::Bitfield => HlTag::Struct,
            hir::TypeDefKind::Enum => HlTag::Enum,
            hir::TypeDefKind::Union | hir::TypeDefKind::Newtype | hir::TypeDefKind::TypeAlias => {
                HlTag::Type
            }
        }),
        Definition::BuiltinType(_) => Highlight::new(HlTag::BuiltinType),
        Definition::Register(_) => {
            let mut highlight = HlTag::Register | HlMod::Static;
            if locals.written.contains(&span.start) {
                highlight |= HlMod::Mutable;
            }
            highlight
        }
        Definition::Let(loc) => {
            let is_var = db
                .and_then(|db| loc.item(db))
                .is_some_and(|(tree, item)| item.def_kind(tree) == DefKind::Var);
            if is_var {
                HlTag::Variable | HlMod::Mutable
            } else {
                HlTag::Variable | HlMod::Readonly
            }
        }
        Definition::Constraint(_) => Highlight::new(HlTag::Variable),
        // A member declared in an `enum`/`union` brace list classifies as a
        // field of its type, which is what hover wants to say about it;
        // for colour it is a member like every use of it.
        Definition::Field(field) => {
            let of_variant_type = file.item_tree().is_some_and(|tree| {
                tree.top_level_items().iter().any(|item| {
                    matches!(item.def_kind(tree), DefKind::Enum | DefKind::Union)
                        && item.fields(tree).iter().any(|f| f.name_span == field.declaration_span)
                })
            });
            Highlight::new(if of_variant_type { HlTag::EnumMember } else { HlTag::Property })
        }
        Definition::EnumVariant(_) => Highlight::new(HlTag::EnumMember),
        Definition::Local(local) => {
            let mut highlight =
                Highlight::new(if local.is_param { HlTag::Parameter } else { HlTag::Variable });
            if locals.mutable_bindings.contains(&local.span.start) {
                highlight |= HlMod::Mutable;
            }
            highlight
        }
        // The resolver answers a builtin type name with an unbound type
        // variable, as `rename` and `shared_edits` also know.
        Definition::TypeVar(var)
            if var.binding.is_none()
                && ide_db::defs::BuiltinType::from_name(var.name.as_str()).is_some() =>
        {
            Highlight::new(HlTag::BuiltinType)
        }
        Definition::TypeVar(_) => Highlight::new(HlTag::TypeParameter),
        // The resolver could not place it: say what the keyword before
        // it says, as before there was a resolver.
        Definition::Unresolved(_) => return Some(lexical(token, previous)),
    };
    if declares {
        highlight |= HlMod::Declaration;
        if def.is_local() || items.defines(span.start) {
            highlight |= HlMod::Definition;
        }
    }
    if let Some(db) = db {
        if def.file_id(db).is_some_and(|id| file.is_library(id)) {
            highlight |= HlMod::DefaultLibrary;
        }
    }
    Some(highlight)
}

/// The highlight of a top-level declaration, from the item tree's kind.
fn item_highlight(kind: DefKind) -> Option<Highlight> {
    Some(match kind {
        DefKind::Function
        | DefKind::Mapping
        | DefKind::ValSpec
        | DefKind::MappingSpec
        | DefKind::Overload
        | DefKind::TerminationMeasure
        | DefKind::Outcome
        | DefKind::Instantiation => Highlight::new(HlTag::Function),
        DefKind::TypeAlias | DefKind::Newtype | DefKind::Union => Highlight::new(HlTag::Type),
        DefKind::Struct | DefKind::Bitfield => Highlight::new(HlTag::Struct),
        DefKind::Enum => Highlight::new(HlTag::Enum),
        DefKind::Register => HlTag::Register | HlMod::Static,
        DefKind::Let => HlTag::Variable | HlMod::Readonly,
        DefKind::Var => HlTag::Variable | HlMod::Mutable,
        // What a scattered head or clause names is decided by its kind
        // field, which only the resolver reads.
        _ => return None,
    })
}

/// What a name is by the keyword before it: the classification of a name
/// nothing resolves.
fn lexical(token: &parser::Token, previous: Option<&parser::Token>) -> Highlight {
    let mut highlight = Highlight::new(token_tag(token, previous).unwrap_or(HlTag::Variable));
    if matches!(
        previous,
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
    ) {
        highlight |= HlMod::Declaration;
    }
    if matches!(previous, Some(parser::Token::KwLet)) {
        highlight |= HlMod::Readonly;
    }
    if matches!(previous, Some(parser::Token::KwVar)) {
        highlight |= HlMod::Mutable;
    }
    if highlight.tag == HlTag::Register {
        highlight |= HlMod::Static;
    }
    highlight
}

fn is_control_flow(token: &parser::Token) -> bool {
    matches!(
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
    )
}
