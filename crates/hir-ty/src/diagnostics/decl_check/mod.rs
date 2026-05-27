//! Declaration-level convention checks.
//! For Sail, enforces:
//! - Function names should be snake_case (warning, not error)
//! - Type variable names should start with `'`

pub mod case_conv;

use hir_def::Span;

/// A naming convention diagnostic.
pub enum DeclCheckDiagnostic {
    /// A function/value name does not follow the expected naming convention.
    FunctionNamingConvention { name: String, expected: CaseStyle, span: Span },
}

/// Expected naming style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseStyle {
    /// `snake_case` (Sail convention for functions/values).
    SnakeCase,
}

/// Check whether a function name follows snake_case convention.
///
/// Returns `Some(diagnostic)` if the name violates the convention.
///
/// Skips:
/// - Names starting with `_` (internal/unused convention)
/// - Operator-style names containing non-alphanumeric characters
/// - Single-character names
pub fn check_fn_name(name: &str, span: Span) -> Option<DeclCheckDiagnostic> {
    // Skip underscore-prefixed, operators, single-char
    if name.starts_with('_') {
        return None;
    }
    if name.len() <= 1 {
        return None;
    }
    // Skip operator-style names (contain non-alphanumeric, non-underscore)
    if name.chars().any(|c| !c.is_alphanumeric() && c != '_') {
        return None;
    }

    if !case_conv::is_snake_case(name) {
        Some(DeclCheckDiagnostic::FunctionNamingConvention {
            name: name.to_string(),
            expected: CaseStyle::SnakeCase,
            span,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_span() -> Span {
        Span { start: 0, end: 0 }
    }

    #[test]
    fn snake_case_valid() {
        assert!(check_fn_name("read_register", dummy_span()).is_none());
        assert!(check_fn_name("x", dummy_span()).is_none());
        assert!(check_fn_name("get_bits64", dummy_span()).is_none());
    }

    #[test]
    fn camel_case_invalid() {
        assert!(check_fn_name("readRegister", dummy_span()).is_some());
        assert!(check_fn_name("GetBits", dummy_span()).is_some());
    }

    #[test]
    fn underscore_prefix_skipped() {
        assert!(check_fn_name("_Internal", dummy_span()).is_none());
    }

    #[test]
    fn operator_names_skipped() {
        assert!(check_fn_name("<=_u", dummy_span()).is_none());
        assert!(check_fn_name("add_vec#", dummy_span()).is_none());
    }
}
