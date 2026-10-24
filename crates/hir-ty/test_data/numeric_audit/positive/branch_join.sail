default Order dec
$include <prelude.sail>

val audit : bool -> range(0, 2)
function audit(b) = if b then 1 else 2
