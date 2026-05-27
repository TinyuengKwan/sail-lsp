//! Stable IDs for top-level syntax nodes via a per-file arena.
//!
//! IDs don't change unless the set of items itself changes.

use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use la_arena::{Arena, Idx, RawIdx};
use rustc_hash::FxHashMap;
use syntax::ast::AstNode;
use syntax::SyntaxNodePtr;

use crate::in_file::InFile;

/// Type-erased version of `FileAstId`.
///
/// Just an index into the `AstIdMap` arena.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ErasedFileAstId(Idx<SyntaxNodePtr>);

impl fmt::Debug for ErasedFileAstId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ErasedFileAstId({:?})", self.0.into_raw())
    }
}

impl ErasedFileAstId {
    /// The raw arena index.
    pub fn into_raw(self) -> RawIdx {
        self.0.into_raw()
    }
}

/// Typed, stable AST node ID within a file. Usable as salsa key/value.
pub struct FileAstId<N: AstNode> {
    raw: ErasedFileAstId,
    _marker: PhantomData<fn() -> N>,
}

// Manual trait impls to avoid bounds on N.

impl<N: AstNode> Clone for FileAstId<N> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}
impl<N: AstNode> Copy for FileAstId<N> {}

impl<N: AstNode> PartialEq for FileAstId<N> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<N: AstNode> Eq for FileAstId<N> {}

impl<N: AstNode> Hash for FileAstId<N> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.raw.hash(hasher);
    }
}

impl<N: AstNode> fmt::Debug for FileAstId<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FileAstId::<{}>({:?})", std::any::type_name::<N>(), self.raw)
    }
}

impl<N: AstNode> FileAstId<N> {
    /// Type-erase this ID.
    #[inline]
    pub fn erase(self) -> ErasedFileAstId {
        self.raw
    }
}

/// `AstId` points to an AST node in any file.
///
/// `pub type AstId<N> = InFile<FileAstId<N>>;`
///
/// It is stable across reparses, and can be used as salsa key/value.
pub type AstId<N> = InFile<FileAstId<N>>;

/// Type-erased `AstId`.
pub type ErasedAstId = InFile<ErasedFileAstId>;

/// Maps AST nodes to stable `FileAstId`s within a single file.
#[derive(Debug, PartialEq, Eq)]
pub struct AstIdMap {
    arena: Arena<SyntaxNodePtr>,
    map: FxHashMap<SyntaxNodePtr, Idx<SyntaxNodePtr>>,
}

impl AstIdMap {
    /// Build by registering every top-level child of `root`.
    pub fn from_source(root: &syntax::SyntaxNode) -> Self {
        let mut arena = Arena::new();
        let mut map = FxHashMap::default();

        // Register the root node itself.
        let root_ptr = SyntaxNodePtr::new(root);
        let root_idx = arena.alloc(root_ptr);
        map.insert(root_ptr, root_idx);

        // Register every direct child node (top-level items).
        for child in root.children() {
            let ptr = SyntaxNodePtr::new(&child);
            if !map.contains_key(&ptr) {
                let idx = arena.alloc(ptr);
                map.insert(ptr, idx);
            }

            // Also register second-level children (e.g., members of a
            // struct/enum/bitfield definition) so scattered clauses,
            // enum members, etc. can be addressed.
            for grandchild in child.children() {
                let gptr = SyntaxNodePtr::new(&grandchild);
                if !map.contains_key(&gptr) {
                    let idx = arena.alloc(gptr);
                    map.insert(gptr, idx);
                }
            }
        }

        AstIdMap { arena, map }
    }

    /// Get the AST ID for `node`. Panics if the node was not registered.
    pub fn ast_id<N: AstNode>(&self, node: &N) -> FileAstId<N> {
        let ptr = SyntaxNodePtr::new(node.syntax());
        let idx = self.map.get(&ptr).copied().unwrap_or_else(|| {
            panic!(
                "AstIdMap::ast_id: node not found: {:?}@{:?}",
                node.syntax().kind(),
                node.syntax().text_range(),
            )
        });
        FileAstId { raw: ErasedFileAstId(idx), _marker: PhantomData }
    }

    /// Look up the `SyntaxNodePtr` for a `FileAstId`.
    pub fn get<N: AstNode>(&self, id: FileAstId<N>) -> SyntaxNodePtr {
        self.arena[id.raw.0]
    }

    /// Look up the `SyntaxNodePtr` for an erased ID.
    pub fn get_erased(&self, id: ErasedFileAstId) -> SyntaxNodePtr {
        self.arena[id.0]
    }

    /// Number of entries in this map.
    pub fn len(&self) -> usize {
        self.arena.len()
    }

    /// Whether this map is empty.
    pub fn is_empty(&self) -> bool {
        self.arena.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntax::ast::SourceFile;
    use syntax::parse_text;

    #[test]
    fn ast_id_map_basic() {
        let (root, _) = parse_text("function f(x) = x\nfunction g(y) = y\n");
        let map = AstIdMap::from_source(&root);

        // Root + 2 top-level items + their children
        assert!(map.len() >= 3);

        // Round-trip for first top-level item
        let first_child = root.children().next().unwrap();
        let ptr = SyntaxNodePtr::new(&first_child);
        let idx = map.map.get(&ptr).unwrap();
        let retrieved = map.arena[*idx];
        assert_eq!(retrieved, ptr);
    }

    #[test]
    fn file_ast_id_round_trip() {
        let (root, _) = parse_text("function f(x) = x\n");
        let map = AstIdMap::from_source(&root);

        let sf = SourceFile::cast(root.clone()).unwrap();
        let defs = sf.callable_defs();
        assert!(!defs.is_empty());

        let id = map.ast_id(&defs[0]);
        let ptr = map.get(id);
        assert_eq!(ptr.text_range(), defs[0].syntax().text_range());
    }

    #[test]
    fn file_ast_id_erase_and_recover() {
        let (root, _) = parse_text("function f(x) = x\n");
        let map = AstIdMap::from_source(&root);

        let sf = SourceFile::cast(root.clone()).unwrap();
        let defs = sf.callable_defs();
        let id = map.ast_id(&defs[0]);
        let erased = id.erase();

        let ptr_typed = map.get(id);
        let ptr_erased = map.get_erased(erased);
        assert_eq!(ptr_typed, ptr_erased);
    }

    #[test]
    fn ast_id_as_in_file() {
        let (root, _) = parse_text("function f(x) = x\n");
        let map = AstIdMap::from_source(&root);

        let sf = SourceFile::cast(root.clone()).unwrap();
        let defs = sf.callable_defs();
        let file_ast_id = map.ast_id(&defs[0]);

        // Create AstId (= InFile<FileAstId<N>>)
        let file_id = base_db::FileId::from_raw(1);
        let ast_id: AstId<_> = InFile::new(file_id, file_ast_id);

        assert_eq!(ast_id.file_id, file_id);
        let ptr = map.get(ast_id.value);
        assert_eq!(ptr.text_range(), defs[0].syntax().text_range());
    }

    #[test]
    fn two_files_independent_maps() {
        let (root1, _) = parse_text("function f(x) = x\n");
        let (root2, _) = parse_text("function g(y) = y\nfunction h(z) = z\n");

        let map1 = AstIdMap::from_source(&root1);
        let map2 = AstIdMap::from_source(&root2);

        // Different files have independent maps
        assert!(map1.len() >= 2); // root + 1 item + children
        assert!(map2.len() >= 3); // root + 2 items + children
    }
}
