//! Extend/shrink selection by AST node granularity.
//!
//! current selection to the next larger CST node, enabling
//! structural selection (Shift+Alt+Right in VS Code).

use base_db::TextRange;
use ide_db::ide_types::SelectionRange;
use ide_db::FileDb;

/// The whole `textDocument/selectionRange` chain for one cursor position.
///
/// rust-analyzer builds this by calling `Analysis::extend_selection` in a
/// loop until it stops growing (`handle_selection_range`), so the chain has
/// exactly the granularity the CST has. This lives in `ide` rather than in
/// the handler because a handler here converts and does not decide.
///
/// The previous implementation scraped the token stream instead: the token
/// under the cursor, every bracket pair spanning it, the enclosing
/// top-level item, the line, the file. Everything between a bracket pair
/// and its item was missing — a binary operation, a `let` statement, a
/// match arm, an argument — and "the line" is not a syntactic unit at all.
pub fn selection_range(file: &dyn FileDb, offset: usize) -> Option<SelectionRange> {
    let text = file.text();
    if text.is_empty() {
        return None;
    }
    let offset = offset.min(text.len());

    let mut ranges = vec![base_db::text_range(offset, offset)];
    loop {
        let cur = *ranges.last().expect("seeded above");
        let next = extend_selection(file, cur);
        if next == cur || !next.contains_range(cur) {
            break;
        }
        ranges.push(next);
        // The file's own range is the fixed point; a malformed tree that
        // never reaches it must not spin.
        if ranges.len() > 128 {
            break;
        }
    }
    // An empty seed is a cursor, not a selection: LSP wants the innermost
    // *node*, so drop it once something real sits above it.
    if ranges.len() > 1 && ranges[0].is_empty() {
        ranges.remove(0);
    }

    let mut parent: Option<Box<SelectionRange>> = None;
    for range in ranges.into_iter().rev() {
        parent = Some(Box::new(SelectionRange { range, parent }));
    }
    parent.map(|node| *node)
}

/// Extend the selection range to the next larger CST node.
///
/// Algorithm (following RA):
/// 1. Find the smallest CST node covering the current range
/// 2. Return that node's parent's range
///
/// If the selection already covers a full node, returns the parent node's range.
pub fn extend_selection(file: &dyn FileDb, range: TextRange) -> TextRange {
    let text = file.text();
    if text.is_empty() {
        return range;
    }

    let (root, _) = syntax::parse_text(text);

    let rowan_range = range;

    use parser::SyntaxKind as SK;

    // An empty range is a cursor, and a cursor sits *between* two tokens.
    // `covering_element` answers with whichever the offset happens to touch
    // first, so a cursor on the `1` of `let x = 1 + 2` came back as the
    // whitespace before it and the whole chain started one node too high.
    // rust-analyzer picks by priority instead (`extend_selection.rs`):
    // trivia loses to anything, and a word loses to nothing.
    if rowan_range.is_empty() {
        if let Some(token) = pick_token_at(&root, rowan_range.start()) {
            return token.text_range();
        }
    }

    // Find the smallest node covering the range
    let covering = root.covering_element(rowan_range);

    let node = match covering {
        rowan::NodeOrToken::Token(token) => {
            let token_range = token.text_range();
            if rowan_range != token_range {
                // E7: String/comment word-level expansion.
                // If cursor is inside a string or comment, first expand to word.
                if matches!(
                    token.kind(),
                    SK::STRING_LIT | SK::DOC_COMMENT | SK::LINE_COMMENT | SK::BLOCK_COMMENT
                ) {
                    if let Some(word_range) = extend_word_in_token(&token, rowan_range.start()) {
                        return base_db::text_range(
                            u32::from(word_range.start()) as usize,
                            u32::from(word_range.end()) as usize,
                        );
                    }
                }
                // Fallback: expand to full token
                return base_db::text_range(
                    u32::from(token_range.start()) as usize,
                    u32::from(token_range.end()) as usize,
                );
            }
            // Token fully selected → try comment grouping before going to parent.
            if let Some(group_range) = extend_comments(&token) {
                return base_db::text_range(
                    u32::from(group_range.start()) as usize,
                    u32::from(group_range.end()) as usize,
                );
            }
            // Go to parent node
            match token.parent() {
                Some(p) => p,
                None => return range,
            }
        }
        rowan::NodeOrToken::Node(node) => {
            if node.text_range() != rowan_range {
                return base_db::text_range(
                    u32::from(node.text_range().start()) as usize,
                    u32::from(node.text_range().end()) as usize,
                );
            }

            // Find shallowest node with same range.
            let shallowest = shallowest_node(&node);

            // E8: List item expansion.
            // If node's parent is a list (ARG_LIST, PARAM_LIST, etc.),
            // expand to include the adjacent comma separator.
            if let Some(parent) = shallowest.parent() {
                if is_list_kind(parent.kind()) {
                    if let Some(list_range) = extend_list_item(&shallowest) {
                        return base_db::text_range(
                            u32::from(list_range.start()) as usize,
                            u32::from(list_range.end()) as usize,
                        );
                    }
                }
            }

            // Go to parent
            match shallowest.parent() {
                Some(p) => p,
                None => return range, // already at root
            }
        }
    };

    // The parent can have exactly the range it was handed: `covering_element`
    // answers with the deepest element, so a literal comes back as the
    // `NUM_LIT` token whose parent `LITERAL_EXPR` spans the same byte. Landing
    // there is not an extension, and the chain stopped at the token because of
    // it. Climb until the range actually grows.
    let mut node = node;
    while node.text_range() == rowan_range {
        match node.parent() {
            Some(p) => node = p,
            None => return range,
        }
    }

    let result = node.text_range();
    base_db::text_range(u32::from(result.start()) as usize, u32::from(result.end()) as usize)
}

