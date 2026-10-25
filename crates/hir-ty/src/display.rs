//! The `HirDisplay` trait, which serves two purposes: Turning various
//! bits from HIR into human-readable strings, and also determining the
//! rendering of types for IDE features like hover.

use std::fmt::{self, Write as _}; // Write must be in scope for write!() macro

use crate::infer::{Kind, Ty, TyArg, TyKind, TypeMismatch};
use crate::ty::{CompareOp, ConstraintExpr, FnSig, NumericExpr, Scalar};

/// A `fmt::Write` extension that can optionally track location links.
pub trait HirWrite: fmt::Write {
    fn start_location_link(&mut self, _location: &str) {}
    fn end_location_link(&mut self) {}
}

impl HirWrite for String {}
impl HirWrite for fmt::Formatter<'_> {}

#[derive(Debug)]
pub enum HirDisplayError {
    /// The maximum size of the formatted output was reached.
    FmtError,
    /// The display was truncated because the output was too long.
    DisplaySourceCode,
}

impl From<fmt::Error> for HirDisplayError {
    fn from(_: fmt::Error) -> Self {
        HirDisplayError::FmtError
    }
}

pub type Result<T = (), E = HirDisplayError> = std::result::Result<T, E>;

/// The formatter context for `HirDisplay`.
/// ```text
/// pub struct HirFormatter<'a, 'db> {
///     pub db: &'db dyn HirDatabase,
///     fmt: &'a mut dyn HirWrite,
///     buf: String,
///     curr_size: usize,
///     max_size: Option<usize>,
///     pub entity_limit: Option<usize>,
/// }
/// ```
///
/// Simplified for Sail (no lifetimes, closures, generics display config).
pub struct HirFormatter<'a> {
    /// The database, for the impls that render a name or a signature.
    pub db: &'a dyn salsa::Database,
    /// The sink to write into.
    fmt: &'a mut dyn HirWrite,
    /// Current size of formatted output.
    curr_size: usize,
    /// Size from which we should truncate the output.
    max_size: Option<usize>,
    /// When rendering something with children, limits how many to show.
    pub entity_limit: Option<usize>,
}

impl<'a> HirFormatter<'a> {
    /// A formatter. The database is required, because an impl that
    /// renders a name needs one and there is no useful thing to print
    /// without it.
    pub fn new_with_db(fmt: &'a mut dyn HirWrite, db: &'a dyn salsa::Database) -> Self {
        Self { db, fmt, curr_size: 0, max_size: None, entity_limit: None }
    }

    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = Some(max_size);
        self
    }

    pub fn with_entity_limit(mut self, limit: usize) -> Self {
        self.entity_limit = Some(limit);
        self
    }

    fn should_truncate(&self) -> bool {
        self.max_size.is_some_and(|max| self.curr_size >= max)
    }
}

impl fmt::Write for HirFormatter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.should_truncate() {
            return Ok(());
        }
        self.curr_size += s.len();
        self.fmt.write_str(s)
    }
}

/// Trait for displaying HIR types in a human-readable format.
pub trait HirDisplay {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result;

    /// Get a `HirDisplayWrapper` for use with `format!` / `write!`.
    ///
    /// The database is not optional, as it is not in rust-analyzer's
    /// `display(db, target)`. It used to be: `HirFormatter::db` was an
    /// `Option` that every constructor left `None`, so every impl that
    /// needed one took its fallback branch and a `Function` rendered as
    /// the word `function` — no name, no signature.
    fn display<'a>(&'a self, db: &'a dyn salsa::Database) -> HirDisplayWrapper<'a, Self>
    where
        Self: Sized,
    {
        HirDisplayWrapper::new(self, db)
    }

    /// Format to a String suitable for inserting into source code.
    fn display_source_code(
        &self,
        db: &dyn salsa::Database,
    ) -> std::result::Result<String, HirDisplayError> {
        let mut s = String::new();
        let mut f = HirFormatter::new_with_db(&mut s, db);
        self.hir_fmt(&mut f)?;
        Ok(s)
    }

    /// Convenience: format to a String.
    fn display_to_string(&self, db: &dyn salsa::Database) -> String {
        let mut s = String::new();
        let mut f = HirFormatter::new_with_db(&mut s, db);
        let _ = self.hir_fmt(&mut f);
        s
    }

    /// Convenience: format with a max size.
    fn display_truncated(&self, db: &dyn salsa::Database, max_size: usize) -> String {
        if max_size == 0 {
            return String::new();
        }
        let mut s = String::new();
        let mut f = HirFormatter::new_with_db(&mut s, db).with_max_size(max_size);
        let _ = self.hir_fmt(&mut f);
        if s.len() >= max_size {
            let mut end = max_size.saturating_sub(1).min(s.len());
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            s.truncate(end);
            s.push('…');
        }
        s
    }
}

