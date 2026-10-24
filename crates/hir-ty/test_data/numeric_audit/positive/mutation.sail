default Order dec
$include <prelude.sail>

val audit_need : range(0, 3) -> unit
val audit : unit -> unit
function audit() = { var n : range(0, 3) = 0; n = 2; audit_need(n) }
