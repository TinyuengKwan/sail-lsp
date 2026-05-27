//! `SyntaxFactory` constructor methods that delegate to [`crate::ast::make`].
//!
//! Each method calls the corresponding `make::*` function, then calls
//! `clone_for_update()` on the result to produce a mutable tree suitable
//! for use with `SyntaxEditor`.

use crate::ast::{make, Name};
use crate::syntax_node::SyntaxNode;
use crate::AstNode;

use super::SyntaxFactory;

impl SyntaxFactory {
    /// Create a `Name` node.
    ///
    /// When mapping tracking is enabled, verifies the mapping table
    /// is accessible (future: will record input→output node mappings
    /// for proper source attribution in assists).
    pub fn name(&self, text: &str) -> Name {
        let node = make::name(text);
        let result = node.clone_for_update();
        // Touch mappings to ensure tracking infrastructure is live.
        // Future: record (input_node, output_node) via SyntaxMappingBuilder.
        if let Some(mut _mappings) = self.mappings() {
            // Mapping will be populated when SyntaxFactory constructor
            // methods receive both input and output nodes.
            let _ = _mappings.is_empty();
        }
        result
    }

    /// Create a name-ref (identifier expression) node.
    pub fn name_ref(&self, text: &str) -> SyntaxNode {
        make::name_ref(text).clone_for_update()
    }

    /// Create a type node from text.
    pub fn ty(&self, text: &str) -> SyntaxNode {
        make::ty(text).clone_for_update()
    }

    /// Create `bits(N)` type.
    pub fn ty_bits(&self, width: &str) -> SyntaxNode {
        make::ty_bits(width).clone_for_update()
    }

    /// Create `int` type.
    pub fn ty_int(&self) -> SyntaxNode {
        make::ty_int().clone_for_update()
    }

    /// Create `bool` type.
    pub fn ty_bool(&self) -> SyntaxNode {
        make::ty_bool().clone_for_update()
    }

    /// Create `unit` type.
    pub fn ty_unit(&self) -> SyntaxNode {
        make::ty_unit().clone_for_update()
    }

    /// Create an arrow type: `(arg1, arg2) -> ret`.
    pub fn ty_arrow(&self, args: &[&str], ret: &str) -> SyntaxNode {
        make::ty_arrow(args, ret).clone_for_update()
    }

    /// Create a tuple type: `(t1, t2, ...)`.
    pub fn ty_tuple(&self, elems: &[&str]) -> SyntaxNode {
        make::ty_tuple(elems).clone_for_update()
    }

    /// Create a type application: `name(arg1, arg2, ...)`.
    pub fn ty_app(&self, name: &str, args: &[&str]) -> SyntaxNode {
        make::ty_app(name, args).clone_for_update()
    }

    /// Create a type variable: `'a`.
    pub fn ty_var(&self, name: &str) -> SyntaxNode {
        make::ty_var(name).clone_for_update()
    }

    /// Create `string` type.
    pub fn ty_string(&self) -> SyntaxNode {
        make::ty_string().clone_for_update()
    }

    /// Create `bit` type.
    pub fn ty_bit(&self) -> SyntaxNode {
        make::ty_bit().clone_for_update()
    }

    /// Create an identifier expression.
    pub fn ident_expr(&self, name: &str) -> SyntaxNode {
        make::expr_ident(name).clone_for_update()
    }

    /// Create a literal expression.
    pub fn literal_expr(&self, text: &str) -> SyntaxNode {
        make::expr_literal(text).clone_for_update()
    }

    /// Create a function call expression from text fragments.
    pub fn call_expr_text(&self, callee: &str, args: &[&str]) -> SyntaxNode {
        make::expr_call(callee, args).clone_for_update()
    }

    /// Create an if expression.
    pub fn if_expr(&self, cond: &str, then_body: &str, else_body: Option<&str>) -> SyntaxNode {
        make::expr_if(cond, then_body, else_body).clone_for_update()
    }

