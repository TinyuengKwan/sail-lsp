default Order dec
$include <prelude.sail>

val need : bool(true) -> bool
val audit : bool -> bool
function audit(b) = if b then need(b) else false
