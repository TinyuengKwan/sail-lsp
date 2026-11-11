<!-- Generated from the `// Diagnostic:` comments in `crates/ide-diagnostics/src/handlers/` by `cargo test -p ide diagnostics_docs`; edit those and run it with `UPDATE_EXPECT=1` to regenerate. -->

# Diagnostics

Every finding sail-lsp reports on its own carries one of the codes below as its `code`, with `source: "sail-lsp"` and this page as its `codeDescription`; a finding relayed from the `sail` compiler by flycheck says `source: "sail"` instead. These codes are what `diagnostics.disabled`, `diagnostics.warningsAsHint` and `diagnostics.warningsAsInfo` take (see [configuration.md](configuration.md)).

A code marked *not produced* has a handler that no analysis pass feeds yet, so it is never reported; its example shows what it would flag.

## circular-include

Severity: error.

This diagnostic is triggered when a chain of `$include` directives leads back to the file that started it. Every file in the cycle is reported.

```sail
$include "main.sail"
```

Handler: `crates/ide-diagnostics/src/handlers/circular_include.rs`

## deprecated-syntax

Severity: warning.

**Not produced: nothing constructs `AnyDiagnostic::DeprecatedSyntax` yet. The one deprecation the server does report, `deprecated-effect-annotation`, comes from the item-tree pass in `parse.rs` and is disabled by default.**

This diagnostic would be triggered for syntax newer Sail accepts but no longer uses, such as an `effect { ... }` set on a `val`. It carries the `Deprecated` tag, so the editor strikes the construct through, and a quick fix removes it.

```sail
val f : unit -> unit effect {wmv}
```

Handler: `crates/ide-diagnostics/src/handlers/deprecated_syntax.rs`

## duplicate-definition

Severity: error.

This diagnostic is triggered when one pattern binds the same name twice — in a tuple, a constructor's payload or a struct pattern — so the second binding has nothing to be.

```sail
function f(p : (int, int)) -> int = match p { (x, x) => x }
```

Handler: `crates/ide-diagnostics/src/handlers/duplicate_binding.rs`

This diagnostic is triggered when a name is declared twice in one project at the top level — two types, two enums, two functions with no `scattered` or `overload` between them. The second declaration is the one reported, with the first as related information.

```sail
type T = int
type T = bool
```

Handler: `crates/ide-diagnostics/src/handlers/duplicate_definition.rs`

## effect-mismatch

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::EffectMismatchInOverride` yet.**

This diagnostic would be triggered when the `forwards` and `backwards` clauses of a mapping have different effects.

```sail
register R : int
mapping m : int <-> int = {
  forwards x => x,
  backwards x => { R = x; x }
}
```

Handler: `crates/ide-diagnostics/src/handlers/effect_mismatch_in_override.rs`

## effect-violation

Severity: error.

**Not produced: nothing constructs `is_declared_pure: true` yet, so no function body is checked in a pure context.**

This diagnostic would be triggered when a function declared pure reads or writes a register, throws, or calls a function that does. Upstream Sail dropped explicit effect annotations, and every type scheme here is built as not declared pure.

```sail
register R : int
val f : unit -> int
function f() = R
```

Handler: `crates/ide-diagnostics/src/handlers/effect_violation.rs`

## expected-function

Severity: error.

**Not produced: the parser forms a call only when the callee is a name — `f(1)(2)` parses as the call `f(1)` followed by a stray `(2)` — so the inference branch that pushes `InferenceDiagnostic::ExpectedFunction` for a non-name callee is one no program reaches.**

This diagnostic would be triggered when a call's callee is an expression that is not a name — the result of another call, say — so there is no signature to check the arguments against. A name bound to a non-function is `type-error`; a `function` defined against a mapping's `val` is `not-a-function-type`, from the item walker.

```sail
val f : int -> int
function g() -> int = f(1)(2)
```

Handler: `crates/ide-diagnostics/src/handlers/expected_function.rs`

## impossible-constraint

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::ImpossibleConstraint` yet; a constraint the solver refutes is reported as `unsolved-constraint`.**

This diagnostic would be triggered when a declaration's constraints can never all hold, so no value has the type it describes.

```sail
val f : forall 'n, 'n > 0 & 'n < 0. int('n) -> unit
```

Handler: `crates/ide-diagnostics/src/handlers/impossible_constraint.rs`

## inactive-code

Severity: hint.

This diagnostic is triggered for a region the preprocessor did not take — an `$ifdef`, `$ifndef` or `$iftarget` whose condition is false for the current configuration, or the `$else` of one that is true. It is a hint, so the editor fades the region rather than flagging it.

```sail
$ifdef NEVER_DEFINED
val hidden : unit -> unit
$endif
```

Handler: `crates/ide-diagnostics/src/handlers/inactive_code.rs`

