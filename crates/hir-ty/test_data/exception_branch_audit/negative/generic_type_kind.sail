default Order dec
$include <prelude.sail>

val audit : forall ('a : Type) ('b : Type). list('a) -> list('b)
function audit(xs) = xs
