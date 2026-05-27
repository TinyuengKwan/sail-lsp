//! Pretty-printing for HIR expressions and patterns.
//! Used for debugging (`view_hir` command) and diagnostic messages.

use std::fmt::Write;

use super::body::Body;
use super::hir::{Expr, ExprId, MatchArm, Pat, PatId, Statement};

/// Pretty-print an expression from a `Body`.
pub fn print_expr(body: &Body, id: ExprId) -> String {
    let mut buf = String::new();
    write_expr(body, id, &mut buf, 0);
    buf
}

/// Pretty-print a pattern from a `Body`.
pub fn print_pat(body: &Body, id: PatId) -> String {
    let mut buf = String::new();
    write_pat(body, id, &mut buf);
    buf
}

/// Pretty-print the entire body (params + body expression).
pub fn print_body(body: &Body) -> String {
    let mut buf = String::new();
    // Params
    if !body.params.is_empty() {
        buf.push('(');
        for (i, &pid) in body.params.iter().enumerate() {
            if i > 0 {
                buf.push_str(", ");
            }
            write_pat(body, pid, &mut buf);
        }
        buf.push(')');
        buf.push_str(" = ");
    }
    write_expr(body, body.body_expr, &mut buf, 0);
    buf
}

fn write_expr(body: &Body, id: ExprId, buf: &mut String, indent: usize) {
    let Some(expr) = body.expr(id) else {
        buf.push_str("<missing>");
        return;
    };
    match expr {
        Expr::Missing => buf.push_str("<missing>"),
        Expr::Error { message } => {
            buf.push_str("<error: ");
            buf.push_str(message);
            buf.push('>');
        }
        Expr::Literal(lit) => write!(buf, "{lit:?}").unwrap(),
        Expr::Ident(name) => buf.push_str(name),
        Expr::TypeVar(name) => {
            buf.push('\'');
            buf.push_str(name);
        }
        Expr::Ref(name) => {
            buf.push_str("ref ");
            buf.push_str(name);
        }
        Expr::Config(parts) => {
            buf.push_str("config ");
            buf.push_str(&parts.join("."));
        }
        Expr::SizeOf { nexp, .. } => {
            buf.push_str("sizeof(");
            buf.push_str(nexp);
            buf.push(')');
        }
        Expr::Constraint(_) => buf.push_str("constraint(...)"),
        Expr::Return(e) => {
            buf.push_str("return ");
            write_expr(body, *e, buf, indent);
        }
        Expr::Throw(e) => {
            buf.push_str("throw ");
            write_expr(body, *e, buf, indent);
        }
        Expr::Exit(e) => {
            buf.push_str("exit");
            if let Some(e) = e {
                buf.push(' ');
                write_expr(body, *e, buf, indent);
            }
        }
        Expr::UnaryOp { op, expr } => {
            buf.push_str(op.as_str());
            write_expr(body, *expr, buf, indent);
        }
        Expr::Cast { expr, .. } => {
            write_expr(body, *expr, buf, indent);
            buf.push_str(" : <ty>");
        }
        Expr::Field { expr, field } => {
            write_expr(body, *expr, buf, indent);
            buf.push('.');
            buf.push_str(field);
        }
        Expr::Attribute { expr } => {
            buf.push_str("@attr ");
            write_expr(body, *expr, buf, indent);
        }
        Expr::Assign { target, value } => {
            write_expr(body, *target, buf, indent);
            buf.push_str(" = ");
            write_expr(body, *value, buf, indent);
        }
        Expr::BinaryOp { lhs, op, rhs } => {
            write_expr(body, *lhs, buf, indent);
            buf.push(' ');
            buf.push_str(op.as_str());
            buf.push(' ');
            write_expr(body, *rhs, buf, indent);
        }
        Expr::Let { pat, value, body: body_expr } => {
            buf.push_str("let ");
            write_pat(body, *pat, buf);
            buf.push_str(" = ");
            write_expr(body, *value, buf, indent);
            buf.push_str(" in ");
            write_expr(body, *body_expr, buf, indent);
        }
        Expr::Var { target, value, body: body_expr } => {
            buf.push_str("var ");
            write_expr(body, *target, buf, indent);
            buf.push_str(" = ");
            write_expr(body, *value, buf, indent);
            buf.push_str(" in ");
            write_expr(body, *body_expr, buf, indent);
        }
        Expr::If { cond, then_branch, else_branch } => {
            buf.push_str("if ");
            write_expr(body, *cond, buf, indent);
            buf.push_str(" then ");
            write_expr(body, *then_branch, buf, indent);
            if let Some(e) = else_branch {
                buf.push_str(" else ");
                write_expr(body, *e, buf, indent);
            }
        }
        Expr::Match { scrutinee, arms } => {
            buf.push_str("match ");
            write_expr(body, *scrutinee, buf, indent);
            buf.push_str(" { ");
            write_arms(body, arms, buf, indent);
            buf.push_str(" }");
        }
        Expr::Try { scrutinee, arms } => {
            buf.push_str("try ");
            write_expr(body, *scrutinee, buf, indent);
            buf.push_str(" { ");
            write_arms(body, arms, buf, indent);
            buf.push_str(" }");
        }
        Expr::While { cond, body: body_expr } => {
            buf.push_str("while ");
            write_expr(body, *cond, buf, indent);
            buf.push_str(" do ");
            write_expr(body, *body_expr, buf, indent);
        }
        Expr::Repeat { body: body_expr, until } => {
            buf.push_str("repeat ");
            write_expr(body, *body_expr, buf, indent);
            buf.push_str(" until ");
            write_expr(body, *until, buf, indent);
        }
        Expr::Foreach { pat, start, end, step, body: body_expr } => {
            buf.push_str("foreach (");
            write_pat(body, *pat, buf);
            buf.push_str(" from ");
            write_expr(body, *start, buf, indent);
            buf.push_str(" to ");
            write_expr(body, *end, buf, indent);
            if let Some(s) = step {
                buf.push_str(" by ");
                write_expr(body, *s, buf, indent);
            }
            buf.push_str(") ");
            write_expr(body, *body_expr, buf, indent);
        }
        Expr::Block(stmts) => {
            buf.push_str("{ ");
            for (i, stmt) in stmts.iter().enumerate() {
                if i > 0 {
                    buf.push_str("; ");
                }
                write_stmt(body, stmt, buf, indent + 1);
            }
            buf.push_str(" }");
        }
        Expr::Call { callee, args } => {
            write_expr(body, *callee, buf, indent);
            buf.push('(');
            for (i, arg) in args.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_expr(body, *arg, buf, indent);
            }
            buf.push(')');
        }
        Expr::Tuple(elems) => {
            buf.push('(');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_expr(body, *e, buf, indent);
            }
            buf.push(')');
        }
        Expr::List(elems) => {
            buf.push_str("[|");
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_expr(body, *e, buf, indent);
            }
            buf.push_str("|]");
        }
        Expr::Array(elems) => {
            buf.push('[');
            for (i, e) in elems.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_expr(body, *e, buf, indent);
            }
            buf.push(']');
        }
        Expr::Struct { name, fields } => {
            buf.push_str("struct ");
            if let Some(n) = name {
                buf.push_str(n);
                buf.push(' ');
            }
            buf.push_str("{ ");
            for (i, (fname, fval)) in fields.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                buf.push_str(fname);
                buf.push_str(" = ");
                write_expr(body, *fval, buf, indent);
            }
            buf.push_str(" }");
        }
        Expr::Update { base, fields } => {
            buf.push_str("{ ");
            write_expr(body, *base, buf, indent);
            buf.push_str(" with ");
            for (i, (fname, fval)) in fields.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                buf.push_str(fname);
                buf.push_str(" = ");
                write_expr(body, *fval, buf, indent);
            }
            buf.push_str(" }");
        }
        Expr::Assert { cond, message } => {
            buf.push_str("assert(");
            write_expr(body, *cond, buf, indent);
            if let Some(m) = message {
                buf.push_str(", ");
                write_expr(body, *m, buf, indent);
            }
            buf.push(')');
        }
        Expr::Index { base, index } => {
            write_expr(body, *base, buf, indent);
            buf.push('[');
            write_expr(body, *index, buf, indent);
            buf.push(']');
        }
        Expr::Subrange { base, hi, lo } => {
            write_expr(body, *base, buf, indent);
            buf.push('[');
            write_expr(body, *hi, buf, indent);
            buf.push_str(" .. ");
            write_expr(body, *lo, buf, indent);
            buf.push(']');
        }
    }
}

