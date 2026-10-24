//! Arbitrary-precision arithmetic for the shared numeric AST. Inline i64
//! evaluation remains a fast path, never the semantic integer domain.
use super::{CompareOp, ConstraintExpr as C, NumericExpr as N, Subst};
use num_bigint::BigInt;
use num_traits::{One, Signed, ToPrimitive, Zero};
use std::collections::{HashMap, HashSet};
const MAX_BITS: u64 = 1 << 20;

pub(super) fn eval(expr: &N, subst: &Subst, assumptions: &[C]) -> Option<BigInt> {
    Evaluator {
        subst,
        assumptions,
        visiting: HashSet::new(),
        fuel: 4096,
        choices: &[],
        split: None,
        branching: false,
    }
    .number(expr)
}
pub(super) fn condition(c: &C, subst: &Subst, assumptions: &[C]) -> Option<bool> {
    Evaluator {
        subst,
        assumptions,
        visiting: HashSet::new(),
        fuel: 4096,
        choices: &[],
        split: None,
        branching: false,
    }
    .boolean(c)
}
struct Evaluator<'a> {
    subst: &'a Subst,
    assumptions: &'a [C],
    visiting: HashSet<String>,
    fuel: usize,
    choices: &'a [(C, bool)],
    split: Option<C>,
    branching: bool,
}

/// Explore only unresolved numeric conditionals, sharing choices between
/// repeated predicates. All paths must give the same answer. Work is bounded
/// across the entire search, and no SMT query or guessed branch is involved.
pub(super) fn conditional(c: &C, subst: &Subst, assumptions: &[C]) -> Option<bool> {
    fn visit(
        c: &C,
        subst: &Subst,
        assumptions: &[C],
        choices: &mut Vec<(C, bool)>,
        fuel: &mut usize,
        states: &mut usize,
    ) -> Option<bool> {
        *states = states.checked_sub(1)?;
        let mut eval = Evaluator {
            subst,
            assumptions,
            visiting: HashSet::new(),
            fuel: *fuel,
            choices,
            split: None,
            branching: true,
        };
        let result = eval.boolean(c);
        *fuel = eval.fuel;
        if result.is_some() {
            return result;
        }
        let split = eval.split?;
        if choices.len() >= 4 {
            return None;
        }
        choices.push((split, true));
        let yes = visit(c, subst, assumptions, choices, fuel, states);
        choices.last_mut().unwrap().1 = false;
        let no = visit(c, subst, assumptions, choices, fuel, states);
        choices.pop();
        match (yes, no) {
            (Some(a), Some(b)) if a == b => Some(a),
            _ => None,
        }
    }
    visit(c, subst, assumptions, &mut Vec::new(), &mut 4096, &mut 31)
}

