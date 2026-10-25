//! Programs that upstream Sail accepts and sail-lsp once rejected.
//!
//! Every test here is a defect found by differential testing against
//! `sail --just-check` (the audit write-ups are in git history),
//! reduced to the smallest program that reproduces it. A fixture with no
//! `//^^^` annotation asserts "no diagnostics at all"; the few that do carry
//! one pin a diagnostic upstream also reports.
//!
//! These run the same `Analysis::file_diagnostics` the server calls, so they
//! cannot pass against a pipeline the server does not use.

use crate::fixture::check_diagnostics;

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

#[test]
fn infix_declaration_binds_tighter_than_concat() {
    // `infix 8 =^` in lib/vector.sail has to outrank `@` (level 5), or
    // `0 =^ 0x0 @ 128 =^ 0x0` groups as `0 =^ (0x0 @ 128) =^ 0x0`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <exception_basic.sail>
$include <flow.sail>
$include <vector_dec.sail>
function main (() : unit) -> unit = {
  assert(0=^0x0 @ 128=^0x0 == 128=^0x0);
}
"#,
    );
}

#[test]
fn a_locally_declared_fixity_is_honoured() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val myop : forall 'len 'v, 'len >= 0 & 'v >= 0. (int('len), bits('v)) -> bits('len)
infix 8 =%
overload operator =% = {myop}
function main (() : unit) -> unit = {
  assert(0=%0x0 @ 128=%0x0 == 128=%0x0);
}
"#,
    );
}

#[test]
fn two_caret_is_the_pow2_prefix_operator() {
    // `"2" wsc* "^"` lexes as `TwoCaret` upstream, not as an infix `^`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : int -> int
function f(n) = 2 ^ n
"#,
    );
}

#[test]
fn assignment_is_an_expression() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register R : int
val f : int -> unit
function f(n) = {
  var i : int = 0;
  let (_, j) = (i = i + 1, i);
  let _ = j;
  if n > 0 then R = 1 else R = 2;
  repeat i = i + 1 until i > 3;
  let v = n in R = v
}
"#,
    );
}

#[test]
fn a_match_arm_body_may_be_an_assignment() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register R : int
val f : int -> unit
function f(n) = match n {
  0 => R = 1,
  _ => R = 2
}
"#,
    );
}

#[test]
fn a_register_deref_is_an_atomic_expression() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register R : bits(16)
val main : unit -> unit
function main() = {
    let x = ref R;
    *x = 0xFFFF;
    print_bits("*x = ", *x)
}
"#,
    );
}

#[test]
fn a_structured_pragma_may_have_a_space_before_its_brace() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type my_pair = (range(0, 64), range(0, 64))
$c_override {
  id = my_pair,
  target = "my_pair_in_c"
}
val g : unit -> unit
"#,
    );
}

#[test]
fn an_attribute_payload_may_contain_a_list() {
    // The terminating `]` is the one matching `$[`, not the first one seen.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
$[sv_function { types = ["int", "int"], return_type = "int", dpi = true }]
val foo : (int, int) -> int
"#,
    );
}

#[test]
fn a_numeric_set_can_head_a_function_type() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val c : {4, 8} -> unit
function c('m) = ()
val run : unit -> unit
function run() = c(4)
"#,
    );
}

#[test]
fn an_unparenthesised_type_variable_is_a_parameter() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val a : {8, 16, 32} -> int
function a 'n = n
val run : unit -> int
function run() = a(8)
"#,
    );
}

// ---------------------------------------------------------------------------
// Preprocessor
// ---------------------------------------------------------------------------

#[test]
fn a_pragma_may_end_in_a_comment() {
    // A missed `$else` never flips its `$ifdef`, which drops *both* branches.
    check_diagnostics(
        r#"
//- /lib.sail
$ifdef NOT_DEFINED
val gone : unit -> unit
$else // the branch that is actually taken
val here : unit -> unit
$endif /* done */
//- /main.sail
default Order dec
$include "lib.sail"
val use_it : unit -> unit
function use_it() = here()
"#,
    );
}

#[test]
fn default_order_inc_is_read() {
    // The handler sat behind a name extraction, and `default Order inc` has
    // no identifier at all — every file was analysed as `dec`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order inc
$include <prelude.sail>
val f : bits(8) -> bits(3)
function f(x) = x[0 .. 2]
"#,
    );
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

#[test]
fn a_register_type_keeps_its_argument() {
    // `register(S)` used to lower to `S`, so it failed to unify with itself.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct S = { value : bits(32) }
register R : S
let X : vector(1, register(S)) = [ref R]
"#,
    );
}

#[test]
fn bitvector_is_a_spelling_of_bits() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val main : unit -> unit
function main () = {
  let x : bit = bitone;
  let y : bitvector(4, dec) = [x, bitone, bitzero, x];
  print_bits("y = ", y)
}
"#,
    );
}

#[test]
fn a_multi_field_vector_update_is_one_update_per_field() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
bitfield Cfg : bits(8) = { X : 0 .. 0, W : 1 .. 1, R : 2 .. 2 }
val f : Cfg -> Cfg
function f(cfg) = [cfg with X = 0b0, W = 0b0, R = 0b0]
"#,
    );
}

// ---------------------------------------------------------------------------
// Mappings
// ---------------------------------------------------------------------------

#[test]
fn a_bidirectional_val_declares_a_mapping() {
    // `eful_unpack(e1, e2)` supplies the components of the `(E, E)` side; as
    // a function it would look like a one-argument callee.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum E = EA | EB
struct eful = { e1 : E, e2 : E }
mapping e_pair_bits : (E, E) <-> bits(2) = {
  (EA, EA) <-> 0b00,
  (EA, EB) <-> 0b01,
  (EB, EA) <-> 0b10,
  (EB, EB) <-> 0b11
}
val eful_unpack : eful <-> (E, E)
mapping eful_unpack = { struct { e1, e2 } <-> (e1, e2) }
val f_bits : eful <-> bits(2)
mapping f_bits = { eful_unpack(e1, e2) <-> e_pair_bits(e1, e2) }
"#,
    );
}

#[test]
fn a_function_may_not_implement_a_mapping_val() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val foo : unit <-> int
  function foo() = 3
//^^^^^^^^^^^^^^^^^^ error: function does not have a function type
"#,
    );
}

#[test]
fn struct_field_punning_binds_on_both_sides() {
    // `struct { e1, e2 }` is `struct { e1 = e1, e2 = e2 }`; treating the
    // labels as non-binding reported every name on the other side.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct eful = { e1 : int, e2 : int }
val eful_unpack : eful <-> (int, int)
mapping eful_unpack = { struct { e1, e2 } <-> (e1, e2) }
"#,
    );
}

// ---------------------------------------------------------------------------
// Generated names
// ---------------------------------------------------------------------------

#[test]
fn undefined_builtins_need_no_declaration() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val foo : int -> (bool, int, bits(3))
function foo(n) = (undefined_bool(), undefined_int(), undefined_bitvector(3))
"#,
    );
}

#[test]
fn enum_with_clauses_generate_accessors() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum E with as_bits -> bits(16), as_string -> string = {
  A => (0xFFFF, "A"),
  B => (0x0001, "B")
}
function main() -> unit = print_bits("A = ", E_as_bits(A))
"#,
    );
}

#[test]
fn an_undefined_call_is_still_reported() {
    // The counterpart to the test above: registering the generated names is
    // what let the name-shape suppression go, so a typo must now surface.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum E with as_bits -> bits(16) = { A => 0xFFFF, B => 0x0001 }
function main() -> unit = print_bits("a = ", NO_SUCH_FN(A))
//                                           ^^^^^^^^^^ error: Function NO_SUCH_FN not found
"#,
    );
}

#[test]
fn enum_attributes_declare_their_generated_bindings() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
$[enum_number_conversions { to_enum = bar, from_enum = baz }]
enum Foo = A | B | C
val main : unit -> unit
function main() = {
  print_int("A = ", baz(A));
  match bar(0) {
    A => print_endline("ok"),
    _ => ()
  }
}
"#,
    );
}

// ---------------------------------------------------------------------------
// Cross-file
// ---------------------------------------------------------------------------

#[test]
fn a_bitfield_updater_resolves_across_files() {
    // `overload update_<field>` is generated beside the bitfield, so a caller
    // in another file only sees it if the cross-file merge synthesizes it.
    check_diagnostics(
        r#"
//- prelude
//- /bits.sail
bitfield Packet : bits(16) = { halt : 7 .. 0 }
//- /main.sail
default Order dec
$include <prelude.sail>
$include "bits.sail"
val f : Packet -> Packet
function f(p) = update_halt(p, 0x03)
"#,
    );
}

#[test]
fn an_outcome_declared_behind_a_commented_else_is_visible() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
$include <concurrency_interface.sail>
$include <concurrency_interface/exception.sail>
instantiation sail_take_exception with 'fault = int
"#,
    );
}

// ---------------------------------------------------------------------------
// IDE features, through the same entry points the server calls
// ---------------------------------------------------------------------------

#[test]
fn an_operand_does_not_inherit_the_expressions_expected_type() {
    // `expected_return` was an ambient field on `LocalEnv`, so the
    // ascribed width reached the operands of `@`: `zeros()` was offered
    // `bits(5)`, pinning `'n = 5` and typing the concat as `bits(6)`.
    check_diagnostics(
        r#"
default Order dec
val zeros : forall 'n. implicit('n) -> bits('n)
val main : unit -> unit
function main() = {
  let _x : bits(5) = zeros() @ 0b1;
  let _y : bits(5) = 0b1 @ zeros();
  ()
}
"#,
    );
}

#[test]
fn bits_and_bitvector_unify_their_widths() {
    // `bits` against `bitvector` returned `true` outright, so the width
    // arguments never met and `'n` stayed unbound at every call site.
    check_diagnostics(
        r#"
default Order dec
val f : forall 'n. bitvector('n, dec) -> bits('n)
val main : unit -> unit
function main() = {
  let _x : bits(4) = f(0xF);
  ()
}
"#,
    );
}

#[test]
fn an_inline_forall_binds_from_the_arguments() {
    // A `function` with an inline `forall` and typed parameters but no
    // `val` never bound `'n` from the arguments: the freshened parameter
    // `bits(?0)` met `bits(4)` in the mixed type/numeric arm of
    // `unify_ty_arg`, which accepted the pair without binding.
    check_diagnostics(
        r#"
default Order dec
function widths forall 'n, 0 <= 'n <= 64. (a : bits('n), b : bits('n)) -> bool =
  a == b
val main : unit -> unit
function main() = {
  let _ = widths(0x0, 0x1);
  ()
}
"#,
    );
}

#[test]
fn a_tuple_payload_may_be_destructured_by_the_constructor_pattern() {
    // `Some(ip, p)` binds the two halves of the pair that `Some : 'a ->
    // option('a)` carries as a single argument. Zipping the arguments
    // against the parameter list gave `ip` the whole tuple, so the call
    // below reported `expected bits(4), found (bits(4), bool)`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val take : bits(4) -> unit
val main : option((bits(4), bool)) -> unit
function main(o) = match o {
  Some(v, _) => take(v),
  None() => (),
}
"#,
    );
}

#[test]
fn a_mapping_call_resolves_a_direction() {
    // Upstream matches `E_app (m, xs) when Env.is_mapping m env` ahead of
    // the general application rules (type_check.ml:3756). sail-lsp checked
    // mappings only after the cross-file function lookup, and a mapping's
    // `val` also registers it as a cross-file function name, so every call
    // to a mapping declared elsewhere came back as `error()`.
    check_diagnostics(
        r#"
//- /lib.sail
default Order dec
mapping bool_bit : bool <-> bits(1) = { true <-> 0b1, false <-> 0b0 }
//- /main.sail
$include "lib.sail"
val take_bit : bits(1) -> unit
val take_bool : bool -> unit
val main : bool -> unit
function main(b) = {
  take_bit(bool_bit(b));
  take_bool(bool_bit(0b1));
}
"#,
    );
}

#[test]
fn a_mapping_clause_checks_each_side_in_its_own_scope() {
    // `is_wide` is bound as a `bool` by the left pattern and from a bit
    // slice by the right one. Sharing one scope let the bit binding win, so
    // the right-hand side read `bool_bit(bits(1))`, resolved the mapping
    // backwards and produced a `bool` for the concatenation. Upstream is
    // explicit that the two sides are checked in different environments
    // (type_check.ml:1673).
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
mapping bool_bit : bool <-> bits(1) = { true <-> 0b1, false <-> 0b0 }
union ast = { OP : (bits(5), bool) }
val encdec : ast <-> bits(8)
mapping encdec = { OP(rs, is_wide) <-> rs @ bool_bit(is_wide) @ 0b00 }
"#,
    );
}

#[test]
fn a_vector_update_keeps_the_vectors_type() {
    // `v[i] = e` desugars to `vector_update#(v, i, e)`, which returned
    // `error()` unconditionally. Upstream rewrites it to the
    // `vector_update` overload, or to a bitfield's update accessor
    // (type_check.ml:4011); both give back the base's type.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val take : bits(8) -> unit
val main : bits(8) -> unit
function main(v) = {
  take([v with 0 = bitzero]);
}
"#,
    );
}

#[test]
fn a_function_clause_binds_the_constructors_payload() {
    // `function clause execute OP(a, b)` declares the constructor's payload,
    // not `execute`'s parameter, and the head lowers to `[OP, a, b]`. Zipping
    // that against the scheme's one `instruction` parameter bound `OP` and
    // left the rest untyped — the largest single cause of untyped
    // identifiers in sail-riscv, where every instruction's operands are
    // declared this way.
    check_diagnostics(
        r#"
//- /lib.sail
default Order dec
scattered union instruction
val execute : instruction -> bool
scattered function execute
//- /main.sail
$include "lib.sail"
union clause instruction = OP : (bits(6), bits(5))
val take : bits(5) -> unit
function clause execute OP(funct6, rs) = {
  take(funct6);
//     ^^^^^^ error: bits(6) is not a subtype of bits(5)
  take(rs);
  true
}
"#,
    );
}

#[test]
fn a_let_binds_for_the_rest_of_the_block() {
    // The parser nests what follows a `let` inside the `LET_EXPR`, and
    // lowering pushed that tail as one `Statement::Expr`. `Expr::Let` pops
    // its scope when it is done, so every binding after the first vanished
    // for the rest of the block — in sail-riscv that was `SEW`, `LMUL_pow`,
    // `num_elem` and everything computed from them.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val take : bits(8) -> unit
val main : unit -> unit
function main() = {
  let a : bits(4) = 0x1;
  let b : bits(8) = 0x22;
  let c : bits(4) = 0x3;
  take(a);
//     ^ error: bits(4) is not a subtype of bits(8)
  take(b);
  take(c);
//     ^ error: bits(4) is not a subtype of bits(8)
}
"#,
    );
}

#[test]
fn one_returning_match_arm_does_not_end_the_block() {
    // `Expr::Match` never touched `self.diverges`, so a `return` in any arm
    // left the context marked as diverging and the block loop stopped
    // there. Everything after the match went uninferred.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val take : bits(8) -> unit
val main : bool -> unit
function main(c) = {
  match c {
    true  => return (),
    false => (),
  };
  let a : bits(4) = 0x1;
  take(a)
//     ^ error: bits(4) is not a subtype of bits(8)
}
"#,
    );
}

#[test]
fn a_quantifier_under_a_type_constructor_binds() {
    // `unify_structural` accepted an inference variable in expected position
    // without binding it, so `forall ('a : Type). register('a) -> 'a`
    // returned an unresolved variable however concrete the argument was.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val read_register : forall ('a : Type). register('a) -> 'a
val take : bits(8) -> unit
register A : bits(32)
val main : unit -> unit
function main() = {
  take(read_register(ref A))
//     ^^^^^^^^^^^^^^^^^^^^ error: bits(32) is not a subtype of bits(8)
}
"#,
    );
}

#[test]
fn a_let_initializer_has_no_expectation() {
    // `expected_return` is ambient on `LocalEnv`, so the enclosing
    // function's return type reached the initializer of an unannotated
    // `let` and bound the call's result variable to it.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val read_register : forall ('a : Type). register('a) -> 'a
val take : bits(8) -> unit
register A : bits(32)
val main : unit -> unit
function main() = {
  let c = read_register(ref A);
  take(c)
//     ^ error: bits(32) is not a subtype of bits(8)
}
"#,
    );
}

#[test]
fn an_ascription_gives_the_expression_its_type() {
    // `Expr::Cast` carried the span of the whole `e : T`, not of `T`, so
    // reading the ascription back produced the expression's own text. It
    // now types the ascription the way a `let` annotation is typed.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val take : bits(8) -> unit
val main : unit -> unit
function main() = {
  take(0x1 : bits(4))
//     ^^^^^^^^^^^^^ error: bits(4) is not a subtype of bits(8)
}
"#,
    );
}

#[test]
fn the_location_builtins_have_types() {
    // Upstream rewrites `__FILE__` and `__LINE__` to literals while building
    // the AST (initial_check.ml:1417), so they never reach the checker as
    // identifiers.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val take : bits(8) -> unit
val main : unit -> unit
function main() = {
  take(__FILE__);
//     ^^^^^^^^ error: Type mismatch between bits(8) and string
  take(__LINE__)
//     ^^^^^^^^ error: Type mismatch between bits(8) and int
}
"#,
    );
}

#[test]
fn register_is_also_a_type_constructor() {
    // `register` starts a definition, so the definition-boundary scan cut
    // `function get_X() -> register(bits(4))` in two at the return type and
    // reported the rest of the file as stray definitions.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
register X : bits(4)
val get_X : unit -> register(bits(4))
function get_X() = ref X
"#,
    );
}

#[test]
fn a_zero_argument_accessor_call_parses() {
    // The lexer folds an empty argument list into one `UNIT` token, so the
    // deprecated accessor form `sysreg.bits()` left the `()` behind — inside
    // a match it started the next arm.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
bitfield Sysreg : bits(32) = { A : 5, B : 4 }
register sysreg : Sysreg
val f : bits(2) -> bits(32)
function f(code) = match code {
  0b00 => sysreg.bits(),
  _ => sysreg.bits
}
"#,
    );
}

#[test]
fn an_outcome_body_may_contain_impl_clauses() {
    // `impl funcl` is an outcome implementation (parser.mly:1401). Without
    // it the `{` of the implementation body was reported as a stray block.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
outcome foo : bits(32) -> unit with 'is_32 = {
  impl emulator(bv) = {
    print_endline("32")
  }
}
"#,
    );
}

#[test]
fn a_record_reached_through_an_alias_has_fields() {
    // `type Foo32 = Foo(32)` names a record through an alias, and the field
    // lookup matched on the type's own name.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct Foo('n) = { field : bits('n) }
type Foo32 = Foo(32)
val take : bits(32) -> unit
val bar : Foo32 -> unit
function bar(foo) = take(foo.field)
"#,
    );
}

#[test]
fn a_concatenation_pattern_annotates_its_parts() {
    // In `function test 0b0 @ _ : bits(5) @ 0b1 = ..` the `bits(5)` types
    // the middle slice, not the parameter. Comparing it against the `val`
    // reported a mismatch that is not one.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val test : bits(7) -> unit
function test 0b0 @ _ : bits(5) @ 0b1 = ()
"#,
    );
}

#[test]
fn a_unit_parameter_still_counts() {
    // `()` is a single token rather than a node, so `(() : unit, i : int)`
    // yielded one parameter instead of two and `i` was matched against the
    // first declared type.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val test : (unit, int) -> int
function test(() : unit, i : int) = i
"#,
    );
}

#[test]
fn a_local_constructor_shadows_an_imported_one() {
    // The prelude declares `Ok` for `result`. A file that declares its own
    // union with an `Ok` variant means that one.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union U('a : Type) = { Err : 'a, Ok : bits(16) }
val main : unit -> unit
function main() = {
  let _ : U(unit) = Ok(0xFFFF);
  ()
}
"#,
    );
}

#[test]
fn a_nested_pattern_parameter_does_not_shift_the_others() {
    // `((instr :: rest), i)` lowers to three patterns against two declared
    // types. Zipping them positionally gave `instr` the list and `rest` the
    // `int`, and both showed up as mismatches elsewhere.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val list_index : forall('n : Int), ('n >= 1). (list(bits('n)), int) -> bits('n)
function list_index ((instr :: rest), i) =
  match i {
    0 => instr,
    _ => list_index(rest, i - 1)
  }
"#,
    );
}

