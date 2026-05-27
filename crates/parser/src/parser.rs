//! Parser machinery: cursor, markers, and tree building primitives.
//!
//! into an `Input` token stream. It emits `Event`s that are later
//! converted to an `Output` (or directly to a rowan `GreenNode`).
//!
//! Grammar rules (in `syntax/src/parsing.rs`) call methods on
//! `Parser` to navigate and consume tokens.

use std::cell::Cell;
use std::collections::HashMap;

use crate::event::Event;
use crate::input::Input;
use crate::syntax_kind::SyntaxKind;
use crate::token_set::TokenSet;

/// Maximum lookahead steps before the parser bails.
pub(crate) const PARSER_STEP_LIMIT: u32 = if cfg!(debug_assertions) { 150_000 } else { 15_000_000 };

/// Fixity context for dynamic operator precedence.
/// Maps operator name → (left_bp, right_bp).
pub type FixityContext = HashMap<String, (u8, u8)>;

/// Event-based parser cursor.
///
/// Operates on a pre-lexed `Input` of non-trivia tokens.
pub struct Parser<'t> {
    inp: &'t Input,
    pos: usize,
    pub(crate) events: Vec<Event>,
    steps: Cell<u32>,
    /// Dynamic fixity context for user-defined operators.
    pub(crate) fixities: FixityContext,
}

impl<'t> Parser<'t> {
    /// Create a parser for the given input.
    pub fn new(inp: &'t Input) -> Parser<'t> {
        Parser {
            inp,
            pos: 0,
            events: Vec::new(),
            steps: Cell::new(0),
            fixities: FixityContext::new(),
        }
    }

    /// Create a parser with a fixity context for dynamic operator precedence.
    pub fn new_with_fixities(inp: &'t Input, fixities: FixityContext) -> Parser<'t> {
        Parser { inp, pos: 0, events: Vec::new(), steps: Cell::new(0), fixities }
    }

    /// Finish parsing and return accumulated events.
    pub fn finish(self) -> Vec<Event> {
        self.events
    }

    /// Kind of the current token.
    pub fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    /// Lookahead: kind of the n-th non-trivia token ahead.
    pub fn nth(&self, n: usize) -> SyntaxKind {
        let steps = self.steps.get();
        if steps > PARSER_STEP_LIMIT {
            return SyntaxKind::EOF;
        }
        self.steps.set(steps + 1);
        self.inp.kind(self.pos + n)
    }

    /// Check if the current token matches `kind`.
    pub fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    /// Check if the n-th token matches `kind`.
    #[allow(dead_code)]
    pub fn nth_at(&self, n: usize, kind: SyntaxKind) -> bool {
        self.nth(n) == kind
    }

    /// Check if the current token is in the given set.
    pub fn at_ts(&self, kinds: &TokenSet) -> bool {
        kinds.contains(self.current())
    }

    /// Check if at end of input.
    pub fn at_end(&self) -> bool {
        self.at(SyntaxKind::EOF)
    }

    /// Text of the current non-trivia token.
    ///
    /// Sail-specific: needed for dynamic fixity lookup .
    /// Reads from `Input::token_text`.
    pub fn current_text(&self) -> &str {
        self.inp.token_text(self.pos)
    }

    /// Current position in the non-trivia token stream.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Consume the current token if it matches `kind`.
    pub fn eat(&mut self, kind: SyntaxKind) -> bool {
        if !self.at(kind) {
            return false;
        }
        self.do_bump(kind, 1);
        true
    }

    /// Assert and consume the current token.
    #[allow(dead_code)]
    pub fn bump(&mut self, kind: SyntaxKind) {
        assert!(self.eat(kind), "expected {:?}, got {:?}", kind, self.current());
    }

    /// Advance by one token regardless of kind.
    pub fn bump_any(&mut self) {
        let kind = self.current();
        if kind == SyntaxKind::EOF {
            return;
        }
        self.do_bump(kind, 1);
    }

    /// Advance by one token, remapping its kind.
    #[allow(dead_code)]
    pub fn bump_remap(&mut self, kind: SyntaxKind) {
        if self.current() == SyntaxKind::EOF {
            // TODO: emit error?
            return;
        }
        self.do_bump(kind, 1);
    }

    /// Eat or emit an error.
    pub fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(format!("expected {:?}", kind));
        false
    }

    /// Start a new composite node.
    pub fn start(&mut self) -> Marker {
        let pos = self.events.len();
        self.events.push(Event::Start { kind: SyntaxKind::TOMBSTONE, forward_parent: None });
        Marker {
            pos: pos as u32,
            bomb: stdx::DropBomb::new("Marker must be completed or abandoned"),
        }
    }

    /// Emit a parse error.
    pub fn error(&mut self, msg: impl Into<String>) {
        self.events.push(Event::Error { msg: msg.into() });
    }

    /// Emit an error and wrap the current token in an ERROR node.
    pub fn err_and_bump(&mut self, message: &str) {
        let m = self.start();
        self.error(message.to_string());
        self.bump_any();
        m.complete(self, SyntaxKind::ERROR);
    }

    /// Error recovery: if not at a recovery token, consume one.
    #[allow(dead_code)]
    pub fn err_recover(&mut self, message: &str, recovery: &TokenSet) -> bool {
        if self.at_ts(recovery) || self.at_end() {
            self.error(message.to_string());
            return true;
        }
        self.err_and_bump(message);
        false
    }

    //          ra fills it with the actual count for composite token advancement.
    fn do_bump(&mut self, kind: SyntaxKind, n_raw_tokens: u8) {
        self.pos += n_raw_tokens as usize;
        self.steps.set(0); // reset fuel — progress was made
        self.events.push(Event::Token {
            kind,
            n_raw_tokens: 0, // placeholder; tree builder resolves actual bytes
        });
    }
}

/// A marker for a not-yet-completed syntax node.
pub struct Marker {
    pos: u32,
    bomb: stdx::DropBomb,
}

impl Marker {
    /// Complete this marker, assigning the given `SyntaxKind`.
    pub fn complete(mut self, p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
        self.bomb.defuse();
        let end_pos = p.events.len() as u32;
        if let Event::Start { kind: slot, .. } = &mut p.events[self.pos as usize] {
            *slot = kind;
        }
        p.events.push(Event::Finish);
        CompletedMarker { start_pos: self.pos, _end_pos: end_pos, kind }
    }