    /// Create a match expression.
    pub fn match_expr(&self, scrutinee: &str, arms: &[(&str, &str)]) -> SyntaxNode {
        make::expr_match(scrutinee, arms).clone_for_update()
    }

    /// Create a let expression.
    pub fn let_expr(&self, name: &str, ty: Option<&str>, init: &str) -> SyntaxNode {
        make::expr_let(name, ty, init).clone_for_update()
    }

    /// Create a struct expression.
    pub fn struct_expr(&self, fields: &[(&str, &str)]) -> SyntaxNode {
        make::expr_struct(fields).clone_for_update()
    }

    /// Create a vector subrange expression.
    pub fn vector_subrange_expr(&self, vec_name: &str, hi: &str, lo: &str) -> SyntaxNode {
        make::expr_vector_subrange(vec_name, hi, lo).clone_for_update()
    }

    /// Create a block expression: `{ stmt1; stmt2; ... }`.
    pub fn block_expr(&self, stmts: &[&str]) -> SyntaxNode {
        make::expr_block(stmts).clone_for_update()
    }

    /// Create a tuple expression: `(a, b, c)`.
    pub fn tuple_expr(&self, elems: &[&str]) -> SyntaxNode {
        make::expr_tuple(elems).clone_for_update()
    }

    /// Create a vector expression: `[a, b, c]`.
    pub fn vector_expr(&self, elems: &[&str]) -> SyntaxNode {
        make::expr_vector(elems).clone_for_update()
    }

    /// Create a list expression: `[|a, b, c|]`.
    pub fn list_expr(&self, elems: &[&str]) -> SyntaxNode {
        make::expr_list(elems).clone_for_update()
    }

    /// Create a prefix expression: `-x`, `~x`.
    pub fn prefix_expr(&self, op: &str, inner: &str) -> SyntaxNode {
        make::expr_prefix(op, inner).clone_for_update()
    }

    /// Create a binary expression: `a + b`.
    pub fn bin_expr(&self, lhs: &str, op: &str, rhs: &str) -> SyntaxNode {
        make::expr_bin(lhs, op, rhs).clone_for_update()
    }

    /// Create a field access expression: `x.field`.
    pub fn field_access_expr(&self, base: &str, field: &str) -> SyntaxNode {
        make::expr_field_access(base, field).clone_for_update()
    }

    /// Create an index expression: `x[i]`.
    pub fn index_expr(&self, base: &str, idx: &str) -> SyntaxNode {
        make::expr_index(base, idx).clone_for_update()
    }

    /// Create an assign expression: `x = e`.
    pub fn assign_expr(&self, lhs: &str, rhs: &str) -> SyntaxNode {
        make::expr_assign(lhs, rhs).clone_for_update()
    }

    /// Create a while expression: `while cond do body`.
    pub fn while_expr(&self, cond: &str, body: &str) -> SyntaxNode {
        make::expr_while(cond, body).clone_for_update()
    }

    /// Create a foreach expression.
    pub fn foreach_expr(&self, var: &str, from: &str, to: &str, body: &str) -> SyntaxNode {
        make::expr_foreach(var, from, to, body).clone_for_update()
    }

    /// Create a return expression: `return e`.
    pub fn return_expr(&self, val: &str) -> SyntaxNode {
        make::expr_return(val).clone_for_update()
    }

    /// Create an assert expression: `assert(cond)`.
    pub fn assert_expr(&self, cond: &str) -> SyntaxNode {
        make::expr_assert(cond).clone_for_update()
    }

    /// Create a ref expression: `ref x`.
    pub fn ref_expr(&self, name: &str) -> SyntaxNode {
        make::expr_ref(name).clone_for_update()
    }

    /// Create an exit expression: `exit()`.
    pub fn exit_expr(&self) -> SyntaxNode {
        make::expr_exit().clone_for_update()
    }

