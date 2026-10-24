default Order dec
$include <prelude.sail>

val audit_need : atom(4) -> unit
val audit : unit -> unit
function audit() = { let x = 3; let y = x; audit_need(y) }
