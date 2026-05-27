//! Inhabitedness analysis for types.
//! A type is "uninhabited" if it has no possible values.
//! This is used by match exhaustiveness to determine if
//! a match arm can never be reached.

/// Whether a type is inhabited (has at least one value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inhabitedness {
    /// Type has at least one value.
    Inhabited,
    /// Type has no values (e.g., empty enum).
    Uninhabited,
}

/// Check if a type is inhabited.
///
/// Currently checks:
/// - Empty enums (no variants) -> Uninhabited
/// - All other types -> Inhabited (conservative)
pub fn is_inhabited(
    type_name: &str,
    enum_variants: &dyn Fn(&str) -> Option<Vec<String>>,
) -> Inhabitedness {
    if let Some(variants) = enum_variants(type_name) {
        if variants.is_empty() {
            return Inhabitedness::Uninhabited;
        }
    }
    Inhabitedness::Inhabited
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_enum_is_inhabited() {
        let variants = |_: &str| -> Option<Vec<String>> { None };
        assert_eq!(is_inhabited("int", &variants), Inhabitedness::Inhabited);
    }

    #[test]
    fn empty_enum_is_uninhabited() {
        let variants = |name: &str| -> Option<Vec<String>> {
            match name {
                "Empty" => Some(vec![]),
                _ => None,
            }
        };
        assert_eq!(is_inhabited("Empty", &variants), Inhabitedness::Uninhabited);
    }

    #[test]
    fn non_empty_enum_is_inhabited() {
        let variants = |name: &str| -> Option<Vec<String>> {
            match name {
                "Color" => Some(vec!["Red".into(), "Green".into(), "Blue".into()]),
                _ => None,
            }
        };
        assert_eq!(is_inhabited("Color", &variants), Inhabitedness::Inhabited);
    }
}