    /// Create a throw expression: `throw e`.
    pub fn throw_expr(&self, val: &str) -> SyntaxNode {
        make::expr_throw(val).clone_for_update()
    }

    /// Create a try expression: `try body catch { pat => expr }`.
    pub fn try_expr(&self, body: &str, catch: &str) -> SyntaxNode {
        make::expr_try(body, catch).clone_for_update()
    }

    /// Create a sizeof expression: `sizeof(ty)`.
    pub fn sizeof_expr(&self, ty: &str) -> SyntaxNode {
        make::expr_sizeof(ty).clone_for_update()
    }

    /// Create a cast expression.
    pub fn cast_expr(&self, ty: &str, val: &str) -> SyntaxNode {
        make::expr_cast(ty, val).clone_for_update()
    }

    /// Create a var expression: `var x = e`.
    pub fn var_expr(&self, name: &str, init: &str) -> SyntaxNode {
        make::expr_var(name, init).clone_for_update()
    }

    /// Create a config expression: `config name`.
    pub fn config_expr(&self, name: &str) -> SyntaxNode {
        make::expr_config(name).clone_for_update()
    }

    /// Create a constraint expression: `constraint(text)`.
    pub fn constraint_expr(&self, text: &str) -> SyntaxNode {
        make::expr_constraint(text).clone_for_update()
    }

    /// Create a wildcard pattern `_`.
    pub fn wildcard_pat(&self) -> SyntaxNode {
        make::wildcard_pat().clone_for_update()
    }

    /// Create an identifier pattern.
    pub fn ident_pat(&self, name: &str) -> SyntaxNode {
        make::ident_pat(name).clone_for_update()
    }

    /// Create a tuple pattern.
    pub fn tuple_pat(&self, pats: &[&str]) -> SyntaxNode {
        make::tuple_pat(pats).clone_for_update()
    }

    /// Create a literal pattern.
    pub fn literal_pat(&self, text: &str) -> SyntaxNode {
        make::literal_pat(text).clone_for_update()
    }

    /// Create a constructor application pattern: `Some(x)`.
    pub fn app_pat(&self, name: &str, args: &[&str]) -> SyntaxNode {
        make::app_pat(name, args).clone_for_update()
    }

    /// Create an as-pattern: `pat as name`.
    pub fn as_pat(&self, inner: &str, name: &str) -> SyntaxNode {
        make::as_pat(inner, name).clone_for_update()
    }

    /// Create a vector pattern: `[|a, b, c|]`.
    pub fn vector_pat(&self, elems: &[&str]) -> SyntaxNode {
        make::vector_pat(elems).clone_for_update()
    }

    /// Create a list pattern: `[a, b]`.
    pub fn list_pat(&self, elems: &[&str]) -> SyntaxNode {
        make::list_pat(elems).clone_for_update()
    }

    /// Create a struct pattern: `struct { f = p, ... }`.
    pub fn struct_pat(&self, fields: &[(&str, &str)]) -> SyntaxNode {
        make::struct_pat(fields).clone_for_update()
    }

    /// Create a typed pattern: `(p : ty)`.
    pub fn typed_pat(&self, pat: &str, ty: &str) -> SyntaxNode {
        make::typed_pat(pat, ty).clone_for_update()
    }

    /// Create a binary pattern: `p @ q`, `p :: q`, `p | q`.
    pub fn bin_pat(&self, lhs: &str, op: &str, rhs: &str) -> SyntaxNode {
        make::bin_pat(lhs, op, rhs).clone_for_update()
    }

    /// Create a range index pattern: `name[hi .. lo]`.
    pub fn range_index_pat(&self, base: &str, hi: &str, lo: &str) -> SyntaxNode {
        make::range_index_pat(base, hi, lo).clone_for_update()
    }

    /// Create a type variable pattern: `'a`.
    pub fn tyvar_pat(&self, name: &str) -> SyntaxNode {
        make::tyvar_pat(name).clone_for_update()
    }

