use std::collections::HashSet;

use super::*;

struct NumericTextParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> NumericTextParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn parse(mut self) -> Option<NumericExpr> {
        let expr = self.parse_if_or_arith()?;
        self.skip_ws();
        (self.pos == self.input.len()).then_some(expr)
    }

    /// Parse `if cond then expr else expr` at the top level,
    /// or fall through to arithmetic.
    fn parse_if_or_arith(&mut self) -> Option<NumericExpr> {
        self.skip_ws();
        if self.consume_keyword("if") {
            let cond = self.consume_until_keyword("then")?;
            let then_expr = self.parse_if_or_arith()?;
            self.skip_ws();
            if !self.consume_keyword("else") {
                return None;
            }
            let else_expr = self.parse_if_or_arith()?;
            Some(NumericExpr::If {
                cond: Box::new(parse_constraint_text(cond.trim())),
                then_expr: Box::new(then_expr),
                else_expr: Box::new(else_expr),
            })
        } else {
            self.parse_add_sub()
        }
    }

    fn parse_add_sub(&mut self) -> Option<NumericExpr> {
        let mut expr = self.parse_mul_div()?;
        loop {
            self.skip_ws();
            let op = if self.consume_char('+') {
                Some(ArithmeticOp::Add)
            } else if self.consume_char('-') {
                Some(ArithmeticOp::Sub)
            } else {
                None
            };
            let Some(op) = op else {
                break;
            };
            let rhs = self.parse_mul_div()?;
            expr = match op {
                ArithmeticOp::Add => NumericExpr::Add(Box::new(expr), Box::new(rhs)),
                ArithmeticOp::Sub => NumericExpr::Sub(Box::new(expr), Box::new(rhs)),
                _ => unreachable!(),
            };
        }
        Some(expr)
    }

    fn parse_mul_div(&mut self) -> Option<NumericExpr> {
        let mut expr = self.parse_exp()?;
        loop {
            self.skip_ws();
            let op = if self.consume_char('*') {
                Some(ArithmeticOp::Mul)
            } else if self.consume_char('/') {
                Some(ArithmeticOp::Div)
            } else if self.consume_char('%') {
                Some(ArithmeticOp::Mod)
            } else {
                None
            };
            let Some(op) = op else {
                break;
            };
            let rhs = self.parse_exp()?;
            expr = match op {
                ArithmeticOp::Mul => NumericExpr::Mul(Box::new(expr), Box::new(rhs)),
                ArithmeticOp::Div => NumericExpr::Div(Box::new(expr), Box::new(rhs)),
                ArithmeticOp::Mod => NumericExpr::Mod(Box::new(expr), Box::new(rhs)),
                _ => unreachable!(),
            };
        }
        Some(expr)
    }

    /// Parse exponentiation (`2^n`). Right-associative, higher
    /// precedence than `*`/`/`. Sail convention: base is always 2.
    fn parse_exp(&mut self) -> Option<NumericExpr> {
        let base = self.parse_unary()?;
        self.skip_ws();
        if self.consume_char('^') {
            let exp = self.parse_exp()?; // right-associative
                                         // Sail convention: exponentiation is always 2^n.
                                         // If the base is literally `2`, emit NumericExpr::Exp(exp).
                                         // Otherwise, treat as symbol (unsupported base).
            if matches!(&base, NumericExpr::Const(2)) {
                Some(NumericExpr::Exp(Box::new(exp)))
            } else {
                None
            }
        } else {
            Some(base)
        }
    }

    fn parse_unary(&mut self) -> Option<NumericExpr> {
        self.skip_ws();
        if self.consume_char('-') {
            Some(match self.parse_unary()? {
                NumericExpr::Const(v) => NumericExpr::from_integer(-num_bigint::BigInt::from(v)),
                NumericExpr::BigConst(v) => NumericExpr::from_integer(-v),
                other => NumericExpr::Neg(Box::new(other)),
            })
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Option<NumericExpr> {
        self.skip_ws();
        if self.consume_char('(') {
            let expr = self.parse_if_or_arith()?;
            self.skip_ws();
            self.consume_char(')').then_some(expr)
        } else {
            let token = self.consume_token()?;
            if self.consume_char('(') {
                let mut args = Vec::new();
                if !self.consume_char(')') {
                    loop {
                        args.push(self.parse_if_or_arith()?);
                        if self.consume_char(')') {
                            break;
                        }
                        if !self.consume_char(',') {
                            return None;
                        }
                    }
                }
                return Some(NumericExpr::App { name: token, args });
            }
            NumericExpr::integer_literal(&token).or_else(|| {
                Some(if token.starts_with('\'') {
                    NumericExpr::Var(token)
                } else {
                    NumericExpr::Symbol(token)
                })
            })
        }
    }

    fn consume_token(&mut self) -> Option<String> {
        self.skip_ws();
        let rest = &self.input[self.pos..];
        let mut len = 0;
        for ch in rest.chars() {
            if ch.is_alphanumeric() || matches!(ch, '_' | '\'' | '#') {
                len += ch.len_utf8();
            } else {
                break;
            }
        }
        if len == 0 {
            None
        } else {
            let token = rest[..len].to_string();
            self.pos += len;
            Some(token)
        }
    }

    fn consume_char(&mut self, expected: char) -> bool {
        self.skip_ws();
        let mut chars = self.input[self.pos..].chars();
        match chars.next() {
            Some(ch) if ch == expected => {
                self.pos += ch.len_utf8();
                true
            }
            _ => false,
        }
    }

    fn skip_ws(&mut self) {
        while let Some(ch) = self.input[self.pos..].chars().next() {
            if ch.is_whitespace() {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
    }

    /// J3-5: Consume a keyword (e.g., "if", "then", "else") if it
    /// appears at the current position, followed by whitespace or end.
    fn consume_keyword(&mut self, kw: &str) -> bool {
        self.skip_ws();
        let rest = &self.input[self.pos..];
        if rest.starts_with(kw) {
            let after = self.pos + kw.len();
            // Ensure the keyword is followed by whitespace, end, or non-ident char
            if after >= self.input.len()
                || !self.input.as_bytes()[after].is_ascii_alphanumeric()
                    && self.input.as_bytes()[after] != b'_'
            {
                self.pos = after;
                return true;
            }
        }
        false
    }

    /// J3-5: Consume text until a keyword is found (at word boundary).
    /// Returns the consumed text (not including the keyword).
    fn consume_until_keyword(&mut self, kw: &str) -> Option<String> {
        self.skip_ws();
        let start = self.pos;
        let mut depth = 0usize;
        loop {
            if self.pos >= self.input.len() {
                return None;
            }
            let rest = &self.input[self.pos..];
            // Check if keyword appears at word boundary
            if depth == 0 && rest.starts_with(kw) {
                let after = self.pos + kw.len();
                let at_boundary = after >= self.input.len()
                    || !self.input.as_bytes()[after].is_ascii_alphanumeric()
                        && self.input.as_bytes()[after] != b'_';
                // Also check that the position is at a word boundary (preceded by non-ident)
                let preceded_ok = self.pos == start
                    || self.pos == 0
                    || !self.input.as_bytes()[self.pos - 1].is_ascii_alphanumeric()
                        && self.input.as_bytes()[self.pos - 1] != b'_';
                if at_boundary && preceded_ok {
                    let text = self.input[start..self.pos].to_string();
                    self.pos = after; // consume the keyword too
                    return Some(text);
                }
            }
            let ch = self.input[self.pos..].chars().next()?;
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth = depth.checked_sub(1)?,
                _ => {}
            }
            self.pos += ch.len_utf8();
        }
    }
}

pub(crate) fn parse_numeric_expr_text(text: &str) -> Option<NumericExpr> {
    NumericTextParser::new(text).parse()
}

pub(super) fn negate_constraint(expr: ConstraintExpr) -> ConstraintExpr {
    match expr {
        ConstraintExpr::Bool(value) => ConstraintExpr::Bool(!value),
        ConstraintExpr::Not(inner) => *inner,
        other => ConstraintExpr::Not(Box::new(other)),
    }
}

/// Source predicates use the same numeric AST as signatures. In particular,
/// a bare configuration name is a Symbol, while `'n` is a quantified Var;
/// separate parsers must not give one name two different SMT identities.
pub(super) fn constraint_expr_from_expr_text(text: &str) -> Option<ConstraintExpr> {
    let parsed = super::parse_constraint_text(text.trim());
    (!matches!(parsed, ConstraintExpr::Unsupported)).then_some(parsed)
}

fn numeric_expr_collect_vars(expr: &NumericExpr, out: &mut HashSet<String>) {
    match expr {
        NumericExpr::Var(name) => {
            out.insert(name.clone());
        }
        NumericExpr::Neg(inner) | NumericExpr::Exp(inner) => {
            numeric_expr_collect_vars(inner, out);
        }
        NumericExpr::Add(lhs, rhs)
        | NumericExpr::Sub(lhs, rhs)
        | NumericExpr::Mul(lhs, rhs)
        | NumericExpr::Div(lhs, rhs)
        | NumericExpr::Mod(lhs, rhs) => {
            numeric_expr_collect_vars(lhs, out);
            numeric_expr_collect_vars(rhs, out);
        }
        NumericExpr::Const(_) | NumericExpr::BigConst(_) | NumericExpr::Symbol(_) => {}
        NumericExpr::App { args, .. } => {
            for a in args {
                numeric_expr_collect_vars(a, out);
            }
        }
        NumericExpr::If { cond: _, then_expr, else_expr } => {
            // Note: we don't parse variable names out of the condition
            // string; callers that need full precision should use Z3.
            numeric_expr_collect_vars(then_expr, out);
            numeric_expr_collect_vars(else_expr, out);
        }
    }
}

fn numeric_symbol_assumption(
    name: &str,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
    visited: &mut HashSet<String>,
) -> Option<i64> {
    if !visited.insert(name.to_string()) {
        return None;
    }
    for assumption in assumptions {
        match assumption {
            ConstraintExpr::Compare {
                lhs: NumericExpr::Symbol(symbol) | NumericExpr::Var(symbol),
                op: CompareOp::Eq,
                rhs,
            } if symbol == name => {
                if let Some(value) =
                    eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)
                {
                    visited.remove(name);
                    return Some(value);
                }
            }
            ConstraintExpr::Compare {
                lhs,
                op: CompareOp::Eq,
                rhs: NumericExpr::Symbol(symbol) | NumericExpr::Var(symbol),
            } if symbol == name => {
                if let Some(value) =
                    eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)
                {
                    visited.remove(name);
                    return Some(value);
                }
            }
            ConstraintExpr::InSet {
                value: NumericExpr::Symbol(symbol) | NumericExpr::Var(symbol),
                items,
            } if symbol == name && items.len() == 1 => {
                if let Some(value) =
                    eval_numeric_expr_with_assumptions(&items[0], subst, assumptions, visited)
                {
                    visited.remove(name);
                    return Some(value);
                }
            }
            _ => {}
        }
    }
    visited.remove(name);
    None
}