#[test]
fn an_annotated_pattern_binds_at_its_annotation() {
    // `Pat::Typed` forwarded whatever the context offered and dropped its
    // own annotation. They differ wherever a pattern annotates a *part*:
    // the parameter below is `bits(8)` while each named slice is narrower,
    // so `splitting` was bound at the full width.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val take4 : bits(4) -> unit
val take2 : bits(2) -> unit
val decode : bits(8) -> unit
scattered function decode
function clause decode splitting : bits(4) @ vop : bits(2) @ rest : bits(2) = {
  take4(splitting);
  take2(vop);
  take2(splitting)
//      ^^^^^^^^^ error: bits(4) is not a subtype of bits(2)
}
"#,
    );
}

#[test]
fn a_clause_does_not_decide_a_vals_arity() {
    // `val f : ((string, string)) -> string` takes one tuple and
    // `function f(s1, s2)` destructures it. The clause's parameter count was
    // used to flatten the declared tuple, so every call site expected two
    // arguments.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val tuple_of_tuple : ((string, string)) -> string
function tuple_of_tuple(s1, s2) = s1
val use_it : string -> string
function use_it(s) = tuple_of_tuple((s, s))
"#,
    );
}

#[test]
fn a_mapping_clause_is_checked_against_its_declared_type() {
    // Both sides of a bidirectional clause were inferred with no
    // expectation, so a clause could produce anything at all.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val m : bits(2) <-> bits(17)
mapping m = { 0b00 <-> 0xFFFF }
//                     ^^^^^^ error: bits(16) is not a subtype of bits(17)
"#,
    );
}

#[test]
fn a_mapping_patterns_tuple_payload_is_destructured() {
    // `hex_bits(2, s)` against `bits('n) <-> (int('n), string)` binds `s` to
    // the string, not to the whole pair — the same rule a constructor
    // pattern follows.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val hex_bits : forall 'n. bits('n) <-> (int('n), string)
mapping hex_bits_2 : bits(2) <-> string = { hex_bits(2, s) <-> s }
"#,
    );
}

#[test]
fn a_synthesized_constructor_checks_its_argument() {
    // The path that lets a `union clause`'s payload come from another file
    // returned the result type without looking at the arguments, so
    // `Mk_B1` — whose signature sail-lsp synthesizes itself and therefore
    // knows exactly — was never checked.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
bitfield B1 : bits(33) = { Field0 : 15..12 }
val main : unit -> unit
function main() = {
  let _ = Mk_B1(0x12345678);
//              ^^^^^^^^^^ error: bits(32) is not a subtype of bits(33)
  ()
}
"#,
    );
}

#[test]
fn a_string_concatenation_pattern_types_its_parts() {
    // `^` is the spelling sail-riscv's `assembly` clauses use. Without it a
    // mapping pattern inside one had nothing to resolve against and bound
    // its argument to the wrong side of the mapping.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { C_NOP : bits(6) }
val hex6 : bits(6) <-> string
val assembly : ast <-> string
mapping clause assembly = C_NOP(imm) <-> "c.nop" ^ hex6(imm)
"#,
    );
}

#[test]
fn an_enum_clause_arm_must_produce_the_declared_type() {
    // The arms of `enum E with f -> T = { M => v }` are part of a type
    // definition, not a function body, so inference never visits them and
    // nothing checked them at all.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
enum F with as_bits -> bits(17) = {
  A2 => 0xFFFF,
//      ^^^^^^ error: bits(16) is not a subtype of bits(17)
  B2 => 0b101
//      ^^^^^ error: bits(3) is not a subtype of bits(17)
}
"#,
    );
}

#[test]
fn a_register_initialiser_is_checked() {
    // Only a top-level `let` produced a body, so nothing ever looked at the
    // right-hand side of a `register` or `var` declaration.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
register R : bits(17) = 0xFFFF
//                      ^^^^^^ error: bits(16) is not a subtype of bits(17)
"#,
    );
}

#[test]
fn a_numeric_range_annotation_is_checked() {
    // Three layers each threw the bounds away: a `range` return type was
    // freshened to a bare inference variable at every call site, a `range`
    // annotation was excluded from being an expected type at all, and the
    // coercion fallback accepted whatever survived. `is_subtype` had known
    // how to decide this the whole time.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val test : bool -> range(0, 1)
let v2 : range(0, 0) = test(true)
//                     ^^^^^^^^^^ error: range(0, 1) is not a subtype of range(0, 0)
"#,
    );
}

#[test]
fn a_numeric_set_keeps_its_bounds() {
    // `{0, 1}` lowered to a bare `int`. It is `range(0, 1)`; a
    // non-contiguous set like `{8, 16, 32}` widens to `range(8, 32)`, which
    // can only accept too much, never report too much.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val test : bool -> {0, 1}
let v2 : range(0, 0) = test(true)
//                     ^^^^^^^^^^ error: range(0, 1) is not a subtype of range(0, 0)
"#,
    );
}

#[test]
fn a_range_annotation_still_accepts_a_literal() {
    // An integer literal is typed `int` here where upstream gives it the
    // singleton `atom(3)`, so the strictness above has to stop at a bare
    // `int` actual or this becomes a false positive.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
let x : range(0, 7) = 3
"#,
    );
}

#[test]
fn a_literal_outside_its_annotated_range_is_reported() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
let x : range(0, 7) = 9
//                    ^ error: Type mismatch between range(0, 7) and atom(9)
"#,
    );
}

#[test]
fn an_ascription_is_checked_against_what_it_ascribes() {
    // `e : T` took `T` as the answer without comparing it to `e`, so the
    // claim was never a claim about anything.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val test : unit -> int
function test () = {
  y : nat = 10 : range(0, 9);
//          ^^ error: Type mismatch between range(0, 9) and atom(10)
  y : int
}
"#,
    );
}

#[test]
fn a_vector_pattern_must_be_as_wide_as_what_it_matches() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(5) -> unit
function f(e) = {
  match e {
    [_,_,_,bitzero] => (),
//  ^^^^^^^^^^^^^^^ error: bits(4) is not a subtype of bits(5)
    _ => ()
  }
}
"#,
    );
}

#[test]
fn square_brackets_are_a_vector_and_bars_are_a_list() {
    // `[a, b]` is `P_vector` and `[|a, b|]` is `P_list` upstream
    // (parser.mly:558). They were the other way round here.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : list(int) -> unit
function f(l) = match l {
  [|1, 2|] => (),
  _ => ()
}
"#,
    );
}

#[test]
fn a_concat_patterns_parts_must_add_up() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(16) -> unit
function f(x) = match x {
  0x00 @ 0b000 @ _ : bits(2) @ 0x0 => (),
//^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: bits(17) is not a subtype of bits(16)
  _ => ()
}
"#,
    );
}

#[test]
fn a_concat_pattern_with_an_open_part_is_left_alone() {
    // `rest` states no width of its own, so the equation cannot be checked
    // and must not be guessed at.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(16) -> unit
function f(x) = match x {
  0x0 @ rest => print_endline(bits_str(rest)),
  _ => ()
}
"#,
    );
}

#[test]
fn an_assignment_is_checked_against_its_target() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register reg : range(0, 9)
val test : unit -> unit
function test () = { reg = 10 }
//                         ^^ error: Type mismatch between range(0, 9) and atom(10)
"#,
    );
}

#[test]
fn a_register_reference_dereferences_to_its_referent() {
    // `*r` typed as `error()`, so an assignment through one had no target
    // type and checked nothing.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val modify : register(range(0, 9)) -> unit
function modify (r) = { (*r) = 10 }
//                             ^^ error: Type mismatch between range(0, 9) and atom(10)
"#,
    );
}

#[test]
fn a_match_arm_is_checked_against_the_expected_type() {
    // The expectation flowed into each arm but was never checked against it.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val test : bool -> range(0, 0)
function test(b) = match b {
  true => 0,
  false => 1
//         ^ error: Type mismatch between range(0, 0) and atom(1)
}
"#,
    );
}

#[test]
fn a_width_variable_is_shared_across_its_occurrences() {
    // `bits('n)` matched a width once, bound it, and then accepted anything
    // at the next occurrence — the whole point of the quantifier.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val g : forall 'n. (bits('n), bits('n)) -> unit
val f : unit -> unit
function f() = g(0xFF, 0xFFFF)
//                     ^^^^^^ error: bits(16) is not a subtype of bits(_)
"#,
    );
}

#[test]
fn a_width_variable_that_agrees_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val g : forall 'n. (bits('n), bits('n)) -> unit
val f : unit -> unit
function f() = g(0xFF, 0xEE)
"#,
    );
}

#[test]
fn a_cross_file_call_binds_its_width_from_the_argument() {
    // Only the return type was instantiated, so `sail_zeros`' `'n` had a
    // variable nothing bound and the call produced a bitvector of unknown
    // width that agreed with any declared width at all.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : unit -> bits(9)
function f() = sail_zeros(8)
//             ^^^^^^^^^^^^^ error: bits(8) is not a subtype of bits(9)
"#,
    );
}

#[test]
fn a_literal_argument_still_has_to_fit_a_non_numeric_parameter() {
    // The singleton is used only where the parameter is numeric and open.
    // Passing `atom(1)` everywhere made `f(1)` fit a `bool` parameter.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bool -> unit
function f(x) = ()
val g : unit -> unit
function g() = f(1)
//               ^ error: Type mismatch between bool and atom(1)
"#,
    );
}

#[test]
fn an_anonymous_struct_literal_gets_its_record_from_the_context() {
    // `struct { .. }` names no record, so nothing could look up what its
    // field values should be. The ascription says which record it is.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct foo = { field1 : bits(33), field2 : int }
val main : unit -> unit
function main() = {
  let _ = struct { field1 = 0xFFFF_ABCD, field2 = 3 } : foo;
//                          ^^^^^^^^^^^ error: bits(32) is not a subtype of bits(33)
  ()
}
"#,
    );
}

#[test]
fn an_anonymous_struct_literal_does_not_claim_missing_fields() {
    // The record is inherited from context, and that context is not always
    // the one the literal is building — sail-riscv has a write request whose
    // expectation reads `Mem_read_request`. Values given are checked; values
    // not given are not accused.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct foo = { field1 : bits(32), field2 : int }
val take : foo -> unit
val main : unit -> unit
function main() = take(struct { field1 = 0xFFFF_ABCD })
"#,
    );
}

#[test]
fn a_comparison_compares_its_operands() {
    // The result of `==` is `bool` whatever the operands are, and nothing
    // had ever looked at them. Upstream reaches it through
    // `eq_bits : forall 'n. (bits('n), bits('n)) -> bool`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(33) -> bool
function f(v) = v == 0x00000000
//                   ^^^^^^^^^^ error: bits(32) is not a subtype of bits(33)
"#,
    );
}

#[test]
fn a_comparison_of_call_results_is_left_alone() {
    // Overload resolution can pick the wrong candidate — two bitfields
    // sharing a field name give `update_Field0(b2, ..).bits` the other one's
    // width — and a comparison is not where that should surface.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
bitfield B1 : bits(32) = { Field0: 31 .. 16 }
bitfield B2 : bits(16) = { Field0: 15 .. 12 }
function main() -> unit = {
  b2 : B2 = Mk_B2(0xabcd);
  assert(update_Field0(b2, 0x1).bits == 0x1bc2);
}
"#,
    );
}

#[test]
fn a_literal_parameter_pattern_is_checked() {
    // `0x0` sits in the parameter list as a bare token, not a node, and both
    // collection paths looked for nodes — so the parameter list came out
    // empty and the literal had nothing to be checked against.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(5) -> int
function f (0x0) = 0
//          ^^^ error: bits(4) is not a subtype of bits(5)
"#,
    );
}

#[test]
fn a_concatenation_parameter_is_one_parameter() {
    // The literal at the end of `a : bits(4) @ .. @ 0b0000110` is part of one
    // parameter, not a parameter — taking it as its own compares `bits(16)`
    // against `bits(7)`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val decode : bits(16) -> int
function decode(a : bits(4) @ b : bits(1) @ c : bits(4) @ 0b0000110) = 1
"#,
    );
}

#[test]
fn arithmetic_on_a_range_narrows_the_result() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val branch : range(0, 31) -> range(0, 62)
function branch x = x + 32
//                  ^^^^^^ error: constraint cannot hold: 0 <= 'x + 32 & 'x + 32 <= 62
"#,
    );
}

#[test]
fn a_comparison_narrows_both_branches() {
    // `if x > 31` says `x >= 32` on one side and `x <= 31` on the other. The
    // else side learnt nothing, and the `>` rule existed only for `x > 0`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val branch : range(0, 63) -> range(0, 63)
function branch x = if x > 31 then x - 32 else x + 32
"#,
    );
}

#[test]
fn a_narrowed_range_still_has_to_fit_the_return_type() {
    // Reported at the branch that does not fit, which is where upstream
    // reports it (`a.sail:4.35-41`, `x + 32`). It used to be reported
    // against the whole `if`, because only the join was checked.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val branch : range(0, 63) -> range(0, 62)
function branch x = if x < 32 then x + 32 else x - 32
//                                 ^^^^^^ error: constraint cannot hold: 0 <= 'x + 32 & 'x + 32 <= 62
"#,
    );
}

#[test]
fn an_if_produces_the_union_of_its_branches() {
    // The then branch alone said `range(0, 31)`, which fits `range(0, 62)`.
    // The `if` produces either value, so its range is the union — and with
    // each branch checked in its own right, the one that overflows says so
    // itself. Upstream reports `x + 32` here too (`a.sail:4.47-53`).
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val branch : range(0, 63) -> range(0, 62)
function branch x = if x > 31 then x - 32 else x + 32
//                                             ^^^^^^ error: constraint cannot hold: 0 <= 'x + 32 & 'x + 32 <= 62
"#,
    );
}

#[test]
fn a_bounded_existential_is_a_range() {
    // `{'n, 0 <= 'n & 'n <= 4. atom('n)}` *is* `range(0, 4)`. Left as an
    // existential it was opaque to everything numeric.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
let B : {'n, 0 <= 'n & 'n <= 4. atom('n)} = 9
//                                          ^ error: Type mismatch between range(0, 4) and atom(9)
"#,
    );
}

#[test]
fn an_unbounded_existential_stays_one() {
    // Only a literal lower *and* upper bound reduce. A witness this cannot
    // see is not a range.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
let B : {'n, 0 <= 'n. atom('n)} = 9
"#,
    );
}

#[test]
fn an_overload_binds_its_cross_file_members_width() {
    // `zeros(64)` resolves to `sail_zeros : forall 'n. int('n) -> bits('n)`,
    // which lives in the prelude. The overload path instantiated only its
    // return type, so the call produced a bitvector of unknown width that
    // agreed with any declared width at all.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val zeros_implicit : forall 'n, 'n >= 0. implicit('n) -> bits('n)
function zeros_implicit (n) = sail_zeros(n)
overload zeros = {sail_zeros, zeros_implicit}
val main : unit -> unit
function main() = { let _ : bits(65) = zeros(64); () }
//                                     ^^^^^^^^^ error: bits(64) is not a subtype of bits(65)
"#,
    );
}

#[test]
fn a_width_equation_with_no_solution_is_reported() {
    // `bits(8 * 'n)` cannot be `bits(65)` for any integer `'n`. The call has
    // no arguments, so `'n` is fixed by the type the caller asked for and by
    // nothing else.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val foo : forall 'n, 'n >= 0. implicit('n) -> bits(8 * 'n)
val f : unit -> unit
function f() = { let w: bits(65) = foo(); print_bits("", w) }
//                                 ^^^^^ error: bits(8 * 'n) is not a subtype of bits(65)
"#,
    );
}

#[test]
fn a_width_equation_with_a_solution_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val foo : forall 'n, 'n >= 0. implicit('n) -> bits(8 * 'n)
val f : unit -> unit
function f() = { let w: bits(64) = foo(); print_bits("", w) }
"#,
    );
}

#[test]
fn a_constraint_proved_false_is_an_error() {
    // `'m == 8 * 'n` with `'m` fixed at 65 by the return type has no integer
    // solution. A constraint mentioning an unbound quantifier used to be
    // filed as "unresolved" without being read.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val bar : forall 'n 'm, 'n >= 0 & 'm == 8 * 'n. implicit('n) -> bits('m)
val f : unit -> unit
function f() = { let v: bits(65) = bar(); print_bits("", v) }
//                                 ^^^ error: constraint cannot hold: 'n >= 0 & 'm == 8 * 'n
"#,
    );
}

#[test]
fn a_constraint_with_a_solution_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val bar : forall 'n 'm, 'n >= 0 & 'm == 8 * 'n. implicit('n) -> bits('m)
val f : unit -> unit
function f() = { let v: bits(64) = bar(); print_bits("", v) }
"#,
    );
}

#[test]
fn a_definition_after_an_outcome_is_still_a_definition() {
    // `outcome o = { .. }` swallowed everything after it: the inner `val`'s
    // end was computed against the whole file and ran past the block's `}`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$sail_internal
$include <prelude.sail>
outcome o : bits(4) -> unit
with
  'a: Type
= {
    val to_bits : 'a -> bits(64)
}
val after : unit -> bits(8)
function after() = 0xFFFF
//                 ^^^^^^ error: bits(16) is not a subtype of bits(8)
"#,
    );
}

#[test]
fn a_struct_field_name_is_not_an_integer() {
    // Only a *bitfield* field name indexes: `v[SSI]` reads bit SSI. Answering
    // `int` for struct fields too made every binding sharing a name with some
    // struct field anywhere in the workspace an integer.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct Request = { value : bool }
val f : bits(32) -> bits(64)
function f(value) = value @ value
"#,
    );
}

#[test]
fn an_instantiation_must_match_the_outcome() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$sail_internal
$include <prelude.sail>
outcome o1 : bits(4) -> unit
with
  'a: Type
= {
    val to_bits : 'a -> bits(65)
}
val inst : bits(32) -> bits(64)
function inst(x) = sail_zero_extend(x, 64)
  instantiation o1 with 'a = bits(32), to_bits = inst
//^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: `inst` has type `bits(32) -> bits(64)`, but `o1` declares `to_bits : bits(32) -> bits(65)`
"#,
    );
}

#[test]
fn a_matching_instantiation_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$sail_internal
$include <prelude.sail>
outcome o1 : bits(4) -> unit
with
  'a: Type
= {
    val to_bits : 'a -> bits(64)
}
val inst : bits(32) -> bits(64)
function inst(x) = sail_zero_extend(x, 64)
instantiation o1 with
  'a = bits(32),
  to_bits = inst
"#,
    );
}

#[test]
fn a_constructors_tuple_payload_is_checked() {
    // `Ctor(x, y, z)` for `Ctor : (A, B, C)` is `Ctor((x, y, z))` — one
    // tuple parameter against three arguments. The counts disagreed, so the
    // check did nothing at all.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { Ctor1 : (nat, int, bits(5)) }
val f : bits(4) -> ast
function f(c) = Ctor1(0, 32, c)
//                           ^ error: bits(4) is not a subtype of bits(5)
"#,
    );
}

#[test]
fn an_existential_ctor_payload_is_opened_first() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type datasize('n: Int) -> Bool = 'n in {32, 64}
union ast = { Ctor1 : {'d, datasize('d). (nat, int('d), bits(5))} }
val f : bits(4) -> ast
function f(c) = Ctor1(0, 32, c)
//                           ^ error: bits(4) is not a subtype of bits(5)
"#,
    );
}

#[test]
fn a_mapping_clauses_constructor_arguments_are_left_alone() {
    // The two sides of a clause are checked in separate scopes on purpose:
    // `AMO(op, aq, ..)` binds `aq : bool` from the constructor on one side
    // and from a bit slice on the other. A disagreement there is the design.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { AMO : (bool, bits(5)) }
val bool_bit : bool <-> bits(1)
val encdec : ast <-> bits(6)
mapping encdec = AMO(aq, rs) <-> bool_bit(aq) @ rs
"#,
    );
}

#[test]
fn a_function_clause_guard_is_checked() {
    // A clause's guard lives inside the parameter list as bare tokens after
    // `if`. Nothing lowered it, so no type error inside one was visible —
    // and its tokens were collected as parameters, which left the pattern
    // they guard unbound.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union foo = { A : bits(33), B : int }
val f : foo -> bool
function f (A(v) if v == 0x00000000) = true
//                       ^^^^^^^^^^ error: bits(32) is not a subtype of bits(33)
and f _ = false
"#,
    );
}

#[test]
fn a_correct_guard_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union foo = { A : bits(32), B : int }
val f : foo -> bool
function f (A(v) if v == 0x00000000) = true
and f _ = false
"#,
    );
}

