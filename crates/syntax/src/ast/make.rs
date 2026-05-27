//! Free-standing functions for creating AST fragments from smaller pieces.
//! Core pattern: `ast_from_text(text)` parses a snippet and casts the
//! result to the desired AST node type. This is an implementation detail
//! — the public API should assemble nodes piecewise.
//!
//! `ext` sub-module provides high-level convenience shortcuts.
//! `tokens` sub-module provides whitespace/token factories.

use crate::{
    ast::{self, SourceFile},
    parsing,
    syntax_node::SyntaxNode,
    AstNode, SyntaxKind,
};

/// Parse `text` as a complete Sail source file and extract the first
/// descendant node of type `N`.
/// This is a private implementation detail — all public constructors
/// should call this instead of exposing it directly.
#[track_caller]
fn ast_from_text<N: AstNode>(text: &str) -> N {
    let parse = SourceFile::parse(text);
    let node = match parse.syntax_node().descendants().find_map(N::cast) {
        Some(it) => it,
        None => {
            let node = std::any::type_name::<N>();
            panic!("Failed to make ast node `{node}` from text {text}")
        }
    };
    let node = node.clone_subtree();
    assert_eq!(node.syntax().text_range().start(), 0.into());
    node
}

/// Create a `Name` node (identifier).
pub fn name(text: &str) -> ast::Name {
    // Wrap in a val spec to get a Name node.
    ast_from_text(&format!("val {text} : int\n"))
}

/// Create a `NameRef` node (identifier reference).
pub fn name_ref(text: &str) -> SyntaxNode {
    // Parse as expression to get an ident reference.
    let (root, _) = parsing::parse_text(&format!("function __make() = {text}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::IDENT_EXPR)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a type node from text.
pub fn ty(text: &str) -> SyntaxNode {
    ty_from_text(text)
}

fn ty_from_text(text: &str) -> SyntaxNode {
    let (root, _) = parsing::parse_text(&format!("val __make : {text}\n"));
    // Find the type node in the parse tree.
    root.descendants()
        .find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::TYPE_NAMED
                    | SyntaxKind::TYPE_APP
                    | SyntaxKind::TYPE_TUPLE
                    | SyntaxKind::TYPE_ARROW
                    | SyntaxKind::TYPE_VAR
            )
        })
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create `bits(N)` type.
pub fn ty_bits(width: &str) -> SyntaxNode {
    ty_from_text(&format!("bits({width})"))
}

/// Create `int` type.
pub fn ty_int() -> SyntaxNode {
    ty_from_text("int")
}

/// Create `bool` type.
pub fn ty_bool() -> SyntaxNode {
    ty_from_text("bool")
}

/// Create `unit` type.
pub fn ty_unit() -> SyntaxNode {
    ty_from_text("unit")
}

/// Create an arrow type: `(arg1, arg2) -> ret`.
pub fn ty_arrow(args: &[&str], ret: &str) -> SyntaxNode {
    let args_str = args.join(", ");
    ty_from_text(&format!("({args_str}) -> {ret}"))
}

/// Create a tuple type: `(t1, t2, ...)`.
pub fn ty_tuple(elems: &[&str]) -> SyntaxNode {
    let inner = elems.join(", ");
    ty_from_text(&format!("({inner})"))
}

/// Create a type application: `name(arg1, arg2, ...)`.
pub fn ty_app(name: &str, args: &[&str]) -> SyntaxNode {
    let args_str = args.join(", ");
    ty_from_text(&format!("{name}({args_str})"))
}

/// Create a type variable: `'a`.
pub fn ty_var(name: &str) -> SyntaxNode {
    ty_from_text(name)
}

/// Create `string` type.
pub fn ty_string() -> SyntaxNode {
    ty_from_text("string")
}

/// Create `bit` type.
pub fn ty_bit() -> SyntaxNode {
    ty_from_text("bit")
}

/// Create an expression from text.
fn expr_from_text(text: &str) -> SyntaxNode {
    let (root, _) = parsing::parse_text(&format!("function __make() = {text}\n"));
    // Find first expression-like node after the `=`.
    root.descendants()
        .find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::IDENT_EXPR
                    | SyntaxKind::LITERAL_EXPR
                    | SyntaxKind::CALL_EXPR
                    | SyntaxKind::BIN_EXPR
                    | SyntaxKind::IF_EXPR
                    | SyntaxKind::MATCH_EXPR
                    | SyntaxKind::BLOCK_EXPR
                    | SyntaxKind::TUPLE_EXPR
                    | SyntaxKind::STRUCT_EXPR
                    | SyntaxKind::LET_EXPR
                    | SyntaxKind::PREFIX_EXPR
                    | SyntaxKind::FIELD_ACCESS_EXPR
                    | SyntaxKind::INDEX_EXPR
                    | SyntaxKind::VECTOR_EXPR
                    | SyntaxKind::FOREACH_EXPR
                    | SyntaxKind::WHILE_EXPR
                    | SyntaxKind::RETURN_EXPR
                    | SyntaxKind::THROW_EXPR
                    | SyntaxKind::ASSERT_EXPR
                    | SyntaxKind::CAST_EXPR
                    | SyntaxKind::LIST_EXPR
                    | SyntaxKind::SUBRANGE_EXPR
                    | SyntaxKind::ASSIGN_EXPR
                    | SyntaxKind::REF_EXPR
                    | SyntaxKind::EXIT_EXPR
                    | SyntaxKind::TRY_EXPR
                    | SyntaxKind::SIZEOF_EXPR
                    | SyntaxKind::VAR_EXPR
                    | SyntaxKind::CONFIG_EXPR
                    | SyntaxKind::CONSTRAINT_EXPR
                    | SyntaxKind::REPEAT_EXPR
                    | SyntaxKind::TYVAR_EXPR
                    | SyntaxKind::UPDATE_EXPR
                    | SyntaxKind::VECTOR_UPDATE_EXPR
            )
        })
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create an identifier expression.
pub fn expr_ident(name: &str) -> SyntaxNode {
    expr_from_text(name)
}

