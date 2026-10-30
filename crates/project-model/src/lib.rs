//! Sail project syntax and configuration-aware module graphs.
//!
//! [`ProjectManifest`] evaluates variables and conditional expressions,
//! resolves module requirements, and selects files in dependency order.
//! It performs no filesystem access and is independent of the LSP server.

pub mod editing;
mod manifest;
#[cfg(test)]
mod upstream_tests;
pub use manifest::{
    ProjectManifest, ProjectOptions, ProjectValue, ResolvedModule, ResolvedProject,
};

/// Kind of inter-module dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    /// `requires A` — module must be processed before this one.
    Requires,
    /// `after A` — ordering constraint (weaker than requires).
    After,
    /// `before A` — reverse ordering constraint.
    Before,
}

/// What is wrong with a manifest, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectError {
    pub kind: ProjectErrorKind,
    /// Byte range in the manifest. `None` when the problem is in the
    /// configuration rather than the text: a root module that is not declared.
    pub range: Option<std::ops::Range<usize>>,
    pub message: String,
}

/// Which kind of mistake a [`ProjectError`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectErrorKind {
    /// The text does not lex or parse.
    Syntax,
    /// A dependency names a module that is not declared.
    UnknownModule,
    /// A configured root module is not declared.
    UnknownRoot,
    /// Modules depend on themselves.
    Cycle,
    /// A module or variable is declared twice.
    Duplicate,
    /// A `$variable` that is never declared or configured.
    UnknownVariable,
    /// A value of the wrong kind: a non-boolean condition, a list where a
    /// path belongs.
    Type,
    /// A `__virtual` file that nothing supplies.
    VirtualFile,
    /// The manifest's own `error("...")`.
    Explicit,
}

impl ProjectError {
    pub(crate) fn new(
        kind: ProjectErrorKind,
        range: std::ops::Range<usize>,
        message: impl Into<String>,
    ) -> Self {
        Self { kind, range: Some(range), message: message.into() }
    }
    fn syntax(at: usize, message: impl Into<String>) -> Self {
        Self::new(ProjectErrorKind::Syntax, at..at + 1, message)
    }
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProjectError {}

/// A token and the bytes it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Token {
    tok: Tok,
    range: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// Bare identifier or one of the recognised keywords.
    Id(String),
    /// `name.ext` or `path/to/name.ext` token. Distinguished from a
    /// bare `Id` because it's the canonical "filename" form.
    FileId(String),
    /// Quoted string literal contents (without the surrounding `"`).
    Str(String),
    /// Variable reference `$name`.
    Var(String),
    LCurly,
    RCurly,
    LSquare,
    RSquare,
    LParen,
    RParen,
    Comma,
    Semi,
    Eq,
    /// Generic comparison / op token (`==`, `!=`, `<=`, `>=`, `<`,
    /// `>`). The MVP doesn't evaluate them but still needs to
    /// recognise the token shape so they don't get mistaken for
    /// something structural.
    OpCompare(String),
    /// `..` (parent reference in expressions).
    DotDot,
    /// `/` (path separator in expressions or division).
    Slash,
    /// Keyword tokens.
    KwFiles,
    KwDirectory,
    KwRequires,
    KwAfter,
    KwBefore,
    KwIf,
    KwThen,
    KwElse,
    KwTrue,
    KwFalse,
    KwVariable,
    KwTest,
}

fn keyword(id: &str) -> Option<Tok> {
    Some(match id {
        "files" => Tok::KwFiles,
        "directory" => Tok::KwDirectory,
        "requires" => Tok::KwRequires,
        "after" => Tok::KwAfter,
        "before" => Tok::KwBefore,
        "if" => Tok::KwIf,
        "then" => Tok::KwThen,
        "else" => Tok::KwElse,
        "true" => Tok::KwTrue,
        "false" => Tok::KwFalse,
        "variable" => Tok::KwVariable,
        "__test" => Tok::KwTest,
        _ => return None,
    })
}

/// project_lexer.mll raises "is a reserved keyword" for each of these.
const RESERVED: &[&str] = &[
    "default",
    "extension",
    "import",
    "namespace",
    "open",
    "optional",
    "use",
    "version",
    "versions",
];

