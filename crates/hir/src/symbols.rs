//! File symbol extraction.
//! Provides `FileSymbol` — a symbol exported from a single file,
//! used for workspace symbol search and navigation targets.

use hir_def::name::Name;
use syntax::SyntaxNodePtr;

use crate::ModuleDef;

/// The kind of a file symbol, for filtering and display.
///
/// variant, but an explicit enum is clearer for consumers).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileSymbolKind {
    Function,
    Struct,
    Union,
    Enum,
    Bitfield,
    Newtype,
    TypeAlias,
    Register,
    Mapping,
    Let,
    Overload,
}

/// Source location of a declaration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeclarationLocation {
    /// The file containing this declaration.
    pub file_id: base_db::FileId,
    /// Pointer to the whole syntax node of the declaration.
    pub ptr: SyntaxNodePtr,
    /// Pointer to the name identifier within the declaration, if available.
    pub name_ptr: Option<SyntaxNodePtr>,
}

/// A symbol exported from a file, used for workspace symbol search.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileSymbol {
    /// The symbol name.
    pub name: Name,
    /// The definition this symbol refers to.
    pub def: ModuleDef,
    /// Source location of the declaration.
    pub loc: DeclarationLocation,
    /// Name of the containing definition, if any (e.g., enum name for members).
    pub container_name: Option<Name>,
    /// The kind of symbol (function, struct, etc.).
    /// Sail-specific: RA derives kind from `def: ModuleDef`, but an explicit
    /// enum is clearer for consumers.
    pub kind: FileSymbolKind,
    /// Whether this is a scattered clause (not the head definition).
    /// Sail-specific (scattered definitions).
    pub is_clause: bool,
    /// Whether this symbol is a type alias.
    pub is_alias: bool,
    /// Whether this is an associated item (method, associated type, etc.).
    ///
    /// (no associated items).
    pub is_assoc: bool,
    /// Whether this symbol is an import (use/open).
    ///
    /// (no use imports).
    pub is_import: bool,
}
