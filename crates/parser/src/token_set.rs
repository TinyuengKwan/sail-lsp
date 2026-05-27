//! Bitset over `SyntaxKind` for fast membership tests.
//!
//! recovery sets ("stop parsing at these tokens") and dispatch.

use crate::syntax_kind::SyntaxKind;

/// A bitset covering all `SyntaxKind` variants (~270 values → 5 u64s).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TokenSet([u64; 5]);

impl TokenSet {
    pub const EMPTY: TokenSet = TokenSet([0; 5]);

    pub const fn new(kinds: &[SyntaxKind]) -> Self {
        let mut inner = [0u64; 5];
        let mut i = 0;
        while i < kinds.len() {
            let kind = kinds[i] as u16 as usize;
            inner[kind / 64] |= 1 << (kind % 64);
            i += 1;
        }
        TokenSet(inner)
    }

    pub const fn contains(&self, kind: SyntaxKind) -> bool {
        let k = kind as u16 as usize;
        self.0[k / 64] & (1 << (k % 64)) != 0
    }

    pub const fn union(self, other: Self) -> Self {
        TokenSet([
            self.0[0] | other.0[0],
            self.0[1] | other.0[1],
            self.0[2] | other.0[2],
            self.0[3] | other.0[3],
            self.0[4] | other.0[4],
        ])
    }

    /// Return a new set with the given kind removed.
    pub const fn remove(self, kind: SyntaxKind) -> Self {
        let k = kind as u16 as usize;
        let mut inner = self.0;
        inner[k / 64] &= !(1 << (k % 64));
        TokenSet(inner)
    }
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSet(...)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax_kind::SyntaxKind as SK;

    #[test]
    fn empty_set_contains_nothing() {
        assert!(!TokenSet::EMPTY.contains(SK::IDENT));
        assert!(!TokenSet::EMPTY.contains(SK::KW_FUNCTION));
    }

    #[test]
    fn singleton_set() {
        let set = TokenSet::new(&[SK::IDENT]);
        assert!(set.contains(SK::IDENT));
        assert!(!set.contains(SK::KW_FUNCTION));
    }

    #[test]
    fn multi_element_set() {
        let set = TokenSet::new(&[SK::IDENT, SK::KW_FUNCTION, SK::R_CURLY]);
        assert!(set.contains(SK::IDENT));
        assert!(set.contains(SK::KW_FUNCTION));
        assert!(set.contains(SK::R_CURLY));
        assert!(!set.contains(SK::KW_VAL));
    }

    #[test]
    fn union_of_sets() {
        let a = TokenSet::new(&[SK::IDENT]);
        let b = TokenSet::new(&[SK::KW_FUNCTION]);
        let c = a.union(b);
        assert!(c.contains(SK::IDENT));
        assert!(c.contains(SK::KW_FUNCTION));
    }
}
