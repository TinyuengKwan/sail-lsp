//! Interned name for identifiers.
//!
//! for global deduplication: structurally equal names share a single allocation.

use std::borrow::Borrow;
use std::fmt;

use intern::Symbol;

/// Interned identifier name. Cheap to clone, hash, and compare.
#[derive(Clone, PartialEq, Eq)]
pub struct Name(Symbol);

impl std::hash::Hash for Name {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state);
    }
}

impl Name {
    pub fn new(s: &str) -> Self {
        Self(Symbol::intern(s))
    }

    pub fn missing() -> Self {
        Self(intern::sym::MISSING_NAME)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Access the underlying [`Symbol`].
    pub fn symbol(&self) -> &Symbol {
        &self.0
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({:?})", self.0.as_str())
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

impl std::ops::Deref for Name {
    type Target = str;
    fn deref(&self) -> &str {
        self.0.as_str()
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.0.as_str()
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        Self(Symbol::intern(&s))
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Self(Symbol::intern(s))
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.0.as_str() == other
    }
}

impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.0.as_str() == *other
    }
}

impl PartialEq<String> for Name {
    fn eq(&self, other: &String) -> bool {
        self.0.as_str() == other.as_str()
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_str().cmp(other.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn interning_deduplication() {
        let a = Name::new("sail_register_x");
        let b = Name::new("sail_register_x");
        // Same symbol — pointer equality
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "sail_register_x");
    }

    #[test]
    fn equality_with_str() {
        let n = Name::new("foo");
        assert_eq!(n, "foo");
        assert_eq!(n, *"foo");
        assert_eq!(n, "foo".to_string());
    }

    #[test]
    fn hashmap_str_lookup() {
        let mut map: HashMap<Name, u32> = HashMap::new();
        map.insert(Name::new("x"), 42);
        // Lookup with &str via Borrow
        assert_eq!(map.get("x"), Some(&42));
    }

    #[test]
    fn display_and_debug() {
        let n = Name::new("my_func");
        assert_eq!(format!("{n}"), "my_func");
        assert_eq!(format!("{n:?}"), "Name(\"my_func\")");
    }

    #[test]
    fn deref_to_str() {
        let n = Name::new("bits");
        let s: &str = &n;
        assert_eq!(s, "bits");
    }

    #[test]
    fn missing_name() {
        let m = Name::missing();
        assert_eq!(m.as_str(), "[missing name]");
    }
}
