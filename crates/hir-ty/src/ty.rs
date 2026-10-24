//! Type representation for the Sail type checker.
//!
//! `Ty` wraps `Interned<TyKind>` for O(1) clones and pointer-equality.

use intern::{impl_internable, Interned};

/// Primitive/scalar types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scalar {
    Int,
    Nat,
    Bool,
    Unit,
    String,
    Real,
    Bit,
}

impl Scalar {
    /// Try to classify a type name as a scalar.
    pub fn from_name(name: &str) -> Option<Scalar> {
        match name {
            "int" => Some(Scalar::Int),
            "nat" => Some(Scalar::Nat),
            "bool" => Some(Scalar::Bool),
            "unit" => Some(Scalar::Unit),
            "string" => Some(Scalar::String),
            "real" => Some(Scalar::Real),
            "bit" => Some(Scalar::Bit),
            _ => None,
        }
    }

    /// The canonical name of this scalar.
    pub fn name(&self) -> &'static str {
        match self {
            Scalar::Int => "int",
            Scalar::Nat => "nat",
            Scalar::Bool => "bool",
            Scalar::Unit => "unit",
            Scalar::String => "string",
            Scalar::Real => "real",
            Scalar::Bit => "bit",
        }
    }
}

/// Inference variable identifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InferTy(pub u32);

/// Function signature type.
///
/// Extracted from the `TyKind::FnPtr` variant for reuse.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub ret: Ty,
}

/// Type constructor for applied types.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyCtor(pub String);

/// The kind of a type.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TyKind {
    /// Error / unknown type. Prevents cascading diagnostics.
    Error,
    /// Primitive scalar type (int, nat, bool, unit, string, real, bit).
    Scalar(Scalar),
    /// A named type, applied to arguments or not: Sail's `Typ_id` and
    /// `Typ_app`, which are the same form with an empty argument list.
    ///
    /// `App` below is this shape again, keeping the source text. The two
    /// have not been merged, so code that compares types has to accept
    /// either — `types_could_unify` in ide-completion has an arm for it.
    Named(String, Vec<TyArg>),
    /// Type parameter / type variable (e.g., `'n`, `'a`).
    Param(String),
    /// Inference variable — placeholder resolved during type inference.
    Infer(InferTy),
    /// Tuple type.
    Tuple(Vec<Ty>),
    /// Function type with parameter types and return type.
    FnPtr(FnSig),
    /// Type application (e.g., `bits('n)`, `vector('n, 'a)`).
    App { name: String, args: Vec<TyArg>, text: String },
    /// Existential type — `{'n, 'n > 0. bits('n)}`.
    Exist { vars: Vec<String>, constraint: ConstraintExpr, inner: Ty },
    /// Bidirectional mapping type — `T1 <-> T2`.
    Bidir { lhs: Ty, rhs: Ty },
    /// Abstract type — `type xlen : Int`.
    Abstract { name: String, kind: Kind },
}

/// `bits(8)` and `bits (8)` both name `bits`. Some `Named`/`App` names
/// carry their arguments in the string; this drops them.
fn trim_args(name: &str) -> &str {
    name.split('(').next().unwrap_or(name).trim_end()
}

impl_internable!(TyKind);

/// Interned type. Use `ty.kind()` to inspect.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Ty(Interned<TyKind>);

impl Ty {
    /// Construct a `Ty` from a `TyKind`, interning it globally.
    pub fn new(kind: TyKind) -> Self {
        Self(Interned::new(kind))
    }

    /// Access the underlying type kind.
    pub fn kind(&self) -> &TyKind {
        &self.0
    }

    /// Access the inner `Interned` (for tests and advanced usage).
    pub fn interned(&self) -> &Interned<TyKind> {
        &self.0
    }

    /// Whether this is an error type.
    pub fn is_error(&self) -> bool {
        matches!(self.kind(), TyKind::Error)
    }

