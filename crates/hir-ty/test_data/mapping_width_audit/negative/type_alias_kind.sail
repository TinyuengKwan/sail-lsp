default Order dec
$include <prelude.sail>

type Wrapped('a : Type) = list('a)
val audit : Wrapped(bits(8)) -> bits(16)
function audit(xs) = match xs { [|x|] => x, _ => 0x0000 }