## incomplete-match

Severity: warning.

This diagnostic is triggered when a `match` does not cover every value its scrutinee can take: an enum member, a union constructor or a literal is missing and no wildcard arm catches it. The missing cases are listed in the message.

```sail
enum E = {A, B, C}
function g(e : E) -> int = match e { A => 1, B => 2 }
```

Handler: `crates/ide-diagnostics/src/handlers/missing_match_arms.rs`

## incomplete-scattered

Severity: error or hint.

**Not produced: nothing constructs `AnyDiagnostic::IncompleteScattered` yet; the per-file name-resolution finding of the same name is dropped because an `end` may be in another file.**

This diagnostic would be triggered when a `scattered` definition has no `end`, or has clauses but no declaration. As a hint it marks a definition that is merely still open; as an error, one the project closes nowhere.

```sail
scattered function exec
function clause exec(x) = x
```

Handler: `crates/ide-diagnostics/src/handlers/incomplete_scattered.rs`

## inconsistent-hex-casing

Severity: warning.

**Not produced: nothing constructs `AnyDiagnostic::InconsistentHexCasing` yet.**

This diagnostic would be triggered when a hexadecimal literal mixes upper- and lower-case digits.

```sail
let x = 0xAbCd
```

Handler: `crates/ide-diagnostics/src/handlers/inconsistent_hex_casing.rs`

## invalid-list-pattern

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::InvalidListPattern` yet.**

This diagnostic would be triggered when a cons pattern `x :: xs` is written where the scrutinee is not a list, or its tail is not itself a list pattern.

```sail
function f(l : list(int)) -> int = match l { x :: 1 => x, _ => 0 }
```

Handler: `crates/ide-diagnostics/src/handlers/invalid_list_pattern.rs`

## invalid-slice-assign

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::InvalidSliceAssign` yet.**

This diagnostic would be triggered when a vector slice is assigned a value of a different width than the slice.

```sail
register R : bits(8)
function f() -> unit = { R[7 .. 4] = 0x000 }
```

Handler: `crates/ide-diagnostics/src/handlers/invalid_slice_assign.rs`

## invalid-string-pattern

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::InvalidStringPattern` yet.**

This diagnostic would be triggered when a string-append pattern `"a" ^ rest` has an operand that is not a string literal or a string binding.

```sail
function f(s : string) -> int = match s { "a" ^ 1 => 0, _ => 1 }
```

Handler: `crates/ide-diagnostics/src/handlers/invalid_string_pattern.rs`

## invalid-vector-concat

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::InvalidVectorConcat` yet; a non-vector operand of `@` is reported as `type-error` by the concatenation check.**

This diagnostic would be triggered when a vector concatenation `@` is applied to something that is not a vector.

```sail
function f() -> bits(8) = 0x0 @ true
```

Handler: `crates/ide-diagnostics/src/handlers/invalid_vector_concat.rs`

## mismatched-arg-count

Severity: error.

This diagnostic is triggered when a call passes a different number of arguments than the callee's signature declares. An unfinished call — one whose `)` has not been typed yet — is not reported.

```sail
val takes_two : (int, int) -> int
function takes_two(x, y) = x
function test() -> int = takes_two(1)
```

Handler: `crates/ide-diagnostics/src/handlers/mismatched_arg_count.rs`

## mismatched-param-count

Severity: error.

This diagnostic is triggered when a function definition binds fewer parameters than its `val` specification declares. One parameter against several is not reported: it binds the whole argument tuple.

```sail
val f : (int, int, int) -> int
function f(x, y) = x
```

Handler: `crates/ide-diagnostics/src/handlers/mismatched_param_count.rs`

## missing-effect-annotation

Severity: warning.

**Not produced: nothing constructs `AnyDiagnostic::MissingEffectAnnotation` yet.**

This diagnostic would be triggered when a function with side effects has no effect annotation. Upstream Sail has dropped explicit effect sets, so there is nothing a fix could add.

```sail
register R : int
val f : unit -> int
function f() = R
```

Handler: `crates/ide-diagnostics/src/handlers/missing_effect_annotation.rs`

## missing-fields

Severity: error.

This diagnostic is triggered when a struct pattern names its struct and leaves out fields the struct declares without a `_` to stand for them, or when a struct literal that names its struct leaves out fields. (A literal that does not name its struct is checked by the item walker instead, as `missing-struct-fields`.) A quick fix on a literal inserts the missing fields with placeholder values.

```sail
struct S = { x : int, y : bool }
function f(s : S) -> unit = match s { struct S { x } => () }
```

Handler: `crates/ide-diagnostics/src/handlers/missing_fields.rs`

## redundant-match-arm

Severity: warning.

This diagnostic is triggered when a `match` arm's pattern is already covered by the arms before it, so the arm can never run.

