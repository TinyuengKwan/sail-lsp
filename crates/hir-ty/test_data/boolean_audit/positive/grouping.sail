default Order dec
$include <prelude.sail>

val audit : forall ('p : Bool) ('q : Bool) ('r : Bool). (bool('p), bool('q), bool('r)) -> bool('p & ('q | 'r))
function audit(p, q, r) = p & (q | r)
