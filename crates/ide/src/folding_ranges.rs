//! CST-aware folding ranges.
//!
//! Walk CST `descendants_with_tokens()` preorder,
//! classify each node by `SyntaxKind` into `FoldKind`.
//! Supports blocks, comments, consecutive $include groups,
//! function definitions, struct/enum/union bodies, match arms,
//! and `// region:` / `// endregion` markers.

use ide_db::FileDb;
use parser::SyntaxKind as SK;
use syntax::SyntaxNode;

/// Kind of fold — determines the FoldingRangeKind in LSP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    /// `{ ... }` block body
    Block,
    /// Consecutive line comments or block comment
    Comment,
    /// Consecutive `$include` directives
    Imports,
    /// `function ... { ... }` or `val ... : ...`
    Function,
    /// `struct ... { ... }`
    Struct,
    /// `enum ... { ... }` or `union ... { ... }`
    Enum,
    /// `match ... { arm => ... }`
    MatchArm,
    /// `// region:` ... `// endregion`
    Region,
}

// FoldKind → LSP conversion done in to_proto.rs (lsp_types confined to binary crate)

/// A single fold range.
pub struct Fold {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: FoldKind,
}

/// Compute folding ranges by walking the CST.
pub fn folding_ranges(file: &dyn FileDb) -> Vec<Fold> {
    let _text = file.text();
    let mut folds = Vec::new();

    // Strategy 1: CST node-based folding (blocks, definitions, match arms)
    // Access the GreenNode via salsa parse query
    if let Some(root) = cst_root_node(file) {
        cst_fold_ranges(file, &root, &mut folds);
    }

    // Strategy 2: Token-based folding (comments, $include groups, regions)
    if let Some(tokens) = file.tokens() {
        comment_fold_ranges(file, tokens, &mut folds);
        include_fold_ranges(file, tokens, &mut folds);
        region_fold_ranges(file, tokens, &mut folds);
    }

    folds
}

/// Get the CST root SyntaxNode.
fn cst_root_node(file: &dyn FileDb) -> Option<SyntaxNode> {
    let text = file.text();
    let (root, _) = syntax::parse_text(text);
    Some(root)
}

/// Walk CST descendants, classify multi-line nodes as folds.
fn cst_fold_ranges(file: &dyn FileDb, root: &SyntaxNode, folds: &mut Vec<Fold>) {
    use rowan::WalkEvent;

    for event in root.preorder() {
        let WalkEvent::Enter(node) = event else {
            continue;
        };
        let kind = node.kind();

        let fold_kind = match kind {
            // Top-level definitions
            SK::CALLABLE_DEF => Some(FoldKind::Function),
            SK::CALLABLE_SPEC => Some(FoldKind::Function),
            SK::NAMED_DEF => {
                // Check first child keyword to determine struct/enum/union/etc.
                let first_kw =
                    node.children_with_tokens().find_map(|c| c.as_token().map(|t| t.kind()));
                match first_kw {
                    Some(SK::KW_STRUCT) | Some(SK::KW_BITFIELD) => Some(FoldKind::Struct),
                    Some(SK::KW_ENUM) | Some(SK::KW_UNION) => Some(FoldKind::Enum),
                    _ => Some(FoldKind::Block),
                }
            }
            SK::SCATTERED_DEF | SK::SCATTERED_CLAUSE_DEF => Some(FoldKind::Function),

            // Expression blocks
            SK::BLOCK_EXPR => Some(FoldKind::Block),
            SK::MATCH_EXPR => Some(FoldKind::Block),
            SK::IF_EXPR => Some(FoldKind::Block),
            SK::TRY_EXPR => Some(FoldKind::Block),
            SK::FOREACH_EXPR => Some(FoldKind::Block),
            SK::WHILE_EXPR => Some(FoldKind::Block),

            // Sub-structures
            SK::MATCH_ARM => Some(FoldKind::MatchArm),
            SK::PARAM_LIST => Some(FoldKind::Block),
            SK::ARG_LIST => Some(FoldKind::Block),
            SK::TYPE_PARAM_LIST => Some(FoldKind::Block),

            _ => None,
        };

        if let Some(kind) = fold_kind {
            // Only fold multi-line nodes
            let text_range = node.text_range();
            let start = file.position_at(text_range.start().into());
            let end = file.position_at(text_range.end().into());
            if end.line > start.line {
                folds.push(Fold { start_line: start.line, end_line: end.line, kind });
            }
        }
    }
}