#[test]
fn a_guarded_mapping_clause_is_checked() {
    // `I(imm) if e == 0b0 <-> ..` never reaches the parser as a `<->`
    // expression: the body node covers `I(imm)` alone and the rest sits
    // outside it, so the clause had no arms and nothing in it was checked.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register enabled : bits(1)
union ast = { I: bits(2) }
val encdec : ast <-> bits(2)
scattered mapping encdec
mapping clause encdec = I(imm) if enabled == 0b0 <-> 0b0 @ imm if enabled == 0b0
//                                                         ^^^ error: bits(1) is not a subtype of bits(2)
end encdec
"#,
    );
}

#[test]
fn a_correct_guarded_mapping_clause_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register enabled : bits(1)
union ast = { I: bits(1) }
val encdec : ast <-> bits(2)
scattered mapping encdec
mapping clause encdec = I(imm) if enabled == 0b0 <-> 0b0 @ imm if enabled == 0b0
end encdec
"#,
    );
}

#[test]
fn a_parenthesised_concat_part_still_counts() {
    // `(_ : bits(3))` arrives as a one-element tuple, and returning no width
    // for it made the whole concatenation unmeasurable.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(4) -> unit
function f(e) = match e { [_,bitzero] @ (_ : bits(3)) => (), _ => () }
//                        ^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: bits(5) is not a subtype of bits(4)
"#,
    );
}

#[test]
fn a_clause_parameter_without_parentheses_is_checked() {
    // `function clause decode <pat> = ..` leaves the parser with no
    // `PARAM_LIST` at all — the pattern collapses to whichever node it
    // started with — so the clause was lowered with zero parameters and
    // nothing about the pattern was checked.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val decode : bitvector(16, dec) -> unit
scattered function decode
function clause decode 0x00 @ 0b000 @ _ : bits(2) @ 0x0 as op_code = ()
//                     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: Type mismatch between bitvector(16) and bits(17)
end decode
"#,
    );
}

#[test]
fn a_correct_clause_parameter_without_parentheses_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val decode : bitvector(16, dec) -> unit
scattered function decode
function clause decode 0x00 @ 0b000 @ _ : bits(1) @ 0x0 as op_code = ()
end decode
"#,
    );
}

#[test]
fn a_concat_width_adds_up_symbolically() {
    // `_ : bits('n) @ _ : bits(64 - 'n)` totals 64 — concrete even though
    // neither part is. The parts are summed as expressions and the sum is
    // evaluated after like terms cancel.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val bar : unit -> {'n, 16 <= 'n <= 48. int('n)}
val foo : bits(65) -> unit
function foo(xs) = {
    let 'n = bar();
    assert('n == 32);
    match xs {
        _ : bits('n) @ _ : bits(64 - 'n) => (),
//      ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: bits(64) is not a subtype of bits(65)
    }
}
"#,
    );
}

#[test]
fn a_symbolic_concat_that_fits_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val bar : unit -> {'n, 16 <= 'n <= 48. int('n)}
val foo : bits(64) -> unit
function foo(xs) = {
    let 'n = bar();
    assert('n == 32);
    match xs {
        _ : bits('n) @ _ : bits(64 - 'n) => (),
    }
}
"#,
    );
}

#[test]
fn a_parenthesised_concat_parameter_is_one_parameter() {
    // `decode(a : bits(5) @ b : bits(1) @ 0b0000011111)` came out as two
    // parameters against one declared type: the list is flat tokens, and the
    // node pass returned the typed pieces separately, losing the `@` and any
    // way to check the widths add up.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val decode : bits(17) -> unit
function clause decode(a : bits(5) @ b : bits(1) @ 0b0000011111) = ()
//                     ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: bits(16) is not a subtype of bits(17)
"#,
    );
}

#[test]
fn a_correct_parenthesised_concat_parameter_is_accepted() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val decode : bits(16) -> unit
function clause decode(a : bits(5) @ b : bits(1) @ 0b0000011111) = ()
"#,
    );
}

#[test]
fn an_unannotated_struct_reports_an_error_but_recovers_its_fields() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct S = { a : bits(4) }
function g() -> bits(8) = {
  let req = struct { a = 0x1 };
//          ^^^^^^^^^^^^^^^^^^ error: Cannot infer type of struct literal
  let z : bits(8) = req.a;
//                  ^^^^^ error: bits(4) is not a subtype of bits(8)
  z
}
"#,
    );
}

#[test]
fn a_match_sees_through_a_type_alias() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct P ('v : Int) = { a : bits(4), b : bool }
type Res ('v : Int) = result((P('v), int), (bool, int))
function g(r : Res(8)) -> bool = {
  match r {
    Ok(struct {a, b}, n) => { let z : bool = a; let _ = b; let _ = n; z }
//                                           ^ error: Type mismatch between bool and bits(4)
    , Err(_, _) => false
  }
}
"#,
    );
}

#[test]
fn a_mapping_call_runs_in_the_direction_its_argument_asks_for() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum W = { A, B }
mapping m : W <-> bits(2) = { A <-> 0b00, B <-> 0b11 }
function h() -> bool = {
  let x : bool = m(0b11);
//               ^^^^^^^ error: Type mismatch between bool and W
  x
}
"#,
    );
}

#[test]
fn a_mapping_call_argument_is_checked() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val bin_bits_2 : bits(3) <-> string
val f : bits(2) -> string
function f(x) = bin_bits_2(x)
//              ^^^^^^^^^^ error: no matching overload for `bin_bits_2`
"#,
    );
}

mod ide_features {
    use crate::fixture::{check_goto_definition, check_hover_contains};

    #[test]
    fn goto_definition_lands_on_the_name() {
        // The scope-aware path runs first and now narrows `focus_range` to
        // the name, so the cursor arrives on `helper`, not on `function`.
        check_goto_definition(
            r#"
default Order dec
val helper : int -> int
function helper(x) = x
//       ^^^^^^
val main : unit -> int
function main() = help$0er(1)
"#,
        );
    }

    #[test]
    fn goto_definition_reaches_a_struct_field() {
        // Fields are not top-level items, so the symbol index — built by
        // walking `top_level_items()` — had nothing under `variety`, and
        // every field name in a struct literal resolved to nothing.
        check_goto_definition(
            r#"
default Order dec
enum Access_variety = { AV_plain, AV_exclusive }
struct Access = {
  variety : Access_variety,
//^^^^^^^
  count : int
}
val f : unit -> Access
function f() = struct { vari$0ety = AV_plain, count = 0 }
"#,
        );
    }

    #[test]
    fn goto_definition_reaches_a_bitfield_field() {
        check_goto_definition(
            r#"
default Order dec
bitfield Interrupts : bits(8) = {
  SSI : 1,
//^^^
  STI : 5
}
val f : Interrupts -> bits(1)
function f(v) = v[S$0SI]
"#,
        );
    }

    #[test]
    fn goto_definition_reaches_a_union_member() {
        check_goto_definition(
            r#"
default Order dec
union ExceptionType = {
  E_Fetch_Addr_Align : unit,
  E_SAMO_Addr_Align : unit
//^^^^^^^^^^^^^^^^^
}
val f : unit -> ExceptionType
function f() = E_SAMO_Addr_$0Align()
"#,
        );
    }

    #[test]
    fn goto_definition_reaches_a_scattered_enum_member() {
        // The clause is indexed under the scattered type's name
        // (`extension`), and every use site writes the member's name.
        check_goto_definition(
            r#"
default Order dec
scattered enum extension
enum clause extension = Ext_S
//                      ^^^^^
val f : unit -> extension
function f() = Ext$0_S
"#,
        );
    }

    #[test]
    fn goto_definition_crosses_files() {
        // Cross-file goes through the workspace symbol index, which now
        // carries a name span of its own, so the cursor lands on the name
        // here too.
        check_goto_definition(
            r#"
//- /lib.sail
val helper : int -> int
function helper(x) = x
//       ^^^^^^
//- /main.sail
default Order dec
$include "lib.sail"
val main : unit -> int
function main() = help$0er(1)
"#,
        );
    }

    #[test]
    fn hover_shows_a_signature() {
        check_hover_contains(
            r#"
default Order dec
val helper : int -> int
function helper(x) = x
val main : unit -> int
function main() = help$0er(1)
"#,
            "helper",
        );
    }

    #[test]
    fn a_type_variable_resolves_for_ide_features() {
        // `token_symbol_key` used to build the key `''n`, so no type variable
        // ever resolved for hover, goto, references, or rename.
        check_hover_contains(
            r#"
default Order dec
val f : forall 'n, 'n > 0. bits('n) -> bits('n)
function f(x) : bits('n) -> bits('n) = {
  let w : int = sizeof('$0n);
  x
}
"#,
            "'n",
        );
    }
}

/// A built-in type constructor applied to the wrong number of arguments.
///
/// Found by broadening `mutation_check`: the arity operator scored 22%
/// because nothing objected to a type constructor missing an argument.
#[test]
fn a_builtin_type_takes_the_arguments_it_takes() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
type bad = list(int, int)
//         ^^^^^^^^^^^^^^ error: Type list takes 1 arguments, not 2
"#,
    );
}

/// `vector('n, 'ord)` counts two arguments, which the two-argument form
/// allows — but that form wants an element type, and `'ord` is an order.
#[test]
fn a_vector_missing_its_element_type_is_an_error() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val f : forall ('n : Int) ('ord : Order). vector('n, 'ord) -> unit
//                                        ^^^^^^^^^^^^^^^^ error: Type vector is missing its element type: `'ord` is an order, so this needs a third argument
"#,
    );
}

/// A struct literal naming a field the struct does not declare.
#[test]
fn a_struct_literal_cannot_invent_a_field() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct S = { a : int, b : int }
let x : S = struct { a = 1, b = 2, zzz = 3 }
//                                 ^^^ error: Struct S has no field zzz
"#,
    );
}

/// The field list came from a second scrape of the CST, which returned
/// nothing for a struct with a constraint — so nothing about a literal of
/// one was ever checked.
#[test]
fn a_constrained_struct_still_has_its_fields() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct S('n : Int), 'n >= 0 = { a : int, b : int }
let x : S(1) = struct { a = 1 }
//             ^^^^^^^^^^^^^^^^ error: struct literal missing fields: b
"#,
    );
}

/// Every registered assist reaches the editor.
///
/// The LSP handler kept its own table of fifteen, named and kinded inline,
/// and called `ide_assists` free functions directly — so the other
/// forty-nine were registered and unreachable. This asserts the one door
/// `Analysis::assists` offers at least what that table did.
#[test]
fn the_assist_registry_is_what_the_editor_sees() {
    let (analysis, file_id, _) = crate::fixture::position(
        r#"
//- /main.sail
default Order dec
function f(x : bool) -> int = if x then 1 else 2
$0
"#,
    );
    let url = analysis.url_for_file_id(file_id).expect("fixture file has a URL").clone();
    use ide_db::SourceFileInfo;
    let text = analysis.file(&url).expect("fixture file").text().to_string();
    let at = text.find("if x").expect("fixture has an `if`");
    let range = base_db::text_range(at, at);

    let labels: Vec<String> = analysis
        .assists(&url, range, &ide_assists::AssistConfig::default())
        .expect("the test database is never cancelled")
        .into_iter()
        .map(|a| a.label.0)
        .collect();
    assert!(
        labels.iter().any(|l| l == "Invert if"),
        "the registry did not offer `Invert if` at an `if`; it offered: {labels:?}"
    );
}

/// A call to an `outcome` declared in another file.
///
/// `outcome` was not a `DefKind`, so the item tree dropped it, the
/// workspace merge never saw it, and every cross-file call to one — eleven
/// to `sail_barrier` in sail-riscv alone — inferred as `error()`.
#[test]
fn an_outcome_crosses_files_like_a_val() {
    check_diagnostics(
        r#"
//- prelude
//- /defs.sail
default Order dec
$include <prelude.sail>
outcome oc : bits(8) -> unit
//- /main.sail
default Order dec
$include <prelude.sail>
$include "defs.sail"
function use_oc(a : bits(8)) -> unit = oc(a)
"#,
    );
}

#[cfg(test)]
mod assist_output {
    /// Every assist offered on a small corpus is one a user could want.
    ///
    /// A probe over twelve sources found thirteen that could not be:
    /// "Add `$include` for `int`" — a built-in needs no include — and the
    /// same for `bool`, `bits`, `bit`, for two struct fields and two pattern
    /// bindings; and "Remove unused parameter `E`" on `function f(e : E)`,
    /// where `E` is the parameter's *type*. That one scanned the token
    /// stream for an identifier between parentheses and decided "unused" by
    /// searching the body text for the name, so it offered to delete the
    /// type of any parameter whose type name did not appear in the body.
    #[test]
    fn every_assist_offered_could_be_wanted() {
        use ide_db::SourceFileInfo;
        const SOURCES: &[&str] = &[
            "default Order dec\nenum E = A | B | C\nfunction f(e : E) -> int = match e { A => 1 }\n",
            "default Order dec\nstruct S = { a : int, b : int }\nfunction f() -> S = struct { a = 1 }\n",
            "default Order dec\nfunction f(c : bool) -> int = if c then 1 else 2\n",
            "default Order dec\nfunction f() -> int = { let x = 1 + 2; x }\n",
            "default Order dec\nfunction f(x : bits(8)) -> bit = x[0]\n",
            "default Order dec\nbitfield B : bits(8) = { lo : 3 .. 0 }\n",
            "default Order dec\nval g : int -> int\nfunction f() -> int = g(1)\n",
            "default Order dec\nscattered function s\nfunction clause s(x) = x\n",
            "default Order dec\nunion U = { Ctor : int }\nfunction f(u : U) -> int = match u { Ctor(v) => v }\n",
            "default Order dec\n// a comment\nfunction f() -> int = 1\n",
            "default Order dec\nmapping m : bool <-> bits(1) = { true <-> 0b1 }\n",
            "default Order dec\nfunction f() -> int = { if true then { return 1 }; 2 }\n",
        ];
        let cfg = ide_assists::AssistConfig::default();
        let mut seen: std::collections::BTreeSet<String> = Default::default();
        for src in SOURCES {
            let fixture = format!("\n//- /main.sail\n{src}");
            let (analysis, files) = crate::fixture::analysis(&fixture);
            let (file_id, _) = files[0];
            let url = analysis.url_for_file_id(file_id).expect("a URL").clone();
            let text = analysis.file(&url).expect("the file").text().to_string();
            for at in 0..text.len() {
                for a in analysis
                    .assists(&url, base_db::text_range(at, at), &cfg)
                    .expect("the test database is never cancelled")
                {
                    if a.source_change.is_some_and(|c| !c.is_empty()) {
                        seen.insert(a.label.0);
                    }
                }
            }
        }
        // No `$include` for anything these sources define or the prelude
        // supplies, and no parameter removal aimed at a type.
        let wrong: Vec<&String> = seen
            .iter()
            .filter(|label| {
                let names_a_builtin =
                    ["`int`", "`bool`", "`bits`", "`bit`"].iter().any(|n| label.ends_with(n));
                (label.starts_with("Add `$include`") && names_a_builtin)
                    || label.starts_with("Remove unused parameter `E`")
                    || label.starts_with("Remove unused parameter `U`")
            })
            .collect();
        assert!(wrong.is_empty(), "assists offered that cannot be right: {wrong:?}");
        assert!(
            seen.len() >= 18,
            "the corpus stopped exercising assists — {} fired, expected at least 18:\n{seen:?}",
            seen.len()
        );
    }
}

/// A parenthesis inside a string is not a parenthesis to remove.
///
/// `remove_parentheses` counted paren depth through the raw bytes, so it
/// matched one inside a string literal and offered to "remove" a pair that
/// was never one. It reads the tree now, which is what the assist context
/// was missing: it held a file handle and a range, and nothing else.
#[test]
fn an_assist_does_not_match_inside_a_string() {
    use ide_db::SourceFileInfo;
    let (analysis, files) = crate::fixture::analysis(
        "\n//- /main.sail\ndefault Order dec\nfunction f() -> string = \"a ( b\"\n",
    );
    let (file_id, _) = files[0];
    let url = analysis.url_for_file_id(file_id).expect("a URL").clone();
    let text = analysis.file(&url).expect("the file").text().to_string();
    let at = text.find("( b").expect("the paren inside the string");
    let offered = analysis
        .assists(&url, base_db::text_range(at, at), &ide_assists::AssistConfig::default())
        .expect("the test database is never cancelled");
    assert!(
        !offered.iter().any(|a| a.label.0 == "Remove parentheses"),
        "offered to remove a parenthesis that is part of a string literal"
    );
}

#[test]
fn a_mapping_clause_binds_its_head_pattern() {
    // `forwards Ctor(r) => ..` had no structure at all: the parser ran the
    // ordinary expression rule over the mapping body, so `forwards` and
    // `=>` were loose tokens, the head was an *expression*, and `r` was
    // bound by nothing. Every use of it resolved through no scope.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { R : bits(5) }
val encdec : ast <-> bits(6)
mapping encdec = {
  forwards R(r) => 0b0 @ r,
  backwards r => R(r[4 .. 0]),
}
val use_it : ast -> bits(6)
function use_it(a) = encdec(a)
"#,
    );
}

#[test]
fn a_mapping_clause_head_binding_is_used_by_its_body() {
    // The two sides of `<->` are the same syntax read twice. Building the
    // expression side from the pattern is what makes a use on the right
    // count as a use of what the left bound — without it every one of them
    // was reported unused.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val bit_name : bits(1) <-> string
mapping bit_name = { 0b0 <-> "0", 0b1 <-> "1" }
val pair : bits(2) <-> string
mapping pair = { i : bits(1) @ o : bits(1) <-> bit_name(i) ^ bit_name(o) }
"#,
    );
}

#[test]
fn an_outcome_body_holds_together() {
    // `constraint` is part of an outcome's header, and the definition-end
    // scan stopped at it — so the `= { .. }` that followed was never seen
    // as this definition's body. The block spilt out into the file, its
    // `impl`s came apart into ERROR tokens, and nothing in them was
    // lowered: `impl f(x) = ..` bound `x` nowhere.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
outcome my_out : (int) -> unit
with 'a : Int
constraint 'a > 0
= {
    val helper : int -> int
    impl emulator(req) = {
        let z : int = req;
        let _ = z;
        ()
    }
}
"#,
    );
}

#[test]
fn a_record_typed_value_is_not_every_type() {
    // `Named` is also how a type alias arrives, and an alias may stand for
    // anything, so the tail of unify's `Named` arm accepted whatever it was
    // given. A `struct` name is not an alias: `let z : int = x` with `x` a
    // record went unreported, and so did every other use of a record-typed
    // value at a type it is not.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct attrs = { f : bool }
val mk : int -> attrs
function h() -> unit = {
  let y : attrs = mk(1);
  let z : int = y;
     //         ^ error: Type mismatch between int and attrs
  let _ = z;
  ()
}
"#,
    );
}

#[test]
fn an_unbraced_loop_body_may_be_an_assignment() {
    // `foreach (j from 16 to 23) w[j] = f(w[j - 16])` — the body was parsed
    // as an expression, so `w[j]` was the whole loop body and the `= f(..)`
    // became a sibling of the loop. Every `j` on the right was outside the
    // loop's scope, and resolved by matching the name against every pattern
    // in the body instead.
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val zk : bool -> int
function f() -> unit = {
  var w : vector(24, dec, int) = undefined;
  foreach (j from 16 to 23)
    w[j] = zk(j);
          //  ^ error: Type mismatch between bool and int
  ()
}
"#,
    );
}

#[test]
fn a_match_reaches_the_union_behind_a_parameterised_alias() {
    // `TR_Result(physaddr, ExceptionType)` is an alias for `result((physaddr,
    // page_based_mem_type, ext_ptw), ..)`, and one normalisation step does
    // not reach the union — so `Ok(paddr, pbmt, _)` bound `pbmt` to nothing
    // and everything read through it was untyped.
    check_diagnostics(
        r#"
//- /a.sail
default Order dec
union myres('a : Type, 'b : Type) = { MyOk : 'a, MyErr : 'b }
type TR('p : Type, 'f : Type) = myres(('p, bool, int), ('f, int))
function tr(x : int) -> TR(string, unit) = MyErr((), 0)
//- /b.sail
default Order dec
function f(x : int) -> unit = {
  match tr(x) {
    MyOk(p, q, _) => { let z : string = q; let _ = z; let _ = p; () },
                    //                  ^ error: Type mismatch between string and bool
    MyErr(e, _) => { let _ = e; () },
  }
}
"#,
    );
}

