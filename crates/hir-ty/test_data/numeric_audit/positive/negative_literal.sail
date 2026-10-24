default Order dec
$include <prelude.sail>

val audit_need : range(-3, 0) -> unit
val audit : unit -> unit
function audit() = audit_need(-2)
