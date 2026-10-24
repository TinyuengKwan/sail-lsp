default Order dec
$include <prelude.sail>

val audit : forall ('p : Bool) ('q : Bool). (bool('p), bool('q)) -> bool('p & 'q)
function audit(p, q) = if p then q else true