fn lex(source: &str) -> Result<Vec<Token>, ProjectError> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut ranges = Vec::new();
    let mut i = 0;
    // Each iteration reads at most one token, from where it started to where
    // the next one starts.
    let mut start_of_last = 0;
    while i < bytes.len() {
        if ranges.len() < out.len() {
            ranges.push(start_of_last..i);
        }
        start_of_last = i;
        let b = bytes[i];
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => {
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                // Line comment.
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                // Block comment with nesting (matches upstream lexer).
                let start = i;
                i += 2;
                let mut depth = 1;
                while i < bytes.len() && depth > 0 {
                    if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                if depth != 0 {
                    return Err(ProjectError::syntax(start, "Unbalanced comment"));
                }
            }
            b'/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            b'{' => {
                out.push(Tok::LCurly);
                i += 1;
            }
            b'}' => {
                out.push(Tok::RCurly);
                i += 1;
            }
            b'[' => {
                out.push(Tok::LSquare);
                i += 1;
            }
            b']' => {
                out.push(Tok::RSquare);
                i += 1;
            }
            b'(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            b')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            b',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            b';' => {
                out.push(Tok::Semi);
                i += 1;
            }
            b'=' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                out.push(Tok::OpCompare(source[i..i + 2].to_owned()));
                i += 2;
            }
            b'=' => {
                out.push(Tok::Eq);
                i += 1;
            }
            b'!' if i + 1 < bytes.len() && bytes[i + 1] == b'=' => {
                out.push(Tok::OpCompare(source[i..i + 2].to_owned()));
                i += 2;
            }
            b'<' | b'>' => {
                if i + 1 < bytes.len() && bytes[i + 1] == b'=' {
                    out.push(Tok::OpCompare(source[i..i + 2].to_owned()));
                    i += 2;
                } else {
                    out.push(Tok::OpCompare(source[i..i + 1].to_owned()));
                    i += 1;
                }
            }
            b'.' if i + 1 < bytes.len() && bytes[i + 1] == b'.' => {
                out.push(Tok::DotDot);
                i += 2;
            }
            b'$' => {
                i += 1;
                let start = i;
                while i < bytes.len() && is_ident_char(bytes[i]) {
                    i += 1;
                }
                if i == start {
                    return Err(ProjectError::syntax(start - 1, "Unexpected character: $"));
                }
                out.push(Tok::Var(source[start..i].to_string()));
            }
            b'"' => {
                let start = i;
                i += 1;
                let mut buf = Vec::new();
                let mut closed = false;
                while i < bytes.len() {
                    let c = bytes[i];
                    if c == b'"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    if c == b'\\' {
                        // project_lexer.mll's escape_sequence, then Scanf.unescaped.
                        let number = |from: usize, len: usize, radix: u32| {
                            let digits = source.get(from..from + len)?;
                            digits.chars().all(|c| c.is_digit(radix)).then_some(())?;
                            u8::from_str_radix(digits, radix).ok()
                        };
                        let (byte, len) = match bytes.get(i + 1) {
                            Some(b'\\') => (b'\\', 2),
                            Some(b'"') => (b'"', 2),
                            Some(b'\'') => (b'\'', 2),
                            Some(b'n') => (b'\n', 2),
                            Some(b't') => (b'\t', 2),
                            Some(b'b') => (8, 2),
                            Some(b'r') => (b'\r', 2),
                            Some(b'x') if number(i + 2, 2, 16).is_some() => {
                                (number(i + 2, 2, 16).unwrap(), 4)
                            }
                            Some(b'0'..=b'9') if number(i + 1, 3, 10).is_some() => {
                                (number(i + 1, 3, 10).unwrap(), 4)
                            }
                            Some(b'\n') if matches!(bytes.get(i + 2), Some(b' ' | b'\t')) => {
                                // A continued line drops the newline and its indentation.
                                i += 2;
                                while matches!(bytes.get(i), Some(b' ' | b'\t')) {
                                    i += 1;
                                }
                                continue;
                            }
                            _ => {
                                return Err(ProjectError::syntax(
                                    i,
                                    "String literal contains illegal backslash escape sequence",
                                ))
                            }
                        };
                        buf.push(byte);
                        i += len;
                        continue;
                    }
                    buf.push(c);
                    i += 1;
                }
                if !closed {
                    return Err(ProjectError::syntax(start, "String literal not terminated"));
                }
                out.push(Tok::Str(String::from_utf8_lossy(&buf).into_owned()));
            }
            _ if is_ident_start(b) => {
                let start = i;
                while i < bytes.len() && is_ident_char(bytes[i]) {
                    i += 1;
                }
                let ident = &source[start..i];
                // Look ahead for `.<ext>` to detect a file id; the
                // upstream lexer recognises `name.ext` as a single
                // FileId token.
                if i < bytes.len() && bytes[i] == b'.' {
                    let dot_at = i;
                    let mut j = i + 1;
                    while j < bytes.len() && is_ident_char(bytes[j]) {
                        j += 1;
                    }
                    if j > dot_at + 1 {
                        // Combine into a single FileId.
                        out.push(Tok::FileId(source[start..j].to_string()));
                        i = j;
                        continue;
                    }
                }
                if let Some(kw) = keyword(ident) {
                    out.push(kw);
                } else {
                    // Upstream rejects these words as bare identifiers, but a
                    // module header (`default {`) is a different token there.
                    let header = bytes[i..]
                        .iter()
                        .find(|b| !matches!(b, b' ' | b'\t'))
                        .is_some_and(|b| *b == b'{');
                    if RESERVED.contains(&ident) && !header {
                        return Err(ProjectError::new(
                            ProjectErrorKind::Syntax,
                            start..i,
                            format!("{ident} is a reserved keyword"),
                        ));
                    }
                    out.push(Tok::Id(ident.to_string()));
                }
            }
            _ if b.is_ascii_digit() => {
                // Numeric literal — not used by the MVP, just skip
                // through digits and treat as Id (a placeholder).
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                out.push(Tok::Id(source[start..i].to_string()));
            }
            _ => {
                let c = source[i..].chars().next().unwrap_or(b as char);
                return Err(ProjectError::new(
                    ProjectErrorKind::Syntax,
                    i..i + c.len_utf8(),
                    format!("Unexpected character: {c}"),
                ));
            }
        }
    }
    if ranges.len() < out.len() {
        ranges.push(start_of_last..i);
    }
    Ok(out.into_iter().zip(ranges).map(|(tok, range)| Token { tok, range }).collect())
}

fn is_ident_start(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphabetic()
}

fn is_ident_char(b: u8) -> bool {
    b == b'_' || b.is_ascii_alphanumeric()
}
