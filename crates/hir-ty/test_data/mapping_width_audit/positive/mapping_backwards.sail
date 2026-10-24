default Order dec
$include <prelude.sail>

mapping encode : bits(5) <-> {1, 2, 4, 8} = { 0b00000 <-> 1, 0b00001 <-> 2, 0b00011 <-> 4, 0b00111 <-> 8 }
val audit : unit -> {1, 2, 4, 8}
function audit() = encode(0b00011)