    /// Whether every part of this type is known — no error, no inference
    /// variable, no type parameter, no unresolved width.
    ///
    /// `references_error` answers "did inference fail somewhere". This asks
    /// the stronger question a *report* needs: is there enough here to say
    /// the program is wrong? A `bits('n)` or a `bits(?0)` looks like a
    /// mismatch against `bits(3)` and is not one — the width simply is not
    ///resolved yet — so anything a checker reports on has to pass this first.
    pub fn is_fully_known(&self) -> bool {
        fn arg_known(arg: &TyArg) -> bool {
            match arg {
                TyArg::Type(t) => t.is_fully_known(),
                // A width written `'n` is a type variable, not a number.
                TyArg::Nexp(n) => !n.to_string_repr().contains('\''),
                TyArg::Bool(c) => !c.to_text().contains('\''),
                TyArg::Order(v) | TyArg::Value(v) => !v.contains('\''),
            }
        }
        match self.kind() {
            TyKind::Error | TyKind::Infer(_) | TyKind::Param(_) | TyKind::Abstract { .. } => false,
            // An existential hides its witness; two of them can be equal
            // without looking it.
            TyKind::Exist { .. } => false,
            TyKind::Scalar(_) => true,
            TyKind::Named(_, args) | TyKind::App { args, .. } => args.iter().all(arg_known),
            TyKind::Tuple(items) => items.iter().all(Ty::is_fully_known),
            TyKind::FnPtr(sig) => {
                sig.params.iter().all(Ty::is_fully_known) && sig.ret.is_fully_known()
            }
            TyKind::Bidir { lhs, rhs } => lhs.is_fully_known() && rhs.is_fully_known(),
        }
    }

    /// Whether this type contains an error type anywhere inside it.
    ///
    /// `is_error` answers "did inference fail outright"; this answers "did it
    /// fail somewhere". rust-analyzer draws the same distinction
    /// (`references_non_lt_error`) and reports both in `analysis-stats`,
    /// because a `bits(?)` whose width never resolved is a different kind of
    /// gap from an expression with no type at all.
    pub fn references_error(&self) -> bool {
        fn arg_has_error(arg: &TyArg) -> bool {
            match arg {
                TyArg::Type(t) => t.references_error(),
                TyArg::Value(_) | TyArg::Bool(_) | TyArg::Order(_) | TyArg::Nexp(_) => false,
            }
        }
        match self.kind() {
            TyKind::Error => true,
            TyKind::Scalar(_) | TyKind::Param(_) | TyKind::Infer(_) | TyKind::Abstract { .. } => {
                false
            }
            TyKind::Named(_, args) | TyKind::App { args, .. } => args.iter().any(arg_has_error),
            TyKind::Tuple(items) => items.iter().any(Ty::references_error),
            TyKind::FnPtr(sig) => {
                sig.params.iter().any(Ty::references_error) || sig.ret.references_error()
            }
            TyKind::Exist { inner, .. } => inner.references_error(),
            TyKind::Bidir { lhs, rhs } => lhs.references_error() || rhs.references_error(),
        }
    }

    /// Whether this is a bitvector type (`bits('n)`).
    pub fn is_bits(&self) -> bool {
        match self.kind() {
            TyKind::App { name, .. } => name == "bits" || name == "bitvector",
            TyKind::Scalar(Scalar::Bit) => true,
            _ => false,
        }
    }

    /// Whether this type is likely a bitvector (including possible aliases).
    pub fn is_bits_like(&self) -> bool {
        if self.is_bits() {
            return true;
        }
        match self.kind() {
            TyKind::Named(name, _) => {
                // Primitive types that are definitely NOT bitvectors.
                !matches!(
                    name.as_str(),
                    "int"
                        | "nat"
                        | "bool"
                        | "string"
                        | "unit"
                        | "real"
                        | "option"
                        | "list"
                        | "vector"
                        | "result"
                )
            }
            _ => false,
        }
    }

    /// Get the type name if this is a Scalar or Named.
    pub fn as_name(&self) -> Option<&str> {
        match self.kind() {
            TyKind::Scalar(s) => Some(s.name()),
            TyKind::Named(name, _) => Some(name.as_str()),
            _ => None,
        }
    }

