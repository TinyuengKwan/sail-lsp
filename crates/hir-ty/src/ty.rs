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
    /// User-defined type with optional type arguments.
    Adt(String, Vec<TyArg>),
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
    #[allow(dead_code)]
    Abstract { name: String, kind: Kind },
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
            TyKind::Adt(name, _) => {
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

    /// Get the type name if this is a Scalar or Adt.
    pub fn as_name(&self) -> Option<&str> {
        match self.kind() {
            TyKind::Scalar(s) => Some(s.name()),
            TyKind::Adt(name, _) => Some(name.as_str()),
            _ => None,
        }
    }

    /// Check if this type has a specific name (Scalar or Adt).
    pub fn is_named(&self, name: &str) -> bool {
        self.as_name() == Some(name)
    }

    pub fn error() -> Self {
        Self::new(TyKind::Error)
    }

    /// Construct a type from a name, classifying into Scalar or Adt.
    pub fn named<S: Into<String>>(s: S) -> Self {
        let name = s.into();
        if let Some(scalar) = Scalar::from_name(&name) {
            Self::new(TyKind::Scalar(scalar))
        } else {
            Self::new(TyKind::Adt(name, Vec::new()))
        }
    }

    /// Construct a scalar (primitive) type by enum value.
    pub fn scalar(s: Scalar) -> Self {
        Self::new(TyKind::Scalar(s))
    }

    /// Construct a user-defined ADT type (no type arguments).
    pub fn adt<S: Into<String>>(s: S) -> Self {
        Self::new(TyKind::Adt(s.into(), Vec::new()))
    }

    /// Construct a user-defined ADT type with type arguments.
    pub fn adt_with_args<S: Into<String>>(s: S, args: Vec<TyArg>) -> Self {
        Self::new(TyKind::Adt(s.into(), args))
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
        Self::new(TyKind::App { name: name.into(), args, text: text.into() })
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
    /// Fallback for unparsable numeric strings (complex expressions).
    Value(String),
}

impl TyArg {
    /// Create a numeric argument, parsing as `NumericExpr` or falling back to `Value`.
    pub fn numeric(s: impl Into<String>) -> Self {
        let text = s.into();
        if let Some(nexp) = NumericExpr::parse(&text) {
            TyArg::Nexp(nexp)
        } else {
            TyArg::Value(text)
        }
    }

    /// String representation of a Value/Nexp arg. `None` for Type.
    pub fn as_value_str(&self) -> Option<String> {
        match self {
            TyArg::Value(s) => Some(s.clone()),
            TyArg::Nexp(n) => Some(n.to_string_repr()),
            TyArg::Type(_) => None,
        }
    }
}

/// Sail kind system (`kind_aux`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Ordinary types.
    Type,
    /// Integer-valued type variables (may be negative).
    Int,
    /// Boolean-valued type-level predicates.
    Bool,
    /// Bitvector ordering (Inc/Dec).
    Order,
}

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
        args: Vec<ConstraintExpr>,
    },
    /// NC_var: a boolean-kinded type variable (e.g. `'p` where `'p : Bool`).
    BoolVar(String),
    Unsupported,
}

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
                let texts: Vec<String> = parts.iter().map(|p| p.to_text()).collect();
                texts.join(" & ")
            }
            ConstraintExpr::Or(parts) => {
                let texts: Vec<String> = parts.iter().map(|p| p.to_text()).collect();
                texts.join(" | ")
            }
            ConstraintExpr::Not(inner) => format!("~({})", inner.to_text()),
            ConstraintExpr::App { name, args } => {
                let args_str: Vec<String> = args.iter().map(|a| a.to_text()).collect();
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
        // Constant (including negative)
        if let Ok(n) = trimmed.parse::<i64>() {
            return Some(NumericExpr::Const(n));
        }
        // Type variable: starts with '\'' followed by alnum/_
        if trimmed.starts_with('\'')
            && trimmed.len() > 1
            && trimmed[1..].chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            return Some(NumericExpr::Var(trimmed.to_string()));
        }
        // Plain symbol/identifier: all alnum or _
        if trimmed.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Some(NumericExpr::Symbol(trimmed.to_string()));
        }
        // Complex expressions (binary ops, if-then-else, etc.) — leave as Value
        None
    }

    /// Display as string for backward compatibility.
    pub fn to_string_repr(&self) -> String {
        match self {
            NumericExpr::Const(n) => n.to_string(),
            NumericExpr::Var(v) => v.clone(),
            NumericExpr::Symbol(s) => s.clone(),
            NumericExpr::Neg(e) => format!("-({})", e.to_string_repr()),
            NumericExpr::Add(a, b) => {
                format!("({} + {})", a.to_string_repr(), b.to_string_repr())
            }
            NumericExpr::Sub(a, b) => {
                format!("({} - {})", a.to_string_repr(), b.to_string_repr())
            }
            NumericExpr::Mul(a, b) => {
                format!("({} * {})", a.to_string_repr(), b.to_string_repr())
            }
            NumericExpr::Div(a, b) => {
                format!("({} / {})", a.to_string_repr(), b.to_string_repr())
            }
            NumericExpr::Mod(a, b) => {
                format!("({} % {})", a.to_string_repr(), b.to_string_repr())
            }
            NumericExpr::Exp(e) => format!("(2 ^ {})", e.to_string_repr()),
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
