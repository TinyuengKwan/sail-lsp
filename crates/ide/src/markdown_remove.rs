//! Removes markdown formatting while preserving plain text and code blocks.
//!

use pulldown_cmark::{Event, Parser, TagEnd};

/// Strip markdown formatting, keeping text and code content.
/// Used for inlay hints, plain-text hover fallback, etc.
#[allow(unused)] // used by hover::doc_to_plain_text + tests; will gain more callers
pub(crate) fn remove_markdown(markdown: &str) -> String {
    let mut out = String::new();
    out.reserve_exact(markdown.len());
    let parser = Parser::new(markdown);

    for event in parser {
        match event {
            Event::Text(text) | Event::Code(text) => out.push_str(&text),
            Event::SoftBreak => out.push(' '),
            Event::HardBreak | Event::Rule => out.push('\n'),
            Event::End(TagEnd::CodeBlock) => out.push('\n'),
            Event::End(TagEnd::Paragraph) => out.push_str("\n\n"),
            Event::Start(_)
            | Event::End(_)
            | Event::Html(_)
            | Event::InlineHtml(_)
            | Event::FootnoteReference(_)
            | Event::TaskListMarker(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => (),
        }
    }

    // Trim trailing whitespace
    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(remove_markdown("hello world"), "hello world");
    }

    #[test]
    fn removes_emphasis() {
        assert_eq!(remove_markdown("This is **bold** and *italic*"), "This is bold and italic");
    }

    #[test]
    fn preserves_code() {
        assert_eq!(remove_markdown("Use `foo()` here"), "Use foo() here");
    }

    #[test]
    fn code_block() {
        let input = "```sail\nlet x = 1\n```";
        let result = remove_markdown(input);
        assert!(result.contains("let x = 1"), "got: {result}");
    }

    #[test]
    fn paragraphs() {
        assert_eq!(remove_markdown("First\n\nSecond"), "First\n\nSecond");
    }
}
