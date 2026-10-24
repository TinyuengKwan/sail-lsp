default Order dec
$include <prelude.sail>

newtype audit_a = AuditA : bits(34)
newtype audit_b = AuditB : bits(32)
type audit_alias = audit_b
val audit_a_bits : audit_a -> bits(34)
val audit_b_bits : audit_b -> bits(32)
overload audit_bits = { audit_a_bits, audit_b_bits }
val audit : (nat, audit_alias) -> bits(34)
function audit(step, addr) = audit_bits(addr)
