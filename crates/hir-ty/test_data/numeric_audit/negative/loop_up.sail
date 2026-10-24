default Order dec
$include <prelude.sail>

val audit_need : range(0, 7) -> unit
val audit : unit -> unit
function audit() = foreach (i from 0 to 8) audit_need(i)
