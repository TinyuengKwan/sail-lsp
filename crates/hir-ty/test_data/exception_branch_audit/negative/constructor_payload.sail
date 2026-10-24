default Order dec
$include <prelude.sail>

enum ExceptionType = { Fault }
union TrapCause = { Exception : ExceptionType }
val audit : int -> TrapCause
function audit(e) = Exception(e)
