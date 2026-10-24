default Order dec
$include <prelude.sail>

val audit_need : atom(0) -> unit
val audit : forall 'n. (int('n), bool) -> unit
function audit(n, b) = match (n, b) { (0, _) => audit_need(n), _ => () }