#[test]
fn a_guarded_concat_parameter_binds_through_its_type() {
    // `((0b1100000 @ index : bits(5), priv) if unsigned(index) >= 3)` — a
    // parameter list is raw tokens at lowering time, and `bits(5)` is four
    // of them. The `x : T` case required exactly three, so the run fell to
    // the catch-all and became a wildcard: `index` was bound by nothing,
    // in the guard and in the body alike.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val cnt : (int, bool) -> bool
val acc : (bits(12), bool) -> bool
function acc((0b1100000 @ index : bits(5), priv) if unsigned(index) >= 3) = cnt(index, priv)
                                                                            //  ^^^^^ error: Type mismatch between int and bits(5)
"#,
    );
}

#[test]
fn a_nullary_enum_member_is_a_value() {
    // `Read_plain`, `Barrier_RISCV_tso` — nothing in value lookup consulted
    // the enums, so a bare member fell through to the constructor branch.
    // The prelude declares the same names as `union barrier_kind = {
    // Barrier_RISCV_i : unit, .. }`, a constructor is a function, and two
    // declarations of one name make that branch give up entirely: the
    // member had no type, and neither did anything reached through it.
    check_diagnostics(
        r#"
//- /a.sail
default Order dec
union bk = { B_i : unit, B_tso : unit }
//- /b.sail
default Order dec
enum bk2 = { B_i, B_tso }
function f() -> unit = {
  let z : int = B_i;
     //         ^^^ error: Type mismatch between int and bk2
  let _ = z;
  ()
}
"#,
    );
}

#[test]
fn a_one_directional_clause_has_one_expression() {
    // `forwards Regidx(r) => zero_extend(r)` lowers with both expression
    // fields pointing at the body. Inferring it twice ran it first against
    // the *other* side's environment, which does not hold what the head
    // bound — `r` came out untyped, and the second pass found the
    // expression already recorded.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val enc : bits(5) <-> bits(6)
mapping enc = {
  forwards r => 0b0 @ r,
  backwards r => r[4 .. 0],
}
val use_it : bits(5) -> bits(6)
function use_it(a) = enc(a)
"#,
    );
}

#[test]
fn a_bidirectional_clauses_guard_is_not_its_other_side() {
    // `Fregidx(i) if get_config_use_abi_names() <-> freg_abi_name_raw(i)`
    // has three children, and the one after the head is the *guard*. Taking
    // it as the other side left the real one unlowered. Which child is
    // which is decided by the `<->`: `if g` is written before it, `when g`
    // after it.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val flag : unit -> bool
val nm : bits(2) <-> string
mapping nm = {
  0b00 if flag() <-> "a",
  0b01 <-> "b" when flag(),
  0b10 <-> "c",
  0b11 <-> "d",
}
"#,
    );
}

#[test]
fn a_clause_head_is_read_with_what_it_binds() {
    // `mapping clause encdec = JAL(imm @ 0b0, rd) <-> imm[19] @ imm[9..0] @
    // ..` — read backwards the head is an *expression*, and it was inferred
    // against the other side's environment alone. That side binds `imm` out
    // of disjoint slices and does not carry it, so `imm` in the head was
    // untyped. Upstream checks each side with the other's environment as a
    // fallback, not instead of its own.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum Reg = { R0, R1 }
union ast = { JAL : (bits(21), Reg) }
val encdec_reg : Reg <-> bits(5)
mapping encdec_reg = { R0 <-> 0b00000, R1 <-> 0b00001 }
val encdec : ast <-> bits(32)
scattered mapping encdec
mapping clause encdec = JAL(imm @ 0b0, rd)
  <-> imm[19] @ imm[9..0] @ imm[10] @ imm[18..11] @ encdec_reg(rd) @ 0b1101111
end encdec
"#,
    );
}

#[test]
fn a_body_is_checked_under_its_own_quantifiers() {
    // A call from `forall 'n, 'n in {16, 32, 64}` to a function declared
    // with *the same* constraint could not discharge it: the assumption set
    // held only top-level `constraint` definitions and local existentials,
    // never the body's own quantifiers. The fact was written on the caller
    // and never read. Upstream seeds them in `check_funcl`, through
    // `Env.add_typquant` (type_env.ml:1143).
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f_is_NaN : forall 'm, 'm in {16, 32, 64}. bits('m) -> bool
function f_is_NaN(xf) = true
val use_n : forall 'n, 'n in {16, 32, 64}. bits('n) -> bool
function use_n(v) = f_is_NaN(v)
"#,
    );
}

#[test]
fn a_quantifier_learns_the_callers_own_variable() {
    // `int(?0)` meeting `int('n)` had no concrete value to read, so the
    // freshened quantifier was bound to `int` itself — losing the name. The
    // callee's `0 < 'n <= 8` then spoke about a variable no assumption
    // mentions, the candidate was dropped for an unresolved quantifier, and
    // the call came back untyped.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val h : forall 'n, 0 < 'n <= 8. (int('n)) -> bool
function h(w) = true
val g : forall 'n, 0 < 'n <= 8. (int('n)) -> bool
function g(w) = h(w)
"#,
    );
}

#[test]
fn an_implicit_parameter_reads_a_literal_argument() {
    // `canonical_NaN(16)` against `forall 'n, 'n in {16, 32, 64, 128}.
    // implicit('n) -> bits('n)`: the argument's *type* is `int` and says
    // nothing, so `'n` stayed unbound. Its value is right there in the call.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val canonical_NaN : forall 'n, 'n in {16, 32, 64, 128}. (implicit('n)) -> bits('n)
function canonical_NaN(n) = sail_zeros(n)
val nan_h : unit -> bits(16)
function nan_h() = canonical_NaN(16)
"#,
    );
}

#[test]
fn a_bare_alias_argument_expands_before_the_candidate_sees_it() {
    // `r1(v)` with `v : bits_H` against `forall 'm, 'm in {16, 32, 64}.
    // bits('m) -> ..`. While the argument is still spelled `bits_H` it tells
    // `bits('m)` nothing, and the substitution the constraint check reads is
    // built by a *different* unifier — the one in `constraint.rs`, which has
    // no aliases to consult. So `'m` was unresolved, the candidate was
    // dropped, and the call and everything read from it was untyped.
    check_diagnostics(
        r#"
//- prelude
//- /a.sail
default Order dec
type bits_H = bits(16)
//- /b.sail
default Order dec
$include <prelude.sail>
val r1 : forall 'm, 'm in {16, 32, 64}. (bits('m)) -> bool
function r1(v) = true
function c1(v : bits_H) -> bool = r1(v)
"#,
    );
}

#[test]
fn a_constraint_synonym_is_expanded_before_it_is_proved() {
    // `forall 'width, is_mem_width('width)` carries the only fact there is
    // about `'width`, and the clause was not recorded at all: the scan for
    // a quantifier's constraint looked for an operator, and an application
    // has none. So `split_misaligned : forall 'width, 'width > 0` could not
    // be called from one, and the call plus everything read from it was
    // untyped.
    check_diagnostics(
        r#"
//- prelude
//- /a.sail
default Order dec
type is_mem_width('w) = 'w in {1, 2, 4, 8}
//- /b.sail
default Order dec
$include <prelude.sail>
val narrow : forall 'w, 'w > 0. (int('w)) -> bool
function narrow(w) = true
val wide : forall 'width, is_mem_width('width). (int('width)) -> bool
function wide(width) = narrow(width)
"#,
    );
}

#[test]
fn a_bitfield_field_read_has_the_fields_type() {
    // `menvcfg[LPE]` is a *field* read, not an element read: the index is a
    // bitfield field name and the result is that field's width. Nothing
    // typed it, so every bitfield read off a register was `error()` — and so
    // was everything computed from one.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
bitfield Cfg : bits(64) = { W : 10 .. 8 }
register menvcfg : Cfg
function f() -> unit = {
  let z : int = menvcfg[W];
     //         ^^^^^^^^^^ error: Type mismatch between int and bits(3)
  let _ = z;
  ()
}
"#,
    );
}

#[test]
fn a_concat_pattern_binds_the_part_nobody_stated() {
    // `JAL(imm @ 0b0, rd)` against a `bits(21)` payload: `0b0` is one bit,
    // so `imm` is the other twenty. Both sides were bound at the error type
    // instead — the arm only handled concatenations built out of subranges —
    // and every use of `imm` went with it, which in a mapping clause is the
    // whole other side of the `<->`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { JAL : (bits(21), bits(5)) }
val enc : ast <-> bits(26)
mapping enc = { JAL(imm @ 0b0, rd) <-> imm @ 0b0 @ rd }
"#,
    );
}

#[test]
fn an_impure_extern_val_still_has_a_signature() {
    // `val cancel_reservation = impure {interpreter: ".."} : unit -> unit`.
    // `impure` was in the comment and not in the code, so the extern block
    // was not recognised, the signature after it was never read as a type,
    // and the `val` contributed no scheme at all — every call to one was
    // untyped.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val cr = impure {interpreter: "P.cr", c: "cr"} : unit -> bits(8)
function f() -> unit = {
  let z : bool = cr();
     //          ^^^^ error: Type mismatch between bool and bits(8)
  let _ = z;
  ()
}
"#,
    );
}

#[test]
fn a_newtype_pattern_opens_its_payload() {
    // `newtype physaddr = Physaddr : physaddrbits` — the payload after the
    // constructor is a *type*, and the expression parser ran over it. The
    // definition ended up with no type node at all, so the constructor was
    // registered with an `error()` payload and `Physaddr(paddr)` bound
    // `paddr` to nothing — in a parameter and in a match arm alike.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
newtype pa = Pa : bits(16)
function f(Pa(p) : pa) -> unit = {
  let z : bool = p;
     //          ^ error: Type mismatch between bool and bits(16)
  let _ = z;
  ()
}
"#,
    );
}

/// An enum argument passed where a bitvector is asked for.
///
/// `unify` accepts an enum wherever the expected type is not a record:
/// enums are deliberately absent from `nominal_types`, because
/// `enum_conflict` is the more careful check. It was only ever reached
/// where an expected type meets an expression — an annotation, an
/// ascription, an assignment target — and never at a call's arguments, so
/// `isAddressAligned(width, vAddr)` for
/// `(bitvector(64, dec), WordType) -> bool` passed silently. Upstream:
/// "WordType is not a subtype of bitvector(64)".
#[test]
fn an_enum_argument_is_checked_against_the_parameter() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
enum WordType = {B, H, W, D}
val isAligned : (bitvector(64, dec), WordType) -> bool
function caller(w : WordType, a : bitvector(64, dec)) -> unit = {
  let b : bool = isAligned(w, a);
//                         ^ error: Type mismatch between bitvector(64) and WordType
//    ^ warn: unused variable `b`
  ()
}
"#,
    );
}

/// The same call written the right way round says nothing.
#[test]
fn the_right_argument_order_is_not_reported() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
enum WordType = {B, H, W, D}
val isAligned : (bitvector(64, dec), WordType) -> bool
function caller(w : WordType, a : bitvector(64, dec)) -> unit = {
  let b : bool = isAligned(a, w);
//    ^ warn: unused variable `b`
  ()
}
"#,
    );
}

/// `pat as name` binds `name`.
///
/// The grammar reads the right of `as` with `parse_type_expr`, matching
/// upstream's `pat1 As typ` (`parser.mly:523`), so the name arrives inside
/// a `TYPE_NAMED` node. The lowering asked `last_ident_text`, which looks
/// at *direct token children* only, and got `None` — so every `as` binding
/// in a match arm or a `let` was lowered with an empty name and its use in
/// the body was reported unresolved.
#[test]
fn an_as_pattern_binds_its_name() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val test : int -> int
function test(n) = match n { m as x => x }
//                           ^ warn: unused variable `m`
"#,
    );
}

/// The same, where the head is not parenthesised.
///
/// Here the grammar bumps `as` and the name straight into the flat
/// `PARAM_LIST` rather than building an `AS_PAT`, so `x` was collected as
/// a *second parameter* of a one-parameter function: it was matched
/// against no declared type and the body's `x` resolved to nothing. This
/// is upstream's `typecheck/pass/as_pattern.sail`.
#[test]
fn an_as_binding_in_an_unparenthesised_head() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val test : int -> int
function test _ : int as x = x
"#,
    );
}

/// A field the record does not declare.
///
/// `is_cross_file_false_positive` suppressed *every* `UnresolvedField`
/// whose receiver named a known record — and `record_names` is
/// workspace-wide, so it could not tell "declared in another file, fields
/// unknown" from "declared right here". The case it was written for is the
/// first; the case it silenced is the second, which is the only one worth
/// reporting at all.
#[test]
fn a_field_the_record_does_not_declare_is_reported() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct Record = {rsig : int, bitf : bits(32)}
function test(r : Record) -> bits(32) = r.zzz
//                                        ^^^ error: no field `zzz` on type `Record`
"#,
    );
}

/// An unbraced assignment is a body.
///
/// `function f(r) = R0 = r.x` — the grammar started the `ASSIGN_EXPR`
/// marker *before* the definition's `=`, so that token ended up inside the
/// expression and the `CALLABLE_DEF` had no `=` of its own. The body
/// lookup found nothing, the callable lowered with an empty body, and
/// every check over it said nothing: no field check, no type check, not
/// even an unused parameter. This is the shape of upstream's
/// `typecheck/pass/simple_record_access.sail`.
#[test]
fn an_unbraced_assignment_is_a_body() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct Record = {rsig : int, bitf : bits(32)}
register R0 : bits(32)
val test : Record -> unit
function test(r) = R0 = r.zzz
//                        ^^^ error: no field `zzz` on type `Record`
"#,
    );
}

/// A struct pattern naming a field the record does not declare.
///
/// `Pat::Struct` looked each name up in the record's field list and typed
/// what it did not find as `error()` — silently. The list was already
/// there, and the other direction of the same comparison was already
/// reported: `MissingPatternFields` says which declared fields the pattern
/// left out. Renaming a field left every pattern matching on the old name
/// unchecked.
#[test]
fn a_struct_pattern_field_the_record_does_not_declare() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct foo = { field1 : bits(32), field2 : int }
val main : foo -> unit
function main(v) = match v { struct { zzz = b, _ } => () }
//                                          ^ error: no field `zzz` on type `foo`
//                                          ^ warn: unused variable `b`
"#,
    );
}

/// An arithmetic operator has no overload that takes a `bool`.
///
/// Upstream reaches `*` through
/// `overload operator * = {mult_atom, mult_int, ..}`, so `4 * g()` where
/// `g` is declared to return `bool` has no possible overloading —
/// "Function (operator *) not found". This is what a wrong *return type*
/// looks like at a use, which is the only place it can be seen: the
/// declaration itself is consistent, and only the caller disagrees.
/// The comparison operators already made this check; the arithmetic ones
/// did not, and they share the classifier now.
#[test]
fn an_arithmetic_operator_rejects_a_bool_operand() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val g : unit -> bool
function f() -> int = 4 * g()
//                        ^^^ error: Type mismatch between int and bool
"#,
    );
}

/// An imperative declaration's annotation is what the binding is.
///
/// `record : A = undefined` used to type `record` from the *value*, and
/// `undefined` has no type to give — so the annotation was thrown away and
/// every later `record.field` had an untyped receiver, field name and all.
/// Upstream checks the value against the declared type (`E_var`); it does
/// not read the type off the value.
#[test]
fn an_imperative_declaration_keeps_its_annotation() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct A = {field_A : bool, shared : int}
val makeA : bool -> A
function makeA(x) = { record : A = undefined; record.zzz = x; record }
//                                                   ^^^ error: no field `zzz` on type `A`
"#,
    );
}

/// A value of a disjoint kind cannot be assigned.
///
/// There was nothing to compare while the target's type *became* the
/// value's. Keeping the declared type made this check possible and made it
/// necessary: `count : int = 0; count = <bool>` is what a wrong return
/// type looks like where the binding is imperative.
#[test]
fn an_assignment_rejects_a_disjoint_value() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val g : unit -> bool
function test() -> int = { count : int = 0; count = g(); count }
//                                                  ^^^ error: Type mismatch between int and bool
"#,
    );
}

/// A struct literal's expected type can come from the callable's return
/// annotation, or from an ascription that follows it.
///
/// `expected_struct_type` looked only at siblings *before* the literal, in
/// a `let`/`NAMED_DEF`/`CAST_EXPR` — so a literal that is a function's
/// whole body was never checked, and neither was
/// `struct { .. } : phantom(32)`, whose annotation comes after it.
///
/// It takes the *outermost* type node of each sibling, never one nested
/// inside it: `let r : Mem_read_request('n, .., RISCV_strong_access) =
/// struct { .. }` has a struct name among the type arguments, and reading
/// the innermost called every field of that literal unknown — 15 false
/// positives on sail-riscv, which is how this rule was found.
#[test]
fn a_struct_literal_is_typed_by_the_return_annotation() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct bar = { fld : bits(8) }
function change(x : bits(8)) -> bar = struct { zzz = x }
//                                    ^^^^^^^^^^^^^^^^^^ error: struct literal missing fields: fld
//                                             ^^^ error: Struct bar has no field zzz
"#,
    );
}

/// A parameter's own annotation is not what the literal must be.
#[test]
fn a_parameter_annotation_does_not_type_a_struct_literal() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct bar = { fld : bits(8) }
  function change(r : bar) = struct { fld = 0x00 }
//^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: Cannot infer function type for unannotated function
"#,
    );
}

/// A `return` is checked against the type the function declares.
///
/// `Expr::Return` called `infer_expr_expecting`, which only feeds the
/// expectation *into* inference; the comparison afterwards lives in
/// `infer_expr_hir_with`. So the tail expression of a body was checked and
/// every early return was not.
#[test]
fn a_return_is_checked_against_the_declared_type() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val f : unit -> bool
function f() = { let x : int = 3; return x }
//                                       ^ error: Type mismatch between bool and int
//                                ^^^^^^^^ hint: unnecessary trailing `return`
"#,
    );
}

/// Two different bitfields do not unify.
///
/// A bitfield is deliberately absent from `nominal_types` — it converts to
/// and from its underlying bitvector — but it does not accept a *different
/// bitfield*, and `unify`'s permissive `Named` tail let it. The accessors a
/// bitfield synthesises are one overload set per field name, so
/// `update_halt` here has a candidate for each of the two bitfields that
/// declare `halt`, and resolution took whichever came first: on sail-riscv
/// that made `rvfi_exec.bits` `bits(192)` for a 704-bit packet.
#[test]
fn two_bitfields_sharing_a_field_name_do_not_unify() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
bitfield Small : bits(8) = { halt : 3 .. 0 }
bitfield Big : bits(32) = { halt : 3 .. 0 }
val use_big : Big -> bits(32)
function use_big(b) = {
  let b = update_halt(b, 0x1);
  b.bits
}
"#,
    );
}

/// A `newtype`'s constructor is a name you can navigate to.
///
/// `newtype virtaddr = Virtaddr : xlenbits` declares *two* names — the
/// type and its constructor — the same way
/// `union virtaddr = { Virtaddr : xlenbits }` does, which is what upstream
/// desugars it to. Only the type was recorded, so `Virtaddr`, `Physaddr`,
/// `Regno` and `Regidx` — the wrappers sail-riscv puts on every address
/// and register number — had no definition to go to.
#[test]
fn a_newtype_constructor_has_a_definition() {
    let (analysis, files) = crate::fixture::analysis(
        "\
default Order dec
newtype regno = Regno : int
val use_it : int -> regno
function use_it(n) = Regno(n)
",
    );
    let url = analysis.url_for_file_id(files[0].0).expect("fixture file has a url");
    let text = analysis.files_ref().file_text(files[0].0).expect("text");
    let source = text.text(analysis.db()).to_string();
    let line_index = ide_db::line_index::LineIndex::new(&source);

    // The use, not the declaration.
    let at = source.rfind("Regno").expect("fixture");
    let targets = analysis.goto_definition(url, line_index.line_col(at)).expect("no cancellation");
    assert!(!targets.is_empty(), "`Regno` has nowhere to go");
    assert_eq!(targets[0].name, "Regno");
}

/// A cross-file numeric return keeps its kind.
///
/// `insert_type_vars` replaced a `range`/`atom` whose bounds mention the
/// callee's quantifiers with a *fresh variable* — erasing the one thing
/// that is true in every scope, that this is a number. So
/// `forall 'n. bits('n) -> range(0, 'n)` called from another file came
/// back as an unbound variable, and its call site unified with `bool`,
/// with `string`, with anything. The `int`/`nat` arm beside it already
/// freshened the *arguments* and kept the constructor.
///
/// On the prelude that is `unsigned`, `signed` and `sizeof`, which is
/// most of what a Sail model computes with.
#[test]
fn a_cross_file_numeric_return_keeps_its_kind() {
    crate::fixture::check_diagnostics(
        r#"
//- /lib.sail
default Order dec
val quantified : forall 'n. bits('n) -> range(0, 'n)
//- /main.sail
default Order dec
$include "lib.sail"
function f(v : bits(8)) -> unit = { let x : bool = quantified(v); () }
//                                                 ^^^^^^^^^^^^^ error: Type mismatch between bool and range(0, 8)
//                                      ^ warn: unused variable `x`
"#,
    );
}

