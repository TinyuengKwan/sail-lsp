default Order dec
$include <prelude.sail>

val consume : forall 'n, 'n in {32, 64}. bits('n) -> unit
val audit : forall 'v, 'v in {32, 39, 48, 57}. bits(8 * 2 ^ (if 'v == 32 then 2 else 3)) -> unit
function audit(pte) = consume(pte)
