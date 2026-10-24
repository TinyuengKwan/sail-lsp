//! Event-based parser for Sail source files.
//!
//! Uses Pratt parsing with `TokenSet` recovery sets instead of explicit
//! end-position limits. Produces a rowan `GreenNode` via Marker/
//! CompletedMarker events with `forward_parent` chains.
//!
//! Grammar rules operate on `crate::parser::Parser` (which takes `&Input`),
//! producing `TopEntryPoint::parse(&Input) → Output`.

mod expressions;
mod items;
mod params;
mod patterns;
mod types;

/// Entry point functions for the parser.
pub(crate) mod entry {
    use super::*;

    pub(crate) mod top {
        use super::*;

        pub(crate) fn source_file(p: &mut Parser<'_>) {
            crate::grammar::parse_source_file(p);
        }

        pub(crate) fn expr(p: &mut Parser<'_>) {
            p.parse_expr();
        }

        pub(crate) fn type_(p: &mut Parser<'_>) {
            p.parse_type_expr(TokenSet::EMPTY);
        }

        pub(crate) fn pattern(p: &mut Parser<'_>) {
            p.parse_pattern(TokenSet::EMPTY);
        }
    }

    pub(crate) mod prefix {
        use super::*;

        #[expect(dead_code)]
        pub(crate) fn ty(p: &mut Parser<'_>) {
            p.parse_type_expr(TokenSet::EMPTY);
        }

        #[expect(dead_code)]
        pub(crate) fn expr(p: &mut Parser<'_>) {
            p.parse_expr();
        }

        #[expect(dead_code)]
        pub(crate) fn pat(p: &mut Parser<'_>) {
            p.parse_pattern(TokenSet::EMPTY);
        }

        #[expect(dead_code)]
        pub(crate) fn pat_top(p: &mut Parser<'_>) {
            p.parse_pattern(TokenSet::EMPTY);
        }

        #[expect(dead_code)]
        pub(crate) fn stmt(p: &mut Parser<'_>) {
            p.parse_expr();
        }

        #[expect(dead_code)]
        pub(crate) fn path(p: &mut Parser<'_>) {
            // Sail has no separate path syntax — identifiers are paths
            if !p.at_end() {
                p.bump_any();
            }
        }

        #[expect(dead_code)]
        pub(crate) fn item(p: &mut Parser<'_>) {
            p.parse_definition();
        }
    }
}

use crate::parser::{CompletedMarker, Parser};
use crate::syntax_kind::SyntaxKind as SK;
use crate::TokenSet;
use crate::T;

const CLOSERS: TokenSet =
    TokenSet::new(&[T![')'], T![']'], T!['}'], SK::R_BRACKET_BAR, SK::R_CURLY_BAR]);

/// Tokens that stop expression parsing.
///
/// extended with Sail-specific keywords (then, else, do, etc.).
const EXPR_RECOVERY_SET: TokenSet = TokenSet::new(&[
    T![')'],
    T![']'],
    T!['}'],
    SK::R_BRACKET_BAR,
    SK::R_CURLY_BAR,
    T![,],
    T![;],
    T![=>],
    T![then],
    T![else],
    T![do],
    T![until],
    T![in],
    T![with],
    T![end],
    SK::EOF,
    SK::TOMBSTONE,
]);

const PAT_RECOVERY_SET: TokenSet =
    TokenSet::new(&[T![=>], T![')'], T![']'], T!['}'], T![,], T![if], T![=]]);

/// Recovery set for parameter lists.
const PARAM_RECOVERY_SET: TokenSet = TokenSet::new(&[T![,], T![')'], T![=], T!['{']]);

const TYPE_RECOVERY: TokenSet = TokenSet::new(&[
    T![')'],
    T![']'],
    T!['}'],
    T![,],
    T![=],
    T![;],
    T![=>],
    T![with],
    T![.],
    T![then],
    T![else],
]);

