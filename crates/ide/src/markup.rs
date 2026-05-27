//! Rich text formatting for IDE display.
//! `Markup` wraps a Markdown string for use in hover tooltips,
//! documentation popups, and other rich-text UI elements.

use std::fmt;

/// A piece of Markdown-formatted text for IDE display.
#[derive(Default, Clone, Hash, PartialEq, Eq)]
pub struct Markup {
    text: String,
}

impl Markup {
    /// Create from raw Markdown text.
    pub fn from(text: impl Into<String>) -> Self {
        Markup { text: text.into() }
    }

    /// The raw Markdown text.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Wrap text in a fenced code block.
    pub fn fenced_block(contents: &dyn fmt::Display) -> Markup {
        Markup { text: format!("```sail\n{contents}\n```") }
    }

    /// Wrap text in a fenced code block (takes &str).
    pub fn fenced_block_text(contents: &str) -> Markup {
        Markup { text: format!("```sail\n{contents}\n```") }
    }
}

impl fmt::Debug for Markup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.text, f)
    }
}

impl fmt::Display for Markup {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.text, f)
    }
}

impl From<String> for Markup {
    fn from(text: String) -> Self {
        Markup { text }
    }
}

impl From<&str> for Markup {
    fn from(text: &str) -> Self {
        Markup { text: text.to_owned() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fenced_block() {
        let m = Markup::fenced_block_text("val foo : int");
        assert!(m.as_str().starts_with("```sail\n"));
        assert!(m.as_str().contains("val foo : int"));
        assert!(m.as_str().ends_with("\n```"));
    }

    #[test]
    fn from_str() {
        let m = Markup::from("hello");
        assert_eq!(m.as_str(), "hello");
    }
}