fn eval_numeric_expr_with_assumptions(
    expr: &NumericExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
    visited: &mut HashSet<String>,
) -> Option<i64> {
    fn numeric_var_expr(name: &str, subst: &Subst) -> Option<NumericExpr> {
        let expr =
            subst.values.get(name).and_then(|value| parse_numeric_expr_text(value)).or_else(|| {
                subst.types.get(name).and_then(|ty| {
                    let text = ty.display_text();
                    parse_numeric_expr_text(&text)
                })
            });
        match expr {
            Some(NumericExpr::Var(bound)) if bound == name => None,
            other => other,
        }
    }

    match expr {
        NumericExpr::Const(value) => Some(*value),
        NumericExpr::BigConst(value) => num_traits::ToPrimitive::to_i64(value),
        NumericExpr::Var(name) => {
            if !visited.insert(name.clone()) {
                return None;
            }
            let result = numeric_var_expr(name, subst).and_then(|expr| {
                eval_numeric_expr_with_assumptions(&expr, subst, assumptions, visited)
            });
            visited.remove(name);
            result.or_else(|| numeric_symbol_assumption(name, subst, assumptions, visited))
        }
        NumericExpr::Symbol(name) => parse_int_literal(name)
            .or_else(|| numeric_symbol_assumption(name, subst, assumptions, visited)),
        NumericExpr::Neg(inner) => {
            eval_numeric_expr_with_assumptions(inner, subst, assumptions, visited)?.checked_neg()
        }
        NumericExpr::Add(lhs, rhs) => {
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                .checked_add(eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?)
        }
        NumericExpr::Sub(lhs, rhs) => {
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                .checked_sub(eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?)
        }
        NumericExpr::Mul(lhs, rhs) => {
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                .checked_mul(eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?)
        }
        NumericExpr::Div(lhs, rhs) => {
            let rhs = eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?;
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                .checked_div_euclid(rhs)
        }
        NumericExpr::Mod(lhs, rhs) => {
            let rhs = eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?;
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                .checked_rem_euclid(rhs)
        }
        NumericExpr::Exp(inner) => {
            // 2^n: evaluate inner, then compute 2^n via bit shift.
            // Mirrors Nexp::Exp::eval in nexp.rs:119-126.
            let n = eval_numeric_expr_with_assumptions(inner, subst, assumptions, visited)?;
            if (0..63).contains(&n) {
                Some(1i64 << n)
            } else {
                None // overflow guard
            }
        }
        NumericExpr::App { .. } => None, // opaque application — not evaluable
        NumericExpr::If { cond, then_expr, else_expr } => {
            match eval_condition(cond, subst, assumptions, visited) {
                Some(true) => {
                    eval_numeric_expr_with_assumptions(then_expr, subst, assumptions, visited)
                }
                Some(false) => {
                    eval_numeric_expr_with_assumptions(else_expr, subst, assumptions, visited)
                }
                None => {
                    let t =
                        eval_numeric_expr_with_assumptions(then_expr, subst, assumptions, visited)?;
                    let e =
                        eval_numeric_expr_with_assumptions(else_expr, subst, assumptions, visited)?;
                    (t == e).then_some(t)
                }
            }
        }
    }
}