/// Wrapper that implements `fmt::Display` for any `HirDisplay` type.
pub struct HirDisplayWrapper<'a, T: HirDisplay> {
    inner: &'a T,
    db: &'a dyn salsa::Database,
    max_size: Option<usize>,
}

impl<'a, T: HirDisplay> HirDisplayWrapper<'a, T> {
    pub fn new(inner: &'a T, db: &'a dyn salsa::Database) -> Self {
        Self { inner, db, max_size: None }
    }

    pub fn with_max_size(mut self, max_size: usize) -> Self {
        self.max_size = Some(max_size);
        self
    }
}

impl<T: HirDisplay> fmt::Display for HirDisplayWrapper<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut hf = HirFormatter::new_with_db(f, self.db);
        if let Some(max) = self.max_size {
            hf = hf.with_max_size(max);
        }
        self.inner.hir_fmt(&mut hf).map_err(|_| fmt::Error)
    }
}

fn is_anonymous_loop_index(ty: &Ty) -> bool {
    let TyKind::App { name, args, .. } = ty.kind() else { return false };
    if name != "atom" || args.len() != 1 {
        return false;
    }
    let value = match &args[0] {
        TyArg::Type(ty) => Some(ty.display_text()),
        arg => arg.as_value_str(),
    };
    value
        .as_deref()
        .and_then(|v| v.strip_prefix("'_sk"))
        .and_then(|v| {
            v.strip_suffix("__loop_index").or_else(|| {
                v.strip_suffix("__flow_int").and_then(|v| v.split_once('_').map(|(id, _)| id))
            })
        })
        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
}

/// Legacy argument diagnostics store text instead of Ty. Apply the same
/// presentation rule without changing the text used by inference substitution.
pub(crate) fn diagnostic_type_text(ty: &Ty) -> String {
    if is_anonymous_loop_index(ty) {
        "int".into()
    } else {
        ty.display_text()
    }
}

impl HirDisplay for Ty {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        match self.kind() {
            TyKind::Error => write!(f, "{{error}}")?,
            TyKind::Scalar(scalar) => write!(f, "{}", scalar.name())?,
            TyKind::Named(name, args) => {
                write!(f, "{name}")?;
                if !args.is_empty() {
                    write!(f, "(")?;
                    for (i, arg) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        match arg {
                            TyArg::Type(ty) => ty.hir_fmt(f)?,
                            TyArg::Nexp(n) => write!(f, "{}", n.to_string_repr())?,
                            TyArg::Bool(c) => c.hir_fmt(f)?,
                            TyArg::Order(v) | TyArg::Value(v) => write!(f, "{v}")?,
                        }
                    }
                    write!(f, ")")?;
                }
            }
            TyKind::Param(name) => write!(f, "{name}")?,
            // An unresolved inference variable. `?{id}` is this checker's
            // own counter and says nothing to a reader; `_` is what a
            // reader can act on — the width is not yet known.
            TyKind::Infer(_) => write!(f, "_")?,
            TyKind::Tuple(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    item.hir_fmt(f)?;
                }
                write!(f, ")")?;
            }
            TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
                write!(f, "(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    p.hir_fmt(f)?;
                }
                write!(f, ") -> ")?;
                ret.hir_fmt(f)?;
            }
            TyKind::App { name, args, text } => {
                // A fresh runtime identity helps inference, but its internal
                // counter conveys no source-level refinement to the reader.
                let anonymous_bool = name == "bool"
                    && matches!(args.as_slice(),
                    [TyArg::Bool(ConstraintExpr::BoolVar(v))]
                    if v.strip_prefix("'_sk").and_then(|v| v.strip_suffix("__flow_bool"))
                        .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())));
                let anonymous_index = is_anonymous_loop_index(self);
                if anonymous_index {
                    write!(f, "int")?;
                } else if args.is_empty() || anonymous_bool {
                    write!(f, "{name}")?;
                } else {
                    // If the original text is more readable, use it
                    if !text.is_empty() {
                        write!(f, "{text}")?;
                    } else {
                        write!(f, "{name}(")?;
                        for (i, arg) in args.iter().enumerate() {
                            if i > 0 {
                                write!(f, ", ")?;
                            }
                            match arg {
                                TyArg::Type(ty) => ty.hir_fmt(f)?,
                                TyArg::Nexp(n) => write!(f, "{}", n.to_string_repr())?,
                                TyArg::Bool(c) => c.hir_fmt(f)?,
                                TyArg::Order(v) | TyArg::Value(v) => write!(f, "{v}")?,
                            }
                        }
                        write!(f, ")")?;
                    }
                }
            }
            TyKind::Exist { vars, inner, .. } => {
                write!(f, "{{")?;
                for (i, v) in vars.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, ". ")?;
                inner.hir_fmt(f)?;
                write!(f, "}}")?;
            }
            TyKind::Bidir { lhs, rhs } => {
                lhs.hir_fmt(f)?;
                write!(f, " <-> ")?;
                rhs.hir_fmt(f)?;
            }
            TyKind::Abstract { name, kind } => {
                write!(f, "{name} : ")?;
                match kind {
                    Kind::Type => write!(f, "Type")?,
                    Kind::Int => write!(f, "Int")?,
                    Kind::Bool => write!(f, "Bool")?,
                    Kind::Order => write!(f, "Order")?,
                }
            }
        }
        Ok(())
    }
}

