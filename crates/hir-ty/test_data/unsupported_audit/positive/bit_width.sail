default Order dec
$include <prelude.sail>

val audit_identity : forall 'n. bits('n) -> bits('n)
function audit_identity(x) = x
// audit-body
val audit : bit -> bits(1)
function audit(x) = audit_identity(x)
