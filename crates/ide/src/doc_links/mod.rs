//! Documentation link extraction and resolution.
//! Extracts definition references from documentation comments,
//! resolving them to navigation targets. This enables clickable
//! links in hover documentation and document links.
//!
//! Sail doc comment conventions:
//! - `/** ... */` — multi-line doc comments
//! - `//!` — module-level doc comments
//! - References via backtick-quoted names: `` `function_name` ``

use ide_db::line_index::TextRange;
use ide_db::FileDb;

/// Links extracted from documentation comments.
#[derive(Debug, Clone, Default)]
pub struct DocumentationLinks {
    /// URL to external documentation (if available).
    pub web_url: Option<String>,
    /// Local file URL for navigation.
    pub local_url: Option<String>,
}

/// A link found in a documentation comment.
#[derive(Debug, Clone)]
pub struct DocLink {
    /// Text range of the link in the source file.
    pub range: TextRange,
    /// The referenced name (e.g., function or type name).
    pub name: String,
}

/// Extract definition references from documentation comments.
/// Scans doc comments for backtick-quoted identifiers and resolves
/// them against the workspace symbol index.
pub fn extract_doc_links(file: &dyn FileDb) -> Vec<DocLink> {
    let text = file.text();
    let mut links = Vec::new();

    // Scan for doc comments (/** ... */ and //! ...) in the source text
    // and extract backtick-quoted references from them.
    //
    // Comments are not emitted as tokens (they're trivia), so we
    // scan the raw text.
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Block doc comment: /** ... */
        if i + 3 < bytes.len() && &bytes[i..i + 3] == b"/**" {
            let start = i;
            if let Some(end) = text[i + 3..].find("*/") {
                let comment = &text[i + 3..i + 3 + end];
                extract_backtick_refs(comment, start + 3, &mut links);
                i = i + 3 + end + 2;
                continue;
            }
        }
        // Line doc comment: //! ...
        if i + 3 < bytes.len() && &bytes[i..i + 3] == b"//!" {
            let line_start = i + 3;
            let line_end =
                text[line_start..].find('\n').map(|p| line_start + p).unwrap_or(text.len());
            let comment = &text[line_start..line_end];
            extract_backtick_refs(comment, line_start, &mut links);
            i = line_end;
            continue;
        }
        i += 1;
    }

    links
}

/// Rewrite documentation links to include navigation targets.
/// Takes raw markdown documentation and resolves any identifier
/// references to their definitions.
pub fn rewrite_links(_file: &dyn FileDb, markdown: &str, _definition_name: &str) -> String {
    // For now, pass through — link resolution will be enhanced
    // when workspace symbol index is integrated.
    markdown.to_string()
}

/// Get external documentation URL for a definition.
/// Sail doesn't have a central docs site like docs.rs,
/// but could link to project-local documentation.
pub fn external_docs(_file: &dyn FileDb, _name: &str) -> DocumentationLinks {
    DocumentationLinks::default()
}

/// Extract backtick-quoted references from text.
///
/// Finds patterns like `` `function_name` `` and records them
/// as doc links with their source ranges.
fn extract_backtick_refs(text: &str, base_offset: usize, links: &mut Vec<DocLink>) {
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            // Find closing backtick
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != b'`' {
                end += 1;
            }
            if end < bytes.len() && end > start {
                let name = &text[start..end];
                // Only include if it looks like an identifier
                if is_identifier_like(name) {
                    links.push(DocLink {
                        range: base_db::text_range(base_offset + start, base_offset + end),
                        name: name.to_string(),
                    });
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
}

/// Check if text looks like an identifier (alphanumeric + underscore).
fn is_identifier_like(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '\'')
        && text.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_backtick_refs() {
        let mut links = Vec::new();
        extract_backtick_refs("see `foo` and `bar_baz`", 0, &mut links);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].name, "foo");
        assert_eq!(links[1].name, "bar_baz");
    }

    #[test]
    fn skips_non_identifiers() {
        let mut links = Vec::new();
        extract_backtick_refs("code `x + y` here", 0, &mut links);
        assert_eq!(links.len(), 0); // `x + y` has spaces, not an identifier
    }

    #[test]
    fn is_identifier_like_basic() {
        assert!(is_identifier_like("foo"));
        assert!(is_identifier_like("bar_baz"));
        assert!(is_identifier_like("_private"));
        assert!(is_identifier_like("type'a"));
        assert!(!is_identifier_like(""));
        assert!(!is_identifier_like("123"));
        assert!(!is_identifier_like("a b"));
    }
}
mod intra_doc_links;
