default Order dec
$include <prelude.sail>

val audit_need : range(0, 7) -> unit
val audit : unit -> unit
function audit() = audit_need(8)
