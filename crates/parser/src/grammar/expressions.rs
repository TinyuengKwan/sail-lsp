use super::*;

impl<'t> Parser<'t> {

    pub(crate) fn parse_expr(&mut self) {
        self.expr_bp(R_DEFAULT, 0);
    }

    /// Parse an expression but stop before DOT tokens.
    /// Used inside `[...]` to avoid consuming `..` as field access.
    pub(crate) fn parse_expr_no_dot(&mut self) {
        self.expr_bp(R_NO_DOT, 0);
    }

    fn expr_bp(&mut self, r: Restrictions, min_bp: u8) -> Option<CompletedMarker> {
        if self.at_end() || self.at_set(&EXPR_RECOVERY_SET) {
            return None;
        }

        let mut lhs = self.parse_lhs(r)?;

        loop {
            if self.at_end() {
                break;
            }

            // Postfix: `.field`, `[index]`, `:=`
            if let Some(cm) = self.try_postfix(lhs, r) {
                lhs = cm;
                continue;
            }

            // Infix: Pratt binding power (built-in + dynamic fixity)
            let kind = self.current();
            let bp = infix_bp(kind).or_else(|| {
                // Check dynamic fixity for IDENT tokens
                if kind == SK::IDENT {
                    let text = self.current_text();
                    self.fixities.get(text).copied()
                } else {
                    None
                }
            });
            let Some((l_bp, r_bp)) = bp else { break };
            if l_bp < min_bp {
                break;
            }

            let m = lhs.precede(self);
            self.bump_any(); // operator
            self.expr_bp(r, r_bp);
            lhs = m.complete(self, SK::BIN_EXPR);
        }

        Some(lhs)
    }

