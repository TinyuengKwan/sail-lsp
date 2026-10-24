default Order dec
$include <prelude.sail>

val audit : range(0, 7) -> range(3, 6)
function audit(n if n < 4) = n + 3
