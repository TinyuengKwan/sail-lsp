default Order dec
$include <prelude.sail>

val audit_same : forall 'n. (int('n), int('n)) -> unit
val audit : (range(1, 16), range(1, 16)) -> unit
function audit(count, other) = audit_same(count, count)