```sail
enum E = {A, B}
function g(e : E) -> int = match e { A => 1, _ => 2, B => 3 }
```

Handler: `crates/ide-diagnostics/src/handlers/redundant_match_arm.rs`

## redundant-type-annotation

Severity: warning.

**Not produced: nothing constructs `AnyDiagnostic::RedundantTypeAnnotation` yet.**

This diagnostic would be triggered when a `let` writes a type annotation that is exactly the type its initializer already has.

```sail
function f() -> int = { let x : int = 1 : int; x }
```

Handler: `crates/ide-diagnostics/src/handlers/redundant_type_annotation.rs`

## remove-trailing-return

Severity: hint.

This diagnostic is triggered when a function body ends in `return e`: the last expression of a body is its value, so the `return` says nothing. A quick fix removes the keyword.

```sail
function f() -> int = { let x : int = 3; return x }
```

Handler: `crates/ide-diagnostics/src/handlers/remove_trailing_return.rs`

## remove-unnecessary-else

Severity: warning.

This diagnostic is triggered when an `if` whose `then` branch always diverges — it returns, throws or exits — still has an `else`. What the `else` guards can be written after the `if` at the same level, which a quick fix does.

```sail
function f(c : bool) -> int = { if c then return 1 else 2 }
```

Handler: `crates/ide-diagnostics/src/handlers/remove_unnecessary_else.rs`

## return-outside-function

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::ReturnOutsideFunction` yet.**

This diagnostic would be triggered when `return` appears where there is no function to return from, such as a top-level `let`.

```sail
let x = return 1
```

Handler: `crates/ide-diagnostics/src/handlers/return_outside_function.rs`

## shadowed-binding

Severity: warning.

**Not produced: nothing constructs `AnyDiagnostic::ShadowedBinding` yet.**

This diagnostic would be triggered when a `let` binds a name an enclosing scope already binds, hiding the outer binding for the rest of the block.

```sail
function f(x : int) -> int = { let x = 2; x }
```

Handler: `crates/ide-diagnostics/src/handlers/shadowed_binding.rs`

## type-error

Severity: error.

This diagnostic is triggered when an operand of the concatenation operator `@` is not a vector: an integer, a boolean or a user type that is not a bitvector alias.

```sail
function f(x : bits(4), n : int) -> bits(8) = x @ n
```

Handler: `crates/ide-diagnostics/src/handlers/concat_type_mismatch.rs`

**Not produced: nothing constructs `InferenceDiagnostic::MappingBindingMismatch` yet.**

This diagnostic would be triggered when a bidirectional `mapping` clause binds a name on one side of `<->` and never mentions it on the other, so one direction of the mapping has no value for it. Such a clause is reported today as the `type-error` the unbound side causes.

```sail
mapping m : bits(2) <-> bits(2) = { 0b0 @ x <-> 0b00 }
```

Handler: `crates/ide-diagnostics/src/handlers/mapping_binding_mismatch.rs`

This diagnostic is triggered when a call names an `overload` and none of its members accepts the arguments given, or when the callee is a name no function declares — upstream's `No possible overloading` and `Function not found`.

```sail
function g() -> unit = nope(1)
```

Handler: `crates/ide-diagnostics/src/handlers/no_overloading.rs`

This diagnostic is triggered when a vector-concatenation pattern binds one name through several subranges that do not join up — `imm[15 .. 6] @ imm[4 .. 0]` leaves bit 5 undefined — or whose lowest subrange does not start at bit 0.

```sail
default Order dec
val unsigned : forall 'n. bits('n) -> int
union ast = { I : bits(16) }
function g(x : ast) -> int = match x {
  I(imm[15 .. 6] @ imm[4 .. 0] @ rest[0 .. 0]) => unsigned(imm) + unsigned(rest)
}
```

Handler: `crates/ide-diagnostics/src/handlers/non_contiguous_subrange.rs`

This diagnostic is triggered when a call's callee is a name that is bound — to a local, a register or a constant — but not to anything callable.

```sail
function f() -> unit = { let x : int = 1; let _ = x(1); () }
```

Handler: `crates/ide-diagnostics/src/handlers/not_a_function.rs`

This diagnostic is triggered when an expression's type is not the one its context expects. Two shapes that line up but whose constraint does not hold say `X is not a subtype of Y`; two shapes that never met say `Type mismatch between Y and X`, as upstream Sail does.

```sail
val g : unit -> bool
function f() -> int = 4 * g()
```

Handler: `crates/ide-diagnostics/src/handlers/type_mismatch.rs`

This diagnostic is triggered when a vector subrange `v[hi .. lo]` writes its indices against the default order: with `default Order dec` the first index must be the greater one.

```sail
default Order dec
val unsigned : forall 'n. bits('n) -> int
union ast = { I : bits(16) }
function f(x : ast) -> int = match x {
  I(imm[0 .. 2] @ rest[12 .. 0]) => unsigned(imm) + unsigned(rest)
}
```

Handler: `crates/ide-diagnostics/src/handlers/vector_subrange_order.rs`

## undeclared-effect

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::UndeclaredEffect` yet.**

