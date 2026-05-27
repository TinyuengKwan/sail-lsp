//! Name suggestion for extracted variables/functions.
//! Given an expression, suggests a reasonable variable name. Used by
//! "Extract Variable" and "Extract Function" assists.

/// Suggest a variable name for the given expression text.
/// Heuristics:
/// 1. Function call `foo(...)` → `foo`
/// 2. Field access `x.field` → `field`
/// 3. Known type patterns like `bits(n)` → `bits`
/// 4. Fallback: `var`
pub fn for_variable(expr_text: &str) -> String {
    let trimmed = expr_text.trim();

    // Function call: extract callee name
    if let Some(paren) = trimmed.find('(') {
        let callee = &trimmed[..paren].trim();
        if !callee.is_empty() && callee.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return to_snake_case(callee);
        }
    }

    // Field access: extract field name
    if let Some(dot) = trimmed.rfind('.') {
        let field = &trimmed[dot + 1..].trim();
        if !field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return to_snake_case(field);
        }
    }

    // Single identifier
    if trimmed.chars().all(|c| c.is_alphanumeric() || c == '_') && !trimmed.is_empty() {
        return to_snake_case(trimmed);
    }

    "var".to_string()
}

/// Convert a camelCase or PascalCase name to snake_case.
fn to_snake_case(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            result.push('_');
        }
        result.push(c.to_lowercase().next().unwrap_or(c));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggest_from_call() {
        assert_eq!(for_variable("foo(x, y)"), "foo");
        assert_eq!(for_variable("get_value()"), "get_value");
    }

    #[test]
    fn suggest_from_field() {
        assert_eq!(for_variable("x.length"), "length");
    }

    #[test]
    fn suggest_from_ident() {
        assert_eq!(for_variable("result"), "result");
    }

    #[test]
    fn suggest_fallback() {
        assert_eq!(for_variable("1 + 2"), "var");
    }

    #[test]
    fn snake_case_conversion() {
        assert_eq!(to_snake_case("myValue"), "my_value");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
    }
}
