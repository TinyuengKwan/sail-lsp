default Order dec
$include <prelude.sail>

val need : bool(true) -> unit
val audit : bool -> unit
function audit(b) = { match b { true => need(b), false => () }; need(b) }
