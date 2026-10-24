default Order dec
$include <prelude.sail>

val audit : {3, 5, 6} -> {8, 32, 64}
function audit(n) = 2 ^ n
