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

    #[allow(dead_code)]
    pub(crate) mod prefix {
        use super::*;

        pub(crate) fn ty(p: &mut Parser<'_>) {
            p.parse_type_expr(TokenSet::EMPTY);
        }

        pub(crate) fn expr(p: &mut Parser<'_>) {
            p.parse_expr();
        }

        pub(crate) fn pat(p: &mut Parser<'_>) {
            p.parse_pattern(TokenSet::EMPTY);
        }

        pub(crate) fn pat_top(p: &mut Parser<'_>) {
            p.parse_pattern(TokenSet::EMPTY);
        }

        pub(crate) fn stmt(p: &mut Parser<'_>) {
            p.parse_expr();
        }

        pub(crate) fn path(p: &mut Parser<'_>) {
            // Sail has no separate path syntax — identifiers are paths
            if !p.at_end() {
                p.bump_any();
            }
        }

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
    SK::DIRECTIVE,
]);

/// Tokens that start a new top-level definition.
/// Used in `err_recover()` to prevent error recovery from
/// consuming the start of the next valid definition.
#[allow(dead_code)]
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
}

const R_DEFAULT: Restrictions = Restrictions { forbid_structs: false, forbid_dot: false };
const R_NO_STRUCT: Restrictions = Restrictions { forbid_structs: true, forbid_dot: false };
const R_NO_DOT: Restrictions = Restrictions { forbid_structs: false, forbid_dot: true };

const PREFIX_BP: u8 = 17;

fn infix_bp(kind: SK) -> Option<(u8, u8)> {
    Some(match kind {
        T![|] => (1, 2),
        T![&] => (3, 4),
        T![==] | T![!=] => (5, 6),
        T![<] | T![>] | T![<=] | T![>=] | SK::OP_IDENT => (7, 8),
        T![@] | T![::] => (9, 10),
        T![<->] => (1, 2), // <-> in mapping bodies
        T![+] | T![-] => (11, 12),
        T![*] | T![/] | T![%] => (13, 14),
        T![^] => (16, 15), // right-assoc
        _ => return None,
    })
}

//
// These methods extend `crate::parser::Parser` with grammar-specific
// helpers that the items/expressions/types/patterns modules need.
// This is the same crate, so inherent impl blocks are allowed.

impl<'t> Parser<'t> {

    pub(crate) fn at_set(&self, set: &TokenSet) -> bool {
        self.at_ts(set)
    }

    // Input contains only non-trivia tokens, so these methods scan
    // Input positions directly (no trivia skip).

    /// Find the position of the next definition-starting token after `from`.
    /// Skips the first non-trivia token at `from` (the current definition's
    /// keyword) and scans forward.
    fn next_def_start_pos(&self, from: usize) -> usize {
        let inp_len = self.inp_len();
        let mut i = from + 1; // skip current token
        while i < inp_len {
            let k = self.nth(i - self.pos());
            if DEF_START.contains(k) && !self.preceded_by_scattered_at(i) {
                return i;
            }
            i += 1;
        }
        inp_len
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
        if p.at(SK::DOLLAR) && p.nth(1) == T!['['] {
            let am = p.start();
            p.bump_any(); // $
            p.bump_any(); // [
            while !p.at_end() && !p.at(T![']']) {
                p.bump_any();
            }
            if p.at(T![']']) {
                p.bump_any();
            }
            am.complete(p, SK::ATTRIBUTE);
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
