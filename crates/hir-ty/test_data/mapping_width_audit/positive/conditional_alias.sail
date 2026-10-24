default Order dec
$include <prelude.sail>

type audit_pte('v) = bits(if 'v == 32 then 32 else 64)
val audit : forall 'v, 'v in {32, 39, 48, 57}. bits(8 * 2 ^ (if 'v == 32 then 2 else 3)) -> audit_pte('v)
function audit(pte) = pte
