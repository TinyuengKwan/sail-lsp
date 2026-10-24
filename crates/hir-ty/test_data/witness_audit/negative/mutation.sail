default Order dec
$include <prelude.sail>

val audit_make : forall 'n, 'n >= 0. int('n) -> bits('n)
val audit_need : range(1, 16) -> unit
val audit : unit -> unit
function audit() = { var count : range(1, 16) = 4; let 'n = count; count = 5; let a : bits('n) = audit_make(count); () }