impl HirDisplay for Kind {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        match self {
            Kind::Type => write!(f, "Type")?,
            Kind::Int => write!(f, "Int")?,
            Kind::Bool => write!(f, "Bool")?,
            Kind::Order => write!(f, "Order")?,
        }
        Ok(())
    }
}

impl HirDisplay for FnSig {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        write!(f, "(")?;
        for (i, param) in self.params.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            param.hir_fmt(f)?;
        }
        write!(f, ") -> ")?;
        self.ret.hir_fmt(f)
    }
}

impl HirDisplay for Scalar {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        write!(f, "{}", self.name())?;
        Ok(())
    }
}

impl HirDisplay for TypeMismatch {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        write!(f, "expected ")?;
        self.expected.hir_fmt(f)?;
        write!(f, ", found ")?;
        self.actual.hir_fmt(f)
    }
}

impl HirDisplay for ConstraintExpr {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        match self {
            ConstraintExpr::Bool(true) => write!(f, "true")?,
            ConstraintExpr::Bool(false) => write!(f, "false")?,
            ConstraintExpr::Compare { lhs, op, rhs } => {
                lhs.hir_fmt(f)?;
                let op_str = match op {
                    CompareOp::Eq => " == ",
                    CompareOp::Neq => " != ",
                    CompareOp::Lt => " < ",
                    CompareOp::Lte => " <= ",
                    CompareOp::Gt => " > ",
                    CompareOp::Gte => " >= ",
                };
                write!(f, "{op_str}")?;
                rhs.hir_fmt(f)?;
            }
            ConstraintExpr::InSet { value, items } => {
                value.hir_fmt(f)?;
                write!(f, " in {{")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    item.hir_fmt(f)?;
                }
                write!(f, "}}")?;
            }
            ConstraintExpr::And(cs) => {
                for (i, c) in cs.iter().enumerate() {
                    if i > 0 {
                        write!(f, " & ")?;
                    }
                    c.hir_fmt(f)?;
                }
            }
            ConstraintExpr::Or(cs) => {
                for (i, c) in cs.iter().enumerate() {
                    if i > 0 {
                        write!(f, " | ")?;
                    }
                    c.hir_fmt(f)?;
                }
            }
            ConstraintExpr::Not(c) => {
                write!(f, "not(")?;
                c.hir_fmt(f)?;
                write!(f, ")")?;
            }
            ConstraintExpr::Unsupported => write!(f, "{{constraint}}")?,
            ConstraintExpr::App { name, args } => {
                write!(f, "{name}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    arg.hir_fmt(f)?;
                }
                write!(f, ")")?;
            }
            ConstraintExpr::BoolVar(v) => write!(f, "{v}")?,
        }
        Ok(())
    }
}

