//! Token text representation.
//! `TokenText` wraps a string that is logically owned by a syntax tree
//! token but may be borrowed or owned depending on the context.

use std::fmt;

use rowan::GreenToken;

/// Text of a token. Either a borrowed reference from a `SyntaxToken`
/// or an owned copy from a `GreenToken`.
pub struct TokenText<'a>(Repr<'a>);

#[allow(dead_code)] // WIP: will be used when SyntaxToken methods are wired
enum Repr<'a> {
    Borrowed(&'a str),
    Owned(GreenToken),
}

impl<'a> TokenText<'a> {
    #[allow(dead_code)] // WIP: will be used when SyntaxToken methods are wired
    pub(crate) fn borrowed(text: &'a str) -> Self {
        TokenText(Repr::Borrowed(text))
    }

    #[allow(dead_code)] // WIP: will be used when SyntaxToken methods are wired
    pub(crate) fn owned(green: GreenToken) -> Self {
        TokenText(Repr::Owned(green))
    }

    pub fn as_str(&self) -> &str {
        match &self.0 {
            Repr::Borrowed(s) => s,
            Repr::Owned(green) => green.text(),
        }
    }
}

impl AsRef<str> for TokenText<'_> {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for TokenText<'_> {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Debug for TokenText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl fmt::Display for TokenText<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl PartialEq<str> for TokenText<'_> {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == other
    }
}

impl PartialEq<&str> for TokenText<'_> {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl PartialEq<String> for TokenText<'_> {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == other.as_str()
    }
}
