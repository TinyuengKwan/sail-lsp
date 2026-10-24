default Order dec
$include <prelude.sail>

val audit_need : range(0, 7) -> unit
val audit : unit -> unit
function audit() = foreach (i from 7 downto -1) audit_need(i)
