//! Highlight Related — paired keyword and reference highlighting.
//! When the cursor is on a control-flow keyword or identifier,
//! highlights the paired/related ranges in the same construct.

use ide_db::line_index::TextRange;
use ide_db::{FileDb, LineCol};
use parser::{Span, Token};

/// A range to highlight, with a category indicating its role.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HighlightedRange {
    /// Byte-offset range to highlight.
    pub range: TextRange,
    /// Reference category for this highlight.
    ///
    /// We use a string tag: "read", "write", "keyword".
    pub category: ReferenceCategory,
}

/// Category for a highlighted range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReferenceCategory {
    /// Read access to a binding.
    Read,
    /// Write/definition of a binding.
    Write,
    /// Control-flow keyword pairing.
    Keyword,
}

/// Configuration for highlight-related functionality.
#[derive(Default, Clone)]
pub struct HighlightRelatedConfig {
    /// Highlight all references to the symbol under cursor.
    pub references: bool,
    /// Highlight exit points (return/throw/exit) when on `function`/`return`.
    pub exit_points: bool,
    /// Highlight break points in loops.
    pub break_points: bool,
    /// Highlight branch exit points (match/if arms).
    pub branch_exit_points: bool,
}

/// Compute highlight-related ranges for the token at `position`.
/// Returns:
/// - keyword pairs for control-flow constructs (match↔=>, if↔else, try↔catch)
/// - all references to the identifier under cursor
/// - exit points when on function/return keywords
pub fn highlight_related(
    file: &dyn FileDb,
    config: &HighlightRelatedConfig,
    position: LineCol,
) -> Vec<HighlightedRange> {
    let (token, span) = match file.token_at(position) {
        Some(t) => t,
        None => return Vec::new(),
    };

    match token {
        // Symbol references
        Token::Id(_) | Token::TyVal(_) if config.references => highlight_references(file, position),
        // Control-flow keywords: highlight paired keywords
        Token::KwMatch if config.branch_exit_points => highlight_match_arms(file, *span),
        Token::KwIf if config.branch_exit_points => highlight_if_else(file, *span),
        Token::KwElse if config.branch_exit_points => highlight_if_else(file, *span),
        Token::KwTry if config.branch_exit_points => highlight_try_catch(file, *span),
        Token::KwCatch if config.branch_exit_points => highlight_try_catch(file, *span),
        // Loop keywords
        Token::KwForeach | Token::KwWhile if config.break_points => {
            vec![HighlightedRange {
                range: base_db::text_range(span.start, span.end),
                category: ReferenceCategory::Keyword,
            }]
        }
        // Return/throw/exit: highlight exit points
        Token::KwReturn | Token::KwThrow if config.exit_points => {
            highlight_exit_points(file, *span)
        }
        _ => Vec::new(),
    }
}

/// Backward-compat entry point (no config, all features enabled).
pub fn highlight_related_default(file: &dyn FileDb, position: LineCol) -> Vec<HighlightedRange> {
    let config = HighlightRelatedConfig {
        references: true,
        exit_points: true,
        break_points: true,
        branch_exit_points: true,
    };
    highlight_related(file, &config, position)
}

/// Highlight all references to the symbol at cursor position.
fn highlight_references(file: &dyn FileDb, position: LineCol) -> Vec<HighlightedRange> {
    let symbol = match crate::references::resolve_symbol_at(file, position) {
        Some(s) => s,
        None => return Vec::new(),
    };
    crate::references::symbol_spans_for_file(file, &symbol, true)
        .into_iter()
        .map(|(span, is_write)| HighlightedRange {
            range: base_db::text_range(span.start, span.end),
            category: if is_write { ReferenceCategory::Write } else { ReferenceCategory::Read },
        })
        .collect()
}