/// Group consecutive line comments into one fold.
fn comment_fold_ranges(
    file: &dyn FileDb,
    tokens: &[(parser::Token, parser::Span)],
    folds: &mut Vec<Fold>,
) {
    let mut comment_start: Option<u32> = None;
    let mut comment_end: u32 = 0;
    let mut prev_was_comment = false;

    for (token, span) in tokens {
        let _is_comment = matches!(token,
            parser::Token::Id(_) if false, // placeholder — actual check below
        );
        // Check via span in text
        let text = file.text();
        let token_text = text.get(span.start..span.end).unwrap_or("");
        let is_line_comment = token_text.starts_with("//") && !token_text.starts_with("///");
        let is_block_comment = token_text.starts_with("/*");

        if is_block_comment {
            let start = file.position_at(span.start);
            let end = file.position_at(span.end);
            if end.line > start.line {
                folds.push(Fold {
                    start_line: start.line,
                    end_line: end.line,
                    kind: FoldKind::Comment,
                });
            }
            prev_was_comment = false;
            continue;
        }

        if is_line_comment {
            let line = file.position_at(span.start).line;
            if prev_was_comment && line == comment_end + 1 {
                comment_end = line;
            } else {
                // Emit previous group
                if prev_was_comment && comment_end > comment_start.unwrap_or(0) {
                    folds.push(Fold {
                        start_line: comment_start.unwrap(),
                        end_line: comment_end,
                        kind: FoldKind::Comment,
                    });
                }
                comment_start = Some(line);
                comment_end = line;
            }
            prev_was_comment = true;
        } else {
            if prev_was_comment && comment_end > comment_start.unwrap_or(0) {
                folds.push(Fold {
                    start_line: comment_start.unwrap(),
                    end_line: comment_end,
                    kind: FoldKind::Comment,
                });
            }
            prev_was_comment = false;
        }
    }
    // Final group
    if prev_was_comment && comment_end > comment_start.unwrap_or(0) {
        folds.push(Fold {
            start_line: comment_start.unwrap(),
            end_line: comment_end,
            kind: FoldKind::Comment,
        });
    }
}

/// Group consecutive $include directives into one fold.
fn include_fold_ranges(
    file: &dyn FileDb,
    tokens: &[(parser::Token, parser::Span)],
    folds: &mut Vec<Fold>,
) {
    let text = file.text();
    let mut include_start: Option<u32> = None;
    let mut include_end: u32 = 0;

    for (token, span) in tokens {
        let token_text = text.get(span.start..span.end).unwrap_or("");
        let is_include =
            matches!(token, parser::Token::Directive { .. }) && token_text.contains("include");

        if is_include {
            let line = file.position_at(span.start).line;
            if include_start.is_some() && line <= include_end + 2 {
                include_end = line;
            } else {
                if let Some(start) = include_start {
                    if include_end > start {
                        folds.push(Fold {
                            start_line: start,
                            end_line: include_end,
                            kind: FoldKind::Imports,
                        });
                    }
                }
                include_start = Some(line);
                include_end = line;
            }
        }
    }
    if let Some(start) = include_start {
        if include_end > start {
            folds.push(Fold { start_line: start, end_line: include_end, kind: FoldKind::Imports });
        }
    }
}

/// `// region:` ... `// endregion` markers (LIFO stack).
fn region_fold_ranges(
    file: &dyn FileDb,
    tokens: &[(parser::Token, parser::Span)],
    folds: &mut Vec<Fold>,
) {
    let text = file.text();
    let mut region_starts: Vec<u32> = Vec::new();

    for (_token, span) in tokens {
        let token_text = text.get(span.start..span.end).unwrap_or("");
        if token_text.starts_with("// region:") || token_text.starts_with("// region ") {
            region_starts.push(file.position_at(span.start).line);
        } else if token_text.starts_with("// endregion") {
            if let Some(start_line) = region_starts.pop() {
                let end_line = file.position_at(span.start).line;
                if end_line > start_line {
                    folds.push(Fold { start_line, end_line, kind: FoldKind::Region });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests require FileDb impl — covered by integration tests in sail-lsp/tests.rs
}