    fn parse_lhs(&mut self, r: Restrictions) -> Option<CompletedMarker> {
        let kind = self.current();
        match kind {
            // Literals
            SK::NUM_LIT
            | SK::BIN_LIT
            | SK::HEX_LIT
            | SK::REAL_LIT
            | SK::STRING_LIT
            | SK::MULTILINE_STRING_LIT
            | T![true]
            | T![false]
            | SK::KW_BITZERO
            | SK::KW_BITONE
            | T![undefined]
            | SK::UNIT => {
                let m = self.start();
                self.bump_any();
                Some(m.complete(self, SK::LITERAL_EXPR))
            }
            SK::TY_VAR => {
                let m = self.start();
                self.bump_any();
                Some(m.complete(self, SK::TYVAR_EXPR))
            }
            SK::IDENT => {
                let m = self.start();
                self.bump_any();
                let cm = m.complete(self, SK::IDENT_EXPR);
                // Call: f(args) or f() where () is UNIT token
                if self.at(T!['(']) {
                    let m2 = cm.precede(self);
                    self.parse_arg_list();
                    return Some(m2.complete(self, SK::CALL_EXPR));
                }
                if self.at(SK::UNIT) {
                    // f() — chumsky tokenizes `()` as single UNIT token
                    let m2 = cm.precede(self);
                    self.parse_unit_arg_list();
                    return Some(m2.complete(self, SK::CALL_EXPR));
                }
                // Named struct: Name { field = val }
                if !r.forbid_structs && self.at(T!['{']) && !self.is_update_expr() {
                    let m2 = cm.precede(self);
                    self.bump_any(); // {
                    self.parse_field_inits();
                    self.expect(T!['}']);
                    return Some(m2.complete(self, SK::STRUCT_EXPR));
                }
                Some(cm)
            }
            T![_] => {
                let m = self.start();
                self.bump_any();
                Some(m.complete(self, SK::IDENT_EXPR))
            }
            T![-] => {
                let m = self.start();
                self.bump_any();
                self.expr_bp(r, PREFIX_BP);
                Some(m.complete(self, SK::PREFIX_EXPR))
            }
            T!['('] => {
                let m = self.start();
                self.bump_any();
                self.parse_comma_sep(T![')'], |p| {
                    // `*expr` (deref) at expression start inside parens.
                    // `*` is not a normal expression-start token, so
                    // handle it specially here.
                    if p.at(T![*]) {
                        let dm = p.start();
                        p.bump_any(); // *
                        p.parse_expr();
                        dm.complete(p, SK::PREFIX_EXPR);
                    } else {
                        p.parse_expr();
                    }
                    // Type ascription: `(expr : type)`.
                    if p.at(T![:]) {
                        p.bump_any(); // :
                        p.parse_type_expr(TYPE_RECOVERY);
                    }
                });
                self.expect(T![')']);
                Some(m.complete(self, SK::TUPLE_EXPR))
            }
            T!['['] => {
                let m = self.start();
                self.bump_any();
                // Check for vector update: [base with ...]
                if !self.at(T![']']) {
                    let checkpoint = self.pos();
                    self.parse_expr();
                    if self.at(T![with]) {
                        // [base with idx = val, ...] → VECTOR_UPDATE_EXPR
                        self.bump_any(); // with
                        while !self.at_end() && !self.at(T![']']) {
                            self.parse_expr_no_dot();
                            if self.at(T![.]) && self.nth(1) == T![.] {
                                self.bump_any();
                                self.bump_any();
                                self.parse_expr_no_dot();
                            }
                            if self.at(T![=]) && self.nth(1) != T![=] {
                                self.bump_any();
                                self.parse_expr();
                            }
                            if self.at(T![,]) {
                                self.bump_any();
                            }
                            if self.pos() == checkpoint {
                                self.bump_any();
                            } // safety
                        }
                        self.expect(T![']']);
                        return Some(m.complete(self, SK::VECTOR_UPDATE_EXPR));
                    }
                    // Not a vector update — parse remaining items as vector literal
                    while self.at(T![,]) {
                        self.bump_any();
                        if self.at(T![']']) {
                            break;
                        }
                        self.parse_expr();
                    }
                }
                self.expect(T![']']);
                Some(m.complete(self, SK::VECTOR_EXPR))
            }
            SK::L_BRACKET_BAR => {
                let m = self.start();
                self.bump_any();
                self.parse_comma_sep(SK::R_BRACKET_BAR, |p| {
                    p.parse_expr();
                });
                self.expect(SK::R_BRACKET_BAR);
                Some(m.complete(self, SK::LIST_EXPR))
            }
            T!['{'] if !r.forbid_structs => {
                if self.is_update_expr() {
                    let m = self.start();
                    self.bump_any(); // {
                    self.parse_expr();
                    self.expect(T![with]);
                    self.parse_field_inits();
                    self.expect(T!['}']);
                    Some(m.complete(self, SK::UPDATE_EXPR))
                } else {
                    self.parse_block()
                }
            }
            T![if] => {
                let m = self.start();
                self.bump_any();
                self.expr_bp(R_NO_STRUCT, 0); // condition must not consume `{`
                if self.at(T![then]) {
                    self.bump_any();
                    self.parse_expr();
                    if self.at(T![else]) {
                        self.bump_any();
                        self.parse_expr();
                    }
                }
                Some(m.complete(self, SK::IF_EXPR))
            }
            T![match] => {
                let m = self.start();
                self.bump_any();
                self.expr_bp(R_NO_STRUCT, 0); // scrutinee must not consume `{`
                self.expect(T!['{']);
                self.parse_match_arms();
                self.expect(T!['}']);
                Some(m.complete(self, SK::MATCH_EXPR))
            }
            T![try] => {
                let m = self.start();
                self.bump_any(); // try
                self.parse_expr();
                if self.at(T![catch]) {
                    self.bump_any(); // catch
                    self.expect(T!['{']);
                    self.parse_match_arms();
                    self.expect(T!['}']);
                }
                Some(m.complete(self, SK::TRY_EXPR))
            }
            T![let] => {
                let m = self.start();
                self.bump_any();
                self.parse_pattern(PAT_RECOVERY_SET);
                self.expect(T![=]);
                self.parse_expr();
                if self.at(T![in]) || self.at(T![;]) {
                    self.bump_any();
                    self.parse_expr();
                }
                Some(m.complete(self, SK::LET_EXPR))
            }
            T![var] => {
                let m = self.start();
                self.bump_any();
                self.parse_pattern(PAT_RECOVERY_SET);
                if self.at(T![:]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                }
                if self.at(T![=]) {
                    self.bump_any();
                    self.parse_expr();
                }
                if self.at(T![in]) || self.at(T![;]) {
                    self.bump_any();
                    self.parse_expr();
                }
                Some(m.complete(self, SK::VAR_EXPR))
            }
            T![return] => {
                let m = self.start();
                self.bump_any();
                if !self.at_end() && !self.at_set(&CLOSERS) && !self.at(T![;]) {
                    self.parse_expr();
                }
                Some(m.complete(self, SK::RETURN_EXPR))
            }
            T![throw] => {
                let m = self.start();
                self.bump_any();
                self.parse_expr();
                Some(m.complete(self, SK::THROW_EXPR))
            }
            T![exit] => {
                let m = self.start();
                self.bump_any();
                if self.at(T!['(']) || self.at(SK::UNIT) {
                    self.parse_expr();
                }
                Some(m.complete(self, SK::EXIT_EXPR))
            }
            T![assert] => {
                let m = self.start();
                self.bump_any();
                self.parse_expr();
                Some(m.complete(self, SK::ASSERT_EXPR))
            }
            T![sizeof] => {
                let m = self.start();
                self.bump_any();
                self.parse_expr();
                Some(m.complete(self, SK::SIZEOF_EXPR))
            }
            T![constraint] => {
                let m = self.start();
                self.bump_any();
                if self.at(T!['(']) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                    self.expect(T![')']);
                }
                Some(m.complete(self, SK::CONSTRAINT_EXPR))
            }
            T![foreach] => {
                let m = self.start();
                self.bump_any(); // foreach
                if self.at(T!['(']) {
                    self.bump_any();
                }
                if self.at(SK::IDENT) {
                    self.bump_any();
                }
                if self.at(T![from]) {
                    self.bump_any();
                }
                let foreach_stop =
                    TokenSet::new(&[T![to], T![downto], T![by], T![in], T![do], T![')']]);
                if !self.at_set(&foreach_stop) {
                    self.parse_expr();
                }
                if self.at(T![to]) || self.at(T![downto]) {
                    self.bump_any();
                }
                if !self.at_set(&foreach_stop) {
                    self.parse_expr();
                }
                if self.at(T![by]) {
                    self.bump_any();
                    self.parse_expr();
                }
                if self.at(T![')']) {
                    self.bump_any();
                }
                if self.at(T![do]) || self.at(T![in]) {
                    self.bump_any();
                }
                self.parse_expr();
                Some(m.complete(self, SK::FOREACH_EXPR))
            }
            T![while] => {
                let m = self.start();
                self.bump_any();
                self.parse_expr();
                // Optional in-place loop measure: termination_measure(expr)
                if self.at(SK::KW_TERMINATION_MEASURE) {
                    self.bump_any(); // termination_measure
                    if self.at(T!['(']) {
                        self.bump_any(); // (
                        self.parse_expr();
                        self.expect(T![')']);
                    }
                }
                if self.at(T![do]) {
                    self.bump_any();
                    self.parse_expr();
                } else if self.at(T!['{']) {
                    // while expr { block } form
                    self.parse_block();
                } else {
                    self.error("expected `do` or `{`".to_string());
                }
                Some(m.complete(self, SK::WHILE_EXPR))
            }
            T![repeat] => {
                let m = self.start();
                self.bump_any();
                // Optional in-place loop measure: termination_measure(expr)
                if self.at(SK::KW_TERMINATION_MEASURE) {
                    self.bump_any(); // termination_measure
                    if self.at(T!['(']) {
                        self.bump_any(); // (
                        self.parse_expr();
                        self.expect(T![')']);
                    }
                }
                self.parse_expr();
                self.expect(T![until]);
                self.parse_expr();
                Some(m.complete(self, SK::REPEAT_EXPR))
            }
            T![ref] => {
                let m = self.start();
                self.bump_any();
                if self.at(SK::IDENT) {
                    self.bump_any();
                }
                Some(m.complete(self, SK::REF_EXPR))
            }
            T![struct] => {
                let m = self.start();
                self.bump_any();
                if self.at(T!['{']) {
                    self.bump_any();
                    self.parse_field_inits();
                    self.expect(T!['}']);
                }
                Some(m.complete(self, SK::STRUCT_EXPR))
            }
            SK::STRUCTURED_DIRECTIVE_START => {
                let m = self.start();
                self.bump_any();
                while !self.at_end() && !self.at(T!['}']) {
                    self.bump_any();
                }
                if self.at(T!['}']) {
                    self.bump_any();
                }
                m.complete(self, SK::ATTRIBUTE);
                self.parse_lhs(r)
            }
            SK::KW_CONFIG => {
                // `config Id` — just a single identifier.
                // Dotted path is field access; `: type` is postfix.
                let m = self.start();
                self.bump_any(); // config
                if self.at(SK::IDENT) {
                    self.bump_any();
                }
                Some(m.complete(self, SK::CONFIG_EXPR))
            }
            SK::DOLLAR => {
                let m = self.start();
                self.bump_any();
                while !self.at_end() && !self.at(T![']']) {
                    self.bump_any();
                }
                if self.at(T![']']) {
                    self.bump_any();
                }
                Some(m.complete(self, SK::CONFIG_EXPR))
            }
            _ => None,
        }
    }

    fn try_postfix(&mut self, lhs: CompletedMarker, r: Restrictions) -> Option<CompletedMarker> {
        match self.current() {
            T![.] if !r.forbid_dot => {
                let m = lhs.precede(self);
                self.bump_any();
                if !self.at_end() {
                    self.bump_any();
                }
                Some(m.complete(self, SK::FIELD_ACCESS_EXPR))
            }
            T![->] if !r.forbid_dot => {
                let m = lhs.precede(self);
                self.bump_any();
                if !self.at_end() {
                    self.bump_any();
                }
                Some(m.complete(self, SK::FIELD_ACCESS_EXPR))
            }
            T!['['] => {
                let m = lhs.precede(self);
                self.bump_any();
                self.parse_expr_no_dot();
                let kind = if self.at(T![.]) && self.nth(1) == T![.] {
                    self.bump_any(); // first dot
                    self.bump_any(); // second dot
                    self.parse_expr_no_dot();
                    SK::SUBRANGE_EXPR
                } else if self.at(T![,]) {
                    self.bump_any();
                    self.parse_expr_no_dot();
                    SK::INDEX_EXPR
                } else {
                    SK::INDEX_EXPR
                };
                self.expect(T![']']);
                Some(m.complete(self, kind))
            }
            // Postfix type annotation: `expr : atomic_typ`.
            // Uses atomic type (no infix) so `x : bool & y` parses as
            // `(x : bool) & y`, not `x : (bool & y)`.
            T![:] => {
                let m = lhs.precede(self);
                self.bump_any(); // :
                self.parse_atomic_type_expr(TYPE_RECOVERY);
                Some(m.complete(self, SK::CAST_EXPR))
            }
            T![:=] => {
                let m = lhs.precede(self);
                self.bump_any();
                self.parse_expr();
                Some(m.complete(self, SK::ASSIGN_EXPR))
            }
            _ => None,
        }
    }

    pub(crate) fn parse_block(&mut self) -> Option<CompletedMarker> {
        let m = self.start();
        self.bump_any(); // {
        while !self.at_end() && !self.at(T!['}']) {
            if self.at(T!['}']) || self.at_end() {
                break;
            }
            let checkpoint = self.pos();
            let bm = self.start();
            let assign_m = self.start();
            self.parse_expr();
            if self.at(T![=]) && self.nth(1) != T![=] && self.nth(1) != T![>] {
                self.bump_any(); // =
                self.parse_expr();
                assign_m.complete(self, SK::ASSIGN_EXPR);
            } else {
                assign_m.abandon(self);
            }
            bm.complete(self, SK::BLOCK_ITEM);
            if self.at(T![;]) {
                self.bump_any();
            }
            // Safety valve
            if self.pos() == checkpoint {
                self.bump_any();
            }
        }
        self.expect(T!['}']);
        Some(m.complete(self, SK::BLOCK_EXPR))
    }

    /// Parse match arms with error recovery.
    pub(crate) fn parse_match_arms(&mut self) {
        while !self.at_end() && !self.at(T!['}']) {
            if self.at(T!['}']) || self.at_end() {
                break;
            }

            if self.at(T!['{']) {
                self.error_block("expected match arm");
                continue;
            }
            if self.at(T![,]) {
                self.err_and_bump("expected pattern");
                continue;
            }

            let checkpoint = self.pos();
            let m = self.start();
            // Handle `$[attr]` before match arm pattern
            while self.at(SK::DOLLAR) && self.nth(1) == T!['['] {
                let am = self.start();
                self.bump_any(); // $
                self.bump_any(); // [
                while !self.at_end() && !self.at(T![']']) {
                    self.bump_any();
                }
                if self.at(T![']']) {
                    self.bump_any();
                }
                am.complete(self, SK::ATTRIBUTE);
            }
            let arm_recovery = PAT_RECOVERY_SET.union(TokenSet::new(&[T![=>], T!['}']]));
            self.parse_pattern(arm_recovery);
            // Check for guard `if expr`
            if self.at(T![if]) {
                self.bump_any();
                self.parse_expr();
            }
            if self.at(T![=>]) {
                self.bump_any();
            } else if self.at(T![=]) {
                self.bump_any(); // =
                if self.at(T![>]) {
                    self.bump_any();
                } // >
            } else {
                self.error("expected `=>`".to_string());
            }
            self.parse_expr();
            m.complete(self, SK::MATCH_ARM);
            if self.at(T![,]) {
                self.bump_any();
            }
            // Safety valve: force progress
            if self.pos() == checkpoint {
                self.bump_any();
            }
        }
    }
}
