default Order dec
$include <prelude.sail>

val audit_source : unit -> range(3, 6)
val audit : unit -> {8, 16, 32}
function audit() = 2 ^ audit_source()