/// Evaluate a conditional with the same cycle guard as its numeric operands.
/// This never invokes SMT or starts a second numeric evaluation context.
fn eval_condition(
    c: &ConstraintExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
    visited: &mut HashSet<String>,
) -> Option<bool> {
    if assumptions.iter().any(|a| a == c) {
        return Some(true);
    }
    if assumptions.iter().any(|a| matches!(a, ConstraintExpr::Not(inner) if inner.as_ref() == c)) {
        return Some(false);
    }
    match c {
        ConstraintExpr::Bool(b) => Some(*b),
        ConstraintExpr::BoolVar(name) => {
            let key = format!("bool:{name}");
            if !visited.insert(key.clone()) {
                return None;
            }
            let value = subst.values.get(name).map(|s| parse_constraint_text(s));
            let result = value.and_then(|c| eval_condition(&c, subst, assumptions, visited));
            visited.remove(&key);
            result
        }
        ConstraintExpr::Compare { lhs, op, rhs } => {
            let lhs = eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?;
            let rhs = eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?;
            Some(match op {
                CompareOp::Eq => lhs == rhs,
                CompareOp::Neq => lhs != rhs,
                CompareOp::Lt => lhs < rhs,
                CompareOp::Lte => lhs <= rhs,
                CompareOp::Gt => lhs > rhs,
                CompareOp::Gte => lhs >= rhs,
            })
        }
        ConstraintExpr::Not(inner) => {
            eval_condition(inner, subst, assumptions, visited).map(|b| !b)
        }
        ConstraintExpr::And(xs) | ConstraintExpr::Or(xs) => {
            let conjunction = matches!(c, ConstraintExpr::And(_));
            let mut unknown = false;
            for x in xs {
                match eval_condition(x, subst, assumptions, visited) {
                    Some(b) if b != conjunction => return Some(b),
                    None => unknown = true,
                    _ => {}
                }
            }
            (!unknown).then_some(conjunction)
        }
        ConstraintExpr::InSet { value, items } => {
            let value = eval_numeric_expr_with_assumptions(value, subst, assumptions, visited)?;
            let mut unknown = false;
            for item in items {
                match eval_numeric_expr_with_assumptions(item, subst, assumptions, visited) {
                    Some(n) if n == value => return Some(true),
                    None => unknown = true,
                    _ => {}
                }
            }
            (!unknown).then_some(false)
        }
        _ => None,
    }
}

