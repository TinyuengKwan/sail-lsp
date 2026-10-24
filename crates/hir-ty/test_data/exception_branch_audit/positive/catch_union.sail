default Order dec
$include <prelude.sail>

union exception = { Failure : string }
union Other = { Foreign : string }
val audit : unit -> string
function audit() = try { "ok" } catch { Failure(s) => s }