fn write_pat(body: &Body, id: PatId, buf: &mut String) {
    let Some(pat) = body.pat(id) else {
        buf.push_str("<missing>");
        return;
    };
    match pat {
        Pat::Missing => buf.push_str("<missing>"),
        Pat::Wild => buf.push('_'),
        Pat::Literal(lit) => write!(buf, "{lit:?}").unwrap(),
        Pat::Bind(name) => buf.push_str(name),
        Pat::TypeVar(name) => {
            buf.push('\'');
            buf.push_str(name);
        }
        Pat::Typed { inner, .. } => {
            write_pat(body, *inner, buf);
            buf.push_str(" : <ty>");
        }
        Pat::Tuple(pats) => {
            buf.push('(');
            for (i, p) in pats.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_pat(body, *p, buf);
            }
            buf.push(')');
        }
        Pat::List(pats) => {
            buf.push_str("[|");
            for (i, p) in pats.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_pat(body, *p, buf);
            }
            buf.push_str("|]");
        }
        Pat::Array(pats) => {
            buf.push('[');
            for (i, p) in pats.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                write_pat(body, *p, buf);
            }
            buf.push(']');
        }
        Pat::App { ctor, args } => {
            buf.push_str(ctor);
            if !args.is_empty() {
                buf.push('(');
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        buf.push_str(", ");
                    }
                    write_pat(body, *a, buf);
                }
                buf.push(')');
            }
        }
        Pat::Struct { name, fields } => {
            buf.push_str("struct ");
            if let Some(n) = name {
                buf.push_str(n);
                buf.push(' ');
            }
            buf.push_str("{ ");
            for (i, (fname, fpat)) in fields.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                buf.push_str(fname);
                buf.push_str(" = ");
                write_pat(body, *fpat, buf);
            }
            buf.push_str(" }");
        }
        Pat::Infix { lhs, op, rhs } => {
            write_pat(body, *lhs, buf);
            buf.push(' ');
            buf.push_str(op);
            buf.push(' ');
            write_pat(body, *rhs, buf);
        }
        Pat::As { pat, binding } => {
            write_pat(body, *pat, buf);
            buf.push_str(" as ");
            buf.push_str(binding);
        }
        Pat::AsType { pat, .. } => {
            write_pat(body, *pat, buf);
            buf.push_str(" : <ty>");
        }
        Pat::Index { name, .. } => {
            buf.push_str(name);
            buf.push_str("[..]");
        }
        Pat::RangeIndex { name, .. } => {
            buf.push_str(name);
            buf.push_str("[.. .. ..]");
        }
    }
}

