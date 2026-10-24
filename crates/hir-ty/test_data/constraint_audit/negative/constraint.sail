default Order dec
$include <prelude.sail>
val need : forall 'n, 'n == 8. bits('n) -> unit
function need(x) = ()
val audit : bits(16) -> unit
function audit(x) = need(x)