/// Highlight exit points (return/throw/exit) for the containing function.
fn highlight_exit_points(file: &dyn FileDb, trigger: Span) -> Vec<HighlightedRange> {
    let tokens = match file.tokens() {
        Some(t) => t,
        None => return Vec::new(),
    };
    let mut result = vec![HighlightedRange {
        range: base_db::text_range(trigger.start, trigger.end),
        category: ReferenceCategory::Keyword,
    }];

    // Find the enclosing function by scanning backward for `function` keyword
    let mut fn_start = None;
    for (tok, span) in tokens.iter() {
        if span.start >= trigger.start {
            break;
        }
        if matches!(tok, Token::KwFunction) {
            fn_start = Some(span.start);
        }
    }

    let fn_start = match fn_start {
        Some(s) => s,
        None => return result,
    };

    // Scan forward from function start, find matching { }, collect return/throw/exit
    let mut depth = 0i32;
    let mut in_fn = false;
    for (tok, span) in tokens.iter() {
        if span.start < fn_start {
            continue;
        }
        match tok {
            Token::KwFunction if span.start == fn_start => {
                // Highlight the function keyword too
                result.push(HighlightedRange {
                    range: base_db::text_range(span.start, span.end),
                    category: ReferenceCategory::Keyword,
                });
            }
            Token::LeftCurlyBracket => {
                depth += 1;
                in_fn = true;
            }
            Token::RightCurlyBracket if in_fn => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            Token::KwReturn | Token::KwThrow
                if in_fn && depth > 0 && span.start != trigger.start =>
            {
                result.push(HighlightedRange {
                    range: base_db::text_range(span.start, span.end),
                    category: ReferenceCategory::Keyword,
                });
            }
            _ => {}
        }
    }

    result.sort_by_key(|r| r.range.start());
    result.dedup_by_key(|r| r.range.start());
    result
}

/// Highlight `match` keyword and all `=>` arms in the same match block.
fn highlight_match_arms(file: &dyn FileDb, trigger: Span) -> Vec<HighlightedRange> {
    let tokens = match file.tokens() {
        Some(t) => t,
        None => return Vec::new(),
    };
    let mut result = vec![HighlightedRange {
        range: base_db::text_range(trigger.start, trigger.end),
        category: ReferenceCategory::Keyword,
    }];

    let mut depth = 0i32;
    let mut in_match = false;
    for (tok, span) in tokens {
        if span.start < trigger.start {
            continue;
        }
        match tok {
            Token::KwMatch if span.start == trigger.start => {
                in_match = true;
            }
            Token::LeftCurlyBracket if in_match => {
                depth += 1;
            }
            Token::RightCurlyBracket if in_match => {
                depth -= 1;
                if depth <= 0 {
                    break;
                }
            }
            Token::FatRightArrow if in_match && depth == 1 => {
                result.push(HighlightedRange {
                    range: base_db::text_range(span.start, span.end),
                    category: ReferenceCategory::Keyword,
                });
            }
            _ => {}
        }
    }
    result
}

/// Highlight paired `if`/`else` keywords.
fn highlight_if_else(file: &dyn FileDb, trigger: Span) -> Vec<HighlightedRange> {
    let tokens = match file.tokens() {
        Some(t) => t,
        None => return Vec::new(),
    };
    let mut result = vec![HighlightedRange {
        range: base_db::text_range(trigger.start, trigger.end),
        category: ReferenceCategory::Keyword,
    }];

    for (tok, span) in tokens {
        if span.start == trigger.start {
            continue;
        }
        let distance = span.start.abs_diff(trigger.start);
        if distance > 2000 {
            continue;
        }

        match tok {
            Token::KwIf | Token::KwElse => {
                result.push(HighlightedRange {
                    range: base_db::text_range(span.start, span.end),
                    category: ReferenceCategory::Keyword,
                });
            }
            _ => {}
        }
    }

    result.sort_by_key(|r| r.range.start());
    result.dedup_by_key(|r| r.range.start());
    result
}

/// Highlight paired `try`/`catch` keywords.
fn highlight_try_catch(file: &dyn FileDb, trigger: Span) -> Vec<HighlightedRange> {
    let tokens = match file.tokens() {
        Some(t) => t,
        None => return Vec::new(),
    };
    let mut result = vec![HighlightedRange {
        range: base_db::text_range(trigger.start, trigger.end),
        category: ReferenceCategory::Keyword,
    }];

    for (tok, span) in tokens {
        if span.start == trigger.start {
            continue;
        }
        let distance = span.start.abs_diff(trigger.start);
        if distance > 2000 {
            continue;
        }

        match tok {
            Token::KwTry | Token::KwCatch => {
                result.push(HighlightedRange {
                    range: base_db::text_range(span.start, span.end),
                    category: ReferenceCategory::Keyword,
                });
            }
            _ => {}
        }
    }
    result
}