This diagnostic would be triggered when a function performs an effect — a register write, say — that its `val` specification's effect set does not declare.

```sail
register R : int
val f : unit -> unit effect {rreg}
function f() = R = 1
```

Handler: `crates/ide-diagnostics/src/handlers/undeclared_effect.rs`

## undeclared-fixity

Severity: error.

This diagnostic is triggered when an infix operator is used without an `infix`, `infixl` or `infixr` declaration giving it a precedence, and it is not one of the operators upstream Sail's grammar hardcodes. Upstream refuses such a program; the parser here still builds a tree.

```sail
val operator =% : (int, int) -> int
function f() -> int = 1 =% 2
```

Handler: `crates/ide-diagnostics/src/handlers/undeclared_fixity.rs`

## undeclared-mapping-type

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::UndeclaredMappingType` yet.**

This diagnostic would be triggered when a `mapping` is defined without a `val` specification or an inline type giving its two sides.

```sail
mapping m = { 0b0 <-> true }
```

Handler: `crates/ide-diagnostics/src/handlers/undeclared_mapping_type.rs`

## unmodified-mutable-variable

Severity: warning.

**Not produced: nothing constructs `AnyDiagnostic::UnnecessaryMutability` yet.**

This diagnostic would be triggered when a `var` is never assigned after its initialization, so it could be a `let`, which a quick fix makes it.

```sail
function f() -> int = { var x : int = 1; x }
```

Handler: `crates/ide-diagnostics/src/handlers/unnecessary_mutability.rs`

## unreachable-code

Severity: hint.

This diagnostic is triggered when a statement follows one that always diverges — a `return`, `throw` or `exit` — in the same block, so it can never run. A quick fix removes it.

```sail
function f() -> int = { return 1; 2 }
```

Handler: `crates/ide-diagnostics/src/handlers/unreachable_code.rs`

## unresolved-field

Severity: error.

This diagnostic is triggered when a field is read, written or matched on a struct or bitfield that does not declare it.

```sail
struct Record = { a : bits(32) }
function test(r : Record) -> bits(32) = r.zzz
```

Handler: `crates/ide-diagnostics/src/handlers/unresolved_field.rs`

## unresolved-ident

Severity: error.

This diagnostic is triggered when a name is used in an expression and nothing in scope — no binding, parameter, register, `let`, enum member or declaration in the project — declares it.

```sail
val g : int -> int
function f() -> int = g(zzz)
```

Handler: `crates/ide-diagnostics/src/handlers/unresolved_ident.rs`

## unresolved-include

Severity: error.

This diagnostic is triggered when an `$include` names a file that is not in the project: not on disk at the path written, and not a library header the configured Sail installation provides.

```sail
$include "missing.sail"
```

Handler: `crates/ide-diagnostics/src/handlers/unresolved_include.rs`

## unresolved-quants

Severity: error.

**Not produced: nothing constructs `AnyDiagnostic::UnresolvedQuants` yet.**

This diagnostic would be triggered when a type variable quantified in a signature appears nowhere in the parameters, so no call can determine it.

```sail
val f : forall 'n. unit -> bits('n)
```

Handler: `crates/ide-diagnostics/src/handlers/unresolved_quants.rs`

## unsolved-constraint

Severity: error.

This diagnostic is triggered when a type-level constraint — a bound on a width or an integer type variable — cannot be shown to hold at a call or a binding. When the solver could not decide the constraint rather than refute it, the finding says so and the server marks the analysis as unknown.

```sail
val q : forall 'n, 'n >= 0. int('n) -> bits('n)
function f(n : int) -> unit = { let _ = q(n); () }
```

Handler: `crates/ide-diagnostics/src/handlers/unsolved_constraint.rs`

## unused-import

Severity: warning.

**Not produced: nothing constructs `AnyDiagnostic::UnusedImport` yet.**

This diagnostic would be triggered when an `$include` brings in a file none of whose declarations the including file uses. A quick fix removes the directive.

```sail
$include "helpers.sail"
function f() -> int = 1
```

Handler: `crates/ide-diagnostics/src/handlers/unused_import.rs`

## unused-variable

Severity: warning.

This diagnostic is triggered when a `let`/`var` binding or a function parameter is never read in its scope. A quick fix prefixes the name with `_`.

```sail
function f() -> int = { let x = 1; 2 }
```

Handler: `crates/ide-diagnostics/src/handlers/unused_variables.rs`
