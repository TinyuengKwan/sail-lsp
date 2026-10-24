default Order dec
$include <prelude.sail>

val audit_source : unit -> {3, 5, 6}
val audit : unit -> {8, 64}
function audit() = 2 ^ audit_source()
