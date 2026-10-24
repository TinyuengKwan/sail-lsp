default Order dec
$include <prelude.sail>

union AuditResult('a : Type) = { AuditOk : 'a, AuditErr : unit }
type AuditMemory('a : Type) = AuditResult('a)
val load : forall 'n, 'n in {4, 8}. int('n) -> AuditMemory(bits(8 * 'n))
val consume : forall 'n, 'n in {32, 128}. bits('n) -> unit
val audit : forall 'v, 'v in {32, 39, 48, 57}. int('v) -> unit
function audit(v) = { let 'log_bytes = if 'v == 32 then 2 else 3; match load(2 ^ log_bytes) { AuditErr(_) => (), AuditOk(pte) => consume(pte) } }
