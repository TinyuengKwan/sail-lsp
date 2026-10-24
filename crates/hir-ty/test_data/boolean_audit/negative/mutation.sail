default Order dec
$include <prelude.sail>

val need : bool(true) -> unit
val audit : unit -> unit
function audit() = { var b : bool = true; b = false; need(b) }