const DEF_START: TokenSet = TokenSet::new(&[
    SK::KW_IMPL,
    T![function],
    T![val],
    T![type],
    T![struct],
    T![enum],
    T![union],
    T![bitfield],
    T![newtype],
    T![register],
    T![let],
    T![var],
    T![overload],
    T![mapping],
    T![scattered],
    T![default],
    T![infix],
    T![infixl],
    T![infixr],
    SK::KW_INSTANTIATION,
    T![end],
    T![constraint],
    SK::KW_TERMINATION_MEASURE,
    T![outcome],
    T![private],
    // `and` joins function clauses: `function f(..) = .. and f(..) = ..`
    // (parser.mly:947-961). It appears nowhere else in the grammar, so it
    // reliably starts a new clause. Without it the second and later clauses
    // were absorbed into the first definition's body and never checked.
    T![and],
    SK::DIRECTIVE,
    // `$name { .. }` — a structured pragma. Its braced body has to be
    // consumed as a unit or it leaks out as top-level junk.
    SK::STRUCTURED_DIRECTIVE_START,
    // `$[attr]` — the lexer only emits a bare DOLLAR for an attribute
    // introducer, so this reliably marks a definition boundary. Without it
    // the preceding definition swallows the attribute and it never becomes
    // an `ATTRIBUTE` sibling of the definition it annotates.
    SK::DOLLAR,
]);

/// Tokens that start a new top-level definition.
/// Used in `err_recover()` to prevent error recovery from
/// consuming the start of the next valid definition.
const DEF_RECOVERY_SET: TokenSet = TokenSet::new(&[
    T![function],
    T![val],
    T![type],
    T![struct],
    T![enum],
    T![union],
    T![register],
    T![bitfield],
    T![mapping],
    T![let],
    T![var],
    T![overload],
    T![scattered],
    T![newtype],
    T![default],
    T![infix],
    T![infixl],
    T![infixr],
    SK::KW_INSTANTIATION,
    T![end],
    T![constraint],
    SK::KW_TERMINATION_MEASURE,
    T![outcome],
    T![private],
    SK::DIRECTIVE,
    T![;],
    SK::EOF,
]);

#[derive(Clone, Copy)]
struct Restrictions {
    forbid_structs: bool,
    /// Stop before DOT tokens (used inside `[...]` for `..` subranges).
    forbid_dot: bool,
    /// Stop before `<->` (a mapping clause's guard ends there).
    forbid_bidi_arrow: bool,
}

const R_DEFAULT: Restrictions =
    Restrictions { forbid_structs: false, forbid_dot: false, forbid_bidi_arrow: false };
const R_NO_STRUCT: Restrictions =
    Restrictions { forbid_structs: true, forbid_dot: false, forbid_bidi_arrow: false };
const R_NO_DOT: Restrictions =
    Restrictions { forbid_structs: false, forbid_dot: true, forbid_bidi_arrow: false };
/// For a mapping clause's guard: `pat if g <-> rhs` ends the guard at `<->`.
const R_NO_BIDI: Restrictions =
    Restrictions { forbid_structs: false, forbid_dot: false, forbid_bidi_arrow: true };

const PREFIX_BP: u8 = 17;