    /// Create a function definition.
    pub fn fn_def(
        &self,
        name: &str,
        params: &[(&str, &str)],
        ret_ty: Option<&str>,
        body: &str,
    ) -> SyntaxNode {
        make::fn_def(name, params, ret_ty, body).clone_for_update()
    }

    /// Create a val spec (type signature).
    pub fn val_spec(&self, name: &str, ty_text: &str) -> SyntaxNode {
        make::val_spec(name, ty_text).clone_for_update()
    }

    /// Create a match arm.
    pub fn match_arm(&self, pat: &str, guard: Option<&str>, body: &str) -> SyntaxNode {
        make::match_arm(pat, guard, body).clone_for_update()
    }

    /// Create a type alias: `type name = ty`.
    pub fn type_alias(&self, name: &str, ty: &str) -> SyntaxNode {
        make::type_alias(name, ty).clone_for_update()
    }

    /// Create an enum definition: `enum name = { V1, V2, ... }`.
    pub fn enum_def(&self, name: &str, variants: &[&str]) -> SyntaxNode {
        make::enum_def(name, variants).clone_for_update()
    }

    /// Create a struct definition: `struct name = { f1 : t1, ... }`.
    pub fn struct_def(&self, name: &str, fields: &[(&str, &str)]) -> SyntaxNode {
        make::struct_def(name, fields).clone_for_update()
    }

    /// Create a register definition: `register name : ty`.
    pub fn register_def(&self, name: &str, ty: &str) -> SyntaxNode {
        make::register_def(name, ty).clone_for_update()
    }

    /// Create a let definition: `let name : ty = value`.
    pub fn let_def(&self, name: &str, ty: Option<&str>, value: &str) -> SyntaxNode {
        make::let_def(name, ty, value).clone_for_update()
    }

    /// Create a param list node: `(p1 : t1, p2 : t2)`.
    pub fn param_list(&self, params: &[(&str, &str)]) -> SyntaxNode {
        make::param_list(params).clone_for_update()
    }

    /// Create an arg list node: `(a, b, c)`.
    pub fn arg_list(&self, args: &[&str]) -> SyntaxNode {
        make::arg_list(args).clone_for_update()
    }

    /// Create a field init node: `name = value`.
    pub fn field_init(&self, name: &str, value: &str) -> SyntaxNode {
        make::field_init(name, value).clone_for_update()
    }

    /// Create a quantifier: `forall 'a 'b.`.
    pub fn quantifier(&self, vars: &[&str]) -> SyntaxNode {
        make::quantifier(vars).clone_for_update()
    }

    /// Create a block item (statement inside a block).
    pub fn block_item(&self, text: &str) -> SyntaxNode {
        make::block_item(text).clone_for_update()
    }

    /// Create a unit expression `()`.
    pub fn expr_unit(&self) -> SyntaxNode {
        make::ext::expr_unit().clone_for_update()
    }

    /// Create a `0` literal.
    pub fn zero_number(&self) -> SyntaxNode {
        make::ext::zero_number().clone_for_update()
    }

    /// Create a `false` literal.
    pub fn default_bool(&self) -> SyntaxNode {
        make::ext::default_bool().clone_for_update()
    }

    /// Create a `true` literal expression.
    pub fn true_expr(&self) -> SyntaxNode {
        make::ext::true_expr().clone_for_update()
    }

    /// Create a `bitzero` literal expression.
    pub fn bitzero(&self) -> SyntaxNode {
        make::ext::bitzero().clone_for_update()
    }

    /// Create a `bitone` literal expression.
    pub fn bitone(&self) -> SyntaxNode {
        make::ext::bitone().clone_for_update()
    }

    /// Create an empty vector literal `[]`.
    pub fn empty_vector(&self) -> SyntaxNode {
        make::ext::empty_vector().clone_for_update()
    }
}