/// The token a cursor at `offset` belongs to.
///
/// Between two tokens the more interesting one wins, as in rust-analyzer:
/// a name or a keyword over punctuation, punctuation over trivia.
fn pick_token_at(
    root: &syntax::SyntaxNode,
    offset: rowan::TextSize,
) -> Option<syntax::SyntaxToken> {
    use parser::SyntaxKind as SK;
    fn priority(kind: parser::SyntaxKind) -> u8 {
        if kind.is_trivia() || kind == SK::DOC_COMMENT {
            0
        } else if matches!(kind, SK::IDENT | SK::STRING_LIT | SK::NUM_LIT) {
            3
        } else {
            2
        }
    }
    match root.token_at_offset(offset) {
        rowan::TokenAtOffset::None => None,
        rowan::TokenAtOffset::Single(t) => Some(t),
        rowan::TokenAtOffset::Between(left, right) => {
            if priority(right.kind()) >= priority(left.kind()) {
                Some(right)
            } else {
                Some(left)
            }
        }
    }
}

// There is no `shrink_selection`. LSP has no shrink request: the server
// answers `textDocument/selectionRange` with a *chain* — a range and its
// parent, and its parent — and a client shrinks by walking back down the
// chain it already has. A function that recomputed a smaller range served
// an operation that does not exist, which is why it had no caller.

/// Expand selection to word boundaries within a string/comment token.
///
/// (`extend_selection.rs:170-220`).
fn extend_word_in_token(
    token: &syntax::SyntaxToken,
    offset: rowan::TextSize,
) -> Option<rowan::TextRange> {
    let text = token.text();
    let token_start = token.text_range().start();
    let relative = u32::from(offset - token_start) as usize;
    if relative >= text.len() {
        return None;
    }
    // Find word boundaries (whitespace/punctuation delimited)
    let bytes = text.as_bytes();
    let is_boundary = |c: u8| {
        c.is_ascii_whitespace()
            || matches!(c, b',' | b';' | b'(' | b')' | b'{' | b'}' | b'[' | b']')
    };
    let start = (0..relative).rev().find(|&i| is_boundary(bytes[i])).map(|i| i + 1).unwrap_or(0);
    let end = (relative..text.len()).find(|&i| is_boundary(bytes[i])).unwrap_or(text.len());
    if start == 0 && end == text.len() {
        return None; // Already at full token content
    }
    if start >= end {
        return None;
    }
    Some(rowan::TextRange::new(
        token_start + rowan::TextSize::from(start as u32),
        token_start + rowan::TextSize::from(end as u32),
    ))
}

/// Find the shallowest ancestor with the same text range as `node`.
///
/// Avoids immediately jumping to parent when multiple nodes share
/// the same range (e.g., wrapper nodes).
fn shallowest_node(node: &syntax::SyntaxNode) -> syntax::SyntaxNode {
    node.ancestors()
        .take_while(|n| n.text_range() == node.text_range())
        .last()
        .unwrap_or_else(|| node.clone())
}

