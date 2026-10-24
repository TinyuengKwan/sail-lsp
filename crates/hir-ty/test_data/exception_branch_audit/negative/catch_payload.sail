default Order dec
$include <prelude.sail>

union exception = { Failure : string }
val audit : unit -> int
function audit() = try { 0 } catch { Failure(s) => s }