/// Create a literal expression.
pub fn expr_literal(text: &str) -> SyntaxNode {
    expr_from_text(text)
}

/// Create a function call expression.
pub fn expr_call(func: &str, args: &[&str]) -> SyntaxNode {
    let args_str = args.join(", ");
    expr_from_text(&format!("{func}({args_str})"))
}

/// Create an if expression.
pub fn expr_if(cond: &str, then_body: &str, else_body: Option<&str>) -> SyntaxNode {
    match else_body {
        Some(e) => expr_from_text(&format!("if {cond} then {then_body} else {e}")),
        None => expr_from_text(&format!("if {cond} then {then_body}")),
    }
}

/// Create a match expression.
pub fn expr_match(scrutinee: &str, arms: &[(&str, &str)]) -> SyntaxNode {
    let arms_str: String =
        arms.iter().map(|(pat, body)| format!("    {pat} => {body},\n")).collect();
    expr_from_text(&format!("match {scrutinee} {{\n{arms_str}}}"))
}

/// Create a let expression.
pub fn expr_let(name: &str, ty: Option<&str>, init: &str) -> SyntaxNode {
    match ty {
        Some(t) => expr_from_text(&format!("let {name} : {t} = {init}")),
        None => expr_from_text(&format!("let {name} = {init}")),
    }
}

/// Create a struct expression.
pub fn expr_struct(fields: &[(&str, &str)]) -> SyntaxNode {
    let fields_str: String =
        fields.iter().map(|(name, val)| format!("{name} = {val}")).collect::<Vec<_>>().join(", ");
    expr_from_text(&format!("struct {{ {fields_str} }}"))
}

/// Create a vector subrange expression: `v[hi .. lo]`.
pub fn expr_vector_subrange(vec_name: &str, hi: &str, lo: &str) -> SyntaxNode {
    expr_from_text(&format!("{vec_name}[{hi} .. {lo}]"))
}

/// Create a block expression: `{ stmt1; stmt2; ... }`.
pub fn expr_block(stmts: &[&str]) -> SyntaxNode {
    let body = stmts.join("; ");
    expr_from_text(&format!("{{ {body} }}"))
}

