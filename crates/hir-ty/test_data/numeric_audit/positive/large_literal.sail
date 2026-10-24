default Order dec
$include <prelude.sail>

val audit_need : atom(340282366920938463463374607431768211456) -> unit
val audit : unit -> unit
function audit() = audit_need(340282366920938463463374607431768211456)
