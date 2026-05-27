//! Diagnostics produced during name resolution / DefMap construction.

use crate::name::Name;

/// Unique definition identifier within a [`DefMap`].
///
/// An index into the per-file `DefMap.data` vector.
pub use super::DefId;

/// A diagnostic produced during name resolution / DefMap construction.
///
/// during its construction (unresolved imports, duplicate definitions,
/// etc.). These are later surfaced by `ide-diagnostics`.
#[derive(Debug, Clone)]
pub enum DefDiagnostic {
    /// An `$include` path could not be resolved.
    UnresolvedInclude {
        /// The include path as written in source.
        path: String,
        /// Byte range of the `$include` directive.
        range: crate::Span,
    },
    /// Two definitions with the same name in the same scope.
    DuplicateDefinition { name: Name, first: DefId, second: DefId },
    /// A scattered definition is missing its `end` marker.
    IncompleteScattered { name: Name },
}
