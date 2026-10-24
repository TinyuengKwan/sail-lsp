default Order dec
$include <prelude.sail>

val audit : forall ('a : Type). list('a) -> list('a)
function audit(xs) = xs
