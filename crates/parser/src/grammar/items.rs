use super::*;

impl<'t> Parser<'t> {
    /// Whether the cursor is at something that can name a definition.
    ///
    /// Upstream's `id` rule admits both a plain identifier and
    /// `operator <symbol>` (parser.mly:267-273); the lexer emits the latter as
    /// `KW_OPERATOR` followed by an `IDENT` holding just the symbol.
    pub(crate) fn at_def_name(&self) -> bool {
        self.at(SK::IDENT) || (self.at(T![operator]) && self.nth_at(1, SK::IDENT))
    }

    /// Consume a definition name, including a leading `operator` keyword.
    /// The caller is responsible for the surrounding `NAME` marker.
    pub(crate) fn bump_def_name(&mut self) {
        if self.at(T![operator]) {
            self.bump_any();
        }
        self.bump_any();
    }
    /// `impl f(x) = e` — an outcome implementation.
    ///
    /// `Impl funcl` is a top-level definition upstream (`def_aux`,
    /// parser.mly:1401), *and* it appears inside an `outcome .. = { .. }`
    /// body. Only the second position knew it here, so a top-level `impl`
    /// was not classified at all and error recovery reported seven syntax
    /// errors on a definition upstream merely type-checks.
    pub(crate) fn parse_impl_def(&mut self, end_pos: usize) {
        let m = self.start();
        self.bump_any(); // impl
        while self.at_attribute() {
            self.error("Attributes or documentation comment not permitted here");
            self.bump_attribute();
        }
        if self.at_def_name() {
            let nm = self.start();
            self.bump_def_name();
            nm.complete(self, SK::NAME);
        }
        self.parse_param_list(end_pos);
        if self.at(T![=]) {
            self.bump_any();
            self.parse_expr();
        }
        m.complete(self, SK::CALLABLE_DEF);
    }