fn write_stmt(body: &Body, stmt: &Statement, buf: &mut String, indent: usize) {
    match stmt {
        Statement::Let { pat, value } => {
            buf.push_str("let ");
            write_pat(body, *pat, buf);
            buf.push_str(" = ");
            write_expr(body, *value, buf, indent);
        }
        Statement::Var { pat, value } => {
            buf.push_str("var ");
            write_pat(body, *pat, buf);
            buf.push_str(" = ");
            write_expr(body, *value, buf, indent);
        }
        Statement::Expr(e) => {
            write_expr(body, *e, buf, indent);
        }
    }
}

fn write_arms(body: &Body, arms: &[MatchArm], buf: &mut String, indent: usize) {
    for (i, arm) in arms.iter().enumerate() {
        if i > 0 {
            buf.push_str(", ");
        }
        write_pat(body, arm.pat, buf);
        if let Some(g) = arm.guard {
            buf.push_str(" if ");
            write_expr(body, g, buf, indent);
        }
        buf.push_str(" => ");
        write_expr(body, arm.body, buf, indent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_print_simple_body() {
        // Create a trivial body with one ident expression
        let mut store = crate::expr_store::ExpressionStore {
            exprs: Default::default(),
            pats: Default::default(),
            bindings: Default::default(),
        };
        let id = store.exprs.alloc(Expr::Ident("x".to_string()));
        let body = Body { store, params: Box::new([]), body_expr: id, mapping_arms: Vec::new() };
        assert_eq!(print_expr(&body, id), "x");
    }
}
