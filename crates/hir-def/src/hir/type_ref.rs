//! Intermediate representation for types between syntax and `Ty`.
//! `TypeRef` is the syntactic representation of a type as written in source.
//! It is used in:
//! - ItemTree signatures (structured, hashed for incremental invalidation)
//! - `TyLoweringContext::lower_ty` input (replaces raw CST nodes)
//!
//! The `Ty` type (in `hir-ty`) is the *semantic* representation after
//! name resolution and type-level evaluation.

use la_arena::Idx;

/// Stable index into a TypeRef arena.
///
/// Used as the `source` field in `TyLoweringDiagnostic` to point
/// back to the TypeRef that caused an error.
pub type TypeRefId = Idx<TypeRef>;

/// A type as written in source code.
///
/// Structurally represents the syntax without any resolution.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeRef {
    /// A named type: `int`, `bits`, `my_type`.
    Named(String),
    /// A type variable: `'n`, `'a`.
    Var(String),
    /// A type application: `bits(32)`, `vector(64, dec, bit)`.
    App { name: String, args: Vec<TypeArg> },
    /// A tuple type: `(int, bool)`.
    Tuple(Vec<TypeRef>),
    /// A function type: `(int, int) -> bool`.
    Fn { params: Vec<TypeRef>, ret: Box<TypeRef> },
    /// A bitvector/mapping bidirectional type: `T <-> U`.
    Bidir { lhs: Box<TypeRef>, rhs: Box<TypeRef> },
    /// Existential type: `{'n, 'n > 0. bits('n)}`.
    Exist { vars: Vec<String>, constraint: Option<Box<ConstraintRef>>, inner: Box<TypeRef> },
    /// Universal quantifier in type position: `forall 'n. T`.
    Forall { vars: Vec<String>, inner: Box<TypeRef> },
    /// Placeholder for unresolvable / parse-error types.
    Error,
}

/// A type argument (either a type or a numeric expression).
///
/// Mirrors the distinction between type-level and value-level args
/// in Sail's dependent type system.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeArg {
    /// A type argument.
    Type(TypeRef),
    /// A numeric/value argument (as text, e.g., "32", "'n + 1").
    Value(String),
}

/// A constraint as written in source (syntactic form).
///
/// Used in existential types: `{'n, 'n > 0. bits('n)}`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstraintRef {
    /// Boolean literal constraint.
    Bool(bool),
    /// Comparison: `'n > 0`, `'m == 'n`.
    Compare { lhs: String, op: String, rhs: String },
    /// Set membership: `'n in {1, 2, 4, 8}`.
    InSet { var: String, members: Vec<String> },
    /// Conjunction.
    And(Box<ConstraintRef>, Box<ConstraintRef>),
    /// Disjunction.
    Or(Box<ConstraintRef>, Box<ConstraintRef>),
    /// Negation.
    Not(Box<ConstraintRef>),
    /// Opaque constraint text (fallback).
    Opaque(String),
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeRef::Named(n) => write!(f, "{n}"),
            TypeRef::Var(v) => write!(f, "'{v}"),
            TypeRef::App { name, args } => {
                write!(f, "{name}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{arg}")?;
                }
                write!(f, ")")
            }
            TypeRef::Tuple(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, ")")
            }
            TypeRef::Fn { params, ret } => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{p}")?;
                }
                write!(f, ") -> {ret}")
            }
            TypeRef::Bidir { lhs, rhs } => write!(f, "{lhs} <-> {rhs}"),
            TypeRef::Exist { vars, constraint, inner } => {
                write!(f, "{{")?;
                for (i, v) in vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "'{v}")?;
                }
                if let Some(c) = constraint {
                    write!(f, ", {c}")?;
                }
                write!(f, ". {inner}}}")
            }
            TypeRef::Forall { vars, inner } => {
                write!(f, "forall ")?;
                for (i, v) in vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "'{v}")?;
                }
                write!(f, ". {inner}")
            }
            TypeRef::Error => write!(f, "{{error}}"),
        }
    }
}

impl std::fmt::Display for TypeArg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TypeArg::Type(ty) => write!(f, "{ty}"),
            TypeArg::Value(v) => write!(f, "{v}"),
        }
    }
}

impl std::fmt::Display for ConstraintRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConstraintRef::Bool(b) => write!(f, "{b}"),
            ConstraintRef::Compare { lhs, op, rhs } => write!(f, "{lhs} {op} {rhs}"),
            ConstraintRef::InSet { var, members } => {
                write!(f, "{var} in {{")?;
                for (i, m) in members.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{m}")?;
                }
                write!(f, "}}")
            }
            ConstraintRef::And(l, r) => write!(f, "{l} & {r}"),
            ConstraintRef::Or(l, r) => write!(f, "{l} | {r}"),
            ConstraintRef::Not(c) => write!(f, "!({c})"),
            ConstraintRef::Opaque(s) => write!(f, "{s}"),
        }
    }
}