impl Evaluator<'_> {
    fn number(&mut self, e: &N) -> Option<BigInt> {
        self.fuel = self.fuel.checked_sub(1)?;
        let value = match e {
            N::Const(n) => (*n).into(),
            N::BigConst(n) => n.clone(),
            N::Var(name) | N::Symbol(name) => {
                if !self.visiting.insert(name.clone()) {
                    return None;
                }
                let expression =
                    self.subst.values.get(name).and_then(|s| N::parse(s)).or_else(|| {
                        self.subst.types.get(name).and_then(|t| N::parse(&t.display_text()))
                    });
                let result = expression
                    .as_ref()
                    .and_then(|e| self.number(e))
                    .or_else(|| self.from_facts(name, self.assumptions));
                self.visiting.remove(name);
                return result;
            }
            N::Neg(n) => -self.number(n)?,
            N::Add(a, b) => self.number(a)? + self.number(b)?,
            N::Sub(a, b) => self.number(a)? - self.number(b)?,
            N::Mul(a, b) => self.number(a)? * self.number(b)?,
            N::Div(a, b) | N::Mod(a, b) => {
                let a = self.number(a)?;
                let b = self.number(b)?;
                if b.is_zero() {
                    return None;
                }
                let mut r = &a % &b;
                if r.is_negative() {
                    r += b.abs();
                }
                if matches!(e, N::Mod(..)) {
                    r
                } else {
                    (a - r) / b
                }
            }
            N::Exp(n) => {
                let n = self.number(n)?.to_u64()?;
                if n >= MAX_BITS {
                    return None;
                }
                BigInt::one() << (n as usize)
            }
            N::If { cond, then_expr, else_expr } => match self.boolean(cond) {
                Some(true) => self.number(then_expr)?,
                Some(false) => self.number(else_expr)?,
                None => {
                    if self.branching && self.split.is_none() {
                        self.split = Some((**cond).clone());
                    }
                    let a = self.number(then_expr)?;
                    let b = self.number(else_expr)?;
                    if a != b {
                        return None;
                    }
                    a
                }
            },
            N::App { name, args } => match (name.as_str(), args.as_slice()) {
                ("div", [a, b]) => {
                    self.number(&N::Div(Box::new(a.clone()), Box::new(b.clone())))?
                }
                ("mod", [a, b]) => {
                    self.number(&N::Mod(Box::new(a.clone()), Box::new(b.clone())))?
                }
                ("abs", [a]) => self.number(a)?.abs(),
                _ => return None,
            },
        };
        (value.bits() <= MAX_BITS).then_some(value)
    }
    fn from_facts(&mut self, name: &str, facts: &[C]) -> Option<BigInt> {
        for fact in facts {
            let value = match fact {
                C::And(xs) => self.from_facts(name, xs),
                C::Compare { lhs: N::Var(n) | N::Symbol(n), op: CompareOp::Eq, rhs }
                    if n == name =>
                {
                    self.number(rhs)
                }
                C::Compare { rhs: N::Var(n) | N::Symbol(n), op: CompareOp::Eq, lhs }
                    if n == name =>
                {
                    self.number(lhs)
                }
                _ => None,
            };
            if value.is_some() {
                return value;
            }
        }
        None
    }
    /// A bounded over-approximation of the concrete values of conditionals.
    /// Branch correlations may be forgotten, but a proof must hold for every
    /// combination. Never select a convenient branch or a range member.
    fn values(&mut self, expr: &N) -> Option<Vec<BigInt>> {
        self.fuel = self.fuel.checked_sub(1)?;
        if let Some(value) = self.number(expr) {
            return Some(vec![value]);
        }
        let mut values = match expr {
            N::Var(name) | N::Symbol(name) => {
                if !self.visiting.insert(name.clone()) {
                    return None;
                }
                let bound = self.subst.values.get(name).and_then(|s| N::parse(s));
                let values = bound.as_ref().and_then(|e| self.values(e));
                self.visiting.remove(name);
                return values;
            }
            N::If { cond, then_expr, else_expr } => match self.boolean(cond) {
                Some(true) => return self.values(then_expr),
                Some(false) => return self.values(else_expr),
                None => {
                    let mut values = self.values(then_expr)?;
                    values.extend(self.values(else_expr)?);
                    values
                }
            },
            N::Neg(x) => self.values(x)?.into_iter().map(|x| -x).collect(),
            N::Add(a, b) | N::Sub(a, b) | N::Mul(a, b) => {
                let a = self.values(a)?;
                let b = self.values(b)?;
                if a.len().saturating_mul(b.len()) > 16 {
                    return None;
                }
                let mut values = Vec::new();
                for a in &a {
                    for b in &b {
                        let value = match expr {
                            N::Add(..) => a + b,
                            N::Sub(..) => a - b,
                            _ => a * b,
                        };
                        if value.bits() > MAX_BITS {
                            return None;
                        }
                        values.push(value);
                    }
                }
                values
            }
            _ => return None,
        };
        values.sort();
        values.dedup();
        (values.len() <= 16).then_some(values)
    }

    fn boolean(&mut self, c: &C) -> Option<bool> {
        self.fuel = self.fuel.checked_sub(1)?;
        if let Some((_, value)) = self.choices.iter().find(|(condition, _)| condition == c) {
            return Some(*value);
        }
        if self.assumptions.contains(c) {
            return Some(true);
        }
        match c {
            C::Bool(b) => Some(*b),
            C::BoolVar(n) => {
                if !self.visiting.insert(n.clone()) {
                    return None;
                }
                let value = self.subst.values.get(n).map(|v| super::parse_constraint_text(v));
                let result = value.as_ref().and_then(|v| self.boolean(v));
                self.visiting.remove(n);
                result
            }
            C::Compare { lhs, op, rhs } => {
                let a = self.values(lhs)?;
                let b = self.values(rhs)?;
                let first = compare(a.first()?, *op, b.first()?);
                a.iter().all(|a| b.iter().all(|b| compare(a, *op, b) == first)).then_some(first)
            }
            C::InSet { value, items } => {
                let values = self.values(value)?;
                let mut result = None;
                for value in values {
                    let mut found = false;
                    let mut unknown = false;
                    for item in items {
                        match self.number(item) {
                            Some(n) if n == value => {
                                found = true;
                                break;
                            }
                            None => unknown = true,
                            _ => {}
                        }
                    }
                    if !found && unknown {
                        return None;
                    }
                    if result.is_some_and(|r| r != found) {
                        return None;
                    }
                    result = Some(found);
                }
                result
            }
            C::Not(x) => self.boolean(x).map(|b| !b),
            C::And(xs) | C::Or(xs) => {
                let is_and = matches!(c, C::And(_));
                let mut unknown = false;
                for x in xs {
                    match self.boolean(x) {
                        Some(b) if b != is_and => return Some(b),
                        None => unknown = true,
                        _ => {}
                    }
                }
                (!unknown).then_some(is_and)
            }
            _ => None,
        }
    }
}
pub(super) fn compare(a: &BigInt, op: CompareOp, b: &BigInt) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Neq => a != b,
        CompareOp::Lt => a < b,
        CompareOp::Lte => a <= b,
        CompareOp::Gt => a > b,
        CompareOp::Gte => a >= b,
    }
}

