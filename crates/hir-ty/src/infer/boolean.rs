//! Bounded propositional entailment. Numeric/opaque assumptions can be ignored
//! to establish a tautology, but never to manufacture a counterexample.
use super::{ConstraintExpr as C, ConstraintStatus as S, Subst};

pub(super) fn decide(goal: &C, subst: &Subst, facts: &[C], universal: bool) -> Option<S> {
    if !matches!(goal, C::Bool(_) | C::BoolVar(_) | C::And(_) | C::Or(_) | C::Not(_)) {
        return None;
    }
    fn collect(c: &C, names: &mut Vec<String>, fuel: &mut usize) -> bool {
        if *fuel == 0 {
            return false;
        }
        *fuel -= 1;
        match c {
            C::Bool(_) => true,
            C::BoolVar(n) => {
                if !names.contains(n) {
                    names.push(n.clone());
                }
                names.len() <= 8
            }
            C::And(xs) | C::Or(xs) => xs.iter().all(|c| collect(c, names, fuel)),
            C::Not(c) => collect(c, names, fuel),
            _ => false,
        }
    }
    fn eval(c: &C, names: &[String], bits: usize) -> bool {
        match c {
            C::Bool(b) => *b,
            C::BoolVar(n) => bits & (1 << names.iter().position(|v| v == n).unwrap()) != 0,
            C::And(xs) => xs.iter().all(|c| eval(c, names, bits)),
            C::Or(xs) => xs.iter().any(|c| eval(c, names, bits)),
            C::Not(c) => !eval(c, names, bits),
            _ => unreachable!("only propositional nodes are evaluated"),
        }
    }
    let goal = super::substitution::constraint(goal, subst);
    let mut names = Vec::new();
    let mut fuel = 256;
    if !collect(&goal, &mut names, &mut fuel) {
        return None;
    }
    let mut usable = Vec::new();
    let mut complete = true;
    for fact in facts {
        let fact = super::substitution::constraint(fact, subst);
        let mark = names.len();
        if collect(&fact, &mut names, &mut fuel) {
            usable.push(fact);
        } else {
            names.truncate(mark);
            complete = false;
        }
        if fuel == 0 {
            return None;
        }
    }
    let (mut yes, mut no) = (false, false);
    for bits in 0..(1 << names.len()) {
        if usable.iter().all(|c| eval(c, &names, bits)) {
            if eval(&goal, &names, bits) {
                yes = true;
            } else {
                no = true;
            }
            if no && universal && complete {
                return Some(S::Failed);
            }
            if yes && no {
                return Some(S::Unknown);
            }
        }
    }
    Some(if !no {
        S::Satisfied
    } else if !yes && complete {
        S::Failed
    } else {
        S::Unknown
    })
}
