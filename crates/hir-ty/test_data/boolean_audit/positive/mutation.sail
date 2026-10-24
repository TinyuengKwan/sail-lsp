default Order dec
$include <prelude.sail>

val audit : unit -> bool
function audit() = { var b : bool = true; b = false; b }
