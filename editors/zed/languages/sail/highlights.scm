; Keywords
;---------
[
  "as" "by" "catch" "clause" "config" "constant" "constraint" "dec" "default"
  "do" "else" "end" "foreach" "forall" "function" "if" "impure" "in" "inc"
  "instantiation" "let" "match" "overload" "pure" "ref" "register"
  "repeat" "return" "scattered" "sizeof" "termination_measure" "then" "throw" 
  "try" "until" "val" "var" "while" "with" "when"
] @keyword

((id) @keyword
  (#eq? @keyword "private"))

((identifier) @keyword
  (#eq? @keyword "private"))

[
  "if" "then" "else" "match" "try" "catch" "throw" "return" "foreach" "while"
  "do" "repeat" "until"
] @keyword.control

["type" "struct" "enum" "union" "bitfield" "mapping"] @keyword.type
["forwards" "backwards"] @keyword
["infix" "infixl" "infixr"] @keyword

; Types
;------
(type_variable) @type.parameter
(typ_var) @type.parameter

(atomic_typ
  (id) @type)

(type_def
  (id) @type.definition)

["Int" "Nat" "Type" "Order" "Bool"] @type.builtin

; Fallback identifier capture (kept before specialized captures so they can override)
(id) @variable

; Functions
;----------
(fun_def
  (funcls
    (id) @function))

(funcl
  (id) @function)

(map_def
  (id) @function)

(atomic_exp
  (id) @function.call
  "(")

(atomic_exp
  (atomic_exp)
  "."
  (id) @method.call
  "(")

(extern_binding
  (id) @function
  ":"
  (string_literal) @string)

(val_spec_def
  "val"
  (id)? @function
  (string_literal)? @string)

; Variables and Properties
;-------------------------
(letbind
  (pat
    (pat1
      (atomic_pat
        (id) @variable))))

(case
  (pat
    (pat1
      (atomic_pat
        (id) @variable))))

(mpexp
  (mpat
    (atomic_mpat
      (id) @variable)))

(fpat
  (id) @parameter)

(fmpat
  (id) @parameter)

(struct_field
  (id) @property)

(atomic_exp
  (atomic_exp)
  "."
  (id) @property)

; Literals
;---------
(lit) @constant
["undefined" "bitzero" "bitone"] @constant.builtin
"()" @constant.builtin
["true" "false"] @boolean

(number) @number
(hexadecimal_literal) @number
(binary_literal) @number
(string_literal) @string

; Operators and Punctuation
;--------------------------
[
  "->" "<->" "=>" "=" "::" ":" ".." "." "," ";" "@" "^" "|" "-" "*"
  "and" "or" "not" "xor" "in"
] @operator

(operator) @operator

["(" ")" "[" "]" "{" "}" "[|" "|]"] @punctuation.bracket
["," ";" ":"] @punctuation.delimiter

; Attributes and Preprocessor
;----------------------------
(attribute
  name: (identifier) @attribute)

(attribute_data
  (number) @number)

(attribute_data
  (string_literal) @string)

(attribute_data
  (identifier) @label)

(attribute_data_key_value
  (identifier) @label)

"pure" @attribute
["Private"] @storageclass

(line_directive
  directive: (line_directive_name) @preproc)

; Special Forms
;--------------
(id
  "operator" @keyword
  (operator) @operator)

(enum_comma
  (id) @constructor)

(type_union
  (id) @constructor)

; Comments
;---------
(comment) @comment
