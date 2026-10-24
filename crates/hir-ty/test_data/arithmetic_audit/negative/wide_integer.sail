default Order dec
$include <prelude.sail>

val audit_source : unit -> range(0, 340282366920938463463374607431768211456)
val audit : unit -> range(0, 1361129467683753853853498429727072845823)
function audit() = audit_source() * 4
