//! Diagnostics for type and path lowering.
//! Collects errors encountered during TypeRef → `Ty` conversion.

use crate::lower::TypeRefId;

/// A diagnostic produced during type lowering.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct TyLoweringDiagnostic {
    /// The source TypeRef that caused the diagnostic.
    /// Points into the TypeRef arena of the current lowering context.
    pub source: TypeRefId,
    pub kind: TyLoweringDiagnosticKind,
}

/// Classification of type lowering errors.
///
/// `PathDiagnostic(PathLoweringDiagnostic)` which wraps all path-related
/// errors. We follow the same nesting pattern for extensibility.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum TyLoweringDiagnosticKind {
    /// Errors related to path/name resolution in type position.
    PathDiagnostic(PathLoweringDiagnostic),
}

/// Diagnostics for path lowering within type expressions.
///
/// type name resolution and application arity (no lifetimes/generics).
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PathLoweringDiagnostic {
    /// A type name could not be resolved.
    ///
    /// in RA this means "this path segment can't have generics"; in Sail
    /// this means "this type name doesn't exist").
    UnresolvedType { name: String },

    /// Wrong number of type arguments.
    IncorrectTypeArgCount { expected: u32, got: u32 },

    /// Invalid type expression (malformed CST/TypeRef node).
    InvalidTypeExpr,
}
