//! Representability check for recursive types.
//! Detects types that cannot be represented in memory because they
//! contain themselves without indirection (e.g., `struct A = { a: A }`).

use std::collections::HashSet;

/// Result of a representability check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Representability {
    /// Type has finite size.
    Representable,
    /// Type is infinitely recursive.
    Infinite,
}

/// Check if a named type is representable (non-recursive).
///
/// `type_fields` maps type names to their field types.
/// Returns `Infinite` if the type contains itself without indirection.
pub fn is_representable(
    type_name: &str,
    type_fields: &dyn Fn(&str) -> Vec<String>,
) -> Representability {
    let mut visited = HashSet::new();
    check_representability(type_name, type_fields, &mut visited)
}

fn check_representability(
    type_name: &str,
    type_fields: &dyn Fn(&str) -> Vec<String>,
    visited: &mut HashSet<String>,
) -> Representability {
    if visited.contains(type_name) {
        return Representability::Infinite;
    }
    visited.insert(type_name.to_string());

    let fields = type_fields(type_name);
    for field_ty in &fields {
        // Strip type application args: "bits(32)" → "bits"
        let base_name = field_ty.split('(').next().unwrap_or(field_ty).trim();
        if check_representability(base_name, type_fields, visited) == Representability::Infinite {
            visited.remove(type_name);
            return Representability::Infinite;
        }
    }

    visited.remove(type_name);
    Representability::Representable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_recursive_is_representable() {
        let fields = |name: &str| -> Vec<String> {
            match name {
                "Foo" => vec!["int".to_string(), "bool".to_string()],
                _ => vec![],
            }
        };
        assert_eq!(is_representable("Foo", &fields), Representability::Representable);
    }

    #[test]
    fn direct_recursion_is_infinite() {
        let fields = |name: &str| -> Vec<String> {
            match name {
                "A" => vec!["A".to_string()],
                _ => vec![],
            }
        };
        assert_eq!(is_representable("A", &fields), Representability::Infinite);
    }

    #[test]
    fn indirect_recursion_is_infinite() {
        let fields = |name: &str| -> Vec<String> {
            match name {
                "A" => vec!["B".to_string()],
                "B" => vec!["A".to_string()],
                _ => vec![],
            }
        };
        assert_eq!(is_representable("A", &fields), Representability::Infinite);
    }
}
