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
                // Non-2 base: fall back to symbol
                Some(NumericExpr::Symbol(format!(
                    "{}^{:?}",
                    match &base {
                        NumericExpr::Const(n) => n.to_string(),
                        _ => "?".to_string(),
                    },
                    exp
                )))
            }
        } else {
            Some(base)
        }
    }

    fn parse_unary(&mut self) -> Option<NumericExpr> {
        self.skip_ws();
        if self.consume_char('-') {
            Some(NumericExpr::Neg(Box::new(self.parse_unary()?)))
        } else {
            self.parse_primary()
        }
    }

    fn parse_primary(&mut self) -> Option<NumericExpr> {
        self.skip_ws();
        if self.consume_char('(') {
            let expr = self.parse_add_sub()?;
            self.skip_ws();
            self.consume_char(')').then_some(expr)
        } else {
            let token = self.consume_token()?;
            parse_int_literal(&token).map(NumericExpr::Const).or_else(|| {
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
            if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '\'' | '#') {
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
        loop {
            if self.pos >= self.input.len() {
                return None;
            }
            let rest = &self.input[self.pos..];
            // Check if keyword appears at word boundary
            if rest.starts_with(kw) {
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
            self.pos += 1;
        }
    }
}

pub(super) fn parse_numeric_expr_text(text: &str) -> Option<NumericExpr> {
    NumericTextParser::new(text).parse()
}

pub(super) fn negate_constraint(expr: ConstraintExpr) -> ConstraintExpr {
    match expr {
        ConstraintExpr::Bool(value) => ConstraintExpr::Bool(!value),
        ConstraintExpr::Not(inner) => *inner,
        other => ConstraintExpr::Not(Box::new(other)),
    }
}

/// Parse a constraint from expression source text (e.g. `x == 8`).
/// Handles the common comparison patterns without requiring a full
/// core_ast parse.
pub(super) fn constraint_expr_from_expr_text(text: &str) -> Option<ConstraintExpr> {
    let text = text.trim();
    // Handle `a == b`, `a != b`, `a < b`, etc.
    for (op_str, op) in &[
        ("==", CompareOp::Eq),
        ("!=", CompareOp::Neq),
        ("<=", CompareOp::Lte),
        (">=", CompareOp::Gte),
        ("<", CompareOp::Lt),
        (">", CompareOp::Gt),
    ] {
        if let Some(pos) = text.find(op_str) {
            let lhs_text = text[..pos].trim();
            let rhs_text = text[pos + op_str.len()..].trim();
            let lhs = numeric_expr_from_text(lhs_text)?;
            let rhs = numeric_expr_from_text(rhs_text)?;
            return Some(ConstraintExpr::Compare { lhs, op: *op, rhs });
        }
    }
    // Handle `x in {a, b, c}` — set membership constraint
    // Handle `x in {a, b, c}` — set membership constraint
    if let Some(in_pos) = text.find(" in ") {
        let var_text = text[..in_pos].trim();
        let set_text = text[in_pos + 4..].trim();
        if let Some(value) = numeric_expr_from_text(var_text) {
            let inner = set_text.trim_start_matches('{').trim_end_matches('}');
            let items: Vec<NumericExpr> =
                inner.split(',').filter_map(|s| numeric_expr_from_text(s.trim())).collect();
            if !items.is_empty() {
                return Some(ConstraintExpr::InSet { value, items });
            }
        }
    }
    // Handle `true` / `false`
    if text == "true" {
        return Some(ConstraintExpr::Bool(true));
    }
    if text == "false" {
        return Some(ConstraintExpr::Bool(false));
    }
    None
}

fn numeric_expr_from_text(text: &str) -> Option<NumericExpr> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // Try as integer
    if let Ok(n) = text.parse::<i64>() {
        return Some(NumericExpr::Const(n));
    }
    // Identifier (variable)
    if text.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '\'') {
        return Some(NumericExpr::Var(text.to_string()));
    }
    None
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
        NumericExpr::Const(_) | NumericExpr::Symbol(_) => {}
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
                lhs: NumericExpr::Symbol(symbol),
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
                rhs: NumericExpr::Symbol(symbol),
            } if symbol == name => {
                if let Some(value) =
                    eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)
                {
                    visited.remove(name);
                    return Some(value);
                }
            }
            ConstraintExpr::InSet { value: NumericExpr::Symbol(symbol), items }
                if symbol == name && items.len() == 1 =>
            {
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
        NumericExpr::Var(name) => numeric_var_expr(name, subst).and_then(|expr| {
            eval_numeric_expr_with_assumptions(&expr, subst, assumptions, visited)
        }),
        NumericExpr::Symbol(name) => parse_int_literal(name)
            .or_else(|| numeric_symbol_assumption(name, subst, assumptions, visited)),
        NumericExpr::Neg(inner) => {
            Some(-eval_numeric_expr_with_assumptions(inner, subst, assumptions, visited)?)
        }
        NumericExpr::Add(lhs, rhs) => Some(
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                + eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?,
        ),
        NumericExpr::Sub(lhs, rhs) => Some(
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                - eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?,
        ),
        NumericExpr::Mul(lhs, rhs) => Some(
            eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)?
                * eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?,
        ),
        NumericExpr::Div(lhs, rhs) => {
            let rhs = eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?;
            (rhs != 0).then_some(
                eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)? / rhs,
            )
        }
        NumericExpr::Mod(lhs, rhs) => {
            let rhs = eval_numeric_expr_with_assumptions(rhs, subst, assumptions, visited)?;
            (rhs != 0).then_some(
                eval_numeric_expr_with_assumptions(lhs, subst, assumptions, visited)? % rhs,
            )
        }
        NumericExpr::Exp(inner) => {
            // 2^n: evaluate inner, then compute 2^n via bit shift.
            // Mirrors Nexp::Exp::eval in nexp.rs:119-126.
            let n = eval_numeric_expr_with_assumptions(inner, subst, assumptions, visited)?;
            if n >= 0 && n <= 63 {
                Some(1i64 << n)
            } else {
                None // overflow guard
            }
        }
        NumericExpr::App { .. } => None, // opaque application — not evaluable
        NumericExpr::If { cond: _, then_expr, else_expr } => {
            // We cannot reliably evaluate the string condition here.
            // If both branches yield the same value, return it; otherwise None.
            let t = eval_numeric_expr_with_assumptions(then_expr, subst, assumptions, visited)?;
            let e = eval_numeric_expr_with_assumptions(else_expr, subst, assumptions, visited)?;
            if t == e {
                Some(t)
            } else {
                None
            }
        }
    }
}