/// Create a tuple expression: `(a, b, c)`.
pub fn expr_tuple(elems: &[&str]) -> SyntaxNode {
    let inner = elems.join(", ");
    expr_from_text(&format!("({inner})"))
}

/// Create a vector expression: `[a, b, c]`.
pub fn expr_vector(elems: &[&str]) -> SyntaxNode {
    let inner = elems.join(", ");
    expr_from_text(&format!("[{inner}]"))
}

/// Create a list expression: `[|a, b, c|]`.
pub fn expr_list(elems: &[&str]) -> SyntaxNode {
    let inner = elems.join(", ");
    expr_from_text(&format!("[|{inner}|]"))
}

/// Create a prefix expression: `-x`, `~x`.
pub fn expr_prefix(op: &str, inner: &str) -> SyntaxNode {
    expr_from_text(&format!("{op}{inner}"))
}

/// Create a binary expression: `a + b`.
pub fn expr_bin(lhs: &str, op: &str, rhs: &str) -> SyntaxNode {
    expr_from_text(&format!("{lhs} {op} {rhs}"))
}

/// Create a field access expression: `x.field`.
pub fn expr_field_access(base: &str, field: &str) -> SyntaxNode {
    expr_from_text(&format!("{base}.{field}"))
}

/// Create an index expression: `x[i]`.
pub fn expr_index(base: &str, idx: &str) -> SyntaxNode {
    expr_from_text(&format!("{base}[{idx}]"))
}