type Polynomial = HashMap<Vec<N>, BigInt>;
fn polynomial(e: &N, fuel: &mut usize) -> Option<Polynomial> {
    *fuel = fuel.checked_sub(1)?;
    if let Some(n) = eval(e, &Subst::default(), &[]) {
        return Some(HashMap::from([(vec![], n)]));
    }
    let mut out = match e {
        N::Add(a, b) | N::Sub(a, b) => {
            let mut a = polynomial(a, fuel)?;
            for (key, n) in polynomial(b, fuel)? {
                *a.entry(key).or_default() += if matches!(e, N::Sub(..)) { -n } else { n };
            }
            a
        }
        N::Neg(x) => polynomial(x, fuel)?.into_iter().map(|(k, v)| (k, -v)).collect(),
        N::Mul(a, b) => {
            let a = polynomial(a, fuel)?;
            let b = polynomial(b, fuel)?;
            if a.len().saturating_mul(b.len()) > 256 {
                return None;
            }
            let mut out = Polynomial::new();
            for (a, x) in a {
                for (b, y) in &b {
                    let mut key = a.clone();
                    key.extend(b.iter().cloned());
                    key.sort_by_cached_key(|n| format!("{n:?}"));
                    *out.entry(key).or_default() += &x * y;
                }
            }
            out
        }
        N::If { .. } => return None,
        _ => HashMap::from([(vec![e.clone()], BigInt::one())]),
    };
    out.retain(|_, n| !n.is_zero());
    if out.len() > 256 || out.values().any(|n| n.bits() > MAX_BITS) {
        None
    } else {
        Some(out)
    }
}
pub(super) fn difference(a: &N, b: &N) -> Option<BigInt> {
    let mut p = polynomial(&N::Sub(Box::new(a.clone()), Box::new(b.clone())), &mut 1024)?;
    let constant = p.remove(&vec![]).unwrap_or_default();
    p.is_empty().then_some(constant)
}
pub(super) fn witness(a: &N, b: &N) -> Option<Result<(String, BigInt), ()>> {
    let mut p = polynomial(&N::Sub(Box::new(a.clone()), Box::new(b.clone())), &mut 1024)?;
    let constant = p.remove(&vec![]).unwrap_or_default();
    if p.len() != 1 {
        return None;
    }
    let (term, coefficient) = p.into_iter().next()?;
    let [N::Var(name)] = term.as_slice() else {
        return None;
    };
    if coefficient.is_zero() {
        return None;
    }
    Some(if (&constant % &coefficient).is_zero() {
        Ok((name.clone(), -constant / coefficient))
    } else {
        Err(())
    })
}

