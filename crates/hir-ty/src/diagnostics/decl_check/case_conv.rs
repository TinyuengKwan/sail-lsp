//! Case conversion utilities for declaration checking.

/// Convert a name to snake_case.
pub fn to_snake_case(name: &str) -> String {
    let mut result = String::with_capacity(name.len() + 4);
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(ch.to_lowercase().next().unwrap());
        } else {
            result.push(ch);
        }
    }
    result
}

/// Convert a name to CamelCase.
pub fn to_camel_case(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect()
}

/// Check if a name is already in snake_case.
pub fn is_snake_case(name: &str) -> bool {
    !name.contains(|c: char| c.is_uppercase())
        && !name.starts_with('_')
        && !name.ends_with('_')
        && !name.contains("__")
}

/// Check if a name is already in CamelCase.
pub fn is_camel_case(name: &str) -> bool {
    !name.contains('_') && name.starts_with(|c: char| c.is_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snake_case_conversion() {
        assert_eq!(to_snake_case("fooBar"), "foo_bar");
        assert_eq!(to_snake_case("FooBar"), "foo_bar");
        assert_eq!(to_snake_case("foo_bar"), "foo_bar");
    }

    #[test]
    fn camel_case_conversion() {
        assert_eq!(to_camel_case("foo_bar"), "FooBar");
        assert_eq!(to_camel_case("hello_world"), "HelloWorld");
    }

    #[test]
    fn snake_case_check() {
        assert!(is_snake_case("foo_bar"));
        assert!(!is_snake_case("fooBar"));
        assert!(!is_snake_case("FooBar"));
    }

    #[test]
    fn camel_case_check() {
        assert!(is_camel_case("FooBar"));
        assert!(!is_camel_case("foo_bar"));
    }
}
