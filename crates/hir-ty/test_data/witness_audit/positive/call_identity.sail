default Order dec
$include <prelude.sail>

val audit_make : forall 'n, 'n >= 0. int('n) -> bits('n)
val audit_count : unit -> range(1, 16)
val audit : unit -> unit
function audit() = { let count = audit_count(); let 'n = count; let a : bits('n) = audit_make(count); let b : bits('n) = audit_make(count); () }
