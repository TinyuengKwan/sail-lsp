default Order dec
$include <prelude.sail>

val audit_source : forall 'n, 'n >= 1. int('n) -> range(0, 'n)
val audit : forall 'n, 'n >= 1. int('n) -> range(0, 4 * 'n)
function audit(n) = audit_source(n) * 4
