default Order dec
$include <prelude.sail>

val audit_need : range(0, 7) -> unit
val audit : unit -> unit
function audit() = foreach (i from 7 to 0 by 1 in dec) audit_need(i)