    pub(crate) fn parse_definition(&mut self) {
        if self.at(SK::KW_IMPL) {
            let end_pos = self.next_def_start_pos(self.pos());
            self.parse_impl_def(end_pos);
            return;
        }
        let def_kind = self.classify_definition();
        let m = self.start();
        // Wrap `private` in a VISIBILITY child node.
        if self.at(T![private]) {
            let vm = self.start();
            self.bump_any(); // private
            vm.complete(self, SK::VISIBILITY);
        }
        let mut end_pos = self.next_def_start_pos(self.pos());
        // An `outcome`'s body is a *list of definitions*, so the scan above
        // ends it at the first `val` inside its own braces: the block spilt
        // out into the file, its `impl`s came apart into ERROR tokens, and
        // nothing in them was ever lowered. Where the header is followed by
        // `= {`, the definition ends at that block's `}`.
        if def_kind == SK::OUTCOME_DEF {
            if let Some(brace) = self.eq_brace_pos_before(self.outcome_header_limit()) {
                end_pos = self.matching_close_brace_pos_from(brace + 1) + 1;
            }
        }

        match def_kind {
            SK::CALLABLE_DEF => {
                // A mapping's body is a list of clauses, not a block of
                // expressions; remember which keyword opened this def.
                let is_mapping = self.current() == T![mapping]
                    || (self.current() == T![private] && self.nth(1) == T![mapping]);
                // Bump keywords + name until structural delimiter.
                let mut bumped_name = false;
                while self.pos() < end_pos && !self.at_end() {
                    let k = self.current();
                    if self.at_attribute() {
                        self.bump_attribute();
                        continue;
                    }
                    // An inline termination measure precedes the name. Its
                    // identifiers belong to the measure, not the function head.
                    if !bumped_name && self.at(T!['{']) {
                        let measure = self.start();
                        let close = self.matching_close_brace_pos_from(self.pos() + 1);
                        self.bump_until_pos((close + 1).min(end_pos));
                        measure.complete(self, SK::TERMINATION_MEASURE_DEF);
                        continue;
                    }
                    // `function f forall 'n, C . (args) -> ret = body` —
                    // upstream's `typquant := kopt_list [',' typ]`, the same
                    // one a `val` spec carries, written on the definition.
                    //
                    // This used to bump every token up to the `.`, so the
                    // quantifiers and the constraint were loose tokens under
                    // `CALLABLE_DEF` and the scheme built from it had
                    // neither. A body was then checked without its own
                    // constraints — `function mmio_read forall 'n, 0 < 'n <=
                    // max_mem_access` could not discharge `'n > 0` at a call
                    // inside it, because the fact was written on the
                    // definition and nothing read it. 33 definitions in
                    // sail-riscv are written this way.
                    if k == T![forall] {
                        self.bump_any(); // forall
                        self.parse_type_param_list();
                        if self.at(T![,]) {
                            self.bump_any();
                            self.parse_type_expr(TokenSet::new(&[SK::DOT]));
                        }
                        if self.at(T![.]) {
                            self.bump_any(); // .
                        } else {
                            // Not the shape this expects. Fall back to what
                            // stood here, so a definition it cannot read is
                            // no worse off than before.
                            while self.pos() < end_pos && !self.at_end() && !self.at(T![.]) {
                                self.bump_any();
                            }
                            if self.at(T![.]) {
                                self.bump_any();
                            }
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
                    // `function a 'n = ..` — a type-variable pattern is an
                    // unparenthesised parameter too (`atomic_pat: | kid`,
                    // parser.mly:541).
                    if bumped_name && (k == SK::IDENT || k == SK::TY_VAR) {
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
                    self.bump_any(); // =
                                     // The marker opens *after* the definition's `=`. Opening
                                     // it first made the `ASSIGN_EXPR` swallow the token that
                                     // separates a head from its body, so the `CALLABLE_DEF`
                                     // had no `=` of its own and the body lookup found
                                     // nothing: `function f(r) = R0 = r.x` was lowered with an
                                     // *empty body*, and every check over it — field, type,
                                     // unused — said nothing at all.
                    let assign_m = self.start();
                    if is_mapping && self.at(T!['{']) {
                        self.parse_mapping_body();
                    } else {
                        self.parse_expr();
                    }
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
                if !found_eq
                    && self.pos() < end_pos
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
            SK::CALLABLE_SPEC => {
                self.bump_any(); // val/mapping
                                 // val can also take a STRING_LIT as the name (val "add_bits" : ...)
                if self.at_def_name() || self.at(SK::STRING_LIT) || self.at(T![cast]) {
                    let nm = self.start();
                    self.bump_def_name();
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
                if self.at_def_name() {
                    let nm = self.start();
                    self.bump_def_name();
                    nm.complete(self, SK::NAME);
                }
                // `type ds('n: Int, 'm: Int) -> Bool = ..`. A synonym's
                // parameters are `kopt`s like a `forall`'s, but written as
                // one comma-separated group, and nothing built them: the
                // kinds they declare are what says `ds(bool)` is wrong.
                //
                // Only the parameters. The body of a *parameterised* alias
                // stays a raw token run for the generic loop below to
                // consume, which is what it was: parsing it as a type here
                // makes `type bits('n) = bitvector('n)` reach the alias
                // scheme through `type_from_cst_node` instead of the text
                // path, and the two disagree about `'n` — every `bits(2) @
                // bits(3)` came out `bits('n + 'n)`.
                if self.at(T!['(']) {
                    self.parse_type_synonym_params();
                } else if self.at(T![=]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                }
            }
            SK::NAMED_DEF => {
                let item_keyword = self.current();
                self.bump_any(); // keyword (struct/enum/union/register/let/var/bitfield/...)
                                 // Name: IDENT or UNDERSCORE (`let _ = expr` is valid Sail)
                                 //
                                 // A `let` may name a type variable instead:
                                 // `atomic_pat: | kid` binds the value
                                 // `id_of_kid kid` (parser.mly:541-543,
                                 // ast_util.ml:983), so `let 'cap_size = 32`
                                 // declares `cap_size`. Without this the
                                 // `'cap_size` was not a name, so nothing
                                 // below consumed it, the `= expr` branch was
                                 // never reached, and the initialiser arrived
                                 // as loose tokens rather than an expression.
                let names_a_type_var =
                    matches!(item_keyword, T![let] | T![var]) && self.at(SK::TY_VAR);
                if self.at_def_name() || self.at(T![_]) || names_a_type_var {
                    let nm = self.start();
                    self.bump_def_name();
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
                    // `newtype pa = Pa : bits(16)` — the payload after the
                    // constructor is a *type*, and running the expression
                    // parser over it left the definition with no type node
                    // at all: the constructor was registered with an
                    // `error()` payload, so `Pa(q)` bound `q` to nothing and
                    // every use of what a newtype pattern opens was untyped.
                    if item_keyword == T![newtype] {
                        if self.at_def_name() {
                            let cm = self.start();
                            self.bump_def_name();
                            cm.complete(self, SK::NAME);
                        }
                        if self.at(T![:]) {
                            self.bump_any();
                            self.parse_type_expr(TYPE_RECOVERY);
                        }
                    } else if item_keyword == T![bitfield] && self.at(T!['{']) {
                        // Bitfield body: `{ field : index_range, ... }`.
                        self.parse_bitfield_body();
                    } else if (item_keyword == T![union]
                        || item_keyword == T![struct]
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
                let type_clause = matches!(self.current(), T![enum] | T![union]);
                self.bump_any(); // enum/function/mapping/union
                self.bump_any(); // clause
                                 // Name: the scattered definition name
                if self.at_def_name() {
                    let nm = self.start();
                    self.bump_def_name();
                    nm.complete(self, SK::NAME);
                }

                // Parse parameter pattern as PARAM_LIST.
                self.parse_param_list(end_pos);

                // Skip remaining tokens before `=`
                #[allow(clippy::nonminimal_bool)]
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
                    if type_clause && self.at(T!['{']) {
                        self.parse_variant_list();
                    } else if type_clause {
                        let member = self.start();
                        while self.at_attribute() {
                            self.bump_attribute();
                        }
                        if self.at(T![private]) {
                            let visibility = self.start();
                            self.bump_any();
                            visibility.complete(self, SK::VISIBILITY);
                        }
                        if self.at_def_name() {
                            self.bump_def_name();
                        }
                        if self.at(T![:]) {
                            self.bump_any();
                            self.parse_type_expr(TYPE_RECOVERY);
                        }
                        member.complete(self, SK::FIELD_INIT);
                    } else {
                        self.parse_expr();
                    }
                }
            }
            SK::SCATTERED_DEF => {
                self.bump_any(); // scattered
                if !self.at_end() {
                    self.bump_any();
                } // inner keyword
                if self.at_def_name() {
                    let nm = self.start();
                    self.bump_def_name();
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
                if self.at_def_name() {
                    let nm = self.start();
                    self.bump_def_name();
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
                        let saved_block_end = self.enclosing_block_end;
                        self.enclosing_block_end = Some(self.matching_close_brace_pos());
                        while !self.at_end() && !self.at(T!['}']) {
                            // `impl emulator(bv) = { .. }` is an outcome
                            // implementation — `Impl funcl` upstream
                            // (parser.mly:1401). Without it the `{` of the
                            // body was reported as "expected definition".
                            if self.at(SK::KW_IMPL) {
                                self.parse_impl_def(end_pos);
                            } else if self.at_set(&DEF_START) {
                                self.parse_definition();
                            } else if self.at(T!['{']) {
                                self.error_block("expected definition");
                            } else {
                                self.bump_any();
                            }
                        }
                        self.enclosing_block_end = saved_block_end;
                        if self.at(T!['}']) {
                            self.bump_any();
                        }
                    }
                }
            }
            SK::INSTANTIATION_DEF => {
                // instantiation id [with subst, ...]
                self.bump_any(); // instantiation
                if self.at_def_name() {
                    let nm = self.start();
                    self.bump_def_name();
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
                if self.at_def_name() {
                    let nm = self.start();
                    self.bump_def_name();
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
            SK::DIRECTIVE_DEF if self.at(SK::STRUCTURED_DIRECTIVE_START) => {
                // `$name { .. }` — the introducer token already carries the
                // opening brace, so consume through the matching close.
                self.bump_any();
                let mut depth = 1u32;
                while !self.at_end() && depth > 0 {
                    if self.at(T!['{']) {
                        depth += 1;
                    }
                    if self.at(T!['}']) {
                        depth -= 1;
                        self.bump_any();
                        continue;
                    }
                    self.bump_any();
                }
            }
            _ => {
                // Default/fixity/end/constraint/directive:
                self.bump_any();
                if self.at_def_name() {
                    let nm = self.start();
                    self.bump_def_name();
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
            // `and f(..) = ..` continues the preceding function definition
            // with another clause.
            T![and] => SK::CALLABLE_DEF,
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
            SK::STRUCTURED_DIRECTIVE_START => SK::DIRECTIVE_DEF,
            T![end] => SK::END_DEF,
            T![constraint] => SK::CONSTRAINT_DEF,
            SK::KW_TERMINATION_MEASURE => SK::TERMINATION_MEASURE_DEF,
            T![outcome] => SK::OUTCOME_DEF,
            T![private] => {
                let next = self.nth(1);
                match next {
                    T![function] | T![mapping] | T![enum] | T![union]
                        if self.nth(2) == T![clause] =>
                    {
                        SK::SCATTERED_CLAUSE_DEF
                    }
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
            // `$[attr]` before a member. Upstream allows it, and
            // `at_attribute`/`bump_attribute` were here to consume it with
            // no field list calling them — so each of the attribute's four
            // tokens fell through this loop's safety valve and left an
            // empty `FIELD_INIT` standing beside it.
            while self.at_attribute() {
                self.bump_attribute();
            }
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
            while self.at_attribute() {
                self.bump_attribute();
            }
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
                self.parse_type_expr(TokenSet::new(&[T![,], T![')'], T!['}'], T![@]]));
                if self.at(T![.]) && self.nth(1) == T![.] {
                    self.bump_any(); // first .
                    self.bump_any(); // second .
                    self.parse_type_expr(TokenSet::new(&[T![,], T![')'], T!['}'], T![@]]));
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
        // Optional purity annotation: pure / monadic / impure.
        //
        // `impure` was in the comment and not in the code, so `val
        // cancel_reservation = impure {interpreter: ".."} : unit -> unit`
        // parsed as neither form: the extern block was not recognised, the
        // signature after it was never read as a type, and the `val`
        // contributed no scheme at all — every call to one was untyped.
        if self.at(T![pure]) || self.at(SK::KW_MONADIC) || self.at(SK::KW_IMPURE) {
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
            // The safety valve its two sibling loops have and this one did
            // not. Three near-copies of the same loop; the one without it
            // can only terminate on the parser's global step limit, which
            // spends the budget the rest of the file needs.
            let checkpoint = self.pos();
            let fm = self.start();
            while self.at_attribute() {
                self.bump_attribute();
            }
            if self.at(SK::IDENT) {
                self.bump_any();
            }
            if self.at(T![=]) {
                self.bump_any();
                self.parse_expr();
            }
            if self.pos() == checkpoint && !self.at(T![,]) {
                // Nothing matched. Take the token into this field rather
                // than beside it, so the tree carries no empty node
                // standing for text.
                self.bump_any();
            }
            fm.complete(self, SK::FIELD_INIT);
            if self.at(T![,]) {
                self.bump_any();
            }
        }
    }
}
