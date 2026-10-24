default Order dec
$include <prelude.sail>


// audit-body
val audit : forall 'n, 'n > 0. (int('n), bits(2 * 'n)) -> bits('n)
function audit(n, x) = x[n - 1 .. 0]