    /// Abandon this marker without producing a node.
    pub fn abandon(mut self, p: &mut Parser<'_>) {
        self.bomb.defuse();
        if let Event::Start { kind, .. } = &mut p.events[self.pos as usize] {
            *kind = SyntaxKind::TOMBSTONE;
        }
    }
}

/// A marker for a completed syntax node.
#[derive(Clone, Copy)]
pub struct CompletedMarker {
    start_pos: u32,
    _end_pos: u32,
    #[allow(dead_code)]
    kind: SyntaxKind,
}

impl CompletedMarker {
    /// Create a new parent node wrapping this completed node.
    ///
    /// Uses `forward_parent` to re-parent: the new `Start` event
    /// points back to this one, so `build_green` emits the new parent
    /// *before* this completed node.
    pub fn precede(self, p: &mut Parser<'_>) -> Marker {
        let new_pos = p.events.len();
        p.events.push(Event::Start { kind: SyntaxKind::TOMBSTONE, forward_parent: None });
        if let Event::Start { forward_parent, .. } = &mut p.events[self.start_pos as usize] {
            *forward_parent = Some((new_pos - self.start_pos as usize) as u32);
        }
        Marker {
            pos: new_pos as u32,
            bomb: stdx::DropBomb::new("Marker must be completed or abandoned"),
        }
    }

