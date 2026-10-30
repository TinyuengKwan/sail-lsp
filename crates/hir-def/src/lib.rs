//! Definition-side HIR layers: body arenas, item tree, DefMap, Resolver.
pub use hir_expand::analysis_scope;
pub use parser::{Span, Token};
/// Bitfield accessor materialization.
pub mod bitfield;
pub mod bodies;
pub mod callable_info;
pub mod callgraph;
pub mod def_query;
pub mod diagnostics;
pub mod effects;
pub mod expr_store;
/// Path finding for auto-import ($include).
pub mod find_path;
pub mod hir;
pub use hir_expand::in_file;
pub use hir_expand::include_graph;
pub mod item_id;
pub mod item_scope;
pub mod item_tree;
pub mod message;
pub mod name;
pub mod nameres;
pub mod per_ns;
pub use project_model as project;
pub mod resolver;
pub mod scattered;
pub mod signatures;
pub mod source_order;
pub mod src;
pub mod type_error;
/// Re-export for backward compatibility.
pub use hir::type_ref;
#[cfg(test)]
mod test_db;
pub mod visibility;
pub mod workspace_def_map;

pub use bodies::{CallableBodies, CallableBody, EffectTag};
pub use expr_store::body::{expr_id_from_raw, pat_id_from_raw, Body, BodySourceMap};
pub use expr_store::hir::{
    Expr, ExprId, ExprOrPatId, MappingArm, MappingDirection, MatchArm, Pat, PatId, Statement,
};
pub use expr_store::{Binding, BindingId};
/// Backward-compat: `crate::body` re-exports from `crate::expr_store::body`.
pub mod body {
    pub use crate::expr_store::body::*;
}
pub use item_scope::ItemScope;
pub use item_tree::{Associativity, DefKind, FixityDecl, ItemTree};
pub use name::Name;
pub use nameres::{DefData, DefDiagnostic, DefIdx, DefMap};
pub use per_ns::{Namespace, PerNs};
pub use resolver::{Resolution, ResolveValueResult, Resolver, TypeNs, ValueNs};
pub use scattered::{
    workspace_scattered_status, InFileSpan, ScatteredHeadLocation, ScatteredKind, ScatteredStatus,
};
pub use visibility::{RawVisibility, Visibility};
/// Sail's built-in type constructors.
///
/// Mirrors upstream `Type_env.builtin_typs` (type_env.ml:353-390). Note what
/// is *not* here: `bit`, `bits`, `option` and `result` come from the prelude
/// (`type bits('n) = bitvector('n)` lives in `lib/vector.sail`, and
/// `option`/`result` are the `reserved_type_ids` only a `$sail_internal` file
/// may define), so treating them as built in makes a file typecheck that
/// upstream would reject for want of an include.
pub fn is_builtin_type_name(name: &str) -> bool {
    builtin_type_kinds(name).is_some()
}

/// Sail's five kinds (`parser.mly:412`).
///
/// A kind is what a *type argument* is, and Sail's type constructors take
/// arguments at three of them: `range` takes two `Int`s, `vector` an `Int`
/// and a `Type`, `atom_bool` a `Bool`. `Nat` exists in the grammar and is
/// `Int` with a non-negativity constraint; `Order` is `inc`/`dec`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Kind {
    Int,
    Type,
    Bool,
    Order,
}

impl Kind {
    /// As upstream names it in `Expected this type to have kind ...`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Int => "Int",
            Kind::Type => "Type",
            Kind::Bool => "Bool",
            Kind::Order => "Order",
        }
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Some(match name {
            // `Nat` is `Int` with `'n >= 0`; nothing here distinguishes them.
            "Int" | "Nat" => Kind::Int,
            "Type" => Kind::Type,
            "Bool" => Kind::Bool,
            "Order" => Kind::Order,
            _ => return None,
        })
    }
}

/// The kinds a built-in type constructor takes, in order.
///
/// Upstream's `Type_env.builtin_typs` (`type_env.ml:364`) carries
/// `(parameter_kinds, kind)` per entry; this used to copy the *names* only,
/// so `range(bool, 3)` and `vector(8, 4)` — which upstream rejects with
/// "Expected this type to have kind Int but found kind Type" — went
/// unreported. The arity check that already existed hand-coded the
/// `vector`/`bitvector` order argument as a special case; that special case
/// is this general rule.
///
/// `vector` and `bitvector` have a second, legacy form carrying an explicit
/// order, so they answer for the length they were given.
pub fn builtin_type_kinds(name: &str) -> Option<&'static [Kind]> {
    use Kind::{Bool, Int, Order, Type};
    Some(match name {
        "range" => &[Int, Int][..],
        "atom" | "implicit" | "itself" => &[Int],
        "vector" => &[Int, Type],
        "bitvector" => &[Int],
        "register" | "list" => &[Type],
        "atom_bool" => &[Bool],
        "unit" | "int" | "nat" | "bool" | "real" | "string" | "string_literal" => &[],
        "float16" | "float32" | "float64" | "float128" | "float_rounding_mode" => &[],
        // The legacy order forms, which `builtin_type_arity` also allows.
        "vector_legacy" => &[Int, Order, Type],
        "bitvector_legacy" => &[Int, Order],
        // `int('n)` and `bool('b)` are how Sail writes `atom('n)` and
        // `atom_bool('b)`; upstream's table has only the nullary spellings
        // because the sugar is desugared before it gets there.
        "int_applied" => &[Int],
        "bool_applied" => &[Bool],
        _ => return None,
    })
}

/// Numeric-valued functions Sail provides at the type level
/// (type_env.ml:387-389).
pub fn is_builtin_type_level_fn(name: &str) -> bool {
    matches!(name, "abs" | "mod" | "div" | "pow")
}

pub use workspace_def_map::{GlobalDef, WorkspaceDefMap};

pub use item_id::{
    DefId, DefWithBodyId, FunctionId, FunctionLoc, LetId, LetLoc, MappingId, MappingLoc,
    OverloadId, OverloadLoc, RegisterId, RegisterLoc, TypeDefId, TypeDefLoc, ValSpecId, ValSpecLoc,
};

pub use in_file::{FilePosition, FileRange, InFile};
pub use src::{def_id_source, def_source, HasSource};
