default Order dec
$include <prelude.sail>

val audit_need : atom(0) -> unit
val audit : forall 'n. int('n) -> unit
function audit(n) = match n { 0 => audit_need(n), _ => () }
