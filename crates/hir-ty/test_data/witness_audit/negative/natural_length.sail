default Order dec
$include <prelude.sail>

val audit_make : forall 'n, 'n >= 0. int('n) -> bits('n)
function audit(count : nat, other : nat) -> unit = { let 'n = count; let a : bits('n) = audit_make(other); () }
