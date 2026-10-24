default Order dec
$include <prelude.sail>

val audit_not : forall ('p : Bool). bool('p) -> bool(not('p))
val audit : unit -> bool(true)
function audit() = audit_not(true)