/// `sizeof` is atomic, and an integer is not a boolean.
///
/// `Sizeof Lparen typ Rparen` (`parser.mly:800`) — an atomic expression
/// over a type, exactly like `constraint`. The grammar called
/// `parse_expr` instead, so `sizeof(vlen_exp) >= 7` came out as one
/// `SIZEOF_EXPR` spanning the comparison and typed as
/// `atom(sizeof(vlen_exp) >= 7)`.
///
/// That is why `is_subtype` accepted `atom(..) <: bool` unconditionally —
/// a coercion covering for a parse, and it made every integer a boolean
/// everywhere. Both are gone.
#[test]
fn sizeof_is_atomic_and_an_integer_is_not_a_boolean() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val f : forall 'n. bits('n) -> bool
function f(v) = sizeof('n)
//              ^^^^^^^^^^ error: Type mismatch between bool and atom('n)
"#,
    );
    // The comparison that motivated the coercion still types as a boolean.
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val f : forall 'n. bits('n) -> bool
function f(v) = sizeof('n) >= 7
"#,
    );
}

/// Recover the known return type after a failed call predicate. A plain int
/// now has a stable runtime identity, so its missing nonnegative bound is
/// diagnosed too; it must not erase the independent bool/int mismatch.
#[test]
fn an_unresolved_quantifier_still_has_a_return_type() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val p : forall 'n, 'n >= 0. int('n) -> bool
function f(n : int) -> unit = { let x : int = 4 * p(n); () }
//                                                ^^^^ error: Type mismatch between int and bool
//                                                ^ error: constraint cannot hold: 'n >= 0
//                                  ^ warn: unused variable `x`
"#,
    );
    // A dependent result retains the argument's identity. An arbitrary int
    // cannot meet the callee's nonnegative-width precondition.
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val q : forall 'n, 'n >= 0. int('n) -> bits('n)
function f(n : int) -> unit = { let x = q(n); () }
//                                      ^ error: constraint cannot hold: 'n >= 0
//                                  ^ warn: unused variable `x`
"#,
    );
}

/// A constructor as a parameter pattern is one parameter.
///
/// `function g(Regno(n)) = n` arrives as flat tokens, and the walk that
/// builds the parameter list took `Regno` for one binding and `n` for
/// another: a one-parameter function came out with *two*, the first
/// shadowing a constructor name — so hovering `Regno` said
/// "parameter Regno" and the payload `n` was matched against the wrong
/// declared type. The fallback loop already recognised `IDENT (`; it never
/// ran, because it only runs when this walk produces nothing.
#[test]
fn a_constructor_parameter_is_one_parameter() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
newtype regno = Regno : int
val g : regno -> int
function g(Regno(n)) = n
"#,
    );
    // A mixed head still goes through the fallback, which reads
    // `IDENT : TYPE` as well as `IDENT (`.
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
newtype regidx = Regidx : int
val amo : (int, int, regidx, regidx) -> bool
function amo(width : int, op : int, Regidx(rs2) : regidx, Regidx(rd) : regidx) -> bool = true
"#,
    );
}

/// An unresolved identifier is reported wherever it is written.
///
/// `name_likely_pattern_binding` scanned the raw source for the name
/// followed by `[ @ , ) :` — or preceded by `(` or `,` — and suppressed the
/// diagnostic when it matched. To a character scan `g(zzz)` and a parameter
/// list are the same text, so a typo inside any call was silent. It was
/// hiding three real defects, each of which is fixed here rather than
/// papered over.
#[test]
fn an_unresolved_call_argument_is_reported() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val g : int -> int
val f : unit -> int
function f() = g(zzz)
//               ^^^ error: unresolved identifier `zzz`
"#,
    );
}

/// `(x, y, z) = some_tuple()` declares all three.
///
/// Upstream's `LE_tuple`. Only the bare and the annotated *single* target
/// were registered, so every later read of a tuple-assigned name was
/// unresolved.
#[test]
fn a_tuple_assignment_declares_its_names() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val some_tuple : unit -> (int, int, int)
val all_declared : unit -> int
function all_declared() = { (x, y, z) = some_tuple(); x + y + z }
"#,
    );
}

/// `[b]` in a concatenation head is a part, and binds.
///
/// A parameter list is raw tokens, so `[b]` arrives as three of them. Both
/// flatteners had an arm for one token and one for `x : T`, and dropped
/// everything else — losing the binding *and* the part's width, so the
/// concatenation summed short as well.
#[test]
fn a_vector_pattern_in_a_concat_head_binds() {
    crate::fixture::check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(4) -> bool
function f(0b01 @ [b] @ 0b0) = b == bitzero
"#,
    );
}

/// Every part of an unparenthesised concatenation head is a parameter.
///
/// The grammar closed the `PARAM_LIST` after the first part, so
/// `function clause decode splitting : bits(4) @ vop : bits(2) @ rest :
/// bits(2)` left two of the three outside it, in the `CALLABLE_DEF`, where
/// nothing lowered them.
#[test]
fn every_part_of_a_concat_head_is_a_parameter() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val take4 : bits(4) -> unit
val take2 : bits(2) -> unit
val decode : bits(8) -> unit
scattered function decode
function clause decode splitting : bits(4) @ vop : bits(2) @ rest : bits(2) = {
  take4(splitting);
  take2(vop);
  take2(rest)
}
"#,
    );
}

/// A constructor's payload is checked against the instantiated parameter.
///
/// `Some : 'a` in `union option('a)` takes whatever the context asks the
/// union to be. Nothing bound `'a` at the call, and `unify` lets a `Param`
/// accept anything, so a payload of the wrong shape passed in silence.
/// The expected type says what the parameters are: match the scheme's
/// return against it and substitute.
#[test]
fn a_constructor_payload_is_checked_against_the_instantiated_parameter() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
union U('a : Type) = { Some : 'a }
val f : unit -> U((int, int))
function f() = Some(1)
//                  ^ error: Type mismatch between (int, int) and atom(1)
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
union U('a : Type) = { Some : 'a }
val f : unit -> U((int, int))
function f() = Some((1, 2))
"#,
    );
}

/// A constructor applied to arguments its payload cannot take.
///
/// `Ctor1 : {'d, datasize('d). (bits(4), int('d), bits(4))}` applied to two
/// arguments, or a one-value payload applied to two: the parameter count
/// and the argument count simply disagreed, so the loop that compares them
/// did nothing and the call went unchecked.
#[test]
fn a_constructor_payload_arity_is_checked() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
union ast = { Three : (bits(4), int, bits(4)), One : int }
val f : unit -> ast
function f() = Three(0x0, 1)
//             ^^^^^ error: expected 3 arguments, found 2
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
union ast = { Three : (bits(4), int, bits(4)), One : int }
val f : unit -> ast
function f() = One(1, 2)
//             ^^^ error: expected 1 argument, found 2
"#,
    );
}

/// A cross-file call's arguments are judged by kind.
///
/// A cross-file scheme is not reliable enough to accuse a call on, so that
/// path unified only to *bind*. By kind it is reliable: Sail has no
/// coercion between a bitvector and a number — `unsigned` and `to_bits`
/// exist because there is none — so the two arguments swapped is wrong
/// whatever the widths turn out to be, which is what upstream says ("Type
/// mismatch between bitvector('n) and int('m)"). Every argument-order
/// mutant whose call crosses a file went unreported without it.
#[test]
fn a_cross_file_call_argument_is_judged_by_kind() {
    crate::fixture::check_diagnostics(
        r#"
//- /lib.sail
default Order dec
val widen : forall 'n 'm. (bits('n), int('m)) -> bits('m)
//- /main.sail
default Order dec
$include "lib.sail"
val f : forall 'n 'm. (int('m), bits('n)) -> bits('m)
function f(m, x) = widen(m, x)
//                       ^ error: Type mismatch between bits(int) and int('m)
//                          ^ error: Type mismatch between int('m) and bits('n)
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /lib.sail
default Order dec
val widen : forall 'n 'm. (bits('n), int('m)) -> bits('m)
//- /main.sail
default Order dec
$include "lib.sail"
val f : forall 'n 'm. (int('m), bits('n)) -> bits('m)
function f(m, x) = widen(x, m)
"#,
    );
}

/// An enum member in a pattern is a use, not a binding.
///
/// Upstream's `bind_pat` looks the name up before it binds anything. It
/// lowers to `Pat::Bind` all the same, and this defined it as a fresh
/// local at whatever type the context offered — so a constructor pattern
/// written the wrong way round silently rebound the member as an integer.
#[test]
fn an_enum_member_in_a_pattern_is_a_use() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
enum E = {A, B}
union U = { F : (int, E) }
val f : U -> unit
function f(u) = match u { F(A, n) => (), _ => () }
//                          ^ error: Type mismatch between int and E
//                             ^ warn: unused variable `n`
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
enum E = {A, B}
union U = { F : (int, E) }
val f : U -> unit
function f(u) = match u { F(n, A) => (), _ => () }
//                          ^ warn: unused variable `n`
"#,
    );
}

/// A field name is not a value in scope.
///
/// `top_level_symbol_exists` counted `known_field_names` — which mixes
/// struct fields with bitfield fields, from every file in the workspace —
/// as things a bare identifier may denote. So any name that happened to
/// be a field somewhere resolved silently: the prelude declares a `sign`
/// field, and `function f() = sign` with no `sign` in scope said nothing.
#[test]
fn a_field_name_is_not_a_value() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct S = { sign : int }
val f : unit -> int
function f() = sign
//             ^^^^ error: unresolved identifier `sign`
"#,
    );
}

/// An assignment whose two sides are fully known must meet.
///
/// `R[0 .. 0] = [b]` for a `bool` `b` puts a `vector(1, bool)` into a
/// `bits(1)`. The kind check beside this cannot see it — a `vector` has no
/// kind of its own — and nothing else compared an assignment's value to
/// its target unless one of them was a literal. Only when both types are
/// decided: anything the solver still has to work on says nothing.
#[test]
fn a_fully_known_assignment_must_meet() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val g : unit -> bool
register R : bits(32)
val f : unit -> unit
function f() = { R[0 .. 0] = [g()] }
//                            ^^^ error: Type mismatch between bit and bool
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val g : unit -> bit
register R : bits(32)
val f : unit -> unit
function f() = { R[0 .. 0] = [g()] }
"#,
    );
}

/// A type-level numeric expression is read, folded and compared.
///
/// `NumericExpr::parse` handled atoms only — a constant, a type variable,
/// a bare symbol — so *every* compound type-level expression stayed an
/// opaque string. `unsigned : forall 'n. bits('n) -> range(0, 2 ^ 'n - 1)`
/// applied to a `bits(4)` resolved to `range(0, 2 ^ 4 - 1)`, which
/// compared with nothing: no range a call produced was ever checked
/// against the one a caller declared.
#[test]
fn a_type_level_expression_is_folded() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order inc
val unsigned : forall 'n. bitvector('n, inc) -> range(0, 2 ^ 'n - 1)
let y : range(0, 14) = unsigned(0xF)
//                     ^^^^^^^^^^^^^ error: range(0, 15) is not a subtype of range(0, 14)
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order inc
val unsigned : forall 'n. bitvector('n, inc) -> range(0, 2 ^ 'n - 1)
let y : range(0, 15) = unsigned(0xF)
"#,
    );
}

/// An `enum ... with` mapping's clause values have the declared type.
///
/// `Enum id With enum_functions Eq Lcurly enum Rcurly`
/// (`parser.mly:1059`) declares one mapping per function, and each
/// member's value has to have the declared type. The header and the body
/// are flat tokens and nothing read either, so a member could give a
/// string where a `bool` was declared.
#[test]
fn an_enum_mapping_clause_has_the_declared_type() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
enum E with as_bits -> bits(16), as_string -> bool = {
  A => (0xFFFF, "A"),
//              ^^^ error: this enum mapping is declared `bool`
  B => (0x0001, "B")
//              ^^^ error: this enum mapping is declared `bool`
}
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
enum E with as_bits -> bits(16), as_string -> string = {
  A => (0xFFFF, "A"),
  B => (0x0001, "B")
}
"#,
    );
}

/// A subrange is as wide as its bounds say.
///
/// `concat_width_expr` had an arm for `imm[3]` — one bit — and none for
/// `imm[31 .. 16]`, so the moment a concatenation contained a subrange its
/// whole width came out unknown and the equation was never checked.
#[test]
fn a_subrange_contributes_its_width_to_a_concatenation() {
    crate::fixture::check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union Foo = { Bar : bits(32) }
mapping baz : Foo <-> bits(33) = { Bar(imm) <-> imm[31 .. 16] @ imm[15 .. 0] }
//                                              ^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: bits(32) is not a subtype of bits(33)
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union Foo = { Bar : bits(32) }
mapping baz : Foo <-> bits(32) = { Bar(imm) <-> imm[31 .. 16] @ imm[15 .. 0] }
"#,
    );
}

/// A quantifier the body mentions must be determined by a named parameter.
///
/// `sizeof('n)` — and a bare `'n`, which upstream desugars to the same
/// thing — has to be rewritten to a runtime value, and the only place that
/// value comes from is a parameter the function actually binds. Putting
/// the `implicit('n)` behind a wildcard leaves nothing to rewrite it with,
/// and upstream says "Cannot re-write sizeof('n)".
#[test]
fn a_quantifier_the_body_uses_is_determined_by_a_parameter() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val test : forall ('n : Int). (implicit('n), unit) -> int
function test(_, n) = 'n
//                    ^^ error: constraint cannot hold: 'n is not determined by any parameter
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val test : forall ('n : Int). (implicit('n), unit) -> int
function test(n, _) = 'n
"#,
    );
}

/// Everything after an `assert` holds under it.
///
/// Upstream adds the condition to the environment (`E_assert`), which is
/// how `let x = signed(f); assert(x > -4); x` returns a `range(-3, 3)`
/// where `signed` gave `range(-4, 3)`. The narrowest useful case, and the
/// only one that needs no solver: a comparison between a local and an
/// integer literal, tightening one end of a concrete range.
#[test]
fn an_assert_narrows_a_local_range() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val g : unit -> range(-4, 3)
val f : unit -> range(-3, 3)
function f() = { let x : range(-4, 3) = g(); assert(x > -4); x }
"#,
    );
    // Without the assert the range does not fit, and that is still said —
    // at the tail expression, where upstream says it (`a.sail:4.45-46`),
    // rather than against the whole block.
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
val g : unit -> range(-4, 3)
val f : unit -> range(-3, 3)
function f() = { let x : range(-4, 3) = g(); x }
//                                           ^ error: constraint cannot hold: -3 <= 'x & 'x <= 3
"#,
    );
}

/// A cross-file call's numeric bound is concrete when the call decides it.
///
/// `unsigned : forall 'n. bits('n) -> range(0, 2 ^ 'n - 1)` applied to a
/// `bits(5)` is a `range(0, 31)`. Freshening the bound before resolving it
/// replaced it with a variable unrelated to the one the argument had just
/// bound, so a range the call site had decided came back unknown.
///
/// Only when resolving lands on a *fully known* type: a half-resolved
/// bound is worse than an honest variable — it is concrete enough to
/// attempt a constraint check and not concrete enough to discharge one.
#[test]
fn a_cross_file_numeric_bound_is_concrete_when_decided() {
    crate::fixture::check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val decode : bits(5) -> unit
function decode(a) = { let r : range(0, 30) = unsigned(a); () }
//                                            ^^^^^^^^^^^ error: range(0, 31) is not a subtype of range(0, 30)
//                         ^ warn: unused variable `r`
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val decode : bits(5) -> unit
function decode(a) = { let r : range(0, 31) = unsigned(a); () }
//                         ^ warn: unused variable `r`
"#,
    );
}

/// An index's *type* has to fit the vector, not just a literal.
///
/// `v[unsigned(n)]` for an `n : bits(32)` indexes with a
/// `range(0, 4294967295)`, which no two- or three-bit vector can hold.
/// Upstream proves `0 <= i < 'n` and rejects it; the bounds check here
/// looked only at an index written as a literal.
///
/// Both ends have to be decided: a width that is a constant or a
/// quantifier the signature bounds, and an index range with a concrete
/// upper bound.
#[test]
fn a_vector_index_range_has_to_fit_the_width() {
    crate::fixture::check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : forall 'n, 'n in {2, 3}. (bits(32), bits('n)) -> bit
function f(n, v) = v[unsigned(n)]
//                 ^^^^^^^^^^^^^^ error: constraint cannot hold: 0 <= index < 3
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : forall 'n, 'n in {2, 3}. (bits('n), bits(32)) -> bit
function f(n, v) = v[unsigned(n)]
"#,
    );
}

/// A symbolic index the signature bounds still has to fit.
///
/// `GPRs[r]` for an `r : implicit('d)` under `'d in {32, 64}` reaches 64,
/// and no four-element vector holds that — upstream says "Could not
/// resolve quantifiers for plain_vector_access: (0 <= 'd & 'd < 4)".
///
/// Only a bound this checker read for itself: a quantifier constrained by
/// a predicate it does not parse — `regno('r)` is `'r >= 0 & 'r < 4` — is
/// not *unbounded*, it is unknown, and treating the two alike reported the
/// unmutated file.
#[test]
fn a_symbolic_index_the_signature_bounds_has_to_fit() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
type regno('n : Int) -> Bool = 'n >= 0 & 'n < 4
register R0 : bits(64)
let GPRs : vector(4, dec, register(bits(64))) = [ref R0, ref R0, ref R0, ref R0]
val get_R : forall 'r 'd, regno('r) & 'd in {32, 64}. (implicit('d), int('r)) -> unit
function get_R(r, datasize) = { let R = GPRs[r]; () }
//                                      ^^^^^^^ error: constraint cannot hold: 0 <= index < 4
"#,
    );
    // The same file with the parameters the right way round: `'r` is
    // constrained by a predicate this checker does not read, which says
    // nothing rather than everything.
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
type regno('n : Int) -> Bool = 'n >= 0 & 'n < 4
register R0 : bits(64)
let GPRs : vector(4, dec, register(bits(64))) = [ref R0, ref R0, ref R0, ref R0]
val get_R : forall 'r 'd, regno('r) & 'd in {32, 64}. (implicit('d), int('r)) -> unit
function get_R(datasize, r) = { let R = GPRs[r]; () }
"#,
    );
}

/// An outcome instantiation's type satisfies the implementation's constraints.
///
/// `instantiation O with 'a = bits(33), to_bits = f` applies `f` to
/// `bits(33)`, so `f`'s own `forall 'n, 'n in {32, 48}` has to admit 33.
/// Upstream proves it — "Could not resolve quantifiers for to_bits:
/// 33 in {32, 48}" — and nothing here read the instantiation body beyond
/// the outcome's name.
#[test]
fn an_instantiation_satisfies_the_implementations_constraints() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
outcome test_outcome : forall 'n, 'n > 0. ('a, bits('n)) -> unit
with
  'a: Type
= {
    val to_bits : forall 'n, 'n == 64. 'a -> bits('n)
}
val impl_to_bits : forall 'a, 'a in {32, 48}. bits('a) -> bits(64)
instantiation test_outcome with
  'a = bits(33),
  to_bits = impl_to_bits
//          ^^^^^^^^^^^^ error: 33 is not in {32, 48}
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
outcome test_outcome : forall 'n, 'n > 0. ('a, bits('n)) -> unit
with
  'a: Type
= {
    val to_bits : forall 'n, 'n == 64. 'a -> bits('n)
}
val impl_to_bits : forall 'a, 'a in {32, 48}. bits('a) -> bits(64)
instantiation test_outcome with
  'a = bits(32),
  to_bits = impl_to_bits
"#,
    );
}

/// A constructor's existential is opened into inference variables.
///
/// `Foo : {'n, 'n in {16, 32}. (int('n), bits('n), bool)}` applied to
/// `(16, imm, ..)` fixes `'n` at 16, so `imm` has to be a `bits(16)` —
/// upstream says "Multiple non-identical unifiers for 'n: 16 and 17".
///
/// Two things had to be true at once. The payload was opened into the
/// `Param`s it was written with, and a `Param` unifies with anything, so
/// nothing was ever shared between the components. And the literal `16` is
/// typed a bare `int`, which fixes nothing — the second argument then bound
/// `'n` to its own width and the two never met.
#[test]
fn a_constructor_existential_shares_one_variable() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
union ast = { Foo : {'n, 'n in {16,32}. (int('n), bits('n), bool)} }
val g : bits(17) -> ast
function g(imm) = Foo(16, imm, true)
//                        ^^^ error: bits(17) is not a subtype of bits(16)
"#,
    );
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec
union ast = { Foo : {'n, 'n in {16,32}. (int('n), bits('n), bool)} }
val g : bits(16) -> ast
function g(imm) = Foo(16, imm, true)
"#,
    );
}