/// Binding powers on upstream's precedence ladder (`infix_parser.mly`).
///
/// Level `L` becomes `(2L+1, 2L+2)` left-associative, `(2L+2, 2L+1)` right —
/// the same encoding `ItemTree::build_fixity_context` produces for an `infix`
/// declaration, so the two are directly comparable.
///
/// Levels are taken from `infix_parser.mly`: `@`/`::` are `exp6 At exp5r`,
/// i.e. level 5 right (338-339); `<`/`>`/`<=`/`>=` sit in `exp4` (318-321);
/// `+`/`-` in `exp6l` (357-358); `*` in `exp7l` (376). The rest come from the
/// built-in fixity table in `initial_check.ml:2415` — `^` right 8, `|`
/// right 2, `&` right 3, `==`/`!=` 4, `/`/`%` left 7.
fn infix_bp(kind: SK) -> Option<(u8, u8)> {
    Some(match kind {
        T![|] => (6, 5),                            // right 2
        T![&] => (8, 7),                            // right 3
        T![==] | T![!=] => (9, 10),                 // 4
        T![<] | T![>] | T![<=] | T![>=] => (9, 10), // 4
        T![@] | T![::] => (12, 11),                 // right 5
        T![+] | T![-] => (13, 14),                  // left 6
        T![*] | T![/] | T![%] => (15, 16),          // left 7
        T![^] => (18, 17),                          // right 8
        // `<->` separates the halves of a mapping clause rather than being an
        // operator upstream; keep it looser than everything else.
        T![<->] => (1, 2),
        // An operator with no `infix` declaration in scope. Upstream rejects
        // the program outright ("Undeclared fixity for operator",
        // initial_check.ml:308); an editor cannot, so assume the level the
        // corpora declare most often — 4, alongside the comparisons that make
        // up most user-defined operators (`<_u`, `>=_s`, ...).
        SK::OP_IDENT => (9, 10),
        _ => return None,
    })
}

/// Operators upstream's infix parser hardcodes as grammar productions rather
/// than looking up in the fixity table, so an `infix` declaration cannot
/// change them (`initial_check.ml:291-300` maps these to dedicated tokens).
fn fixity_is_fixed(kind: SK) -> bool {
    matches!(
        kind,
        T![@] | T![::] | T![+] | T![-] | T![*] | T![<] | T![>] | T![<=] | T![>=] | T![<->]
    )
}

//
// These methods extend `crate::parser::Parser` with grammar-specific
// helpers that the items/expressions/types/patterns modules need.
// This is the same crate, so inherent impl blocks are allowed.

impl<'t> Parser<'t> {
    pub(crate) fn at_set(&self, set: &TokenSet) -> bool {
        self.at_ts(set)
    }

    /// At the introducer of an `$[attr]` attribute.
    pub(crate) fn at_attribute(&self) -> bool {
        self.at(SK::DOLLAR) && self.nth(1) == T!['[']
    }

    /// Consume a whole `$[..]` attribute as one `ATTRIBUTE` node.
    ///
    /// The payload is structured data — objects, lists, strings
    /// (`attribute_data`, parser.mly:873-889) — so the terminating `]` is the
    /// one that matches the introducer, not the first one seen. Stopping at
    /// the first `]` cuts `$[f { xs = ["a", "b"] }]` in half and leaves the
    /// remainder to be reported as a broken definition.
    pub(crate) fn bump_attribute(&mut self) {
        let m = self.start();
        self.bump_attribute_tokens();
        m.complete(self, SK::ATTRIBUTE);
    }

    /// The `$[..]` itself, without wrapping it in a node.
    ///
    /// Two callers: `bump_attribute`, where the attribute *is* the node,
    /// and the expression parser, where it wraps what follows. That second
    /// one had its own copy which stopped at the first `]`, so
    /// `$[f { xs = ["a", "b"] }] e` was cut in half — the same defect this
    /// function's own doc comment was written about.
    pub(crate) fn bump_attribute_tokens(&mut self) {
        self.bump_any(); // $
        self.bump_any(); // [
        let mut depth = 1usize;
        while !self.at_end() {
            if self.at(T!['[']) {
                depth += 1;
            } else if self.at(T![']']) {
                depth -= 1;
                if depth == 0 {
                    self.bump_any();
                    break;
                }
            }
            self.bump_any();
        }
    }

    // Input contains only non-trivia tokens, so these methods scan
    // Input positions directly (no trivia skip).

