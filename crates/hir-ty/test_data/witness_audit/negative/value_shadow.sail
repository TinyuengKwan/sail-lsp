default Order dec
$include <prelude.sail>

val audit_make : forall 'n, 'n >= 0. int('n) -> bits('n)
val audit : (range(1, 16), range(1, 16)) -> unit
function audit(count, other) = { let 'n = count; { let count = other; let a : bits('n) = audit_make(count); () }; () }
