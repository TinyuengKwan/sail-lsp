default Order dec
$include <prelude.sail>

val same : forall ('p : Bool). (bool('p), bool('p)) -> unit
val audit : (bool, bool) -> unit
function audit(p, q) = same(p, q)