    /// Find the position of the next definition-starting token after `from`.
    /// Skips the first non-trivia token at `from` (the current definition's
    /// keyword) and scans forward.
    fn next_def_start_pos(&self, from: usize) -> usize {
        let inp_len = self.inp_len();
        let mut i = from + 1; // skip current token
        let limit = self.enclosing_block_end.unwrap_or(inp_len).min(inp_len);
        while i < limit {
            let k = self.nth(i - self.pos());
            if DEF_START.contains(k)
                && !self.preceded_by_scattered_at(i)
                && !self.is_expression_keyword_at(i)
                && !self.is_expression_attribute_at(i)
            {
                return i;
            }
            i += 1;
        }
        limit
    }

    /// Where an `outcome`'s header can still be running.
    ///
    /// Like `next_def_start_pos`, except that `constraint` is part of an
    /// outcome header — `outcome f : .. with 'a : Int constraint 'a > 0 =
    /// { .. }` — so stopping at it hides the `= {` that follows.
    fn outcome_header_limit(&self) -> usize {
        let inp_len = self.inp_len();
        let mut i = self.pos() + 1;
        let limit = self.enclosing_block_end.unwrap_or(inp_len).min(inp_len);
        while i < limit {
            let k = self.nth(i - self.pos());
            if DEF_START.contains(k)
                && k != T![constraint]
                && !self.preceded_by_scattered_at(i)
                && !self.is_expression_keyword_at(i)
                && !self.is_expression_attribute_at(i)
            {
                return i;
            }
            i += 1;
        }
        limit
    }

    /// The `{` of an `= {` between the cursor and `limit`, if there is one.
    fn eq_brace_pos_before(&self, limit: usize) -> Option<usize> {
        let mut i = self.pos();
        while i + 1 < limit.min(self.inp_len()) {
            if self.nth(i - self.pos()) == T![=] && self.nth(i + 1 - self.pos()) == T!['{'] {
                return Some(i + 1);
            }
            i += 1;
        }
        None
    }