#[test]
fn an_if_branch_is_checked_against_the_expectation() {
    // The then branch satisfied the annotation, so the `if` did too, and the
    // else branch was inferred with the expectation flowing in and never
    // compared against it. Upstream reports it at the branch
    // (`a.sail:5.33-37`).
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function f(c : bool) -> int = {
  let x : int = if c then 3 else true;
//                               ^^^^ error: Type mismatch between int and bool(true)
  x
}
"#,
    );
}

#[test]
fn a_match_arm_is_checked_against_the_expectation() {
    // Same defect, same fix: the arm is a value position with an expected
    // type. A literal-range check beside the arm covered `=> 1` against a
    // narrow `range` and said nothing at all about `=> true`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function f(c : bool) -> int = {
  let y : int = match c { true => 5, false => true };
//                                            ^^^^ error: Type mismatch between int and bool(true)
  y
}
"#,
    );
}

#[test]
fn a_type_argument_has_a_kind() {
    // Upstream's `builtin_typs` (`type_env.ml:364`) carries
    // `(parameter_kinds, kind)` per constructor; this crate copied the names
    // only, so none of these was reported. Upstream's wording, verbatim.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val h : range(bool, 3) -> unit
//            ^^^^ error: Expected this type to have kind Int but found kind Type
function h(x) = ()
"#,
    );
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val k : vector(8, 4) -> unit
//                ^ error: Expected this type to have kind Type but found kind Int
function k(x) = ()
"#,
    );
    // The kind a quantifier writes down is what its uses are judged against.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : forall ('n : Int). list('n) -> unit
//                              ^^ error: Expected this type to have kind Type but found kind Int
function f(x) = ()
"#,
    );
}

#[test]
fn a_synonym_is_judged_by_the_kinds_it_declares() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type ds('n: Int) -> Bool = 'n in {32, 64}
val f : forall 'n, ds(bool). int('n) -> unit
//                    ^^^^ error: Expected this type to have kind Int but found kind Type
function f(x) = ()
"#,
    );
    // A synonym that writes no kind is not judged: `type len = 32` is an
    // `Int`, and `type bits('n) = bitvector('n)` leaves `'n` to be inferred
    // from the body. Guessing `Type` for either reports the corpus.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type len = 32
val f : range(0, len) -> unit
function f(x) = ()
"#,
    );
}

#[test]
fn an_overload_no_member_accepts_is_reported() {
    // The cross-file overload path returned `error()` in silence when
    // nothing was plausible, so this said nothing — while the *direct* call
    // `f1("s")` reported the mismatch, which is what made it findable.
    // Upstream: "No possible overloading for f".
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f1 : int -> unit
function f1(x) = ()
val f2 : bool -> unit
function f2(x) = ()
overload f = {f1, f2}
function g() -> unit = f("s")
//                     ^ error: no matching overload for `f`
"#,
    );
}

#[test]
fn a_struct_literal_typed_by_its_context_still_names_its_fields() {
    // `classify_struct_field` read the type off an annotation beside the
    // literal and, failing that, guessed from a field name unique in the
    // file. A literal passed straight to a call has neither — but inference
    // does, because the expectation is what carries into it and makes its
    // field values checkable at all.
    //
    // Hovering `strength` here said nothing before.
    crate::fixture::check_hover_contains(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct S = { variety : int, strength : int }
val take : S -> unit
function take(s) = ()
function g() -> unit = take(struct { variety = 1, stre$0ngth = 2 })
"#,
        "S",
    );
}

#[test]
fn calling_something_that_is_not_a_function_is_reported() {
    // `name_known_in_workspace` counts a `let` as known, which is right for
    // "is this name bound at all" and wrong for "may it be called" — so the
    // no-overloading branch was suppressed and nothing else spoke. Upstream
    // looks the callee up in the function namespace alone: "Function x not
    // found. Did you mean f?".
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
let x : int = 3
function f() -> unit = { let _ = x(1); () }
//                               ^ error: `x` is not a function
"#,
    );
}

#[test]
fn a_parenthesised_binding_in_a_concat_pattern_binds() {
    // `function clause decode (0b11@(opc:bits(2))@(Rm:bits(5))@..)` — the
    // flat parameter walk skipped *every* parenthesis, so `(c : bits(6))`
    // came out as four elements rather than a name, a colon and a type, the
    // arm that recognises a typed binding did not match, and the whole part
    // was dropped: the name unresolved and the concatenation short.
    //
    // Two parts hid it — a different path handles those — so it took three.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : bits(8) -> bool
function clause f (0b0@0b0@(c:bits(6))) = c == c
"#,
    );
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val g : bits(8) -> bool
function clause g (0b0@[s]@(c:bits(6))) = c == c
"#,
    );
}

#[test]
fn a_range_is_admitted_by_an_existential_that_covers_it() {
    // Existential *introduction*: `unify` freshens the bound variables and
    // asks whether one particular `atom(?k)` equals `range(0, 31)`, which
    // nothing can. The question is whether a witness exists, and for an
    // interval constraint the range's own bounds answer it.
    //
    // The alias made it worse: `type regno ('n : Int), 0 <= 'n < 32 =
    // atom('n)` was split at the first `=`, which is the one inside `<=`,
    // so `regno` expanded to `'n < 32 = atom('n)`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type regno ('n : Int), 0 <= 'n < 32 = atom('n)
val f : bits(5) -> {'n, 0 <= 'n < 32. regno('n)}
function f b = unsigned(b)
"#,
    );
    // A range wider than the constraint is still an error.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val g : bits(6) -> {'n, 0 <= 'n < 32. atom('n)}
function g b = unsigned(b)
//             ^^^^^^^^^^^ error: range(0, 63) is not a subtype of range(0, 31)
"#,
    );
}

#[test]
fn an_overload_picks_the_member_whose_singleton_matches() {
    // `shl_int` is an overload of `_shl1`, `_shl8` and `_shl32`, each taking
    // a singleton first argument — `int(1)`, `int(8)`, `int(32)` — and
    // returning a different set. The cross-file overload path took the first
    // plausible candidate, and "plausible" says yes to all three for a
    // literal `8`, so `shl_int(8, ..)` returned `{1, 2, 4, 8}`.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register vsize : bits(2)
function g() -> {8, 16, 32, 64} = shl_int(8, unsigned(vsize))
"#,
    );
}

#[test]
fn a_unit_parameter_is_called_with_an_empty_argument_list() {
    // `rPC : unit -> bits(64)` is called `PC()`. Both overload checks
    // counted that parameter as one argument and the call as zero, so
    // `overload PC = {rPC, wPC}` had no possible member — in two of
    // upstream's `lean` files.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register _PC : bits(64)
val rPC : unit -> bits(64)
function rPC() = _PC
val wPC : bits(64) -> unit
function wPC(v) = { _PC = v }
overload PC = {rPC, wPC}
function f() -> unit = { let _base = PC(); () }
"#,
    );
}

#[test]
fn an_attribute_on_an_expression_parses_and_is_honoured() {
    // Upstream's grammar has `exp := attributes exp` (`parser.mly:648`) and
    // `type_check.ml:2205` *skips* the completeness check when a match says
    // `$[complete]` or `$[incomplete]`. It is the sanctioned way to silence
    // exactly the warning this checker emits — and following it produced
    // three syntax errors and lost the whole function, because `$[` was a
    // definition boundary and the attribute was completed as a `CONFIG_EXPR`
    // that stopped before the expression it annotates.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum E = A | B | C
function f(e : E) -> int = $[incomplete] match e { A => 1, B => 2 }
"#,
    );
    // Without it, the warning stands.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum E = A | B | C
function g(e : E) -> int = match e { A => 1, B => 2 }
//                         ^^^^^^^^^^^^^^^^^^^^^^^^^^ warn: non-exhaustive match: missing C
"#,
    );
}

#[test]
fn a_struct_literal_takes_the_annotation_before_it() {
    // Sail nests what follows a `let` into the binding's body, so looking
    // *after* the literal for its type finds the next statement's
    // annotation: `let x : S1 = struct { f1 = 0 }` was judged against `S2`.
    // Only an ascription — `struct { .. } : T` — is annotated afterwards.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct S1 = { f1 : int }
struct S2 = { f2 : int }
function f() -> int = {
  let x : S1 = struct { f1 = 0 };
  var y : S2 = struct { f2 = 1 };
  y.f2 = x.f1;
  y.f2
}
"#,
    );
}

#[test]
fn a_type_variable_pattern_binds_a_value() {
    // `atomic_pat: | kid` is `P_var (P_id (id_of_kid kid), ATyp_var kid)`
    // (`parser.mly:541-543`), and `id_of_kid` drops the leading `'`
    // (`ast_util.ml:983`) — so `let 'cap_size = 32` declares a value named
    // `cap_size` and constrains it to `atom('cap_size)`.
    //
    // The item tree took only `SK::IDENT` as a definition's name, so this
    // definition was not recorded at all and every mention of it was
    // unresolved. `sail/test/c/tl_let.sail`, which upstream accepts.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
let 'cap_size = 32
function main() -> int = {
  let x : int = cap_size;
  x
}
"#,
    );
}

#[test]
fn a_field_list_that_nests_still_ends_where_it_ends() {
    // `hir-ty` built its `RecordInfo` with a second reader of the same
    // declaration, and that one tracked a single `in_braces` flag with no
    // depth: the inner `}` of `{16, 32}` closed the struct as far as it was
    // concerned, so every field declared after it was invisible and
    // `s.Three` reported "no field `Three` on type `st`".
    // `sail/test/rocq/pass/existentials2.sail`, which upstream accepts.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct st = {
  One : (int, int),
  Two : {'n, 'n in {16, 32}. (int('n), bits('n))},
  Three : (int, {'n, 'n in {16, 32}. (int('n), bits('n))}),
}
function f(v : bits(16), s0 : st) -> st = {
  var s1 = s0;
  s1.Two = (16, v);
  s1.Three = (1, (16, v));
  s1
}
"#,
    );
}

#[test]
fn an_unknown_operand_does_not_make_an_operator_boolean() {
    // `|` is `or_bool` on booleans and `or_vec` on bitvectors, and with no
    // known operand nothing chooses. Concluding `bool` is a claim, and the
    // assignment below then checks it against the target's `bits(32)` and
    // reports on a file upstream accepts (`sail/test/c/pointer_assign.sail`).
    //
    // Both operands arrive unknown for the same reason: a top-level `let`
    // with no annotation has no type here, where upstream takes it from the
    // initialiser.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
register A : bits(32)
let STATUS_1 = 0x0000_0002
val read_register = "reg_deref" : forall ('a : Type). register('a) -> 'a
function main() -> unit = {
  let c = read_register(AB[0]);
  A = c | STATUS_1
}
let AB = [ref A]
"#,
    );
}

#[test]
fn a_mapping_pattern_takes_its_arguments_from_the_other_side() {
    // `bind_mpat`'s `MP_app` case unifies the mapping's *right* side with
    // what the pattern matches and gives the sub-pattern the *left* side
    // (`type_check.ml:4390`); only if that fails does it swap
    // (`type_check.ml:4406`). Both the order and the no-information
    // fallback were the other way round here, so a mapping pattern whose
    // expected type nothing had decided checked its arguments against the
    // side it was matching: `zero_int_bits(2)` bound the literal `2`
    // against `bits('m)`, and `e_pair_bits(e1, EB)` bound `EB : E` against
    // `bits(2)`. `sail/test/c/poly_mapping2.sail` and
    // `sail/test/c/issue243_fixed.sail`, both accepted upstream.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum E = EA | EB
mapping e_pair_bits : (E, E) <-> bits(2) = {
  (EA, EA) <-> 0b00,
  (EA, EB) <-> 0b01,
  (EB, EB) <-> 0b11,
}
struct eful = { e1 : E, v1 : bits(1) }
union F = { FE : eful }
val f_bits : F <-> bits(3)
mapping f_bits = { FE(struct { e1 = e1, v1 = v1 }) <-> e_pair_bits(e1, EB) @ v1 }
"#,
    );
}

#[test]
fn a_nested_mapping_pattern_picks_the_side_that_names_the_type() {
    // `try_unify` lets a non-primitive `Named` accept anything, so ordering
    // the two sides by it alone put `m1 : bits(1) <-> E1` the wrong way
    // round when matched against a `bits(1)` — and the `m2(A2)` nested
    // inside it then checked `A2 : E2` against `E1`. The expression path
    // already compares head names for the same reason.
    // `sail/test/typecheck/pass/nested_mapping.sail`, accepted upstream.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum E1 = {A1, B1}
enum E2 = {A2, B2}
val m1 : bits(1) <-> E1
mapping m1 = { 0b0 <-> A1, 0b1 <-> B1 }
val m2 : E1 <-> E2
mapping m2 = { A1 <-> A2, B1 <-> B2 }
function main() -> int = match 0b10 {
  0b00 => 1,
  m1(m2(A2)) @ 0b0 => 2,
  _ => 3,
}
"#,
    );
}

#[test]
fn a_mapping_pattern_that_matches_neither_side_is_reported() {
    // When neither direction accepts what the pattern is matching, upstream
    // stops: "Unification error when pattern matching against mapping
    // constructor: Type mismatch between bitvector(2) and string"
    // (`type_check.ml:4417`), at the mapping application itself. This was a
    // silent fallback, and the `ret` mutant that changes `bin_bits_2`'s
    // string side to a `bool` is what found it.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val bin_bits_2 : bits(2) <-> bool
val assembly : bits(3) <-> string
mapping assembly = {
  0b0 @ (x : bits(2)) <-> "zero" ^ bin_bits_2(x),
//                                 ^^^^^^^^^^^^^ error: Type mismatch between string and bits(2)
}
"#,
    );
}

#[test]
fn a_top_level_let_takes_its_type_from_a_literal_initialiser() {
    // Upstream checks the whole `letbind` and binds each name at what it
    // infers (`check_letdef`). This environment is built before anything can
    // be inferred against it, so the initialisers that need no environment
    // are read and the rest stay untyped — a literal is one, and it is the
    // shape `let STATUS_1 = 0x0000_0002` and `let 'cap_size = 32` take.
    //
    // Without a type the binding was not wrong, it was unchecked: the
    // mismatch below went unreported.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
let STATUS_1 = 0x0000_0002
let 'cap_size = 32
function f() -> bool = STATUS_1
//                     ^^^^^^^^ error: Type mismatch between bool and bits(32)
function g() -> bool = cap_size
//                     ^^^^^^^^ error: Type mismatch between bool and int
"#,
    );
}

#[test]
fn a_termination_measure_names_a_function_that_exists() {
    // `check_termination_measure_decl` opens with `Env.get_val_spec id env`
    // (`type_check.ml:4760`), so a measure for a name nothing declares fails
    // there: "Function test_rec not found" (`type_error.ml:322`), at the
    // name. `sail/test/rocq/complex/move_termination/termination.sail`,
    // which upstream rejects standalone and this accepted.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
termination_measure test_rec(x) = x
//                  ^^^^^^^^ error: Function test_rec not found
"#,
    );
    // A measure beside the function it measures is fine, and so is one in a
    // different file — which is the shape the corpus actually uses.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function test_rec(x : int) -> int = if x <= 0 then 0 else test_rec(x - 1)
//- /measure.sail
termination_measure test_rec(x) = x
"#,
    );
}

#[test]
fn a_subrange_pattern_states_its_indices_in_the_declared_order() {
    // `bitvector_typ_from_vector_subrange` raises `Err_vector_subrange`
    // before it computes a width (`type_check.ml:1763-1777`) and `bind_pat`
    // reaches it for every `P_vector_subrange` (`:3095`); the message is
    // upstream's own (`type_error.ml:301-315`).
    //
    // The width this checker computes takes the indices either way round, so
    // a reversed subrange whose width still adds up was accepted in silence.
    // The check existed for the *expression* form only, and a concat chain
    // does not recurse through `bind_pattern_hir`, so the form the corpus
    // actually writes went through neither.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { I : bits(16) }
function f(x : ast) -> int = match x {
  I(imm[0 .. 2] @ rest[12 .. 0]) => unsigned(imm) + unsigned(rest)
//  ^^^^^^^^^^^ error: First index 0 must be greater than or equal to second index 2 (when default Order dec)
}
"#,
    );
    // The same pattern the right way round says nothing, and so does a
    // subrange standing on its own.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { I : bits(16) }
function g(x : ast) -> int = match x {
  I(imm[2 .. 0] @ rest[12 .. 0]) => unsigned(imm) + unsigned(rest)
}
"#,
    );
}

#[test]
fn a_subrange_pattern_binds_each_name_from_bit_zero() {
    // `each_pattern_vector_subranges` groups a concatenation's subranges by
    // the identifier they bind and checks each group on its own
    // (`type_check.ml:1786-1806`), with two messages: one range that does not
    // reach bit 0, or more than one range after merging.
    //
    // This merged every name's pieces into a single coverage map, so
    // `imm[15 .. 5] @ rest[4 .. 0]` looked like full coverage of `bits(16)`
    // and said nothing — and it bound only the first name it saw, at the
    // total width of all of them, so `imm` was `bits(16)` rather than
    // `bits(11)` and `rest` was not bound at all.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { I : bits(16) }
function f(x : ast) -> int = match x {
  I(imm[15 .. 5] @ rest[4 .. 0]) => unsigned(imm) + unsigned(rest)
//  ^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: Cannot bind imm as pattern subranges do not start at bit 0 (lowest bit is 5).
}
"#,
    );
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { I : bits(16) }
function g(x : ast) -> int = match x {
  I(imm[15 .. 6] @ imm[4 .. 0] @ rest[0 .. 0]) => unsigned(imm) + unsigned(rest)
//  ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ error: Cannot bind imm as pattern subranges are non-contiguous. imm[5] is not defined.
}
"#,
    );
    // `v[2]` is `P_vector_subrange (v, 2, 2)` (`parser.mly:547`), so a
    // single-index piece closes the gap it sits in. Upstream accepts this;
    // recording it without bounds is why the contiguity check used to be
    // skipped for every concatenation containing one.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { I : bits(16) }
function h(x : ast) -> int = match x {
  I(imm[15 .. 1] @ imm[0]) => unsigned(imm)
}
"#,
    );
}

#[test]
fn a_foreach_body_is_inside_the_loop() {
    // `loop_exp` is `id From exp To exp By exp In order=typ`
    // (`parser.mly:622`) — the `in` clause is the loop's *order*, part of
    // the header, and the body forms that follow it are
    // `Lparen loop_exp Rparen Do? exp` and its siblings (`:678-686`). There
    // is no `foreach .. in body`.
    //
    // Treating `in` as a body introducer ended the loop at the `in`, so
    // `inc`, the closing paren and the whole block became *siblings* of the
    // loop: the loop's body was the `by` step, and every name the header
    // bound was out of scope inside the block. `i` below was unresolved,
    // masked by a name-based fallback that is gone now.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function f() -> unit = {
  foreach (i from 0 to 32 by 1 in inc) {
    assert(i >= 0)
  }
}
"#,
    );
}

#[test]
fn a_loop_with_a_braced_body_keeps_the_rest_of_its_function() {
    // `While exp in_place_loop_measure Lcurly block Rcurly`
    // (`parser.mly:691`) and `Foreach loop_exp Lcurly block Rcurly`
    // (`:681`) — a `{` after the condition or the last bound opens the
    // body, not a struct literal.
    //
    // Both parsed their header with the plain expression parser, so
    // `while c { .. }` read `c { .. }` as a struct expression and the
    // definition ended *inside* it: the rest of the function became
    // top-level tokens. `if` and a `match` scrutinee had used
    // `R_NO_STRUCT` all along; these were the two positions left plain.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function f(n : int) -> unit = {
  var c : bool = true;
  var t : int = 0;
  while c {
    t = t + 1;
    c = t < n
  };
  foreach i from 0 to n {
    t = t + i
  };
  assert(t >= 0)
}
"#,
    );
}

#[test]
fn an_if_without_then_is_reported_once() {
    // Upstream has only `If_ exp Then exp` and `If_ exp Then exp Else exp`
    // (`parser.mly:663-673`); `if c { 1 }` is a Syntax error there, at the
    // token that should have been `then`. We accepted it silently.
    //
    // The parser stays lenient — a half-typed `if` must still give the IDE
    // a tree — and `syntax::validation` reports it, which is
    // rust-analyzer's division of labour and the reason that layer exists.
    //
    // Once, not twice: validation used to run at parse time *and* again in
    // `Parse::errors()`, so every syntax error in the workspace was
    // duplicated.
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function f() -> int = { if true { 1 } }
//                      ^^^^^^^ error: Syntax Error: expected `then`
"#,
    );
}

