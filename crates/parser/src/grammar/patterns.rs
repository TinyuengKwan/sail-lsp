use super::*;

impl<'t> Parser<'t> {
    pub(crate) fn parse_pattern(&mut self, recovery: TokenSet) -> Option<CompletedMarker> {
        let kind = self.current();
        if kind == SK::EOF || recovery.contains(kind) {
            return None;
        }

        let mut lhs = match kind {
            T![_] => {
                let m = self.start();
                self.bump_any();
                Some(m.complete(self, SK::WILD_PAT))
            }
            SK::NUM_LIT
            | SK::BIN_LIT
            | SK::HEX_LIT
            | SK::STRING_LIT
            | T![true]
            | T![false]
            | SK::KW_BITZERO
            | SK::KW_BITONE
            | T![undefined]
            | SK::UNIT => {
                let m = self.start();
                self.bump_any();
                Some(m.complete(self, SK::LITERAL_PAT))
            }
            SK::TY_VAR => {
                let m = self.start();
                self.bump_any();
                Some(m.complete(self, SK::TYVAR_PAT))
            }
            SK::IDENT => {
                let m = self.start();
                self.bump_any();
                let mut cm = m.complete(self, SK::IDENT_PAT);
                // Constructor App: Name(args) or Name()
                if self.at(T!['(']) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_comma_sep(T![')'], |p| {
                        p.parse_pattern(recovery);
                    });
                    self.expect(T![')']);
                    cm = m2.complete(self, SK::APP_PAT);
                } else if self.at(SK::UNIT) {
                    let m2 = cm.precede(self);
                    self.bump_any(); // consume UNIT
                    cm = m2.complete(self, SK::APP_PAT);
                } else if self.at(T!['[']) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_expr_no_dot();
                    let pat_kind = if self.at(T![.]) && self.nth(1) == T![.] {
                        self.bump_any();
                        self.bump_any();
                        self.parse_expr_no_dot();
                        SK::RANGE_INDEX_PAT
                    } else {
                        SK::INDEX_PAT
                    };
                    self.expect(T![']']);
                    cm = m2.complete(self, pat_kind);
                } else if self.at(T!['{']) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_field_init_pats(recovery);
                    self.expect(T!['}']);
                    cm = m2.complete(self, SK::STRUCT_PAT);
                }
                Some(cm)
            }
            T!['('] => {
                let m = self.start();
                self.bump_any();
                self.parse_comma_sep(T![')'], |p| {
                    p.parse_pattern(recovery);
                });
                self.expect(T![')']);
                Some(m.complete(self, SK::TUPLE_PAT))
            }
            T!['['] => {
                let m = self.start();
                self.bump_any();
                self.parse_comma_sep(T![']'], |p| {
                    p.parse_pattern(recovery);
                });
                self.expect(T![']']);
                Some(m.complete(self, SK::LIST_PAT))
            }
            SK::L_BRACKET_BAR => {
                let m = self.start();
                self.bump_any();
                self.parse_comma_sep(SK::R_BRACKET_BAR, |p| {
                    p.parse_pattern(recovery);
                });
                self.expect(SK::R_BRACKET_BAR);
                Some(m.complete(self, SK::VECTOR_PAT))
            }
            T![struct] => {
                let m = self.start();
                self.bump_any();
                if self.at(SK::IDENT) {
                    self.bump_any();
                }
                if self.at(T!['{']) {
                    self.bump_any();
                    self.parse_field_init_pats(recovery);
                    self.expect(T!['}']);
                }
                Some(m.complete(self, SK::STRUCT_PAT))
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
                self.parse_pattern(recovery)
            }
            _ => {
                self.err_recover_grammar("expected pattern", recovery);
                return None;
            }
        }?;

        // Infix suffixes: `as`, `:`, `::`, `@`, `|`
        // Loop to handle chains like `_ : bits(62) @ 0b00`
        // (TYPED_PAT followed by BIN_PAT concat).
        loop {
            match self.current() {
                // `pat as typ` binding.
                T![as] => {
                    let m = lhs.precede(self);
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY.union(recovery));
                    lhs = m.complete(self, SK::AS_PAT);
                }
                // `pat : typ` type annotation.
                T![:] if !recovery.contains(T![:]) => {
                    let m = lhs.precede(self);
                    self.bump_any();
                    let type_recovery = TYPE_RECOVERY.union(recovery).remove(T![if]);
                    self.parse_type_expr(type_recovery);
                    lhs = m.complete(self, SK::TYPED_PAT);
                }
                // Pattern operators: @, ::, | (vector concat, cons, alternation).
                T![::] | T![@] | T![|] => {
                    let m = lhs.precede(self);
                    self.bump_any();
                    self.parse_pattern(recovery);
                    lhs = m.complete(self, SK::BIN_PAT);
                }
                _ => break,
            }
        }
        Some(lhs)
    }

    fn parse_field_init_pats(&mut self, recovery: TokenSet) {
        while !self.at_end() && !self.at(T!['}']) {
            if self.at(T!['}']) || self.at_end() {
                break;
            }
            let checkpoint = self.pos();
            let fm = self.start();
            if self.at(SK::IDENT) {
                self.bump_any();
            } else if self.at(T![_]) {
                self.bump_any();
            }
            if self.at(T![=]) {
                self.bump_any();
                self.parse_pattern(recovery);
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
    }
}