    /// As `matching_close_brace_pos`, starting inside a block already open.
    fn matching_close_brace_pos_from(&self, start: usize) -> usize {
        let inp_len = self.inp_len();
        let mut depth: i32 = 0;
        let mut i = start;
        while i < inp_len {
            match self.nth(i - self.pos()) {
                T!['{'] => depth += 1,
                T!['}'] => {
                    if depth == 0 {
                        return i;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        inp_len
    }

    fn matching_close_brace_pos(&self) -> usize {
        let inp_len = self.inp_len();
        let mut depth: i32 = 0;
        let mut i = self.pos();
        while i < inp_len {
            match self.nth(i - self.pos()) {
                T!['{'] => depth += 1,
                T!['}'] => {
                    if depth == 0 {
                        return i;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            i += 1;
        }
        inp_len
    }

    /// `$[..]` on an *expression*, not on a definition.
    ///
    /// Upstream's grammar has both — `exp := attributes exp`
    /// (`parser.mly:648`) and the definition form — and the one that matters
    /// here is `$[complete] match e { .. }`, which `type_check.ml:2205`
    /// treats as the sanctioned way to silence the completeness check. A
    /// `DOLLAR` in `DEF_START` ended the enclosing body at it, so following
    /// upstream's own advice produced three syntax errors and lost the
    /// function.
    ///
    /// The attribute introduces a definition when what follows its `]` does.
    fn is_expression_attribute_at(&self, pos: usize) -> bool {
        if self.nth(pos - self.pos()) != SK::DOLLAR {
            return false;
        }
        let mut i = pos + 1;
        if self.nth(i - self.pos()) != T!['['] {
            // A bare `$` is the pragma introducer, not an attribute.
            return false;
        }
        let limit = self.inp_len();
        let mut depth = 0i32;
        while i < limit {
            match self.nth(i - self.pos()) {
                T!['['] => depth += 1,
                T![']'] => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let after = i + 1;
        if after >= limit {
            return false;
        }
        let k = self.nth(after - self.pos());
        // Another attribute keeps whatever the first one was.
        if k == SK::DOLLAR || k == SK::STRUCTURED_DIRECTIVE_START {
            return false;
        }
        !DEF_START.contains(k)
    }

    /// Whether the definition-start keyword at `pos` is really being used
    /// inside an expression or pattern.
    ///
    /// `struct` opens a struct literal or pattern as well as a definition, so
    /// `let struct { f1 = _ } : S = ..` would otherwise be cut in two at the
    /// `struct`, turning the pattern's field names into definitions.
    fn is_expression_keyword_at(&self, pos: usize) -> bool {
        if pos == 0 {
            return false;
        }
        let k = self.nth(pos - self.pos());
        let prev = self.nth((pos - 1).saturating_sub(self.pos()));
        match k {
            T![private] => {
                prev == T![=] && self.current() == T![union] && self.nth(1) == T![clause]
            }
            T![struct] => matches!(
                prev,
                T![let] | T![var] | T![=] | T![,] | T!['('] | T!['{'] | T![:] | T![->] | T![<->]
            ),
            // `register` is a type constructor as well as a definition
            // keyword: `function get_X() -> register(bits(4))`. Treating the
            // one in the return type as the start of the next definition cut
            // the function in two.
            T![register] => matches!(prev, T![,] | T!['('] | T![:] | T![->] | T![<->]),
            _ => false,
        }
    }

    /// Check if position `pos` in Input is preceded by `scattered` keyword.
    fn preceded_by_scattered_at(&self, pos: usize) -> bool {
        if pos == 0 {
            return false;
        }
        // In the non-trivia Input, the preceding token is at pos-1
        let prev_kind = self.nth((pos - 1).saturating_sub(self.pos()));
        prev_kind == T![scattered]
    }

    /// Check if the current keyword is followed by `clause` (in non-trivia Input).
    fn followed_by_clause(&self) -> bool {
        self.nth(1) == T![clause]
    }

    /// Lookahead to determine if `{` starts a struct update expression
    /// `{ base with field = val }` vs a regular block `{ stmt; ... }`.
    ///
    /// Scans non-trivia tokens from current position looking for `with`
    /// at bracket depth 0 before `}` or `;`.
    pub(crate) fn is_update_expr(&self) -> bool {
        let mut offset = 1; // skip the `{` itself
        let mut curly_depth = 0u32;
        let mut bracket_depth = 0u32;
        let mut paren_depth = 0u32;
        loop {
            let k = self.nth(offset);
            if k == SK::EOF {
                return false;
            }
            match k {
                T!['{'] => curly_depth += 1,
                T!['}'] if curly_depth > 0 => curly_depth -= 1,
                T!['}'] => return false,
                T!['['] => bracket_depth += 1,
                T![']'] if bracket_depth > 0 => bracket_depth -= 1,
                T!['('] => paren_depth += 1,
                T![')'] if paren_depth > 0 => paren_depth -= 1,
                T![with] if curly_depth == 0 && bracket_depth == 0 && paren_depth == 0 => {
                    return true;
                }
                T![;] if curly_depth == 0 && bracket_depth == 0 && paren_depth == 0 => {
                    return false;
                }
                _ => {}
            }
            offset += 1;
        }
    }

    /// Number of tokens in the Input.
    fn inp_len(&self) -> usize {
        // Walk until EOF
        let mut n = 0;
        loop {
            if self.nth(n) == SK::EOF && (self.pos() + n) >= self.pos() {
                // To get the real length we need to find where EOF starts
                // Actually we can compute: pos + n is the first EOF position
                return self.pos() + n;
            }
            n += 1;
            // Safety: if n gets very large, bail
            if n > 1_000_000 {
                return self.pos() + n;
            }
        }
    }

    /// Bump tokens until position `end_pos` in the non-trivia Input stream.
    fn bump_until_pos(&mut self, end_pos: usize) {
        while self.pos() < end_pos && !self.at_end() {
            self.bump_any();
        }
    }

    /// Skip an entire `{...}` block, wrapping it as ERROR.
    fn error_block(&mut self, message: &str) {
        let m = self.start();
        self.error(message.to_string());
        self.bump_any(); // {
        let mut depth = 1i32;
        while !self.at_end() && depth > 0 {
            match self.current() {
                T!['{'] => {
                    depth += 1;
                    self.bump_any();
                }
                T!['}'] => {
                    depth -= 1;
                    if depth > 0 {
                        self.bump_any();
                    }
                }
                _ => {
                    self.bump_any();
                }
            }
        }
        if self.at(T!['}']) {
            self.bump_any();
        }
        m.complete(self, SK::ERROR);
    }

    /// Create an error node and consume the next token unless it is
    /// in the recovery set or is a brace.
    ///
    /// Returns `true` if recovery kicked in (token NOT consumed).
    fn err_recover_grammar(&mut self, message: &str, recovery: TokenSet) -> bool {
        let kind = self.current();
        if matches!(kind, T!['{'] | T!['}']) {
            self.error(message.to_string());
            return true;
        }
        if recovery.contains(kind) {
            self.error(message.to_string());
            return true;
        }
        self.err_and_bump(message);
        false
    }

    /// Parse a comma-separated list until `terminator`.
    ///
    /// - Double comma `(a, , b)` → wrap extra comma in ERROR node
    /// - Missing comma `(a b)` → emit "expected `,`" if next looks like item
    /// - Unexpected token → break (caller handles closing bracket)
    fn parse_comma_sep(&mut self, terminator: SK, mut parse_one: impl FnMut(&mut Self)) {
        if self.at(terminator) {
            return;
        }
        parse_one(self);
        loop {
            if self.at(terminator) || self.at_end() {
                break;
            }
            if self.at(T![,]) {
                self.bump_any();
                if self.at(terminator) || self.at_end() {
                    break; // trailing comma
                }
                // Double comma = missing item.
                if self.at(T![,]) {
                    let m = self.start();
                    self.error("expected item, found `,`".to_string());
                    self.bump_any();
                    m.complete(self, SK::ERROR);
                    continue;
                }
                parse_one(self);
            } else {
                // No comma — check if next token looks like it starts
                // another item (missing comma case).
                let k = self.current();
                if k == SK::IDENT || k == T!['('] || k == T![ref] || k == T![forall] || k == T![_] {
                    self.error("expected `,`".to_string());
                    parse_one(self);
                } else {
                    break;
                }
            }
        }
    }
}

/// Re-export the FixityContext type for downstream use.
/// The syntax crate imports this as `parser::grammar::FixityContext`.
pub use crate::parser::FixityContext;

// Tree construction lives in the syntax crate (`syntax/src/parsing.rs`).
// The public API is:
//   - `TopEntryPoint::parse(&input) → Output`  (this crate)
//   - `syntax::parse_text(text) → SyntaxNode`  (syntax crate)

/// Parse a source file using the parser pipeline.
///
/// This is the real implementation behind `TopEntryPoint::SourceFile.parse()`.
pub(crate) fn parse_source_file(p: &mut Parser<'_>) {
    let m = p.start();
    while !p.at_end() {
        if p.at_end() {
            break;
        }
        // Skip doc comments (non-trivia) before definitions.
        if p.at(SK::DOC_COMMENT) {
            p.bump_any();
            continue;
        }
        // Skip `$[attr]` attribute pragmas before definitions.
        if p.at_attribute() {
            p.bump_attribute();
            continue;
        }
        if p.at_set(&DEF_START) {
            p.parse_definition();
        } else {
            // Error recovery
            match p.current() {
                T!['{'] => p.error_block("expected definition"),
                T!['}'] => {
                    let m = p.start();
                    p.error("unmatched `}`".to_string());
                    p.bump_any();
                    m.complete(p, SK::ERROR);
                }
                SK::EOF => p.error("expected definition".to_string()),
                _ => p.err_and_bump("expected definition"),
            }
        }
    }
    m.complete(p, SK::SOURCE_FILE);
}