    /// The type constructor's name, for any of the three ways a named type
    /// is represented: a scalar, a `Named`, or an `App`.
    ///
    /// Classifying by `display_text().starts_with("bits(")` instead misses
    /// `bits (8)` — a space the renderer sometimes emits — which is how six
    /// bitvector concatenations per sail-riscv run came out untyped.
    pub fn ctor_name(&self) -> Option<&str> {
        match self.kind() {
            TyKind::Scalar(s) => Some(s.name()),
            TyKind::Named(name, _) => Some(trim_args(name)),
            TyKind::App { name, .. } => Some(trim_args(name)),
            _ => None,
        }
    }

    /// Check if this type has a specific name (Scalar or Named).
    pub fn is_named(&self, name: &str) -> bool {
        self.as_name() == Some(name)
    }

    pub fn error() -> Self {
        Self::new(TyKind::Error)
    }

    /// Construct a type from a name, classifying into Scalar or Named.
    pub fn named<S: Into<String>>(s: S) -> Self {
        let name = s.into();
        if let Some(scalar) = Scalar::from_name(&name) {
            Self::new(TyKind::Scalar(scalar))
        } else {
            Self::new(TyKind::Named(name, Vec::new()))
        }
    }

    /// Construct a scalar (primitive) type by enum value.
    pub fn scalar(s: Scalar) -> Self {
        Self::new(TyKind::Scalar(s))
    }

    /// Construct a user-defined ADT type (no type arguments).
    pub fn adt<S: Into<String>>(s: S) -> Self {
        Self::new(TyKind::Named(s.into(), Vec::new()))
    }

    /// Construct a user-defined ADT type with type arguments.
    pub fn adt_with_args<S: Into<String>>(s: S, args: Vec<TyArg>) -> Self {
        Self::new(TyKind::Named(s.into(), args))
    }

    pub fn param<S: Into<String>>(s: S) -> Self {
        Self::new(TyKind::Param(s.into()))
    }

    pub fn tuple(items: Vec<Ty>) -> Self {
        Self::new(TyKind::Tuple(items))
    }

    pub fn function(params: Vec<Ty>, ret: Ty) -> Self {
        Self::new(TyKind::FnPtr(FnSig { params, ret }))
    }

    /// Construct a function type from a `FnSig`.
    pub fn from_fn_sig(sig: FnSig) -> Self {
        Self::new(TyKind::FnPtr(sig))
    }