/// Create an assign expression: `x = e` (parsed inside a block as ASSIGN_EXPR).
pub fn expr_assign(lhs: &str, rhs: &str) -> SyntaxNode {
    // Assignment in Sail is `lhs = rhs` inside a block.
    let (root, _) = parsing::parse_text(&format!("function __make() = {{ {lhs} = {rhs} }}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::ASSIGN_EXPR)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a while expression: `while cond do body`.
pub fn expr_while(cond: &str, body: &str) -> SyntaxNode {
    expr_from_text(&format!("while {cond} do {body}"))
}

/// Create a foreach expression: `foreach (var from start to end) body`.
pub fn expr_foreach(var: &str, from: &str, to: &str, body: &str) -> SyntaxNode {
    expr_from_text(&format!("foreach ({var} from {from} to {to}) {body}"))
}

/// Create a return expression: `return e`.
pub fn expr_return(val: &str) -> SyntaxNode {
    expr_from_text(&format!("return {val}"))
}

/// Create an assert expression: `assert(cond)`.
pub fn expr_assert(cond: &str) -> SyntaxNode {
    expr_from_text(&format!("assert({cond})"))
}

/// Create a ref expression: `ref x`.
pub fn expr_ref(name: &str) -> SyntaxNode {
    expr_from_text(&format!("ref {name}"))
}

/// Create an exit expression: `exit()`.
pub fn expr_exit() -> SyntaxNode {
    expr_from_text("exit()")
}

/// Create a throw expression: `throw e`.
pub fn expr_throw(val: &str) -> SyntaxNode {
    expr_from_text(&format!("throw {val}"))
}

/// Create a try expression: `try body catch { pat => expr }`.
pub fn expr_try(body: &str, catch: &str) -> SyntaxNode {
    expr_from_text(&format!("try {body} catch {{ {catch} }}"))
}

/// Create a sizeof expression: `sizeof(ty)`.
pub fn expr_sizeof(ty: &str) -> SyntaxNode {
    expr_from_text(&format!("sizeof({ty})"))
}

/// Create a cast expression: `(ty)(val)` — Sail casts via call-like syntax.
pub fn expr_cast(ty: &str, val: &str) -> SyntaxNode {
    // Sail casts are typically written as function calls in practice.
    expr_from_text(&format!("{ty}({val})"))
}

/// Create a var expression: `var x = e`.
pub fn expr_var(name: &str, init: &str) -> SyntaxNode {
    expr_from_text(&format!("var {name} = {init}"))
}

/// Create a config expression: `config name`.
pub fn expr_config(name: &str) -> SyntaxNode {
    let (root, _) = parsing::parse_text(&format!("function __make() = config {name}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::CONFIG_EXPR)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a constraint expression: `constraint(text)`.
pub fn expr_constraint(text: &str) -> SyntaxNode {
    expr_from_text(&format!("constraint({text})"))
}

fn pat_from_text(text: &str) -> SyntaxNode {
    let (root, _) =
        parsing::parse_text(&format!("function __make() = match () {{ {text} => () }}\n"));
    root.descendants()
        .find(|n| {
            matches!(
                n.kind(),
                SyntaxKind::IDENT_PAT
                    | SyntaxKind::WILD_PAT
                    | SyntaxKind::TUPLE_PAT
                    | SyntaxKind::LITERAL_PAT
                    | SyntaxKind::STRUCT_PAT
                    | SyntaxKind::APP_PAT
                    | SyntaxKind::AS_PAT
                    | SyntaxKind::TYPED_PAT
                    | SyntaxKind::VECTOR_PAT
                    | SyntaxKind::LIST_PAT
                    | SyntaxKind::BIN_PAT
                    | SyntaxKind::RANGE_INDEX_PAT
                    | SyntaxKind::INDEX_PAT
                    | SyntaxKind::TYVAR_PAT
            )
        })
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a wildcard pattern `_`.
pub fn wildcard_pat() -> SyntaxNode {
    pat_from_text("_")
}

/// Create an identifier pattern.
pub fn ident_pat(name: &str) -> SyntaxNode {
    pat_from_text(name)
}

/// Create a tuple pattern.
pub fn tuple_pat(pats: &[&str]) -> SyntaxNode {
    let inner = pats.join(", ");
    pat_from_text(&format!("({inner})"))
}

/// Create a literal pattern.
pub fn literal_pat(text: &str) -> SyntaxNode {
    pat_from_text(text)
}

/// Create a constructor application pattern: `Some(x)`.
pub fn app_pat(name: &str, args: &[&str]) -> SyntaxNode {
    let args_str = args.join(", ");
    pat_from_text(&format!("{name}({args_str})"))
}

/// Create an as-pattern: `pat as name`.
pub fn as_pat(inner: &str, name: &str) -> SyntaxNode {
    pat_from_text(&format!("{inner} as {name}"))
}

/// Create a vector pattern: `[|a, b, c|]` (Sail uses `[|..|]` for vector patterns).
pub fn vector_pat(elems: &[&str]) -> SyntaxNode {
    let inner = elems.join(", ");
    pat_from_text(&format!("[|{inner}|]"))
}

/// Create a list pattern: `[a, b]` (Sail uses `[..]` for list patterns).
pub fn list_pat(elems: &[&str]) -> SyntaxNode {
    let inner = elems.join(", ");
    pat_from_text(&format!("[{inner}]"))
}

/// Create a struct pattern: `struct { f = p, ... }`.
pub fn struct_pat(fields: &[(&str, &str)]) -> SyntaxNode {
    let fields_str: String =
        fields.iter().map(|(name, pat)| format!("{name} = {pat}")).collect::<Vec<_>>().join(", ");
    pat_from_text(&format!("struct {{ {fields_str} }}"))
}

/// Create a typed pattern: `(p : ty)`.
pub fn typed_pat(pat: &str, ty: &str) -> SyntaxNode {
    pat_from_text(&format!("{pat} : {ty}"))
}

/// Create a binary pattern: `p @ q`, `p :: q`, `p | q`.
pub fn bin_pat(lhs: &str, op: &str, rhs: &str) -> SyntaxNode {
    pat_from_text(&format!("{lhs} {op} {rhs}"))
}

/// Create a range index pattern: `name[hi .. lo]`.
pub fn range_index_pat(base: &str, hi: &str, lo: &str) -> SyntaxNode {
    pat_from_text(&format!("{base}[{hi} .. {lo}]"))
}

/// Create a type variable pattern: `'a`.
pub fn tyvar_pat(name: &str) -> SyntaxNode {
    pat_from_text(name)
}

/// Create a function definition.
pub fn fn_def(name: &str, params: &[(&str, &str)], ret_ty: Option<&str>, body: &str) -> SyntaxNode {
    let params_str: String =
        params.iter().map(|(n, t)| format!("{n} : {t}")).collect::<Vec<_>>().join(", ");
    let ret = match ret_ty {
        Some(t) => format!(" -> {t}"),
        None => String::new(),
    };
    let (root, _) = parsing::parse_text(&format!("function {name}({params_str}){ret} = {body}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::CALLABLE_DEF)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a val spec (type signature).
pub fn val_spec(name: &str, ty_text: &str) -> SyntaxNode {
    let (root, _) = parsing::parse_text(&format!("val {name} : {ty_text}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::CALLABLE_SPEC)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a match arm: `pattern => expression`.
pub fn match_arm(pat: &str, guard: Option<&str>, body: &str) -> SyntaxNode {
    let guard_str = match guard {
        Some(g) => format!(" if {g}"),
        None => String::new(),
    };
    let (root, _) = parsing::parse_text(&format!(
        "function __make() = match () {{ {pat}{guard_str} => {body} }}\n"
    ));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::MATCH_ARM)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a type alias: `type name = ty`.
pub fn type_alias(name: &str, ty: &str) -> SyntaxNode {
    let (root, _) = parsing::parse_text(&format!("type {name} = {ty}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::TYPE_ALIAS_DEF)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create an enum definition: `enum name = { V1, V2, ... }`.
pub fn enum_def(name: &str, variants: &[&str]) -> SyntaxNode {
    let variants_str = variants.iter().map(|v| format!("{v}")).collect::<Vec<_>>().join(", ");
    let (root, _) = parsing::parse_text(&format!("enum {name} = {{ {variants_str} }}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::NAMED_DEF)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a struct definition: `struct name = { f1 : t1, f2 : t2, ... }`.
pub fn struct_def(name: &str, fields: &[(&str, &str)]) -> SyntaxNode {
    let fields_str: String =
        fields.iter().map(|(n, t)| format!("{n} : {t}")).collect::<Vec<_>>().join(", ");
    let (root, _) = parsing::parse_text(&format!("struct {name} = {{ {fields_str} }}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::NAMED_DEF)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a register definition: `register name : ty`.
pub fn register_def(name: &str, ty: &str) -> SyntaxNode {
    let (root, _) = parsing::parse_text(&format!("register {name} : {ty}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::NAMED_DEF)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a let definition: `let name : ty = value` or `let name = value`.
pub fn let_def(name: &str, ty: Option<&str>, value: &str) -> SyntaxNode {
    let ty_str = match ty {
        Some(t) => format!(" : {t}"),
        None => String::new(),
    };
    let (root, _) = parsing::parse_text(&format!("let {name}{ty_str} = {value}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::NAMED_DEF)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a param list node: `(p1 : t1, p2 : t2)`.
pub fn param_list(params: &[(&str, &str)]) -> SyntaxNode {
    let params_str: String =
        params.iter().map(|(n, t)| format!("{n} : {t}")).collect::<Vec<_>>().join(", ");
    let (root, _) = parsing::parse_text(&format!("function __make({params_str}) = ()\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::PARAM_LIST)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create an arg list node: `(a, b, c)`.
pub fn arg_list(args: &[&str]) -> SyntaxNode {
    let args_str = args.join(", ");
    let (root, _) = parsing::parse_text(&format!("function __make() = foo({args_str})\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::ARG_LIST)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a field init node: `name = value`.
pub fn field_init(name: &str, value: &str) -> SyntaxNode {
    let (root, _) =
        parsing::parse_text(&format!("function __make() = struct {{ {name} = {value} }}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::FIELD_INIT)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a quantifier: `forall 'a 'b.` (returns the text as part of a val spec).
pub fn quantifier(vars: &[&str]) -> SyntaxNode {
    let vars_str = vars.join(" ");
    let (root, _) = parsing::parse_text(&format!("val __make : forall {vars_str}. int\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::QUANTIFIER)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Create a block item (statement inside a block).
pub fn block_item(text: &str) -> SyntaxNode {
    let (root, _) = parsing::parse_text(&format!("function __make() = {{ {text} }}\n"));
    root.descendants()
        .find(|n| n.kind() == SyntaxKind::BLOCK_ITEM)
        .map(|n| n.clone_subtree())
        .unwrap_or(root)
}

/// Token factory functions for whitespace and common tokens.
pub mod tokens {
    use crate::{ast::SourceFile, SyntaxKind, SyntaxToken};

    /// Create a single space token.
    pub fn single_space() -> SyntaxToken {
        let parse = SourceFile::parse("val x : int\n");
        parse
            .syntax_node()
            .clone_for_update()
            .descendants_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|it| it.kind() == SyntaxKind::WHITESPACE && it.text() == " ")
            .unwrap()
    }

    /// Create a whitespace token with arbitrary content.
    pub fn whitespace(text: &str) -> SyntaxToken {
        assert!(text.trim().is_empty(), "whitespace token must contain only whitespace");
        let parse = SourceFile::parse(text);
        let root = parse.syntax_node().clone_for_update();
        root.first_child_or_token().and_then(|it| it.into_token()).unwrap_or_else(|| {
            // Fallback: parse with content to get a whitespace token.
            let parse2 = SourceFile::parse(&format!("{text}val x : int\n"));
            parse2
                .syntax_node()
                .clone_for_update()
                .first_child_or_token()
                .and_then(|it| it.into_token())
                .unwrap()
        })
    }

    /// Create a single newline token.
    pub fn single_newline() -> SyntaxToken {
        let tok = whitespace("\n");
        tok.detach();
        tok
    }

    /// Create a blank line (double newline) token.
    pub fn blank_line() -> SyntaxToken {
        whitespace("\n\n")
    }

    /// Create a comma token.
    pub fn comma() -> SyntaxToken {
        let parse = SourceFile::parse("function __make(a, b) = ()\n");
        parse
            .syntax_node()
            .clone_for_update()
            .descendants_with_tokens()
            .filter_map(|it| it.into_token())
            .find(|it| it.kind() == SyntaxKind::COMMA)
            .unwrap()
    }
}

/// High-level convenience shortcuts for common constructions.
pub mod ext {
    use super::*;

    /// Create a unit expression `()`.
    pub fn expr_unit() -> SyntaxNode {
        super::expr_from_text("()")
    }

    /// Create an identifier path.
    pub fn ident_path(name: &str) -> SyntaxNode {
        super::expr_ident(name)
    }

    /// Create a `0` literal.
    pub fn zero_number() -> SyntaxNode {
        super::expr_literal("0")
    }

    /// Create a `false` literal.
    pub fn default_bool() -> SyntaxNode {
        super::expr_literal("false")
    }

    /// Create an empty string literal.
    pub fn empty_str() -> SyntaxNode {
        super::expr_literal("\"\"")
    }

    /// Create a `true` literal expression.
    pub fn true_expr() -> SyntaxNode {
        super::expr_literal("true")
    }

    /// Create a `bitzero` literal expression.
    pub fn bitzero() -> SyntaxNode {
        super::expr_literal("bitzero")
    }

    /// Create a `bitone` literal expression.
    pub fn bitone() -> SyntaxNode {
        super::expr_literal("bitone")
    }

    /// Create an empty vector literal `[]`.
    pub fn empty_vector() -> SyntaxNode {
        super::expr_from_text("[]")
    }
}

/// Template-based AST construction.
/// Provides a simpler approach than a full `quote!` macro: template-based
/// construction that fills in `$`-prefixed placeholders.
pub mod quote {
    use super::{expr_from_text, pat_from_text, ty_from_text};
    use crate::syntax_node::SyntaxNode;

    /// Substitute `$name` placeholders in `template` with the corresponding
    /// values from `vars`, then parse the result as an expression.
    ///
    /// # Example
    /// ```ignore
    /// let node = expr_template("$lhs + $rhs", &[("lhs", "x"), ("rhs", "y")]);
    /// assert!(node.text().to_string().contains("x + y"));
    /// ```
    pub fn expr_template(template: &str, vars: &[(&str, &str)]) -> SyntaxNode {
        let text = substitute(template, vars);
        expr_from_text(&text)
    }

    /// Substitute `$name` placeholders in `template` and parse as a type.
    pub fn ty_template(template: &str, vars: &[(&str, &str)]) -> SyntaxNode {
        let text = substitute(template, vars);
        ty_from_text(&text)
    }

    /// Substitute `$name` placeholders in `template` and parse as a pattern.
    pub fn pat_template(template: &str, vars: &[(&str, &str)]) -> SyntaxNode {
        let text = substitute(template, vars);
        pat_from_text(&text)
    }

    /// Perform `$name` substitution in a template string.
    fn substitute(template: &str, vars: &[(&str, &str)]) -> String {
        let mut result = template.to_string();
        for &(name, value) in vars {
            let placeholder = format!("${name}");
            result = result.replace(&placeholder, value);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_name() {
        let n = name("foo");
        assert!(n.syntax().text().to_string().contains("foo"));
    }

    #[test]
    fn make_ty_int() {
        let t = ty_int();
        assert_eq!(t.text().to_string(), "int");
    }

    #[test]
    fn make_ty_bits() {
        let t = ty_bits("32");
        assert_eq!(t.text().to_string(), "bits(32)");
    }

    #[test]
    fn make_ty_bool() {
        let t = ty_bool();
        assert_eq!(t.text().to_string(), "bool");
    }

    #[test]
    fn make_expr_ident() {
        let e = expr_ident("x");
        assert_eq!(e.text().to_string(), "x");
    }

    #[test]
    fn make_expr_literal_number() {
        let e = expr_literal("42");
        assert!(e.text().to_string().contains("42"));
    }

    #[test]
    fn make_expr_call() {
        let e = expr_call("foo", &["x", "y"]);
        let text = e.text().to_string();
        assert!(text.contains("foo"), "got: {text}");
        assert!(text.contains("x"), "got: {text}");
        assert!(text.contains("y"), "got: {text}");
    }

    #[test]
    fn make_wildcard_pat() {
        let p = wildcard_pat();
        assert_eq!(p.text().to_string(), "_");
    }

    #[test]
    fn make_val_spec() {
        let v = val_spec("add", "(int, int) -> int");
        let text = v.text().to_string();
        assert!(text.contains("add"), "got: {text}");
        assert!(text.contains("int"), "got: {text}");
    }

    #[test]
    fn make_fn_def() {
        let f = fn_def("add", &[("x", "int"), ("y", "int")], Some("int"), "x + y");
        let text = f.text().to_string();
        assert!(text.contains("add"), "got: {text}");
        assert!(text.contains("x : int"), "got: {text}");
    }

    #[test]
    fn make_match_arm() {
        let a = match_arm("true", None, "1");
        let text = a.text().to_string();
        assert!(text.contains("true"), "got: {text}");
        assert!(text.contains("1"), "got: {text}");
    }

    #[test]
    fn make_tokens_single_space() {
        let tok = tokens::single_space();
        assert_eq!(tok.text(), " ");
        assert_eq!(tok.kind(), SyntaxKind::WHITESPACE);
    }

    #[test]
    fn make_tokens_whitespace() {
        let tok = tokens::whitespace("\n  ");
        assert_eq!(tok.text(), "\n  ");
    }

    #[test]
    fn make_tokens_single_newline() {
        let tok = tokens::single_newline();
        assert_eq!(tok.text(), "\n");
    }

    #[test]
    fn make_ext_expr_unit() {
        let e = ext::expr_unit();
        assert_eq!(e.text().to_string(), "()");
    }

    #[test]
    fn make_ext_zero_number() {
        let e = ext::zero_number();
        assert!(e.text().to_string().contains("0"));
    }

    #[test]
    fn make_ty_arrow() {
        let t = ty_arrow(&["int", "int"], "bool");
        let text = t.text().to_string();
        assert!(text.contains("int"), "got: {text}");
        assert!(text.contains("bool"), "got: {text}");
    }

    #[test]
    fn make_ty_tuple() {
        let t = ty_tuple(&["int", "bool"]);
        let text = t.text().to_string();
        assert!(text.contains("int"), "got: {text}");
        assert!(text.contains("bool"), "got: {text}");
    }

    #[test]
    fn make_ty_app() {
        let t = ty_app("vector", &["32", "dec", "bit"]);
        let text = t.text().to_string();
        assert!(text.contains("vector"), "got: {text}");
    }

    #[test]
    fn make_expr_block() {
        let e = expr_block(&["x", "y"]);
        let text = e.text().to_string();
        assert!(text.contains("x"), "got: {text}");
        assert!(text.contains("y"), "got: {text}");
    }

    #[test]
    fn make_expr_tuple() {
        let e = expr_tuple(&["a", "b"]);
        let text = e.text().to_string();
        assert!(text.contains("a"), "got: {text}");
        assert!(text.contains("b"), "got: {text}");
    }

    #[test]
    fn make_expr_vector() {
        let e = expr_vector(&["1", "2", "3"]);
        let text = e.text().to_string();
        assert!(text.contains("1"), "got: {text}");
    }

    #[test]
    fn make_expr_bin() {
        let e = expr_bin("a", "+", "b");
        let text = e.text().to_string();
        assert!(text.contains("a"), "got: {text}");
        assert!(text.contains("+"), "got: {text}");
        assert!(text.contains("b"), "got: {text}");
    }

    #[test]
    fn make_expr_while() {
        let e = expr_while("true", "()");
        let text = e.text().to_string();
        assert!(text.contains("while"), "got: {text}");
    }

    #[test]
    fn make_expr_return() {
        let e = expr_return("42");
        let text = e.text().to_string();
        assert!(text.contains("return"), "got: {text}");
        assert!(text.contains("42"), "got: {text}");
    }

    #[test]
    fn make_expr_throw() {
        let e = expr_throw("err");
        let text = e.text().to_string();
        assert!(text.contains("throw"), "got: {text}");
    }

    #[test]
    fn make_app_pat() {
        let p = app_pat("Some", &["x"]);
        let text = p.text().to_string();
        assert!(text.contains("Some"), "got: {text}");
        assert!(text.contains("x"), "got: {text}");
    }

    #[test]
    fn make_as_pat() {
        let p = as_pat("x", "y");
        let text = p.text().to_string();
        assert!(text.contains("as"), "got: {text}");
    }

    #[test]
    fn make_type_alias() {
        let d = type_alias("mybits", "bits(32)");
        let text = d.text().to_string();
        assert!(text.contains("mybits"), "got: {text}");
    }

    #[test]
    fn make_register_def() {
        let d = register_def("PC", "bits(64)");
        let text = d.text().to_string();
        assert!(text.contains("register"), "got: {text}");
        assert!(text.contains("PC"), "got: {text}");
    }

    #[test]
    fn make_let_def_test() {
        let d = let_def("x", Some("int"), "42");
        let text = d.text().to_string();
        assert!(text.contains("let"), "got: {text}");
        assert!(text.contains("x"), "got: {text}");
    }

    #[test]
    fn make_param_list_test() {
        let p = param_list(&[("x", "int"), ("y", "bool")]);
        let text = p.text().to_string();
        assert!(text.contains("x"), "got: {text}");
        assert!(text.contains("int"), "got: {text}");
    }

    #[test]
    fn make_quote_expr_template() {
        let e = quote::expr_template("$lhs + $rhs", &[("lhs", "x"), ("rhs", "y")]);
        let text = e.text().to_string();
        assert!(text.contains("x"), "got: {text}");
        assert!(text.contains("y"), "got: {text}");
    }

    #[test]
    fn make_quote_ty_template() {
        let t = quote::ty_template("bits($n)", &[("n", "32")]);
        let text = t.text().to_string();
        assert!(text.contains("32"), "got: {text}");
    }

    #[test]
    fn make_ext_true_expr() {
        let e = ext::true_expr();
        assert!(e.text().to_string().contains("true"));
    }

    #[test]
    fn make_ext_empty_vector() {
        let e = ext::empty_vector();
        let text = e.text().to_string();
        assert!(text.contains("["), "got: {text}");
    }
}