pub(super) fn eval_numeric_expr(
    expr: &NumericExpr,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> Option<i64> {
    eval_numeric_expr_with_assumptions(expr, subst, assumptions, &mut HashSet::new())
}

fn linear_numeric_expr(
    expr: &NumericExpr,
    target: &str,
    subst: &Subst,
    assumptions: &[ConstraintExpr],
) -> Option<(i64, i64)> {
    match expr {
        NumericExpr::Const(value) => Some((0, *value)),
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
            .and_then(|expr| linear_numeric_expr(&expr, target, subst, assumptions)),
        NumericExpr::Symbol(name) => {
            eval_numeric_expr(&NumericExpr::Symbol(name.clone()), subst, assumptions)
                .map(|value| (0, value))
        }
        NumericExpr::Neg(inner) => {
            let (coeff, constant) = linear_numeric_expr(inner, target, subst, assumptions)?;
            Some((-coeff, -constant))
        }
        NumericExpr::Add(lhs, rhs) => {
            let (left_coeff, left_const) = linear_numeric_expr(lhs, target, subst, assumptions)?;
            let (right_coeff, right_const) = linear_numeric_expr(rhs, target, subst, assumptions)?;
            Some((left_coeff + right_coeff, left_const + right_const))
        }
        NumericExpr::Sub(lhs, rhs) => {
            let (left_coeff, left_const) = linear_numeric_expr(lhs, target, subst, assumptions)?;
            let (right_coeff, right_const) = linear_numeric_expr(rhs, target, subst, assumptions)?;
            Some((left_coeff - right_coeff, left_const - right_const))
        }
        NumericExpr::Mul(lhs, rhs) => match (
            linear_numeric_expr(lhs, target, subst, assumptions),
            linear_numeric_expr(rhs, target, subst, assumptions),
        ) {
            (Some((0, left_const)), Some((right_coeff, right_const))) => {
                Some((left_const * right_coeff, left_const * right_const))
            }
            (Some((left_coeff, left_const)), Some((0, right_const))) => {
                Some((left_coeff * right_const, left_const * right_const))
            }
            _ => None,
        },
        NumericExpr::Div(lhs, rhs) => {
            let (left_coeff, left_const) = linear_numeric_expr(lhs, target, subst, assumptions)?;
            let rhs = eval_numeric_expr(rhs, subst, assumptions)?;
            (rhs != 0 && left_coeff % rhs == 0 && left_const % rhs == 0)
                .then_some((left_coeff / rhs, left_const / rhs))
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
                    TyArg::Value(value) => value.clone(),
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
        | TyKind::Adt(_, _) => false,
        TyKind::Param(var) => var == name,
        TyKind::Tuple(items) => items.iter().any(|item| ty_contains_var(item, name)),
        TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
            params.iter().any(|param| ty_contains_var(param, name)) || ty_contains_var(ret, name)
        }
        TyKind::App { args, .. } => args.iter().any(|arg| match arg {
            TyArg::Type(ty) => ty_contains_var(ty, name),
            TyArg::Nexp(_) | TyArg::Value(_) => false,
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

pub(super) fn unify_numeric_expr(
    expected: &NumericExpr,
    actual: &NumericExpr,
    subst: &mut Subst,
) -> bool {
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

    // Try linear solving from expected side
    let mut unresolved = HashSet::new();
    numeric_expr_collect_vars(expected, &mut unresolved);
    unresolved.retain(|name| !subst.values.contains_key(name) && !subst.types.contains_key(name));
    if unresolved.len() == 1 {
        let variable = unresolved.into_iter().next().expect("single unresolved variable");
        if let Some(actual_value) = eval_numeric_expr(actual, subst, &[]) {
            if let Some((coeff, constant)) = linear_numeric_expr(expected, &variable, subst, &[]) {
                let delta = actual_value - constant;
                if coeff != 0 && delta % coeff == 0 {
                    subst.values.insert(variable, (delta / coeff).to_string());
                    return true;
                }
            }
        }
    }

    // Symmetric: try linear solving from actual side
    let mut unresolved_actual = HashSet::new();
    numeric_expr_collect_vars(actual, &mut unresolved_actual);
    unresolved_actual
        .retain(|name| !subst.values.contains_key(name) && !subst.types.contains_key(name));
    if unresolved_actual.len() == 1 {
        let variable = unresolved_actual.into_iter().next().expect("single unresolved variable");
        if let Some(expected_value) = eval_numeric_expr(expected, subst, &[]) {
            if let Some((coeff, constant)) = linear_numeric_expr(actual, &variable, subst, &[]) {
                let delta = expected_value - constant;
                if coeff != 0 && delta % coeff == 0 {
                    subst.values.insert(variable, (delta / coeff).to_string());
                    return true;
                }
            }
        }
    }

    // Structural unification for If-then-else: conditions must match
    // textually and both branches must unify.
    if let (
        NumericExpr::If { cond: c1, then_expr: t1, else_expr: e1 },
        NumericExpr::If { cond: c2, then_expr: t2, else_expr: e2 },
    ) = (expected, actual)
    {
        // Compare conditions structurally (via PartialEq) or textually as fallback.
        if c1 == c2 || normalized_value_text(&c1.to_text()) == normalized_value_text(&c2.to_text()) {
            return unify_numeric_expr(t1, t2, subst) && unify_numeric_expr(e1, e2, subst);
        }
    }
    // If one side is If and the other is not, be permissive:
    // the LSP can't evaluate the condition without full constraint context.
    if matches!(expected, NumericExpr::If { .. }) || matches!(actual, NumericExpr::If { .. }) {
        return true;
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
pub(super) fn contains_non_polynomial(expr: &NumericExpr) -> bool {
    match expr {
        NumericExpr::Const(_) | NumericExpr::Var(_) | NumericExpr::Symbol(_) => false,
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
