default Order dec
$include <prelude.sail>


val audit_length : forall 'n. bits('n) -> int('n)
// audit-body
val audit : forall 'n, 'n > 0. bits('n) -> bits('n)
function audit(x) = x[audit_length(x) - 1 .. 0]
