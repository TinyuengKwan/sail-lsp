//! Pattern utility functions — pretty-printing for diagnostics.

use super::{LiteralKey, MatchPat};

impl MatchPat {
    /// Render a witness pattern as a short string for the user-facing
    /// diagnostic. Wildcards become `_`, constructors `Foo` or `Foo(_)`,
    /// tuples `(a, b, c)`, literals their textual form.
    pub fn display_text(&self) -> String {
        match self {
            MatchPat::Wild => "_".to_string(),
            MatchPat::Ctor { name, args } if args.is_empty() => name.clone(),
            MatchPat::Ctor { name, args } => {
                let inner = args.iter().map(MatchPat::display_text).collect::<Vec<_>>().join(", ");
                format!("{name}({inner})")
            }
            MatchPat::Tuple(items) => {
                let inner = items.iter().map(MatchPat::display_text).collect::<Vec<_>>().join(", ");
                format!("({inner})")
            }
            MatchPat::Literal(key) => match key {
                LiteralKey::Bool(true) => "true".to_string(),
                LiteralKey::Bool(false) => "false".to_string(),
                LiteralKey::Unit => "()".to_string(),
                LiteralKey::Number(s)
                | LiteralKey::String(s)
                | LiteralKey::Binary(s)
                | LiteralKey::Hex(s) => s.clone(),
                LiteralKey::Undefined => "undefined".to_string(),
            },
            MatchPat::Or(branches) => {
                branches.iter().map(MatchPat::display_text).collect::<Vec<_>>().join(" | ")
            }
            MatchPat::Nil => "[||]".to_string(),
            MatchPat::VectorConcat { width } => format!("bits({width})"),
            MatchPat::Cons(hd, tl) => {
                format!("{} :: {}", hd.display_text(), tl.display_text())
            }
            MatchPat::Vec(items) => {
                let inner = items.iter().map(MatchPat::display_text).collect::<Vec<_>>().join(", ");
                format!("[{inner}]")
            }
            MatchPat::Struct { fields } => {
                let inner = fields
                    .iter()
                    .map(|(n, p)| format!("{n} = {}", p.display_text()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("struct {{ {inner} }}")
            }
        }
    }
}