pub(super) fn eval_numeric_expr(
    expr: &NumericExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> Option<i64> {
    eval_numeric_expr_with_assumptions(expr, subst, assumptions, &mut HashSet::new()).or_else(
        || {
            super::integer::eval(expr, subst, assumptions)
                .and_then(|n| num_traits::ToPrimitive::to_i64(&n))
        },
    )
}

fn linear_numeric_expr(
    expr: &NumericExpr,
    target: &str,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> Option<(i64, i64)> {
    linear_numeric_expr_inner(expr, target, subst, assumptions, 0)
}
fn linear_numeric_expr_inner(
    expr: &NumericExpr,
    target: &str,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
    depth: usize,
) -> Option<(i64, i64)> {
    if depth > 64 {
        return None;
    }
    match expr {
        NumericExpr::Const(value) => Some((0, *value)),
        NumericExpr::BigConst(value) => num_traits::ToPrimitive::to_i64(value).map(|n| (0, n)),
        NumericExpr::Var(name) if name == target => Some((1, 0)),
        NumericExpr::Var(name) => subst
            .values
            .get(name)
            .and_then(|value| parse_numeric_expr_text(value))
            .or_else(|| {
                subst.types.get(name).and_then(|ty| {
                    let text = ty.display_text();
                    parse_numeric_expr_text(&text)
                })
            })
            .and_then(|expr| {
                linear_numeric_expr_inner(&expr, target, subst, assumptions, depth + 1)
            }),
        NumericExpr::Symbol(name) => {
            eval_numeric_expr(&NumericExpr::Symbol(name.clone()), subst, assumptions)
                .map(|value| (0, value))
        }
        NumericExpr::Neg(inner) => {
            let (coeff, constant) =
                linear_numeric_expr_inner(inner, target, subst, assumptions, depth + 1)?;
            Some((coeff.checked_neg()?, constant.checked_neg()?))
        }
        NumericExpr::Add(lhs, rhs) => {
            let (left_coeff, left_const) =
                linear_numeric_expr_inner(lhs, target, subst, assumptions, depth + 1)?;
            let (right_coeff, right_const) =
                linear_numeric_expr_inner(rhs, target, subst, assumptions, depth + 1)?;
            Some((left_coeff.checked_add(right_coeff)?, left_const.checked_add(right_const)?))
        }
        NumericExpr::Sub(lhs, rhs) => {
            let (left_coeff, left_const) =
                linear_numeric_expr_inner(lhs, target, subst, assumptions, depth + 1)?;
            let (right_coeff, right_const) =
                linear_numeric_expr_inner(rhs, target, subst, assumptions, depth + 1)?;
            Some((left_coeff.checked_sub(right_coeff)?, left_const.checked_sub(right_const)?))
        }
        NumericExpr::Mul(lhs, rhs) => match (
            linear_numeric_expr_inner(lhs, target, subst, assumptions, depth + 1),
            linear_numeric_expr_inner(rhs, target, subst, assumptions, depth + 1),
        ) {
            (Some((0, left_const)), Some((right_coeff, right_const))) => {
                Some((left_const.checked_mul(right_coeff)?, left_const.checked_mul(right_const)?))
            }
            (Some((left_coeff, left_const)), Some((0, right_const))) => {
                Some((left_coeff.checked_mul(right_const)?, left_const.checked_mul(right_const)?))
            }
            _ => None,
        },
        NumericExpr::Div(lhs, rhs) => {
            let (left_coeff, left_const) =
                linear_numeric_expr_inner(lhs, target, subst, assumptions, depth + 1)?;
            let rhs = eval_numeric_expr(rhs, subst, assumptions)?;
            if left_coeff.checked_rem(rhs)? != 0 || left_const.checked_rem(rhs)? != 0 {
                return None;
            }
            Some((left_coeff.checked_div(rhs)?, left_const.checked_div(rhs)?))
        }
        NumericExpr::Mod(_, _)
        | NumericExpr::Exp(_)
        | NumericExpr::App { .. }
        | NumericExpr::If { .. } => None, // not linear
    }
}

pub(super) fn app_text(name: &str, args: &[TyArg]) -> String {
    if args.is_empty() {
        name.to_string()
    } else {
        format!(
            "{}({})",
            name,
            args.iter()
                .map(|arg| match arg {
                    TyArg::Type(ty) => ty.display_text(),
                    TyArg::Nexp(n) => n.to_string_repr(),
                    TyArg::Bool(c) => c.to_text(),
                    TyArg::Order(value) | TyArg::Value(value) => value.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

pub(super) fn normalized_value_text(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

pub(super) fn ty_contains_var(ty: &Ty, name: &str) -> bool {
    match ty.kind() {
        TyKind::Error
        | TyKind::Infer(crate::ty::InferTy(_))
        | TyKind::Scalar(_)
        | TyKind::Named(_, _) => false,
        TyKind::Param(var) => var == name,
        TyKind::Tuple(items) => items.iter().any(|item| ty_contains_var(item, name)),
        TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
            params.iter().any(|param| ty_contains_var(param, name)) || ty_contains_var(ret, name)
        }
        TyKind::App { args, .. } => args.iter().any(|arg| match arg {
            TyArg::Type(ty) => ty_contains_var(ty, name),
            TyArg::Nexp(_) | TyArg::Bool(_) | TyArg::Order(_) | TyArg::Value(_) => false,
        }),
        TyKind::Exist { vars, inner, .. } => {
            // If the name is bound by the existential, it's not free
            if vars.iter().any(|v| v == name) {
                return false;
            }
            ty_contains_var(inner, name)
        }
        TyKind::Bidir { lhs, rhs } => ty_contains_var(lhs, name) || ty_contains_var(rhs, name),
        TyKind::Abstract { .. } => false, // abstract types have no free variables
    }
}

/// Equality can bind inference parameters, but a failed/unknown attempt never
/// leaks partial substitutions. Callers needing recovery retain the goal.
pub(super) fn numeric_equality(
    expected: &NumericExpr,
    actual: &NumericExpr,
    subst: &mut Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    let status = numeric_equality_without_binding(expected, actual, subst, assumptions);
    if status != ConstraintStatus::Unknown {
        return status;
    }
    if matches!(expected, NumericExpr::If { .. }) || matches!(actual, NumericExpr::If { .. }) {
        return status;
    }
    let mut candidate = subst.clone();
    if bind_numeric_equality(expected, actual, &mut candidate) {
        *subst = candidate;
        ConstraintStatus::Satisfied
    } else {
        ConstraintStatus::Unknown
    }
}

/// Equality of rigid expressions must never solve a free variable by binding it.
pub(super) fn numeric_equality_without_binding(
    expected: &NumericExpr,
    actual: &NumericExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> ConstraintStatus {
    if super::integer::contradictory(assumptions, subst) {
        return ConstraintStatus::Satisfied;
    }
    if expected == actual {
        return ConstraintStatus::Satisfied;
    }
    for (conditional, other) in [(expected, actual), (actual, expected)] {
        if let NumericExpr::If { cond, then_expr, else_expr } = conditional {
            if let Some(value) = eval_condition(cond, subst, assumptions, &mut HashSet::new()) {
                return numeric_equality_without_binding(
                    if value { then_expr } else { else_expr },
                    other,
                    subst,
                    assumptions,
                );
            }
            let a = numeric_equality_without_binding(then_expr, other, subst, assumptions);
            let b = numeric_equality_without_binding(else_expr, other, subst, assumptions);
            return if a == b { a } else { ConstraintStatus::Unknown };
        }
    }
    for (expression, constant) in [(expected, actual), (actual, expected)] {
        let expression = subst_numeric_expr(expression, subst);
        if let Some(value) = eval_numeric_expr(constant, subst, assumptions) {
            if let Some(Err(())) = super::nexp_simplify::solve_linear(&expression, value) {
                return ConstraintStatus::Failed;
            }
        }
    }
    let e = subst_numeric_expr(expected, subst);
    let a = subst_numeric_expr(actual, subst);
    if let Some(delta) = super::integer::difference(&e, &a) {
        return if num_traits::Zero::is_zero(&delta) {
            ConstraintStatus::Satisfied
        } else {
            ConstraintStatus::Failed
        };
    }
    if matches!(super::integer::witness(&e, &a), Some(Err(()))) {
        return ConstraintStatus::Failed;
    }
    let goal =
        ConstraintExpr::Compare { lhs: expected.clone(), op: CompareOp::Eq, rhs: actual.clone() };
    super::solver::solve_without_smt(&goal, subst, assumptions)
}

pub(super) fn unify_numeric_expr(
    expected: &NumericExpr,
    actual: &NumericExpr,
    subst: &mut Subst,
) -> bool {
    numeric_equality(expected, actual, subst, &[]) == ConstraintStatus::Satisfied
}

fn bind_numeric_equality(expected: &NumericExpr, actual: &NumericExpr, subst: &mut Subst) -> bool {
    if let (Some(expected), Some(actual)) =
        (eval_numeric_expr(expected, subst, &[]), eval_numeric_expr(actual, subst, &[]))
    {
        return expected == actual;
    }

    // Polynomial-form equivalence: convert each side to a canonical sum of
    // terms `coefficient * variable_product + constant`. Two expressions
    // unify if their canonical polynomials are identical after substitution.
    // This handles cases like:
    //   `('m - i - 1) - ('m - i - 8) + 1` == 8
    //   `(i + 7) - i + 1` == 8
    // which require constant-folding across multiple variables.
    let expected_subst = subst_numeric_expr(expected, subst);
    let actual_subst = subst_numeric_expr(actual, subst);
    if let (Some(expected_poly), Some(actual_poly)) =
        (polynomial_from_numeric_expr(&expected_subst), polynomial_from_numeric_expr(&actual_subst))
    {
        if expected_poly == actual_poly {
            return true;
        }
    }

    if let NumericExpr::Var(name) = expected {
        if !name.starts_with("'_sk")
            && !subst.values.contains_key(name)
            && !subst.types.contains_key(name)
        {
            let mut free = HashSet::new();
            numeric_expr_collect_vars(actual, &mut free);
            if !free.contains(name) {
                subst.values.insert(name.clone(), actual.to_string_repr());
                return true;
            }
        }
    }

    // Try linear solving from expected side
    let mut unresolved = HashSet::new();
    numeric_expr_collect_vars(expected, &mut unresolved);
    unresolved.retain(|name| !subst.values.contains_key(name) && !subst.types.contains_key(name));
    if unresolved.len() == 1 {
        let variable = unresolved.into_iter().next().expect("single unresolved variable");
        if let Some(actual_value) = eval_numeric_expr(actual, subst, &[]) {
            if let Some((coeff, constant)) = linear_numeric_expr(expected, &variable, subst, &[]) {
                if let Some(delta) = actual_value.checked_sub(constant) {
                    if coeff != 0 && delta.checked_rem(coeff) == Some(0) {
                        if let Some(value) = delta.checked_div(coeff) {
                            subst.values.insert(variable, value.to_string());
                            return true;
                        }
                    }
                }
            }
        }
    }

    if let Some(Ok((variable, value))) = super::integer::witness(&expected_subst, &actual_subst) {
        let mut free = HashSet::new();
        numeric_expr_collect_vars(expected, &mut free);
        if free.contains(&variable)
            && !variable.starts_with("'_sk")
            && !subst.values.contains_key(&variable)
            && !subst.types.contains_key(&variable)
        {
            subst.values.insert(variable, value.to_string());
            return true;
        }
    }

    // Branch-dependent equations cannot bind a variable unconditionally.
    // Their equality is decided by the contextual solver or retained as Unknown.
    if matches!(expected, NumericExpr::If { .. }) || matches!(actual, NumericExpr::If { .. }) {
        return expected_subst == actual_subst;
    }

    // Z3 fallback for non-polynomial expressions (div, mod, exp).
    //
    // When polynomial normalization fails (e.g., `2^n - 1` vs `2^n - 1`),
    // delegate to Z3 for numeric equality checking.
    //
    // Only attempt Z3 when the expressions contain operations that
    // polynomial form can't represent (div/mod/exp with variables).
    #[cfg(feature = "z3-solver")]
    {
        let expected_subst = subst_numeric_expr(expected, subst);
        let actual_subst = subst_numeric_expr(actual, subst);
        if contains_non_polynomial(&expected_subst) || contains_non_polynomial(&actual_subst) {
            let constraint = ConstraintExpr::Compare {
                lhs: expected_subst,
                op: CompareOp::Eq,
                rhs: actual_subst,
            };
            let status = z3_solver::try_solve(&constraint, subst, &[]);
            if matches!(status, ConstraintStatus::Satisfied) {
                return true;
            }
        }
    }

    false
}

/// Check if a numeric expression contains operations that
/// can't be represented in polynomial form (division, modulo,
/// exponentiation with non-constant exponent).
#[cfg(any(feature = "z3-solver", test))]
pub(super) fn contains_non_polynomial(expr: &NumericExpr) -> bool {
    match expr {
        NumericExpr::Const(_)
        | NumericExpr::BigConst(_)
        | NumericExpr::Var(_)
        | NumericExpr::Symbol(_) => false,
        NumericExpr::Neg(inner) => contains_non_polynomial(inner),
        NumericExpr::Add(lhs, rhs) | NumericExpr::Sub(lhs, rhs) | NumericExpr::Mul(lhs, rhs) => {
            contains_non_polynomial(lhs) || contains_non_polynomial(rhs)
        }
        // Division and modulo are never polynomial
        NumericExpr::Div(_, _) | NumericExpr::Mod(_, _) => true,
        // Exponentiation is non-polynomial unless the exponent is constant
        NumericExpr::Exp(inner) => !matches!(inner.as_ref(), NumericExpr::Const(_)),
        // App is opaque — treat as non-polynomial
        NumericExpr::App { .. } => true,
        // If-then-else is never polynomial
        NumericExpr::If { .. } => true,
    }
}

pub static DEF_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
pub static DEF_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
