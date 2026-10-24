default Order dec
$include <prelude.sail>

union audit_result('a : Type) = { AuditOk : 'a, AuditErr : unit }
val audit_pack : forall 'n, 'n >= 0. int('n) -> audit_result((vector('n, bits(8)), bits('n)))
val audit : range(1, 16) -> unit
function audit(count) = { let 'n = count; let (v, mask) : (vector('n, bits(8)), bits('n)) = match audit_pack(count + 1) { AuditOk(v) => v, AuditErr(()) => exit() }; () }
