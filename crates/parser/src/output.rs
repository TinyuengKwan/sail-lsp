//! Packed parse output.
//!
//! (a compact event stream) which the syntax crate's tree builder
//! converts into a rowan `GreenNode`.
//!
//! Events are packed into `u32` words for memory efficiency:
//! ```text
//! |  16 bit kind  |  8 bit n_input_tokens  |  4 bit tag  |  4 bit extra  |
//! ```

use crate::syntax_kind::SyntaxKind;

const TAG_TOKEN: u8 = 0;
const TAG_ENTER: u8 = 1;
const TAG_EXIT: u8 = 2;
const TAG_ERROR: u8 = 3;

/// Compact parse tree output.
pub struct Output {
    event: Vec<u32>,
    error: Vec<String>,
}

/// A single step when iterating over `Output`.
///
/// No `FloatSplit` variant — Sail has no float literal ambiguity.
pub enum Step<'a> {
    /// A token was consumed from the input.
    Token { kind: SyntaxKind, n_input_tokens: u8 },
    /// Enter a composite node.
    Enter { kind: SyntaxKind },
    /// Exit a composite node.
    Exit,
    /// A parse error.
    Error { msg: &'a str },
}

impl Output {
    pub(crate) fn new() -> Self {
        Self { event: Vec::new(), error: Vec::new() }
    }

    /// Iterate over all steps in the output.
    pub fn iter(&self) -> impl Iterator<Item = Step<'_>> {
        let mut error_idx = 0usize;
        let error = &self.error;
        self.event.iter().map(move |&packed| {
            let tag = (packed & 0xF) as u8;
            let extra = ((packed >> 4) & 0xF) as u8;
            let n_input_tokens = ((packed >> 8) & 0xFF) as u8;
            let kind_raw = (packed >> 16) as u16;
            match tag {
                TAG_TOKEN => Step::Token { kind: SyntaxKind::from(kind_raw), n_input_tokens },
                TAG_ENTER => Step::Enter { kind: SyntaxKind::from(kind_raw) },
                TAG_EXIT => Step::Exit,
                TAG_ERROR => {
                    let idx = error_idx;
                    error_idx += 1;
                    Step::Error { msg: error.get(idx).map(|s| s.as_str()).unwrap_or("") }
                }
                _ => {
                    // Treat unknown tags as exit to avoid panic.
                    let _ = extra;
                    Step::Exit
                }
            }
        })
    }
}

impl Output {
    /// Emit a token event.
    pub(crate) fn token(&mut self, kind: SyntaxKind, n_tokens: u8) {
        let kind_bits = (kind as u16 as u32) << 16;
        let n_bits = (n_tokens as u32) << 8;
        self.event.push(kind_bits | n_bits | TAG_TOKEN as u32);
    }

    /// Enter a composite node.
    pub(crate) fn enter_node(&mut self, kind: SyntaxKind) {
        let kind_bits = (kind as u16 as u32) << 16;
        self.event.push(kind_bits | TAG_ENTER as u32);
    }

    /// Exit a composite node.
    pub(crate) fn leave_node(&mut self) {
        self.event.push(TAG_EXIT as u32);
    }

    /// Record a parse error.
    pub(crate) fn error(&mut self, error: String) {
        self.error.push(error);
        self.event.push(TAG_ERROR as u32);
    }
}

// SyntaxKind From<u16> impl lives in syntax_kind.rs (the wrapper module).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_token() {
        let mut out = Output::new();
        out.token(SyntaxKind::IDENT, 1);
        let steps: Vec<_> = out.iter().collect();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            Step::Token { kind, n_input_tokens } => {
                assert_eq!(*kind, SyntaxKind::IDENT);
                assert_eq!(*n_input_tokens, 1);
            }
            _ => panic!("expected Token"),
        }
    }

    #[test]
    fn round_trip_enter_exit() {
        let mut out = Output::new();
        out.enter_node(SyntaxKind::SOURCE_FILE);
        out.token(SyntaxKind::KW_VAL, 1);
        out.leave_node();
        let steps: Vec<_> = out.iter().collect();
        assert_eq!(steps.len(), 3);
        assert!(matches!(&steps[0], Step::Enter { kind } if *kind == SyntaxKind::SOURCE_FILE));
        assert!(matches!(&steps[1], Step::Token { kind, .. } if *kind == SyntaxKind::KW_VAL));
        assert!(matches!(&steps[2], Step::Exit));
    }

    #[test]
    fn round_trip_error() {
        let mut out = Output::new();
        out.error("expected `;`".to_string());
        let steps: Vec<_> = out.iter().collect();
        assert_eq!(steps.len(), 1);
        match &steps[0] {
            Step::Error { msg } => assert_eq!(*msg, "expected `;`"),
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn mixed_events() {
        let mut out = Output::new();
        out.enter_node(SyntaxKind::SOURCE_FILE);
        out.enter_node(SyntaxKind::CALLABLE_DEF);
        out.token(SyntaxKind::KW_FUNCTION, 1);
        out.token(SyntaxKind::IDENT, 1);
        out.error("expected `(`".to_string());
        out.leave_node();
        out.leave_node();
        let steps: Vec<_> = out.iter().collect();
        assert_eq!(steps.len(), 7);
    }
}