/// Bellman-Ford detects inconsistent integer difference bounds, including
/// transitive cycles. Only exact linear relations enter the graph.
pub(super) fn contradictory(assumptions: &[C], subst: &Subst) -> bool {
    let mut facts = Vec::new();
    fn flatten<'a>(c: &'a C, out: &mut Vec<&'a C>) {
        if let C::And(xs) = c {
            for x in xs {
                flatten(x, out);
            }
        } else {
            out.push(c);
        }
    }
    for a in assumptions {
        flatten(a, &mut facts);
    }
    if facts.len() > 256 {
        return false;
    }
    let set: HashSet<_> = facts.iter().copied().collect();
    let mut nodes = HashMap::<N, usize>::new();
    let mut edges = Vec::<(usize, usize, BigInt)>::new();
    for fact in facts {
        if condition(fact, subst, &[]) == Some(false) {
            return true;
        }
        if let C::Not(inner) = fact {
            if set.contains(inner.as_ref()) {
                return true;
            }
        }
        let C::Compare { lhs, op, rhs } = fact else {
            continue;
        };
        if let (Some(a), Some(b)) = (eval(lhs, subst, assumptions), eval(rhs, subst, assumptions)) {
            if !compare(&a, *op, &b) {
                return true;
            }
        }
        if *op == CompareOp::Neq
            && (set.contains(&C::Compare { lhs: lhs.clone(), op: CompareOp::Eq, rhs: rhs.clone() })
                || set.contains(&C::Compare {
                    lhs: rhs.clone(),
                    op: CompareOp::Eq,
                    rhs: lhs.clone(),
                }))
        {
            return true;
        }
        let lhs = super::constraint::subst_numeric_expr(lhs, subst);
        let rhs = super::constraint::subst_numeric_expr(rhs, subst);
        let pairs: Vec<_> = match op {
            CompareOp::Eq => vec![(&lhs, &rhs, false), (&rhs, &lhs, false)],
            CompareOp::Lte => vec![(&lhs, &rhs, false)],
            CompareOp::Lt => vec![(&lhs, &rhs, true)],
            CompareOp::Gte => vec![(&rhs, &lhs, false)],
            CompareOp::Gt => vec![(&rhs, &lhs, true)],
            CompareOp::Neq => vec![],
        };
        for (a, b, strict) in pairs {
            let Some(mut p) =
                polynomial(&N::Sub(Box::new(a.clone()), Box::new(b.clone())), &mut 256)
            else {
                continue;
            };
            let constant = p.remove(&vec![]).unwrap_or_default() + BigInt::from(u8::from(strict));
            let mut positive = 0;
            let mut negative = 0;
            let mut exact = true;
            for (term, coefficient) in p {
                let [atom] = term.as_slice() else {
                    exact = false;
                    break;
                };
                let next = nodes.len() + 1;
                let id = *nodes.entry(atom.clone()).or_insert(next);
                if coefficient == BigInt::one() && positive == 0 {
                    positive = id;
                } else if coefficient == -BigInt::one() && negative == 0 {
                    negative = id;
                } else {
                    exact = false;
                    break;
                }
            }
            if exact {
                edges.push((negative, positive, -constant));
            }
        }
    }
    let mut distance = vec![BigInt::zero(); nodes.len() + 1];
    for round in 0..distance.len() {
        let mut changed = false;
        for (from, to, weight) in &edges {
            let new = &distance[*from] + weight;
            if new < distance[*to] {
                distance[*to] = new;
                changed = true;
            }
        }
        if !changed {
            return false;
        }
        if round + 1 == distance.len() {
            return true;
        }
    }
    false
}
