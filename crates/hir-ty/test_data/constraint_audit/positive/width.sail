default Order dec
$include <prelude.sail>
val need : bits(8) -> unit
function need(x) = ()
val audit : bits(8) -> unit
function audit(x) = need(x)