/// A bitfield read off a `let` bound to a `match` has a type.
///
/// `let cfg = match behaviour { .. }` gives `cfg` the match's *variable*,
/// which the arms bind to the real type — and `bitfield_info_for_type`
/// matched on `TyKind::Named`, so it saw a variable and gave up. Every
/// bitfield read off such a binding was untyped, and so was everything
/// downstream: in sail-riscv that is `pmpAddrMatchType_encdec(cfg[A])` in
/// `pmpWriteCfg`, which is how this was found — 275 lines bisected to
/// twenty, and four hypotheses (the shadowing `let`, the `[cfg with F = v]`
/// update, a `forall ('a : Type)` arm, `if`/`else`) killed by variants that
/// type cleanly on the way.
///
/// Hover is the assertion because an untyped expression reports no
/// diagnostic — nothing is *wrong*, the checker simply knows nothing — so
/// `check_no_diagnostics` would have passed throughout.
#[test]
fn a_bitfield_read_off_a_match_binding_has_a_type() {
    crate::fixture::check_hover_contains(
        r#"
//- /main.sail
default Order dec

enum Behaviour = {Fatal, Clear}

bitfield Cfg : bits(8) = {
  A : 4 .. 3
}

register behaviour : Behaviour

function f(v : bits(8)) -> bits(2) = {
  let cfg = Mk_Cfg(v);
  let other = match behaviour { Fatal => cfg, Clear => cfg };
  oth$0er[A]
}
"#,
        "Cfg",
    );
}

/// A call whose width comes from a configured alias has a type.
///
/// `nan_unbox(F(i))` in sail-riscv's `rF_H` passes a `flenbits`, and
/// `flenbits` is `bits(flen)` where `flen` is fixed by a configuration.
/// Looking through such an alias is refused where the question is whether
/// something is *proved*, and that refusal reached the place where the only
/// question was what `'n` stands for: `'n` stayed unbound, the candidate was
/// dropped for an unresolved quantifier, and the call — and the function
/// body around it — had no type at all.
///
/// The alias lives in another file on purpose. That is where sail-riscv
/// keeps it, and the per-file alias map does not carry it; only the scheme
/// table does.
#[test]
fn a_width_from_a_configured_alias_binds_the_signatures_quantifier() {
    crate::fixture::check_hover_contains(
        r#"
//- /flen.sail
default Order dec

type ext_d_supported : Bool = config extensions.D.supported
type flen_bytes : Int = if ext_d_supported then 8 else 4
type flen : Int = flen_bytes * 8
type flenbits = bits(flen)

let flen = sizeof(flen)
//- /use.sail
val nan_unbox : forall 'n 'm, 'm in {16, 32, 64, 128} & 'n >= 'm . (implicit('m), bits('n)) -> bits('m)

val read_F : unit -> flenbits

function rF_H() -> bits(16) = {
  assert(flen >= 16);
  let r = nan_unbox(16, read_F());
  $0r
}
"#,
        "bits(16)",
    );
}

/// `assert(flen >= 16)` is a fact about `flen` under every one of its names.
///
/// The same function with the alias declared beside it, which is how every
/// single-file corpus writes it. Here the unifier reaches the width through
/// the file's own alias map, which expands one level further — the goal is
/// about `flen_bytes * 8` while the assert stated its fact about `flen`, and
/// two names for one integer match on nothing. The assumption is therefore
/// stated at every unfolding; see `constraint::numeric_alias_unfoldings`.
#[test]
fn an_assert_states_its_fact_at_every_unfolding_of_the_alias() {
    crate::fixture::check_hover_contains(
        r#"
//- /main.sail
default Order dec

type ext_d_supported : Bool = config extensions.D.supported
type flen_bytes : Int = if ext_d_supported then 8 else 4
type flen : Int = flen_bytes * 8
type flenbits = bits(flen)

let flen = sizeof(flen)

val nan_unbox : forall 'n 'm, 'm in {16, 32, 64, 128} & 'n >= 'm . (implicit('m), bits('n)) -> bits('m)

val read_F : unit -> flenbits

function rF_H() -> bits(16) = {
  assert(flen >= 16);
  let r = nan_unbox(16, read_F());
  $0r
}
"#,
        "bits(16)",
    );
}

/// A `var`'s initialiser is checked against its annotation.
///
/// The annotation is an expectation, exactly as a `let`'s is — reading it
/// for `let` and not for `var` meant nothing compared the two, and a `var`
/// initialised with the wrong width went unreported. It also left the
/// initialiser itself untyped: `var ys : bits('n) = zeros()` is the only
/// place that width is written down, and `zeros : forall 'n, 'n >= 0.
/// implicit('n) -> bits('n)` has nowhere else to fill its implicit from.
///
/// `undefined` is the same shape one step further on: it names a value of
/// *some* type and says nothing about which, so there is nothing to infer
/// and upstream checks it against the expectation instead (`L_undef` in
/// `check_exp`). Typing it as `error()` left `var mask : bits('n) =
/// undefined` with an untyped initialiser — seven of sail-riscv's then-21
/// untyped expressions, every one that single line. Only the coverage probe
/// sees that half: hover is keyed by symbol, so a literal has no hover to
/// assert on and an initialiser that is merely *unknown* is nothing wrong.
#[test]
fn a_var_initialiser_is_checked_against_its_annotation() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec

val h : bits(4) -> bits(4)

function f() -> bits(8) = {
  var ys : bits(8) = h(0x0);
//                   ^^^^^^ error: bits(4) is not a subtype of bits(8)
  ys
}
"#,
    );
}

/// A body is checked under the `typquant` written on its own definition.
///
/// `function f forall 'n, C . (args) -> ret = body` binds and constrains
/// exactly as a `val` spec does — upstream builds one `typquant` for both.
/// The parser bumped the whole preamble as loose tokens up to the `.`, so
/// the scheme built from such a definition had no quantifiers and no
/// constraints, and the facts written on a definition's own head were read
/// by nobody: 33 definitions in sail-riscv, `mmio_read` and
/// `update_and_write_pte` among them, whose calls could not be discharged.
///
/// The call here is refused because no member of `'n in {1, 2}` is greater
/// than 8. Without the preamble the caller states nothing about `'n`, and a
/// constraint that cannot be decided is not an error.
#[test]
fn a_definitions_own_typquant_constrains_its_body() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec

val g : forall 'n, 'n > 8. int('n) -> bits(8)

function f forall 'n, 'n in {1, 2} . (w : int('n)) -> bits(8) = g(w)
//                                                              ^ error: constraint cannot hold: 'n > 8
"#,
    );
}

/// A setter's parameter is what the assigned value has to be.
///
/// `F(i) = nan_box(data)` desugars upstream to `F(i, nan_box(data))`, and
/// resolving it that way here reports valid code — a setter's first
/// parameter often needs a coercion this checker does not perform — so the
/// pieces are inferred separately. The value was then inferred against
/// nothing at all, and `nan_box : forall 'n 'm, 'n <= 'm. (implicit('m),
/// bits('n)) -> bits('m)` has nowhere else to learn `'m`: the setter's
/// parameter is the only place the width is written.
///
/// Which candidate is the setter matters, and arity alone does not say. A
/// zero-argument getter carries one parameter by the `unit` convention, so
/// `PC() = addr` for `overload PC = {rPC, wPC}` matched `rPC : unit ->
/// bits(64)` and checked `addr` against `unit` — a false positive on two of
/// upstream's own files. A setter returns `unit`; that is what picks it.
#[test]
fn a_setters_parameter_types_the_assigned_value() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec

val rPC : unit -> bits(64)
val wPC : bits(64) -> unit
overload PC = {rPC, wPC}

val g : bits(32) -> bits(32)

function f(addr : bits(64)) -> unit = {
  PC() = addr;
  PC() = g(0x00000000)
//       ^^^^^^^^^^^^^ error: bits(32) is not a subtype of bits(64)
}
"#,
    );
}

/// A struct literal is the record with exactly the fields it writes.
///
/// Sail's `E_struct` is the whole record written out — `E_struct_update` is
/// the form that names some of them — so a literal listing every field of
/// exactly one record *is* that record. Asking only that a record *contain*
/// the written fields made a one-field literal ambiguous wherever a larger
/// record declares that field too, and ambiguity here means no type at all:
/// sail-riscv's `AK_arch(struct { variety = .. })` matched both
/// `RISCV_strong_access` and `Explicit_access_kind` and went untyped, while
/// the two-field literal on the line above it typed cleanly.
///
/// An unannotated binding is rejected by Sail. Recovery still identifies
/// the record and checks its field values, keeping editor feedback useful.
#[test]
fn a_struct_literal_is_the_record_whose_fields_it_writes() {
    crate::fixture::check_diagnostics(
        r#"
//- /main.sail
default Order dec

struct Small = {
  variety : bits(4),
}

struct Large = {
  variety : bits(4),
  strength : bits(4),
}

// No expectation reaches the literal: the fields are all it has to say
// which record it is.
function f() -> unit = {
  let s = struct { variety = 0x00 };
//        ^^^^^^^^^^^^^^^^^^^^^^^^^ error: Cannot infer type of struct literal
//    ^ warn: unused variable `s`
//                           ^^^^ error: bits(8) is not a subtype of bits(4)
  ()
}
"#,
    );
}

/// A struct literal's field resolves to a record declared in another file.
///
/// Whichever way the record's name is reached — written on an annotation,
/// or inferred from what the context asked for — the *declaration* was then
/// looked for in the literal's own file alone, so a name with no
/// declaration beside it classified as nothing and hover said the bare
/// word. sail-riscv writes `Mem_write_request` and `Explicit_access_kind`
/// literals in `phys_mem_interface.sail` while the structs are in the
/// bundled concurrency interface: 19 of its 57 unanswered hovers, every one
/// a field name in a struct literal.
///
/// Which file declares a *type* is a question nothing above `hir-ty` could
/// ask — the workspace index kept only the value namespace — so the answer
/// is a new entry there, kept apart from the values.
#[test]
fn hovering_a_struct_literals_field_names_its_record() {
    crate::fixture::check_hover_contains(
        r#"
//- /decl.sail
default Order dec

enum Variety = {AV_plain, AV_exclusive}

struct Small = {
  variety : Variety,
}
//- /use.sail
val take : Small -> unit

function f() -> unit = take(struct { var$0iety = AV_plain })
"#,
        "Small.variety",
    );
}

#[test]
fn declaration_checks_use_cross_file_signatures_and_quantifiers() {
    check_diagnostics(
        r#"
//- /types.sail
val f : forall ('a : Type). 'a -> 'a
val encode : int <-> bool
//- /main.sail
function f(x) = { let y : 'a = x; y }
mapping encode = { 1 <-> true }
"#,
    );
}

#[test]
fn parameterized_alias_accepts_the_expanded_numeric_width() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type combined('n : Int, 'nn : Int) = bits('nn + 'n)
val good : unit -> combined(4, 8)
function good() = 0xabc
"#,
    );
}

#[test]
fn parameterized_alias_rejects_an_incorrect_result_width() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type combined('n : Int, 'nn : Int) = bits('nn + 'n)
val bad : unit -> combined(4, 8)
function bad() = 0xabcd
//               ^^^^^^ error: Type mismatch between combined(4, 8) and bits(16)
"#,
    );
}

#[test]
fn tuple_expectations_check_each_component_with_literal_evidence() {
    check_diagnostics(
        r#"
//- /main.sail
val good : unit -> (range(0, 10), bool)
function good() = (5, true)
val bad : unit -> (range(0, 10), bool)
function bad() = (15, true)
//                ^^ error: Type mismatch between range(0, 10) and atom(15)
"#,
    );
}

#[test]
fn dependent_match_constraints_are_checked_in_the_arm_scope() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val good : forall 'n, 'n in {8, 16}. int('n) -> bits('n)
function good(n) = match 'n { 8 => 0xff, 16 => 0xffff }
val bad : forall 'n, 'n in {8, 16}. int('n) -> bits('n)
function bad(n) = match 'n { 8 => 0xff, 16 => 0xff }
//                                            ^^^^ error: constraint cannot hold: 8 == 'n
"#,
    );
}

#[test]
fn local_numeric_binder_preserves_singleton_identity() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val width : unit -> range(8, 64)
val combine : forall 'n. (bits('n), int('n)) -> bits('n)
val f : unit -> unit
function f() = {
    let 'n = width();
    var v : bits('n) = undefined;
    v = combine(v, n);
    ()
}
"#,
    );
}

#[test]
fn local_numeric_binder_retains_its_range_constraint() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val width : unit -> range(8, 64)
val f : unit -> bits(128)
function f() = {
    let 'n = width();
    let v : bits('n) = undefined;
    v
//  ^ error: constraint cannot hold: 'n == 128
}
"#,
    );
}

#[test]
fn quantified_values_cannot_be_narrowed_or_share_independent_witnesses() {
    for source in [
        "val make : unit -> {'n, 'n in {8, 16}. bits('n)}\nval bad : unit -> bits(8)\nfunction bad() = make()",
        "val make : unit -> {'n, 'n in {8, 16}. bits('n)}\nval same : forall 'n. (bits('n), bits('n)) -> unit\nval bad : unit -> unit\nfunction bad() = { let x = make(); let y = make(); same(x, y) }",
        "val make : unit -> {'n, 'n in {8, 16}. bits('n)}\nval bad : forall 'm. unit -> bits('m)\nfunction bad() = make()",
    ] {
        let (analysis, files) =
            crate::fixture::analysis(&format!("//- /main.sail\ndefault Order dec\n{source}\n"));
        let diagnostics = analysis
            .file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new())
            .unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "accepted invalid quantified program: {source}\n{diagnostics:?}"
        );
    }
}
#[test]
fn quantified_values_can_be_unpacked_and_repacked_with_their_predicate() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val make : unit -> {'n, 'n in {8, 16}. bits('n)}
val good : unit -> {'m, 'm > 0. bits('m)}
function good() = { let x = make(); x }
val exact : unit -> {'n, 'n == 8. bits('n)}
val concrete : unit -> bits(8)
function concrete() = { let x = exact(); x }
"#,
    );
}
#[test]
fn conditional_widths_reject_impossible_results() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
type selected = bits(if true then 8 else 16)
val good : unit -> selected
function good() = 0xff
val bad : unit -> selected
function bad() = 0xffff
//               ^^^^^^ error: Type mismatch between selected and bits(16)
"#,
    );
}

#[test]
fn existential_type_pattern_aliases_and_nested_blocks_preserve_scope() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val make : unit -> {'n, 'n in {8, 16}. bits('n)}
val good : unit -> {'m, 'm > 0. bits('m)}
function good() = {
    let v = { let x as bits('width) = make(); x };
    v
}
val shadow : forall 'width. bits('width) -> bits('width)
function shadow(x) = {
    let y as bits('width) = make();
    let _ = y;
    x
}
"#,
    );
}

#[test]
fn shadowed_existential_numeric_binders_keep_previous_value_identity() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val one : int(1) -> unit
val two : int(2) -> unit
function main() -> unit = {
    let 'x : {'z, 'z == 1. int('z)} = 1;
    let 'y = x;
    one(x);
    let 'x : {'z, 'z == 2. int('z)} = 2;
    one(y);
    two(x)
}
"#,
    );
}

#[test]
fn local_numeric_names_allow_coarse_dependent_call_recovery() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val read_vector : forall 'n 'm. (int('n), int('m)) -> vector('n, bits('m))
function use_vector(num : int, width : int) -> unit = {
    let 'n = num;
    let 'm = width;
    let value : vector('n, bits('m)) = read_vector(num, width);
    let _ = value;
    ()
}
"#,
    );
}

#[test]
fn existential_tuple_witnesses_are_independent_of_component_order() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
function first(i : int, v : bits(16)) -> {'n, 'n in {16, 32}. (int, int('n), bits('n))} = (i, 16, v)
function last(i : int, v : bits(16)) -> {'n, 'n in {16, 32}. (int, bits('n), int('n))} = (i, v, 16)
function bad(v : bits(16)) -> {'n, 'n in {16, 32}. (bits('n), int('n))} = (v, 32)
//                                                                        ^^^^^^^ error: Type mismatch between {'n. (bits('n), int('n))} and (bits(16), atom(32))
"#,
    );
}

#[test]
fn p2_large_integer_literals_are_checked_without_machine_overflow() {
    check_diagnostics(
        r#"
//- /main.sail
function good() -> range(18446744073709551616, 18446744073709551618) = 18446744073709551617
function negative() -> range(-18446744073709551618, -18446744073709551616) = -18446744073709551617
"#,
    );
    for source in [
        "function bad() -> range(0, 18446744073709551616) = 18446744073709551617",
        "function bad() -> range(-18446744073709551616, 0) = -18446744073709551617",
        "function bad(x : range(-3, -1)) -> nat = if x != 0 then x else 0",
        "function bad(x : int) -> int = match x { 0 if true => false, _ => 1 }",
    ] {
        let (analysis, files) = crate::fixture::analysis(&format!("//- /main.sail\n{source}\n"));
        let ds = analysis
            .file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new())
            .unwrap();
        assert!(
            ds.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "accepted {source}\n{ds:?}"
        );
    }
}
#[test]
fn p2_branch_facts_are_scoped_and_contradictions_are_unreachable() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val good : forall 'n. int('n) -> bits(8)
function good(n) = if constraint('n > 0 & 'n < 0) then 0xffff else 0xff
val select : forall 'n, 'n in {8, 16}. int('n) -> bits('n)
function select(n) = if constraint('n == 8) then 0xff else 0xffff
"#,
    );
}
#[test]
fn p2_branch_joins_preserve_tuple_components_and_width_alternatives() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
val accept : {'n, 'n in {8, 16}. bits('n)} -> unit
function good(c : bool) -> unit = {
    let value = if c then 0xff else 0xffff;
    accept(value)
}
function tuple_join(c : bool) -> (range(0, 2), bool) = {
    let pair = if c then (1, true) else (2, false);
    pair
}
"#,
    );
}
#[test]
fn p2_boolean_alias_arguments_are_instantiated() {
    check_diagnostics(
        r#"
//- /main.sail
type predicate('p : Bool) = bool('p)
val identity : forall ('p : Bool). predicate('p) -> predicate('p)
function identity(x) = x
"#,
    );
}

#[test]
fn p2_immutable_boolean_guards_preserve_conditional_value_identity() {
    check_diagnostics(
        r#"
//- /main.sail
register R : bool
val only32 : int(32) -> unit
val only64 : int(64) -> unit
function main() -> unit = {
    let b = R;
    let x = if b then 32 else 64;
    if b then only32(x) else only64(x)
}
"#,
    );
}
#[test]
fn p2_dependent_arithmetic_call_arguments_keep_their_value() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val topbit : forall 'n, 'n in {8,16}. bits('n) -> bits('n)
function topbit(v) = [v['n - 1]] @ sail_zeros('n - 1)
"#,
    );
}
#[test]
fn p2_record_literals_retain_instantiated_alias_arguments() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
struct request('n : Int, 'a : Type) = {size : int('n), payload : 'a}
type req('n : Int) = request('n, bits(8))
function good() -> req(8) = struct {size = 8, payload = 0xff}
"#,
    );
}

#[test]
fn p2_match_refines_configuration_width_before_joining() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
type xlen : Int = config base.xlen
let xlen = sizeof(xlen)
type xlenbits = bits(xlen)
val accept : bits(64) -> unit
function good(value : xlenbits) -> unit = accept(match xlen {
    32 => 0x00000000 @ value,
    64 => value,
})
"#,
    );
}