/// Extend selection to include adjacent comments of the same group.
///
/// A comment group is a sequence of comment tokens separated only
/// by single-newline whitespace; a double-newline breaks the group.
fn extend_comments(token: &syntax::SyntaxToken) -> Option<rowan::TextRange> {
    use parser::SyntaxKind as SK;
    if !matches!(token.kind(), SK::LINE_COMMENT | SK::BLOCK_COMMENT | SK::DOC_COMMENT) {
        return None;
    }

    // Walk backward to find the first comment in the group
    let first = adj_comment(token, rowan::Direction::Prev);
    // Walk forward to find the last comment in the group
    let last = adj_comment(token, rowan::Direction::Next);

    let range = rowan::TextRange::new(first.text_range().start(), last.text_range().end());
    // Only return if we actually grouped multiple comments
    if range != token.text_range() {
        Some(range)
    } else {
        None
    }
}

/// Walk siblings in `dir` to find the furthest adjacent comment.
///
/// Stops at non-whitespace tokens or double-newline whitespace.
fn adj_comment(token: &syntax::SyntaxToken, dir: rowan::Direction) -> syntax::SyntaxToken {
    use parser::SyntaxKind as SK;
    let mut res = token.clone();
    for element in token.siblings_with_tokens(dir) {
        let Some(tok) = element.as_token() else { break };
        if matches!(tok.kind(), SK::LINE_COMMENT | SK::BLOCK_COMMENT | SK::DOC_COMMENT) {
            res = tok.clone();
        } else if tok.kind() == SK::WHITESPACE {
            // Allow single-newline whitespace between comments;
            // stop at double-newline (paragraph break).
            if tok.text().contains("\n\n") {
                break;
            }
        } else {
            break;
        }
    }
    res
}

use parser::SyntaxKind as SK2;

/// List-context SyntaxKinds where items can be expanded to include separators.
fn is_list_kind(kind: parser::SyntaxKind) -> bool {
    matches!(
        kind,
        SK2::PARAM_LIST
        | SK2::ARG_LIST
        | SK2::TYPE_PARAM_LIST
        | SK2::STRUCT_EXPR       // struct literal fields: struct { a = 1, b = 2 }
        | SK2::STRUCT_PAT        // struct pattern fields
        | SK2::MATCH_ARM         // match arms (parent of arms list)
        | SK2::TUPLE_EXPR        // tuple elements: (a, b, c)
        | SK2::TUPLE_PAT         // tuple pattern elements
        | SK2::VECTOR_EXPR // vector literal elements: [1, 2, 3]
    )
}

/// Expand a list item to include the adjacent comma separator.
fn extend_list_item(node: &syntax::SyntaxNode) -> Option<rowan::TextRange> {
    let parent = node.parent()?;
    // Look for a comma after this node
    let mut found_self = false;
    for sibling in parent.children_with_tokens() {
        if sibling.as_node() == Some(node) {
            found_self = true;
            continue;
        }
        if found_self {
            if let Some(token) = sibling.as_token() {
                if token.kind() == SK2::COMMA {
                    // Extend to include the comma (and trailing whitespace)
                    return Some(rowan::TextRange::new(
                        node.text_range().start(),
                        token.text_range().end(),
                    ));
                }
                if !token.kind().is_trivia() {
                    break;
                }
            }
        }
    }
    // No comma after — check for comma before
    let mut prev_comma = None;
    for sibling in parent.children_with_tokens() {
        if sibling.as_node() == Some(node) {
            if let Some(comma_range) = prev_comma {
                return Some(rowan::TextRange::new(comma_range, node.text_range().end()));
            }
            break;
        }
        if let Some(token) = sibling.as_token() {
            if token.kind() == SK2::COMMA {
                prev_comma = Some(token.text_range().start());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn extend_from_token() {
        let file = TestFile::new("val foo : int\n");
        // Select "fo" (partial token) → should expand to "foo" (full token)
        let result = extend_selection(&file, base_db::text_range(4, 6));
        assert!(
            result.len() >= rowan::TextSize::from(3),
            "should expand to at least 'foo': got {:?}",
            result
        );
    }

    #[test]
    fn extend_grows_to_parent() {
        let file = TestFile::new("function f(x) = x + 1\n");
        // Select "x + 1" region → should grow to include more context
        let r1 = extend_selection(&file, base_db::text_range(16, 21));
        assert!(r1.len() > rowan::TextSize::from(5), "should grow beyond 'x + 1'");
        // Extend again → should grow further
        let r2 = extend_selection(&file, r1);
        assert!(r2.len() >= r1.len(), "second extend should be >= first");
    }

    #[test]
    fn extend_at_root_stays() {
        let file = TestFile::new("val x : int\n");
        let full = base_db::text_range(0, 12);
        let result = extend_selection(&file, full);
        // At root, can't extend further
        assert!(result.len() >= full.len());
    }
}
