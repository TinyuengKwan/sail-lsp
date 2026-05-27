//! Parser event protocol and GreenNode tree sink.
//!
//! The parser emits a stream of `Event`s; the `TreeSink` converts
//! them into a rowan `GreenNode`.

use crate::syntax_kind::SyntaxKind;

/// A single parser event.
#[derive(Debug, Clone)]
pub enum Event {
    /// Start a composite node.
    Start {
        kind: SyntaxKind,
        /// For left-recursive re-wrapping: the index of a prior
        /// `Start` event that this node should precede. `None` for
        /// normal forward starts.
        forward_parent: Option<u32>,
    },
    /// Finish the most recently started node.
    Finish,
    /// Consume one token from the input.
    Token {
        kind: SyntaxKind,
        /// Number of raw tokens this token covers in the input.
        /// Always 0 in sail-lsp (unused); ra uses this to advance the raw cursor.
        #[allow(dead_code)]
        n_raw_tokens: usize,
    },
    /// A parse error at the current position.
    Error { msg: String },
}

/// A parse error with a byte offset.
#[derive(Debug, Clone)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

// Note: The GreenNode builder (TreeSink) lives in
// `syntax/src/parsing/mod.rs::build_tree_from_output` which correctly handles
// forward_parent chains. The legacy `build_tree` that ignored
// forward_parent was removed in .

/// Convert a flat event list into an `Output`.
///
/// Resolves `forward_parent` chains so the Output is a proper tree.
pub fn process(mut events: Vec<Event>) -> crate::output::Output {
    let mut output = crate::output::Output::new();
    let mut forward_parents = Vec::new();
    let mut i = 0;

    while i < events.len() {
        match &events[i] {
            Event::Start { kind, forward_parent } => {
                // Abandoned marker — skip
                if *kind == SyntaxKind::TOMBSTONE && forward_parent.is_none() {
                    i += 1;
                    continue;
                }

                // Collect forward_parent chain
                forward_parents.clear();
                let mut idx = i;
                loop {
                    forward_parents.push(idx);
                    let Event::Start { forward_parent, .. } = &events[idx] else {
                        break;
                    };
                    let Some(delta) = forward_parent else { break };
                    idx += *delta as usize;
                }

                // Emit parents outermost-first (reverse order)
                for &fp_idx in forward_parents.iter().rev() {
                    if let Event::Start { kind, .. } = &events[fp_idx] {
                        if *kind != SyntaxKind::TOMBSTONE {
                            output.enter_node(*kind);
                        }
                    }
                    // Mark as processed
                    if fp_idx != i {
                        events[fp_idx] =
                            Event::Start { kind: SyntaxKind::TOMBSTONE, forward_parent: None };
                    }
                }
            }
            Event::Finish => {
                output.leave_node();
            }
            Event::Token { kind, .. } => {
                // n_raw_tokens is ignored in Output; tree builder resolves
                // actual text from the full token stream.
                output.token(*kind, 1);
            }
            Event::Error { msg } => {
                output.error(msg.clone());
            }
        }
        i += 1;
    }
    output
}