    pub fn app<N: Into<String>, T: Into<String>>(name: N, args: Vec<TyArg>, text: T) -> Self {
        let name = name.into();
        let mut text = text.into();
        if text.is_empty() {
            text = format!(
                "{}({})",
                name,
                args.iter()
                    .map(|a| match a {
                        TyArg::Type(t) => t.display_text(),
                        _ => a.as_value_str().unwrap_or_default(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        Self::new(TyKind::App { name, args, text })
    }

    pub fn exist(vars: Vec<String>, constraint: ConstraintExpr, inner: Ty) -> Self {
        Self::new(TyKind::Exist { vars, constraint, inner })
    }

    pub fn bidir(lhs: Ty, rhs: Ty) -> Self {
        Self::new(TyKind::Bidir { lhs, rhs })
    }
}

/// A type argument (type or numeric value).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TyArg {
    Type(Ty),
    /// Structured numeric argument (parsed from a string).
    Nexp(NumericExpr),
    Bool(ConstraintExpr),
    Order(String),
    /// Fallback for unparsable numeric strings (complex expressions).
    Value(String),
}

impl TyArg {
    /// Create a numeric argument, parsing as `NumericExpr` or falling back to `Value`.
    pub fn numeric(s: impl Into<String>) -> Self {
        let text = s.into();
        match text.trim() {
            "true" => return TyArg::Bool(ConstraintExpr::Bool(true)),
            "false" => return TyArg::Bool(ConstraintExpr::Bool(false)),
            "inc" | "dec" => return TyArg::Order(text.trim().to_owned()),
            _ => {}
        }
        if let Some(nexp) = NumericExpr::parse(&text) {
            TyArg::Nexp(nexp)
        } else {
            TyArg::Value(text)
        }
    }

    /// String representation of a Value/Nexp arg. `None` for Type.
    pub fn as_value_str(&self) -> Option<String> {
        match self {
            TyArg::Value(s) | TyArg::Order(s) => Some(s.clone()),
            TyArg::Bool(c) => Some(c.to_text()),
            TyArg::Nexp(n) => Some(n.to_string_repr()),
            TyArg::Type(_) => None,
        }
    }
}

pub use hir_def::Kind;

/// A constraint expression (type-level boolean).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConstraintExpr {
    Bool(bool),
    Compare {
        lhs: NumericExpr,
        op: CompareOp,
        rhs: NumericExpr,
    },
    InSet {
        value: NumericExpr,
        items: Vec<NumericExpr>,
    },
    And(Vec<ConstraintExpr>),
    Or(Vec<ConstraintExpr>),
    Not(Box<ConstraintExpr>),
    /// NC_app: general constraint function application (e.g. `app(arg1, arg2)`).
    App {
        name: String,
        args: Vec<TyArg>,
    },
    /// NC_var: a boolean-kinded type variable (e.g. `'p` where `'p : Bool`).
    BoolVar(String),
    Unsupported,
}

/// How this crate renders a type variable inference has not resolved.
///
/// It is a constant because two places read it: the renderer, and the
/// check in `unify` that refuses to push a constraint still mentioning
/// one. That check used to grep the rendered text for `?`, so changing
/// the renderer changed what the checker *did* — 91 warnings appeared on
/// sail-riscv from a change to a display string. One name, or they drift
/// again.
pub const UNRESOLVED_VAR: &str = "_";

impl ConstraintExpr {
    /// Render as a human-readable text string (for display and debug purposes).
    pub fn to_text(&self) -> String {
        match self {
            ConstraintExpr::Bool(b) => b.to_string(),
            ConstraintExpr::Compare { lhs, op, rhs } => {
                let op_str = match op {
                    CompareOp::Eq => "==",
                    CompareOp::Neq => "!=",
                    CompareOp::Lt => "<",
                    CompareOp::Lte => "<=",
                    CompareOp::Gt => ">",
                    CompareOp::Gte => ">=",
                };
                format!("{} {} {}", lhs.to_string_repr(), op_str, rhs.to_string_repr())
            }
            ConstraintExpr::InSet { value, items } => {
                let items_str: Vec<String> = items.iter().map(|i| i.to_string_repr()).collect();
                format!("{} in {{{}}}", value.to_string_repr(), items_str.join(", "))
            }
            ConstraintExpr::And(parts) => {
                let texts: Vec<String> = parts
                    .iter()
                    .map(|p| match p {
                        ConstraintExpr::Or(_) => format!("({})", p.to_text()),
                        _ => p.to_text(),
                    })
                    .collect();
                texts.join(" & ")
            }
            ConstraintExpr::Or(parts) => {
                let texts: Vec<String> = parts.iter().map(|p| p.to_text()).collect();
                texts.join(" | ")
            }
            ConstraintExpr::Not(inner) => format!("~({})", inner.to_text()),
            ConstraintExpr::App { name, args } => {
                let args_str: Vec<String> = args
                    .iter()
                    .map(|a| {
                        a.as_value_str().unwrap_or_else(|| match a {
                            TyArg::Type(t) => t.display_text(),
                            _ => unreachable!(),
                        })
                    })
                    .collect();
                format!("{}({})", name, args_str.join(", "))
            }
            ConstraintExpr::BoolVar(v) => v.clone(),
            ConstraintExpr::Unsupported => "<unsupported>".to_string(),
        }
    }
}

/// Comparison operator for numeric constraints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CompareOp {
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
}

/// A numeric expression (type-level integer arithmetic).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NumericExpr {
    Const(i64),
    /// Integer outside the inline i64 fast path.
    BigConst(num_bigint::BigInt),
    Var(String),
    Symbol(String),
    Neg(Box<NumericExpr>),
    Add(Box<NumericExpr>, Box<NumericExpr>),
    Sub(Box<NumericExpr>, Box<NumericExpr>),
    Mul(Box<NumericExpr>, Box<NumericExpr>),
    Div(Box<NumericExpr>, Box<NumericExpr>),
    Mod(Box<NumericExpr>, Box<NumericExpr>),
    /// Exponentiation: `2^n`. Base is always 2 (Sail convention).
    Exp(Box<NumericExpr>),
    /// Function application in type-level arithmetic: `div('n, 8)`, `mod('n, 4)`.
    App {
        name: String,
        args: Vec<NumericExpr>,
    },
    /// Type-level if-then-else.
    If {
        cond: Box<ConstraintExpr>,
        then_expr: Box<NumericExpr>,
        else_expr: Box<NumericExpr>,
    },
}

impl NumericExpr {
    /// Parse a simple numeric string (constant, type variable, or plain symbol).
    pub fn parse(text: &str) -> Option<NumericExpr> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        crate::infer::numeric::parse_numeric_expr_text(trimmed)
    }