/// Parse a textual type annotation into a `TypeRef`.
///
/// This is the primary entry point for converting type text
/// (from ItemTree signatures) into structured form.
pub fn type_ref_from_text(text: &str) -> TypeRef {
    let text = text.trim();
    if text.is_empty() {
        return TypeRef::Error;
    }

    // Try to parse as bidir FIRST (before fn type, since `<->` contains `->`)
    if let Some(idx) = find_top_level(text, "<->") {
        let lhs = type_ref_from_text(&text[..idx].trim_end());
        let rhs = type_ref_from_text(&text[idx + 3..].trim_start());
        return TypeRef::Bidir { lhs: Box::new(lhs), rhs: Box::new(rhs) };
    }

    // Try to parse as function type: `(...) -> T`
    if let Some(ty) = try_parse_fn_type(text) {
        return ty;
    }

    // Try to parse as tuple: `(T1, T2, ...)`
    if text.starts_with('(') && text.ends_with(')') {
        let inner = &text[1..text.len() - 1];
        let parts = split_top_level(inner, ',');
        if parts.len() > 1 {
            return TypeRef::Tuple(parts.iter().map(|p| type_ref_from_text(p.trim())).collect());
        }
        // Single-element parens: unwrap
        if parts.len() == 1 {
            return type_ref_from_text(parts[0].trim());
        }
    }

    // Try to parse as type application: `name(args)`
    if let Some(paren_start) = text.find('(') {
        if text.ends_with(')') {
            let name = text[..paren_start].trim().to_string();
            let args_text = &text[paren_start + 1..text.len() - 1];
            let args = split_top_level(args_text, ',')
                .iter()
                .map(|a| {
                    let a = a.trim();
                    if a.starts_with('\'') || a.chars().all(|c| c.is_ascii_digit()) {
                        TypeArg::Value(a.to_string())
                    } else {
                        TypeArg::Type(type_ref_from_text(a))
                    }
                })
                .collect();
            return TypeRef::App { name, args };
        }
    }

    // Type variable: starts with '
    if text.starts_with('\'') {
        return TypeRef::Var(text[1..].to_string());
    }

    // Simple named type
    TypeRef::Named(text.to_string())
}

/// Try to parse `(T1, T2) -> R` or `T -> R`.
fn try_parse_fn_type(text: &str) -> Option<TypeRef> {
    // Find top-level `->`
    let arrow_idx = find_top_level(text, "->")?;
    let params_text = text[..arrow_idx].trim();
    let ret_text = text[arrow_idx + 2..].trim();

    let params = if params_text.starts_with('(') && params_text.ends_with(')') {
        let inner = &params_text[1..params_text.len() - 1];
        split_top_level(inner, ',').iter().map(|p| type_ref_from_text(p.trim())).collect()
    } else {
        vec![type_ref_from_text(params_text)]
    };

    Some(TypeRef::Fn { params, ret: Box::new(type_ref_from_text(ret_text)) })
}

/// Find a substring at the top level (not inside parens/brackets).
/// Note: `<` and `>` are NOT treated as brackets here (they're part of `<->` arrow).
fn find_top_level(text: &str, needle: &str) -> Option<usize> {
    let mut depth = 0i32;
    let bytes = text.as_bytes();
    let needle_bytes = needle.as_bytes();
    let nlen = needle_bytes.len();
    for i in 0..text.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 && i + nlen <= text.len() && &bytes[i..i + nlen] == needle_bytes {
            return Some(i);
        }
    }
    None
}

/// Split text by a delimiter at the top level (not inside parens/brackets).
fn split_top_level(text: &str, delim: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, c) in text.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            c if c == delim && depth == 0 => {
                parts.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_named() {
        assert_eq!(type_ref_from_text("int"), TypeRef::Named("int".into()));
    }

    #[test]
    fn parse_var() {
        assert_eq!(type_ref_from_text("'n"), TypeRef::Var("n".into()));
    }

    #[test]
    fn parse_app() {
        let ty = type_ref_from_text("bits(32)");
        match ty {
            TypeRef::App { name, args } => {
                assert_eq!(name, "bits");
                assert_eq!(args.len(), 1);
                assert!(matches!(&args[0], TypeArg::Value(v) if v == "32"));
            }
            _ => panic!("expected App, got {ty:?}"),
        }
    }

    #[test]
    fn parse_fn_type() {
        let ty = type_ref_from_text("(int, bool) -> string");
        match ty {
            TypeRef::Fn { params, ret } => {
                assert_eq!(params.len(), 2);
                assert_eq!(*ret, TypeRef::Named("string".into()));
            }
            _ => panic!("expected Fn, got {ty:?}"),
        }
    }

    #[test]
    fn parse_tuple() {
        let ty = type_ref_from_text("(int, bool)");
        // Without `->` this is a tuple, not fn params
        match ty {
            TypeRef::Tuple(items) => assert_eq!(items.len(), 2),
            _ => panic!("expected Tuple, got {ty:?}"),
        }
    }

    #[test]
    fn parse_bidir() {
        let ty = type_ref_from_text("int <-> bool");
        assert!(matches!(ty, TypeRef::Bidir { .. }));
    }
}
