default Order dec
$include <prelude.sail>

scattered union AuditInstruction
val audit : AuditInstruction <-> bits(8)
union clause AuditInstruction = AuditMove : {1, 2, 4, 8}
mapping encode_string : {1, 2, 4, 8} <-> string = { 1 <-> "1", 2 <-> "2", 4 <-> "4", 8 <-> "8" }
mapping clause audit = AuditMove(n) <-> encode_string(n)
end AuditInstruction
