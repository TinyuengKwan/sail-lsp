default Order dec
$include <prelude.sail>

val audit_zeros : forall 'n, 'n >= 0. implicit('n) -> bits('n)
// audit-body
val audit : unit -> bits(6)
function audit() = audit_zeros(5)
