default Order dec
$include <prelude.sail>

val audit : forall 'n, 'n >= 0. int('n) -> range(0, 'n)
function audit(n) = { var count : range(0, 'n) = -1; count }