    pub fn from_integer(value: num_bigint::BigInt) -> Self {
        use num_traits::ToPrimitive;
        value.to_i64().map(Self::Const).unwrap_or_else(|| Self::BigConst(value))
    }

    pub fn integer_literal(text: &str) -> Option<Self> {
        let text = text.replace('_', "");
        let (digits, radix) = if let Some(s) = text.strip_prefix("0x") {
            (s, 16)
        } else if let Some(s) = text.strip_prefix("0b") {
            (s, 2)
        } else {
            (text.as_str(), 10)
        };
        num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix).map(Self::from_integer)
    }

    /// Display as string.
    ///
    /// Parenthesised by *precedence*, not at every node: a message reading
    /// `bits((8 * 'n))` says nothing the reader wanted, and the string is
    /// re-read by `parse` now, so the two have to agree.
    pub fn to_string_repr(&self) -> String {
        self.render(0)
    }

    /// Binding power: `+ -` 1, `* / %` 2, `^` 3, an atom 4.
    fn precedence(&self) -> u8 {
        match self {
            NumericExpr::Add(..) | NumericExpr::Sub(..) => 1,
            NumericExpr::Mul(..) | NumericExpr::Div(..) | NumericExpr::Mod(..) => 2,
            NumericExpr::Exp(_) => 3,
            _ => 4,
        }
    }

    fn render(&self, parent: u8) -> String {
        let me = self.precedence();
        let inner = self.render_inner();
        if me < parent {
            format!("({inner})")
        } else {
            inner
        }
    }

    fn render_inner(&self) -> String {
        match self {
            NumericExpr::Const(n) => n.to_string(),
            NumericExpr::BigConst(n) => n.to_string(),
            NumericExpr::Var(v) => v.clone(),
            NumericExpr::Symbol(s) => s.clone(),
            NumericExpr::Neg(e) => format!("-{}", e.render(4)),
            NumericExpr::Add(a, b) => {
                format!("{} + {}", a.render(1), b.render(2))
            }
            NumericExpr::Sub(a, b) => {
                format!("{} - {}", a.render(1), b.render(2))
            }
            NumericExpr::Mul(a, b) => {
                format!("{} * {}", a.render(2), b.render(3))
            }
            NumericExpr::Div(a, b) => {
                format!("{} / {}", a.render(2), b.render(3))
            }
            NumericExpr::Mod(a, b) => {
                format!("{} % {}", a.render(2), b.render(3))
            }
            NumericExpr::Exp(e) => format!("2 ^ {}", e.render(4)),
            NumericExpr::App { name, args } => {
                let args_str: Vec<String> = args.iter().map(|a| a.to_string_repr()).collect();
                format!("{}({})", name, args_str.join(", "))
            }
            NumericExpr::If { cond, then_expr, else_expr } => format!(
                "(if {} then {} else {})",
                cond.to_text(),
                then_expr.to_string_repr(),
                else_expr.to_string_repr()
            ),
        }
    }
}
