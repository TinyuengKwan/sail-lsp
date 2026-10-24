default Order dec
$include <prelude.sail>

val audit_source : unit -> nat
val audit : unit -> nat
function audit() = audit_source() * 4 + audit_source()