#[test]
fn dependent_range_arguments_cannot_select_a_convenient_member() {
    let (analysis, files) = crate::fixture::analysis(
        r#"
//- /main.sail
val only_zero : forall 'n, 'n == 0. int('n) -> unit
function bad(n : range(0, 31)) -> unit = only_zero(n)
"#,
    );
    let diagnostics =
        analysis.file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new()).unwrap();
    assert!(
        diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn dependent_call_proofs_preserve_narrowed_values_and_set_aliases() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type modes = {32, 39, 48, 57}
val use_mode : forall 'v, 'v in {32, 39, 48, 57}. int('v) -> unit
function mode_twice(mode : modes) -> unit = { use_mode(mode); use_mode(mode) }
val bounded : forall 'n, 0 < 'n <= 32. int('n) -> unit
val narrow : forall 'n, 'n > 0. int('n) -> unit
function narrow(n) = if n <= 32 then bounded(n) else ()
let configured_exp : range(0, 12) = config memory.exp
val within : forall 'n 'r, 0 <= 'r <= 64 & 1 <= 'n <= 2 ^ 'r. (int('n), int('r)) -> bool
val check : forall 'n, 'n > 0. int('n) -> bool
function check(n) = {
    let exp = configured_exp;
    let size = 2 ^ exp;
    if n > size then return false;
    within(n, exp)
}
"#,
    );
}

#[test]
fn numeric_set_arguments_reject_gaps_and_accept_members() {
    for (value, rejected) in [(1, false), (2, true), (3, false)] {
        let source = format!(
            "//- /main.sail\ntype sparse = {{1, 3}}\nval consume : sparse -> unit\nfunction caller() -> unit = consume({value})\n"
        );
        let (analysis, files) = crate::fixture::analysis(&source);
        let diagnostics = analysis
            .file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new())
            .unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            rejected,
            "value {value}: {diagnostics:?}"
        );
    }
}

#[test]
fn symbolic_bitfield_endpoints_keep_the_complete_width_expression() {
    check_diagnostics(
        r#"
//- /main.sail
default Order dec
type capacity : Int = config base.width
constraint capacity in {32, 64}
bitfield Address : bits(capacity) = {
    Base : capacity - 1 .. 2, // symbolic endpoint
    Mode : 1 .. 0 // last field
}
function aligned(addr : Address) -> bits(capacity) = addr[Base] @ 0b00
"#,
    );
}

#[test]
fn comparisons_between_ranges_refine_both_branches_without_widening() {
    check_diagnostics(
        r#"
//- /main.sail
type limit : Int = 16
function smaller(x : range(0, 1023), y : range(0, limit)) -> range(0, limit) =
    if x <= y then x else y
function larger(x : range(0, 16), y : range(8, 32)) -> range(8, 32) =
    if x >= y then x else y
function retain(x : range(0, 8), y : range(0, 16)) -> range(0, 8) =
    if x <= y then x else x
"#,
    );
}

#[test]
fn a_cross_file_overload_uses_the_nominal_payload_type() {
    check_diagnostics(
        r#"
//- /types.sail
default Order dec
newtype audit_phys = AuditPhys : bits(34)
newtype audit_virt = AuditVirt : bits(32)
type audit_addr = audit_virt
val audit_phys_bits : audit_phys -> bits(34)
val audit_virt_bits : audit_virt -> bits(32)
overload audit_bits = { audit_phys_bits, audit_virt_bits }
//- /main.sail
default Order dec
$include "types.sail"
val audit : (nat, audit_addr) -> bits(32)
function audit(_step, addr) = audit_bits(addr)
"#,
    );
}

#[test]
fn scattered_mapping_keeps_sparse_payload_across_files() {
    check_diagnostics(
        r#"
//- /types.sail
default Order dec
scattered union instruction
val audit : instruction <-> bits(6)
mapping nreg : bits(5) <-> {1, 2, 4, 8} = { 0b00000 <-> 1, 0b00001 <-> 2, 0b00011 <-> 4, 0b00111 <-> 8 }
//- /clause.sail
union clause instruction = Move : {1, 2, 4, 8}
mapping clause audit = Move(n) <-> 0b1 @ nreg(n)
"#,
    );
}

#[test]
fn conditional_pte_width_propagates_across_files() {
    check_diagnostics(
        r#"
//- prelude
//- /types.sail
default Order dec
$include <prelude.sail>
type pte_bits('v) = bits(if 'v == 32 then 32 else 64)
val consume : forall 'n, 'n in {32, 64}. bits('n) -> unit
val read_pte : forall 'n, 'n in {4, 8}. int('n) -> bits(8 * 'n)
val extend : forall 'n, 'n <= 64. bits('n) -> bits(64)
//- /main.sail
val audit : forall 'v, 'v in {32, 39, 48, 57}. int('v) -> pte_bits('v)
function audit(v) = {
    let 'log_bytes = if 'v == 32 then 2 else 3;
    let pte = read_pte(2 ^ log_bytes);
    consume(pte);
    let extended : bits(64) = extend(pte);
    consume(extended);
    pte
}
"#,
    );
}

#[test]
fn exception_constructor_uses_model_payload_across_files() {
    check_diagnostics(
        r#"
//- /unused_library.sail
union exception = { Exception : unit }
//- /types.sail
enum ExceptionType = { Fault }
union TrapCause = { Exception : ExceptionType }
val consume : TrapCause -> unit
//- /main.sail
val audit : ExceptionType -> unit
function audit(e) = consume(Exception(e))
"#,
    );
}

#[test]
fn catch_payload_and_branch_equalities_reach_return_checks() {
    check_diagnostics(
        r#"
//- prelude
//- /errors.sail
union exception = { Failure : string }
//- /main.sail
default Order dec
$include <prelude.sail>
val audit : forall 'n 'm, 'n >= 'm & 'm >= 0. (implicit('m), bits('n), bits('m)) -> bits('m)
function audit(m, x, fallback) = try {
    if 'n == 'm then x else fallback
} catch {
    Failure(s) => { print(s); fallback }
}
"#,
    );
}

#[test]
fn upstream_p1_poly_list() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec

$include <prelude.sail>

function cons(x: int, xs: list(int)) -> list(int) = {
  x :: xs
}

function poly_cons forall ('a : Type). (x: 'a, xs: list('a)) -> list('a) = {
  x :: xs
}

function main((): unit) -> unit = {
  let _ = cons(1, [|2, 3, 4|]);
  let _ : list(int) = poly_cons(1, [|2, 3, 4|]);
  // TODO: This fails due to different order of instantiation

  ()
}

"#,
    );
}

#[test]
fn upstream_p1_shadow_leak_check() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec

function some_int() -> int = {
  4
}

function some_other_int() -> int = {
  5
}

val test : forall 'n 'm, 'n == 'm. (int('n), int('m)) -> unit

function main() -> unit = {
  let 'x = some_int();

  let 'y: int('x) = {
     let 'x = some_other_int();
     x
  };

  _prove(constraint('x == 'y));

  test(x, y)
}

"#,
    );
}

#[test]
fn upstream_p1_short_circuit_bool_ex() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec

$include <prelude.sail>
$include <string.sail>

val assertive : forall 'n. int('n) -> range(0,'n) effect {escape}

function assertive(n) = {
  assert(n > 0);
  0
}

val ok : unit -> bool(0 == 1) effect {escape}

function ok() = {
  let n = -1;
  let a = assertive(n) > 0 in
  if (true | a) then true else true
}

/*
val bad : unit -> bool(0 == 1) effect {escape}

function bad() = {
  let n = -1;
  if (true | assertive(n) > 0) then true else true
}
*/

val main : unit -> unit effect {escape}

function main() =
  if ok() then
    print_endline("0 = 1")
  else
    print_endline("0 != 1")
"#,
    );
}

#[test]
fn upstream_p1_fixes_preserve_rejections() {
    for source in [
        "function poly_cons forall ('a : Type). (x : 'a, xs : list('a)) -> list('a) = x :: xs\nfunction bad() -> unit = { let _ : list(int) = poly_cons(1, [|true|]); () }",
        "function first() -> int = 4\nfunction second() -> int = 5\nfunction bad() -> unit = { let 'x = first(); let 'y : int('x) = { let 'z = second(); z }; () }",
        "val assertive : forall 'n. int('n) -> range(0, 'n)\nfunction bad() -> bool(false) = { let n = -1; if true | assertive(n) > 0 then true else true }",
        "val assertive : forall 'n. int('n) -> range(0, 'n)\nfunction bad() -> bool(false) = { let n = 1; let a = assertive(n) > 0; if true | a then true else true }",
        "val condition : unit -> bool\nval assertive : forall 'n. int('n) -> range(0, 'n)\nfunction bad() -> bool(false) = { if condition() | assertive(-1) > 0 then true else true }",
    ] {
        let (analysis, files) = crate::fixture::analysis(&format!(
            "//- prelude\n//- /main.sail\ndefault Order dec\n$include <prelude.sail>\n{source}\n"
        ));
        let diagnostics = analysis
            .file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new())
            .unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "accepted invalid program: {source}\n{diagnostics:?}"
        );
    }
}

#[test]
fn upstream_p1_negative_widths_have_semantic_reasons() {
    let cases = [
        (
            "struct Foo = { xs : {'n, 'n == -1. bits('n)} }\nfunction bad() -> unit = { let x : Foo = undefined; () }",
            "cannot be undefined",
        ),
        (
            "struct Foo = { xs : bits(-3) }\nfunction bad() -> unit = { let x : Foo = undefined; () }",
            "cannot be undefined",
        ),
        ("function bad() -> unit = { let x : (bits(-1), int) = undefined; () }", "could be empty"),
        (
            "union Foo('n : Int) = { Bar : bits('n - 2) }\nfunction bad() -> unit = { let x : Foo(1) = undefined; () }",
            "cannot be undefined",
        ),
        (
            "union Foo('n : Int) = { Bar : bits('n - 2), Baz : unit }\nfunction bad() -> unit = { let x : Foo(1) = undefined; () }",
            "cannot be undefined",
        ),
    ];
    for (source, reason) in cases {
        for strict in [false, true] {
            let marker = if strict { "//- strict-bitvector\n" } else { "" };
            let (analysis, files) = crate::fixture::analysis(&format!(
                "{marker}//- /main.sail\ndefault Order dec\n{source}\n"
            ));
            let diagnostics = analysis
                .file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new())
                .unwrap();
            let expected = if strict { "Well-formedness check failed" } else { reason };
            assert!(
                diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error
                    && d.message.contains(expected)),
                "missing {expected}: {source}\n{diagnostics:?}"
            );
        }
    }
    check_diagnostics(
        "//- /main.sail\ndefault Order dec\nfunction f() -> unit = { let _ : list(bits(-1)) = [||]; () }\n",
    );
}

#[test]
fn upstream_p1_preserves_boolean_existential_branch_scope() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec

$include <prelude.sail>

type datasize('n: Int) -> Bool = 'n in {32, 64}

union ast = {
  Ctor : {'d 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n))}
}

val decode : bits(16) -> option(ast)

// Can have a constrained pair

function clause decode(a : bits(5) @ b : bits(1) @ 0b0000011111) = {
  let (datasize, n) : {'d 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n))} =
    if b == 0b0 then (64, unsigned(b @ a)) else (32, unsigned(a));
  Some(Ctor(datasize, n))
}

// Or just lift the function body into the body if the if for flow typing

function clause decode(a : bits(5) @ b : bits(1) @ 0b0000011111) = {
  if b == 0b0 then {
    Some(Ctor(64, unsigned(b @ a)))
  } else {
    Some(Ctor(32, unsigned(a)))
  }
}

// Or use boolean constraint

function clause decode(a : bits(5) @ b : bits(1) @ 0b0000011111) = {
  let 'is_64 = b == 0b0;
  let 'datasize : {'d, ('is_64 & 'd == 64) | (not('is_64) & 'd == 32). int('d)} =
    if is_64 then 64 else 32;
  let _n : range(0, 'datasize) = if is_64 then unsigned(b @ a) else unsigned(a);
  None()
}

// Other variants

function clause decode(a : bits(5) @ b : bits(1) @ 0b0000011111) = {
  let (datasize, n) : {'d 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n))} =
    if b == 0b0 then {
      let c = unsigned(b @ a);
      (64, c)
    } else (32, unsigned(a));
  Some(Ctor(datasize, n))
}
"#,
    );
}

#[test]
fn upstream_p1_strict_widths_and_register_defaults() {
    for (source, reason) in [
        (
            "function f() -> unit = { let _ : list(bits(-1)) = [||]; () }",
            "Well-formedness check failed",
        ),
        (
            "struct S('n : Int) = { x : bitvector('n, dec) }\nregister R : S(1)",
            "Well-formedness check failed",
        ),
    ] {
        let (analysis, files) = crate::fixture::analysis(&format!(
            "//- strict-bitvector\n//- /main.sail\ndefault Order dec\n{source}\n"
        ));
        let diagnostics = analysis
            .file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new())
            .unwrap();
        assert!(
            diagnostics
                .iter()
                .any(|d| d.severity == ide_diagnostics::Severity::Error
                    && d.message.contains(reason)),
            "{diagnostics:?}"
        );
    }
    let (analysis, files) = crate::fixture::analysis(
        "//- /main.sail\ndefault Order dec\nstruct S('n : Int) = { x : bitvector('n, dec) }\nregister R : S(1)\n",
    );
    let diagnostics =
        analysis.file_diagnostics(files[0].0, &ide_diagnostics::DiagnosticsConfig::new()).unwrap();
    assert!(
        diagnostics.iter().any(|d| d.message.contains("Must provide a default register value")),
        "{diagnostics:?}"
    );
    check_diagnostics(
        r#"
//- strict-bitvector
//- /main.sail
default Order dec
struct Inferred('n) = { x : bitvector('n) }
struct Explicit('n : Nat) = { x : bitvector('n) }
struct Constrained('n : Int), 'n >= 0 = { x : bitvector('n) }
type Nonnegative('n : Int), 'n >= 0 -> Type = bitvector('n)
register R : Explicit(1)
register R2 : Inferred(0)
function f() -> unit = { let _ : list(bits(0)) = [||]; () }
"#,
    );
}

#[test]
fn upstream_extended_vector_elements_have_a_common_type() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
function contextual(x : int, y : int) -> vector(2, int) = [x, y]
function inferred(x : int, y : int) -> int = {
  let xs = [x, y];
  xs[0]
}
"#,
    );
}

#[test]
fn upstream_extended_constructor_payload_keeps_existential_facts() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type reg = range(0,15)
union instr = { Mov : {'d, 'd in {8,16}. (int('d), reg, bits('d))} }
val exec : instr -> bits(32)
function exec(Mov(sz, r, imm)) = {
  let _ = (sz,r);
  sail_zero_extend(imm, 32)
}
"#,
    );
}

#[test]
fn upstream_extended_symbolic_subtraction_preserves_width() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
val f : forall 'n, 'n in {16, 32}. (implicit('n), bits('n)) -> bits('n)
function f(n, v) = {
  result : bits('n) = v;
  result = 0x5 @ sail_zeros(n - 4);
  result
}
"#,
    );
}

#[test]
fn upstream_extended_mutable_set_bounds_survive_match() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
enum choice = A | B | C
function f(x : choice, v : bits(64)) -> bits(64) = {
  sz : {|8,10,15|} = 10;
  match x { A => sz = 15, B => sz = 8, _ => () };
  let w = v[20 .. sz];
  sail_zero_extend(w, 64)
}
"#,
    );
}

#[test]
fn upstream_extended_nested_existential_record_update() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
struct st = { Two : {'n, 'n in {16,32}. (int('n), bits('n))}, Three : (int, {'n, 'n in {16,32}. (int('n), bits('n))}) }
function create(i : int, v : bits(16)) -> st = struct { Two = (16, v), Three = (i, (16, v)) }
function update(v : bits(16), st0 : st) -> st = {
  var st1 = st0;
  st1.Two = (16, v);
  st1.Three = (0, (16, v));
  st1
}
"#,
    );
}

#[test]
fn upstream_extended_nested_existential_constructor() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { Foo : {'n, 'n in {16,32}. (int, (int('n), bits('n)))} }
function make(i : int, v : bits(16)) -> ast = Foo(i,(16,v))
"#,
    );
}

#[test]
fn upstream_extended_payload_type_pattern_names_opened_witness() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union ast = { SomeOp : {'n, 'n in {8,16}. (int('n),bits('n))} }
val execute : ast -> bits(32)
function execute(SomeOp(n as int('n),v)) = {
  a : bits('n) = sail_zero_extend(0x12,n);
  b : bits('n) = and_vec(v, a);
  sail_zero_extend(b,32)
}
"#,
    );
}

#[test]
fn upstream_extended_constructor_annotations_keep_outer_signature() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
union Quux('a : Type) = { Quux : 'a }
function quux(Quux(x as int('x)) : Quux({'n, 'n >= 3. int('n)})) -> unit = {
  let _ = x;
  _prove(constraint('x >= 3));
  ()
}
union ast('D : Int), 'D in {32,64,128} = { Instr : {'R, 'R in {32,64}. (int('R), bits('D))} }
val execute : forall 'd, 'd in {32,64,128}. ast('d) -> unit
function execute(Instr((r as int('R), d))) = {
  let _ = (r, d);
  _prove(constraint('R in {32,64}));
  ()
}
"#,
    );
}

#[test]
fn upstream_extended_constructor_keeps_local_finite_set() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>
type datasize('n : Int) -> Bool = 'n in {32,64}
union ast = { Ctor : {'d, datasize('d). (bits(4), int('d), bits(4))} }
function decode(a : bits(4), b : bool, c : bits(4)) -> ast = {
  let x : {|32,64|} = if b then 32 else 64;
  Ctor(a, x, c)
}
"#,
    );
}

#[test]
fn upstream_extended_exact_constraint_ctor() {
    check_diagnostics(
        r#"
//- prelude
//- strict-bitvector
//- /main.sail
default Order dec

$include <flow.sail>

union Foo = {
  Foo : {'n, 'n >= 3. int('n)}
}

function foo(Foo(x as int('x)): Foo) -> unit = {
  _prove(constraint('x >= 3));
}

union Bar('m), 'm <= 100 = {
  Bar : {'n, 'n >= 'm. int('n)}
}

function bar(Bar(x as int('x)) : Bar(23)) -> unit = {
  _prove(constraint('x >= 23));
  ()
}

union Quux('a : Type) = {
  Quux : 'a
}

function quux(Quux(x as int('x)) : Quux({'n, 'n >= 3. int('n)})) -> unit = {
  _prove(constraint('x >= 3));
  ()
}

"#,
    );
}

#[test]
fn upstream_extended_exact_execute_decode_hard() {
    check_diagnostics(
        r#"
//- prelude
//- strict-bitvector
//- /main.sail
default Order dec

$include <prelude.sail>

union ast('D: Int), 'D in {32, 64, 128} = {
  Instr1 : {'R, 'R in {32, 64}. (int('R), bits('D))}
}

val execute : forall 'd, 'd in {32, 64, 128}. ast('d) -> unit

function clause execute(Instr1(r as int('R), d)) = {
  _prove(constraint('R in {32, 64}));
  if length(d) == 64 then {
    let _ = d[r - 1 .. 0];
    ()
  }
}

function clause execute(Instr1((r as int('R), d))) = {
  _prove(constraint('R in {32, 64}));
  if length(d) == 64 then {
    let _ = d[r - 1 .. 0];
    ()
  }
}


"#,
    );
}

#[test]
fn upstream_extended_exact_exist1() {
    check_diagnostics(
        r#"
//- prelude
//- strict-bitvector
//- /main.sail
let v1 : {|0, 1|} = 0

val test : bool -> {|0, 1|}

function test flag = if flag then 0 else 1

let v2 : range(0, 1) = test(true)

val add : forall 'a 'b. (atom('a), atom('b)) -> atom('a + 'b)

let v3 : {|3, 4|} = 3

let v4 : {'q, 'q in {0, 1}. atom('q + 3)} = v3

let v5 = add(test(true), 4)

let v6 : atom(4) = 4

val unit_fn : atom(4) -> unit

function unit_fn x : atom(4) = ()

val s_add : forall 'a. (atom('a), atom('a)) -> atom('a + 'a)

let v7 : {'k, 'k == 4. atom('k)} = 4

let v8 = s_add(v7, 4)

"#,
    );
}

#[test]
fn upstream_extended_exact_global_type_var() {
    check_diagnostics(
        r#"
//- prelude
//- strict-bitvector
//- /main.sail
$include <flow.sail>

let (size as 'size) : {|32, 64|} = 32

val zeros : forall 'n. atom('n) -> vector ('n, dec, bit)

val test : atom('size) -> unit

function test x =
  if x == 32 then {
    ()
  } else {
    let _y : atom(64) = size in
    ()
  }

val test2 : unit -> atom('size)

function test2 () = size

"#,
    );
}

#[test]
fn upstream_extended_throw_fact() {
    check_diagnostics(
        r#"
//- prelude
//- /main.sail
default Order dec
$include <prelude.sail>

union exception = {
  BadInput : int
}

val test1 : int -> nat effect {escape}

function test1(n) = {
  if (n < 0) then {
    throw (BadInput(n))
  };
  n
}

val test2 : int -> nat effect {escape}

function test2(n) = {
  m : nat = 0;
  if (n < 0) then {
    throw (BadInput(n))
  } else { m = 1 };
  n+m
}

"#,
    );
}
