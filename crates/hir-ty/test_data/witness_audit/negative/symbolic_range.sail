default Order dec
$include <prelude.sail>

type audit_limit : Int
constraint audit_limit >= 1
val audit_make : forall 'n, 'n >= 0. int('n) -> bits('n)
function audit(count : range(1, audit_limit), other : range(1, audit_limit)) -> unit = { let 'n = count; let a : bits('n) = audit_make(other); () }
