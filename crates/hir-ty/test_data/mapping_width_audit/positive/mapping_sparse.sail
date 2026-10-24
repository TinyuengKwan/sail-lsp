default Order dec
$include <prelude.sail>

scattered union AuditInstruction
val audit : AuditInstruction <-> bits(6)
union clause AuditInstruction = AuditMove : (bits(1), {1, 2, 4, 8})
mapping encode : bits(5) <-> {1, 2, 4, 8} = { 0b00000 <-> 1, 0b00001 <-> 2, 0b00011 <-> 4, 0b00111 <-> 8 }
mapping clause audit = AuditMove(b, n) <-> b @ encode(n)
end AuditInstruction
