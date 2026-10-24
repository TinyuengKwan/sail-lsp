default Order dec
$include <prelude.sail>

val audit_left : unit -> range(-3, 2)
val audit_right : unit -> range(-4, 5)
val audit : unit -> range(-14, 12)
function audit() = audit_left() * audit_right()
