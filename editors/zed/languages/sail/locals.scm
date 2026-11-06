; Scopes
[
  (funcl_patexp)
  (funcl_patexp_typ)
  (letbind)
  (block)
  (case)
  (mapcl0)
  (quantifier)
] @local.scope

; Definitions
(funcl_patexp
  (pat
    (pat1
      (atomic_pat
        (id) @local.definition))))

(funcl_patexp_typ
  (pat
    (pat1
      (atomic_pat
        (id) @local.definition))))

(letbind
  (pat
    (pat1
      (atomic_pat
        (id) @local.definition))))

(case
  (pat
    (pat1
      (atomic_pat
        (id) @local.definition))))

(mpexp
  (mpat
    (atomic_mpat
      (id) @local.definition)))

(fpat
  (id) @local.definition)

(fmpat
  (id) @local.definition)

(quantifier
  (kopt
    (typ_var) @local.definition))

(param_kopt
  (typ_var) @local.definition)

(exp
  "foreach"
  "("
  (id) @local.definition)

(exp
  "var"
  (atomic_exp
    (id) @local.definition)
  "=")

(block
  "var"
  (atomic_exp
    (id) @local.definition)
  "=")

; References
(id) @local.reference
(typ_var) @local.reference
