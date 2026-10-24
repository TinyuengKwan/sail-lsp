default Order dec
$include <prelude.sail>

val audit : forall 'n 'm, 'n >= 'm & 'm >= 0. (implicit('m), bits('n), bits('m)) -> bits('m)
function audit(m, x, fallback) = if 'n != 'm then x else fallback
