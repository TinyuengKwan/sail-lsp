use super::*;

impl<'t> Parser<'t> {

    pub(crate) fn parse_definition(&mut self) {
        let def_kind = self.classify_definition();
        let m = self.start();
        // Wrap `private` in a VISIBILITY child node.
        if self.at(T![private]) {
            let vm = self.start();
            self.bump_any(); // private
            vm.complete(self, SK::VISIBILITY);
        }
        let end_pos = self.next_def_start_pos(self.pos());

        match def_kind {
            SK::CALLABLE_DEF => {
                // Bump keywords + name until structural delimiter.
                let mut bumped_name = false;
                while self.pos() < end_pos && !self.at_end() {
                    let k = self.current();
                    // Skip forall quantifier: bump everything until `.`
                    if k == T![forall] {
                        self.bump_any(); // forall
                        while self.pos() < end_pos && !self.at_end() && !self.at(T![.]) {
                            self.bump_any();
                        }
                        if self.at(T![.]) {
                            self.bump_any(); // .
                        }
                        continue;
                    }
                    // Stop scanning function name at structural
                    // delimiters and pattern-start tokens.
                    if k == T!['(']
                        || k == SK::UNIT
                        || k == T![->]
                        || k == T![:]
                        || k == T![_]
                        || (k == T![=] && self.nth(1) != T![=] && self.nth(1) != T![>])
                    {
                        break;
                    }
                    // After bumping the function name (first IDENT), if the
                    // next token is also IDENT, it's a bare parameter — stop
                    // so Phase 2 can handle it as a PARAM_LIST.
                    // e.g., `function set_FPEXC value_name = { ... }`
                    //        `function test flag = if flag then 0 else 1`
                    if bumped_name && k == SK::IDENT {
                        break;
                    }
                    if self.pos() >= end_pos {
                        break;
                    }
                    if k == SK::IDENT {
                        bumped_name = true;
                    }
                    self.bump_any();
                }

                // Parse param list(s) as PARAM_LIST nodes.
                self.parse_param_list(end_pos);

                // Optional `-> RetType` or `: Type`.
                if self.at(T![->]) && self.pos() < end_pos {
                    self.bump_any(); // ->
                    self.parse_type_expr(TYPE_RECOVERY);
                } else if self.at(T![:]) && self.pos() < end_pos {
                    self.bump_any(); // :
                    self.parse_type_expr(TYPE_RECOVERY);
                }

                // `= body`.
                let mut found_eq = false;
                if self.at(T![=])
                    && self.pos() < end_pos
                    && self.nth(1) != T![=]
                    && self.nth(1) != T![>]
                {
                    found_eq = true;
                    let assign_m = self.start();
                    self.bump_any(); // =
                    self.parse_expr();
                    // Handle `target = value` assignment in non-block body.
                    // Without this, `function f() -> unit = reg = expr`
                    // parses body as just `reg` (Ident), losing the assignment.
                    if self.at(T![=])
                        && self.pos() < end_pos
                        && self.nth(1) != T![=]
                        && self.nth(1) != T![>]
                    {
                        self.bump_any(); // =
                        self.parse_expr();
                        assign_m.complete(self, SK::ASSIGN_EXPR);
                    } else {
                        assign_m.abandon(self);
                    }
                }

                // Detect missing `=` when unparsed content remains.
                if !found_eq && self.pos() < end_pos {
                    if self.pos() < end_pos
                        && !self.at_end()
                        && !self.at_set(&DEF_RECOVERY_SET)
                        && !matches!(
                            self.current(),
                            T!['{'] | T!['}'] | T![;] | T![<->] | T![->] | T![:] | T![<]
                        )
                    {
                        self.error("expected `=` before function body".to_string());
                    }
                }
            }
            SK::CALLABLE_SPEC => {
                self.bump_any(); // val/mapping
                                 // val can also take a STRING_LIT as the name (val "add_bits" : ...)
                if self.at(SK::IDENT) || self.at(SK::STRING_LIT) || self.at(T![cast]) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                    // After `cast`, there may be an IDENT name
                    if self.at(SK::IDENT) {
                        let nm2 = self.start();
                        self.bump_any();
                        nm2.complete(self, SK::NAME);
                    }
                }
                // Form 1: val name = externs : typschm
                if self.at(T![=]) && self.nth(1) != T![=] && self.nth(1) != T![>] {
                    self.bump_any(); // =
                    self.parse_externs(end_pos);
                    if self.at(T![:]) {
                        self.bump_any();
                        self.parse_type_expr(TYPE_RECOVERY);
                    }
                    // Optional deprecated effect annotation.
                    self.skip_effect_annotation();
                }
                // Form 2: val name : typschm [= externs]
                else if self.at(T![:]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                    // Optional deprecated effect annotation.
                    self.skip_effect_annotation();
                    // Optional extern binding after type scheme
                    if self.at(T![=])
                        && self.nth(1) != T![=]
                        && self.nth(1) != T![>]
                        && self.pos() < end_pos
                    {
                        self.bump_any(); // =
                        self.parse_externs(end_pos);
                    }
                }
            }
            SK::TYPE_ALIAS_DEF => {
                self.bump_any(); // type
                if self.at(SK::IDENT) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
                if self.at(T![=]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                }
            }
            SK::NAMED_DEF => {
                let item_keyword = self.current();
                self.bump_any(); // keyword (struct/enum/union/register/let/var/bitfield/...)
                                 // Name: IDENT or UNDERSCORE (`let _ = expr` is valid Sail)
                if self.at(SK::IDENT) || self.at(T![_]) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
                // Type parameter list: `('a, 'b)` or `('n : Int)`
                if self.at(T!['(']) {
                    let pm = self.start();
                    self.bump_any(); // (
                    let mut depth = 1u32;
                    while !self.at_end() && depth > 0 {
                        if self.at(T!['(']) {
                            depth += 1;
                        }
                        if self.at(T![')']) {
                            depth -= 1;
                        }
                        self.bump_any();
                    }
                    pm.complete(self, SK::PARAM_LIST);
                }
                if self.at(T![:]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                }
                if self.at(T![=]) {
                    self.bump_any();
                    if item_keyword == T![bitfield] && self.at(T!['{']) {
                        // Bitfield body: `{ field : index_range, ... }`.
                        self.parse_bitfield_body();
                    } else if (item_keyword == T![union] || item_keyword == T![struct]
                        || item_keyword == T![enum])
                        && self.at(T!['{'])
                    {
                        self.parse_variant_list();
                    } else {
                        self.parse_expr();
                    }
                }
            }
            SK::SCATTERED_CLAUSE_DEF => {
                self.bump_any(); // enum/function/mapping/union
                self.bump_any(); // clause
                                 // Name: the scattered definition name
                if self.at(SK::IDENT) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }

                // Parse parameter pattern as PARAM_LIST.
                self.parse_param_list(end_pos);

                // Skip remaining tokens before `=`
                while self.pos() < end_pos
                    && !self.at_end()
                    && !(self.at(T![=]) && self.nth(1) != T![=] && self.nth(1) != T![>])
                    && !self.at(T!['{'])
                {
                    self.bump_any();
                }

                // Parse body after `=` or `{`
                if self.at(T![=]) && self.pos() < end_pos {
                    self.bump_any(); // =
                    self.parse_expr();
                }
            }
            SK::SCATTERED_DEF => {
                self.bump_any(); // scattered
                if !self.at_end() {
                    self.bump_any();
                } // inner keyword
                if self.at(SK::IDENT) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
                if self.at(T![:]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                }
            }
            SK::OUTCOME_DEF => {
                // outcome id : typschm [with typaram] [= { defs }]
                self.bump_any(); // outcome
                if self.at(SK::IDENT) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
                if self.at(T![:]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                }
                // Optional `with` type parameters
                if self.at(T![with]) {
                    self.bump_any(); // with
                                     // Parse type parameters until `=` or end of def
                    while self.pos() < end_pos
                        && !self.at_end()
                        && !self.at(T![=])
                        && !self.at_set(&DEF_RECOVERY_SET)
                    {
                        self.bump_any();
                    }
                }
                // Optional `= { defs_list }`
                if self.at(T![=]) && self.pos() < end_pos {
                    self.bump_any(); // =
                    if self.at(T!['{']) {
                        self.bump_any(); // {
                                         // Parse nested definitions until `}`
                        while !self.at_end() && !self.at(T!['}']) {
                            if self.at_set(&DEF_START) {
                                self.parse_definition();
                            } else if self.at(T!['{']) {
                                self.error_block("expected definition");
                            } else {
                                self.bump_any();
                            }
                        }
                        if self.at(T!['}']) {
                            self.bump_any();
                        }
                    }
                }
            }
            SK::INSTANTIATION_DEF => {
                // instantiation id [with subst, ...]
                self.bump_any(); // instantiation
                if self.at(SK::IDENT) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
                // Optional `with` substitutions: type = type, ...
                if self.at(T![with]) {
                    self.bump_any(); // with
                                     // Parse comma-separated substitutions until end_pos
                    loop {
                        if self.pos() >= end_pos || self.at_end() || self.at_set(&DEF_RECOVERY_SET)
                        {
                            break;
                        }
                        // Parse left side (kid or id)
                        if self.at(SK::TY_VAR) || self.at(SK::IDENT) {
                            self.bump_any();
                        } else {
                            break;
                        }
                        // Expect `=`
                        if self.at(T![=]) && self.nth(1) != T![=] {
                            self.bump_any(); // =
                        } else {
                            break;
                        }
                        // Parse right side (type or id)
                        if self.at(SK::IDENT) || self.at(SK::TY_VAR) {
                            self.parse_type_expr(TYPE_RECOVERY);
                        } else {
                            break;
                        }
                        // Optional comma
                        if self.at(T![,]) {
                            self.bump_any();
                        } else {
                            break;
                        }
                    }
                }
            }
            SK::TERMINATION_MEASURE_DEF => {
                // termination_measure id loop_measures
                // or: termination_measure id pat = exp
                self.bump_any(); // termination_measure
                if self.at(SK::IDENT) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
                // Parse remaining content: either `pat = exp` or `loop_measures`
                // Loop measures: (until|repeat|while) exp [, ...]
                // Pat = exp form: pattern = expression
                // We parse generically: consume until end_pos
                while self.pos() < end_pos && !self.at_end() {
                    if self.at(T![=]) && self.nth(1) != T![=] && self.nth(1) != T![>] {
                        self.bump_any(); // =
                        self.parse_expr();
                    } else if self.at(T![until]) || self.at(T![repeat]) || self.at(T![while]) {
                        self.bump_any(); // keyword
                        self.parse_expr();
                        if self.at(T![,]) {
                            self.bump_any();
                        }
                    } else {
                        // Parse pattern tokens (for the `pat = exp` form)
                        self.parse_pattern(PAT_RECOVERY_SET);
                    }
                }
            }
            _ => {
                // Default/fixity/end/constraint/directive:
                self.bump_any();
                if self.at(SK::IDENT) {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
            }
        }
        // Sweep remaining tokens in this definition
        self.bump_until_pos(end_pos);
        m.complete(self, def_kind);
    }

    fn classify_definition(&self) -> SK {
        match self.current() {
            T![function] | T![mapping] => {
                if self.followed_by_clause() {
                    SK::SCATTERED_CLAUSE_DEF
                } else {
                    SK::CALLABLE_DEF
                }
            }
            T![enum] | T![union] => {
                if self.followed_by_clause() {
                    SK::SCATTERED_CLAUSE_DEF
                } else {
                    SK::NAMED_DEF
                }
            }
            T![val] => SK::CALLABLE_SPEC,
            T![type] => SK::TYPE_ALIAS_DEF,
            T![struct]
            | T![bitfield]
            | T![newtype]
            | T![register]
            | T![let]
            | T![var]
            | T![overload] => SK::NAMED_DEF,
            T![scattered] => SK::SCATTERED_DEF,
            T![default] => SK::DEFAULT_DEF,
            T![infix] | T![infixl] | T![infixr] => SK::FIXITY_DEF,
            SK::KW_INSTANTIATION => SK::INSTANTIATION_DEF,
            T![end] => SK::END_DEF,
            T![constraint] => SK::CONSTRAINT_DEF,
            SK::KW_TERMINATION_MEASURE => SK::TERMINATION_MEASURE_DEF,
            T![outcome] => SK::OUTCOME_DEF,
            T![private] => {
                let next = self.nth(1);
                match next {
                    T![function] | T![mapping] => SK::CALLABLE_DEF,
                    T![val] => SK::CALLABLE_SPEC,
                    T![type] => SK::TYPE_ALIAS_DEF,
                    T![struct]
                    | T![enum]
                    | T![union]
                    | T![bitfield]
                    | T![newtype]
                    | T![register]
                    | T![let]
                    | T![var]
                    | T![overload] => SK::NAMED_DEF,
                    T![scattered] => SK::SCATTERED_DEF,
                    _ => SK::DEFINITION,
                }
            }
            SK::DIRECTIVE => SK::DIRECTIVE_DEF,
            _ => SK::DEFINITION,
        }
    }

    /// Parse union/struct variant list: `{ Ctor1 : Type1, Ctor2 : (T1, T2), ... }`.
    fn parse_variant_list(&mut self) {
        let m = self.start();
        self.bump_any(); // {
        while !self.at_end() && !self.at(T!['}']) {
            if self.at(T!['}']) || self.at_end() {
                break;
            }
            let checkpoint = self.pos();
            let fm = self.start();
            // Variant name
            if self.at(SK::IDENT) {
                self.bump_any();
            }
            // Optional type annotation: `: Type`
            if self.at(T![:]) {
                self.bump_any(); // :
                self.parse_type_expr(TYPE_RECOVERY);
            }
            fm.complete(self, SK::FIELD_INIT);
            if self.at(T![,]) {
                self.bump_any();
            }
            // Safety valve
            if self.pos() == checkpoint {
                self.bump_any();
            }
        }
        self.expect(T!['}']);
        m.complete(self, SK::STRUCT_EXPR);
    }

    /// Skip deprecated effect annotation: `effect { id, ... }` or `effect pure`.
    ///
    /// Deprecated in newer Sail but still accepted by the compiler.
    fn skip_effect_annotation(&mut self) {
        if self.at(T![effect]) {
            self.bump_any(); // effect
            if self.at(T!['{']) {
                self.bump_any(); // {
                while !self.at_end() && !self.at(T!['}']) {
                    self.bump_any();
                }
                if self.at(T!['}']) {
                    self.bump_any(); // }
                }
            } else if self.at(T![pure]) {
                self.bump_any(); // pure
            }
        }
    }

    /// Parse bitfield body: `{ field : index_range, ... }`
    ///
    /// Index ranges support `@` concatenation and `..` range syntax:
    ///   Field0: (31..16 @ 7..0)
    ///   Field1: 15..12
    fn parse_bitfield_body(&mut self) {
        let m = self.start();
        self.bump_any(); // {
        while !self.at_end() && !self.at(T!['}']) {
            let checkpoint = self.pos();
            let fm = self.start();
            if self.at(SK::IDENT) {
                self.bump_any();
            }
            if self.at(T![:]) {
                self.bump_any(); // :
                self.parse_index_range();
            }
            fm.complete(self, SK::FIELD_INIT);
            if self.at(T![,]) {
                self.bump_any();
            }
            if self.pos() == checkpoint {
                self.bump_any();
            }
        }
        self.expect(T!['}']);
        m.complete(self, SK::STRUCT_EXPR);
    }

    /// Parse a bitfield index range expression.
    ///
    ///   index_range := paren_index_range (@ index_range)*
    ///   paren_index_range := ( index_range ) | atomic_index_range
    ///   atomic_index_range := typ | typ .. typ
    fn parse_index_range(&mut self) {
        loop {
            if self.at(T!['(']) {
                self.bump_any(); // (
                self.parse_index_range(); // recursive for nested parens
                self.expect(T![')']);
            } else {
                // atomic_index_range: typ or typ..typ
                // Absorb numeric literals, type vars, identifiers
                self.parse_type_expr(TokenSet::new(&[
                    T![,], T![')'], T!['}'], T![@],
                ]));
                if self.at(T![.]) && self.nth(1) == T![.] {
                    self.bump_any(); // first .
                    self.bump_any(); // second .
                    self.parse_type_expr(TokenSet::new(&[
                        T![,], T![')'], T!['}'], T![@],
                    ]));
                }
            }
            // Check for @ concatenation
            if self.at(T![@]) {
                self.bump_any();
            } else {
                break;
            }
        }
    }

    /// Parse extern bindings: `STRING` or `pure/monadic/impure STRING`
    /// or `{ id : STRING, ... }` or `pure/monadic/impure { id : STRING, ... }`
    fn parse_externs(&mut self, end_pos: usize) {
        // Optional purity annotation: pure / monadic / impure
        if self.at(T![pure]) || self.at(SK::KW_MONADIC) {
            self.bump_any();
        }
        if self.at(SK::STRING_LIT) {
            // Simple extern: just a string literal
            self.bump_any();
        } else if self.at(T!['{']) {
            // Extern binding list: { id : "string", ... }
            self.bump_any(); // {
            while !self.at_end() && !self.at(T!['}']) && self.pos() < end_pos {
                // Each binding: id : string_lit  OR  _ : string_lit
                if self.at(SK::IDENT) || self.at(T![_]) {
                    self.bump_any(); // id or _
                }
                if self.at(T![:]) {
                    self.bump_any(); // :
                }
                if self.at(SK::STRING_LIT) {
                    self.bump_any(); // string
                }
                if self.at(T![,]) {
                    self.bump_any();
                } else {
                    break;
                }
            }
            if self.at(T!['}']) {
                self.bump_any();
            }
        }
    }

    pub(crate) fn parse_field_inits(&mut self) {
        while !self.at_end() && !self.at(T!['}']) {
            if self.at(T!['}']) || self.at_end() {
                break;
            }
            let fm = self.start();
            if self.at(SK::IDENT) {
                self.bump_any();
            }
            if self.at(T![=]) {
                self.bump_any();
                self.parse_expr();
            }
            fm.complete(self, SK::FIELD_INIT);
            if self.at(T![,]) {
                self.bump_any();
            }
        }
    }
}
