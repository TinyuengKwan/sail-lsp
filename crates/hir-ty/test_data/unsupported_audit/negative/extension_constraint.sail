default Order dec
$include <prelude.sail>

val audit_extend : forall 'n 'm, 'm >= 'n & 'n >= 0. (implicit('m), bits('n)) -> bits('m)
// audit-body
val audit : bits(8) -> bits(4)
function audit(x) = audit_extend(x)
