default Order dec
$include <prelude.sail>

val identity : forall ('p : Bool). bool('p) -> bool('p)
val audit : unit -> bool(true)
function audit() = identity(false)