    /// Extend to the left, wrapping the given earlier marker.
    #[allow(dead_code)]
    pub fn extend_to(self, p: &mut Parser<'_>, mut m: Marker) -> CompletedMarker {
        m.bomb.defuse();
        let end_pos = p.events.len() as u32;
        if let Event::Start { kind, .. } = &mut p.events[m.pos as usize] {
            *kind = self.kind;
        }
        p.events.push(Event::Finish);
        CompletedMarker { start_pos: m.pos, _end_pos: end_pos, kind: self.kind }
    }

    /// The `SyntaxKind` of this completed node.
    #[allow(dead_code)]
    pub fn kind(&self) -> SyntaxKind {
        self.kind
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_parse() {
        let input = Input::with_capacity(0);
        let p = Parser::new(&input);
        assert_eq!(p.current(), SyntaxKind::EOF);
        let events = p.finish();
        assert!(events.is_empty());
    }

    #[test]
    fn basic_markers() {
        let mut input = Input::with_capacity(2);
        input.push(SyntaxKind::KW_VAL, 3);
        input.push(SyntaxKind::IDENT, 1);

        let mut p = Parser::new(&input);
        let m = p.start();
        p.bump(SyntaxKind::KW_VAL);
        p.bump(SyntaxKind::IDENT);
        m.complete(&mut p, SyntaxKind::CALLABLE_SPEC);

        let events = p.finish();
        // Start + Token + Token + Finish
        assert_eq!(events.len(), 4);
        assert!(
            matches!(&events[0], Event::Start { kind, .. } if *kind == SyntaxKind::CALLABLE_SPEC)
        );
        assert!(matches!(&events[3], Event::Finish));
    }

    #[test]
    fn precede_creates_forward_parent() {
        let mut input = Input::with_capacity(3);
        input.push(SyntaxKind::IDENT, 1);
        input.push(SyntaxKind::PLUS, 1);
        input.push(SyntaxKind::IDENT, 1);

        let mut p = Parser::new(&input);
        let m = p.start();
        p.bump(SyntaxKind::IDENT);
        let lhs = m.complete(&mut p, SyntaxKind::IDENT_EXPR);

        // Now wrap lhs in an infix expression
        let m2 = lhs.precede(&mut p);
        p.bump(SyntaxKind::PLUS);
        let m3 = p.start();
        p.bump(SyntaxKind::IDENT);
        m3.complete(&mut p, SyntaxKind::IDENT_EXPR);
        m2.complete(&mut p, SyntaxKind::BIN_EXPR);

        let events = p.finish();
        // Should have forward_parent set on the first Start event
        if let Event::Start { forward_parent, .. } = &events[0] {
            assert!(forward_parent.is_some());
        } else {
            panic!("expected Start event");
        }
    }

    #[test]
    fn abandon_does_not_panic() {
        let input = Input::with_capacity(0);
        let mut p = Parser::new(&input);
        let m = p.start();
        m.abandon(&mut p);
        let events = p.finish();
        assert_eq!(events.len(), 1); // Just the TOMBSTONE Start
    }

    #[test]
    fn error_recovery() {
        let mut input = Input::with_capacity(2);
        input.push(SyntaxKind::IDENT, 3);
        input.push(SyntaxKind::SEMICOLON, 1);

        let recovery = TokenSet::new(&[SyntaxKind::SEMICOLON]);
        let mut p = Parser::new(&input);
        // Not at recovery, so err_recover will bump
        p.err_recover("unexpected token", &recovery);
        // Now should be at SEMICOLON
        assert!(p.at(SyntaxKind::SEMICOLON));
    }

    #[test]
    fn step_limit_returns_eof() {
        let mut input = Input::with_capacity(1);
        input.push(SyntaxKind::IDENT, 1);

        let p = Parser::new(&input);
        // Exhaust the step limit
        p.steps.set(PARSER_STEP_LIMIT + 1);
        assert_eq!(p.current(), SyntaxKind::EOF);
    }
}