impl HirDisplay for NumericExpr {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        match self {
            NumericExpr::Const(n) => write!(f, "{n}")?,
            NumericExpr::BigConst(n) => write!(f, "{n}")?,
            NumericExpr::Var(v) => write!(f, "{v}")?,
            NumericExpr::Symbol(s) => write!(f, "{s}")?,
            NumericExpr::Neg(e) => {
                write!(f, "-(")?;
                e.hir_fmt(f)?;
                write!(f, ")")?;
            }
            NumericExpr::Add(a, b) => {
                a.hir_fmt(f)?;
                write!(f, " + ")?;
                b.hir_fmt(f)?;
            }
            NumericExpr::Sub(a, b) => {
                a.hir_fmt(f)?;
                write!(f, " - ")?;
                b.hir_fmt(f)?;
            }
            NumericExpr::Mul(a, b) => {
                a.hir_fmt(f)?;
                write!(f, " * ")?;
                b.hir_fmt(f)?;
            }
            NumericExpr::Div(a, b) => {
                a.hir_fmt(f)?;
                write!(f, " / ")?;
                b.hir_fmt(f)?;
            }
            NumericExpr::Mod(a, b) => {
                a.hir_fmt(f)?;
                write!(f, " % ")?;
                b.hir_fmt(f)?;
            }
            NumericExpr::Exp(e) => {
                write!(f, "2^(")?;
                e.hir_fmt(f)?;
                write!(f, ")")?;
            }
            NumericExpr::App { name, args } => {
                write!(f, "{}", name)?;
                if !args.is_empty() {
                    write!(f, "(")?;
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        a.hir_fmt(f)?;
                    }
                    write!(f, ")")?;
                }
            }
            NumericExpr::If { cond, then_expr, else_expr } => {
                write!(f, "if {} then ", cond.to_text())?;
                then_expr.hir_fmt(f)?;
                write!(f, " else ")?;
                else_expr.hir_fmt(f)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infer::Ty;

    /// The formatter has no database-less path.
    ///
    /// `HirFormatter::db` was an `Option` that every constructor left
    /// `None`, so every impl that needs a database took a fallback branch:
    /// a `Function` rendered as the word `function`, a `struct` as
    /// `struct`, with no name and no signature. rust-analyzer's
    /// `display(db, target)` takes the database, which is what makes that
    /// impossible. Ours does now, and the fallbacks are gone.
    ///
    /// Still open, and a different gap: outside `Ty`, no feature calls any
    /// of these impls — `ide/src/hover` builds its markup from the
    /// per-file scrape's `Decl` rather than from a `Def`.
    #[test]
    fn the_formatter_always_has_a_database() {
        let src = include_str!("display.rs");
        let optional_db = ["pub db: ", "Option"].concat();
        assert!(!src.contains(&optional_db), "the formatter can be built without a database");
        let hir_display = include_str!("../../hir/src/display.rs");
        assert!(
            !hir_display.contains("if let Some(db) = f.db"),
            "an impl still has a database-less fallback"
        );
    }

    /// A database, because `display` requires one — a `Ty` does not read
    /// it, but an impl that renders a name does, and the parameter is
    /// what makes that impossible to forget.
    fn db() -> salsa::DatabaseImpl {
        salsa::DatabaseImpl::new()
    }

    #[test]
    fn display_error() {
        assert_eq!(Ty::error().display_to_string(&db()), "{error}");
    }

    #[test]
    fn display_named() {
        assert_eq!(Ty::named("int").display_to_string(&db()), "int");
        assert_eq!(Ty::named("bool").display_to_string(&db()), "bool");
    }

    #[test]
    fn display_tuple() {
        let ty = Ty::tuple(vec![Ty::named("int"), Ty::named("bool")]);
        assert_eq!(ty.display_to_string(&db()), "(int, bool)");
    }

    #[test]
    fn display_function() {
        let ty = Ty::function(vec![Ty::named("int")], Ty::named("bool"));
        assert_eq!(ty.display_to_string(&db()), "(int) -> bool");
    }

    #[test]
    fn display_truncated() {
        let ty = Ty::named("very_long_type_name_that_should_be_truncated");
        let s = ty.display_truncated(&db(), 10);
        assert!(s.ends_with('…'), "expected truncation ellipsis, got: {s}");
        // '…' is 3 bytes in UTF-8, so max byte len = 9 + 3 = 12
        assert!(s.len() <= 12, "too long: {}", s.len());
    }

    #[test]
    fn truncated_unicode_names_keep_utf8_boundaries() {
        let ty = Ty::named("中文🦀类型");
        assert_eq!(ty.display_truncated(&db(), 0), "");
        for limit in 1..20 {
            let text = ty.display_truncated(&db(), limit);
            let prefix = text.strip_suffix('…').unwrap_or(&text);
            assert!("中文🦀类型".starts_with(prefix));
            assert!(text.len() <= limit + 2);
        }
    }

    // Verify all type kinds display correctly

    #[test]
    fn display_existential() {
        use crate::ty::ConstraintExpr;
        let ty = Ty::exist(vec!["'n".to_string()], ConstraintExpr::Bool(true), Ty::named("int"));
        let s = ty.display_to_string(&db());
        assert!(s.contains("'n"), "should show quantifier: {s}");
        assert!(s.contains("int"), "should show inner type: {s}");
    }

    #[test]
    fn display_bidir() {
        let ty = Ty::bidir(Ty::named("int"), Ty::named("string"));
        let s = ty.display_to_string(&db());
        assert_eq!(s, "int <-> string");
    }
}

impl HirDisplay for TyArg {
    fn hir_fmt(&self, f: &mut HirFormatter<'_>) -> Result {
        match self {
            TyArg::Type(t) => t.hir_fmt(f),
            TyArg::Nexp(n) => n.hir_fmt(f),
            TyArg::Bool(c) => c.hir_fmt(f),
            TyArg::Order(v) | TyArg::Value(v) => Ok(write!(f, "{v}")?),
        }
    }
}
