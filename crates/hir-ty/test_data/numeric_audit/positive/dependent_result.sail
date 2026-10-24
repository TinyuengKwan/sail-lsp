default Order dec
$include <prelude.sail>

val audit_identity : forall 'n. int('n) -> int('n)
val audit : unit -> atom(3)
function audit() = audit_identity(3)
