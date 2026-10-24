default Order dec
$include <prelude.sail>

val audit_nonnegative : forall 'n, 'n >= 0. int('n) -> int('n)
function audit_nonnegative(n) = n
// audit-body
val audit : range(-2, -1) -> int
function audit(n) = audit_nonnegative(n)
