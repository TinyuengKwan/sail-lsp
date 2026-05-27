use super::*;

impl<'t> Parser<'t> {
    /// Absorb arithmetic infix operators and their operands in type
    /// position. Handles `8 * 'n`, `'n + 1`, `2 ** 16`, etc.
    fn absorb_numeric_infix(&mut self, _recovery: TokenSet) {
        loop {
            let k = self.current();
            if matches!(k, T![+] | T![-] | T![*] | T![%] | T![^]) {
                self.bump_any(); // operator
                let ok = self.current();
                if matches!(
                    ok,
                    SK::NUM_LIT
                        | SK::TY_VAR
                        | SK::IDENT
                        | T!['(']
                        | SK::BIN_LIT
                        | SK::HEX_LIT
                        | T![-]
                ) {
                    if self.at(T!['(']) {
                        let mut d: u32 = 0;
                        loop {
                            if self.at_end() {
                                break;
                            }
                            if self.at(T!['(']) {
                                d += 1;
                            }
                            if self.at(T![')']) {
                                d -= 1;
                                self.bump_any();
                                if d == 0 {
                                    break;
                                }
                                continue;
                            }
                            self.bump_any();
                        }
                    } else {
                        self.bump_any(); // single operand
                    }
                    continue;
                }
            }
            break;
        }
    }

    /// Parse an atomic type — no infix operators (`&`, `|`, `+`, etc.).
    pub(crate) fn parse_atomic_type_expr(&mut self, recovery: TokenSet) -> Option<CompletedMarker> {
        self.parse_type_expr_inner(recovery, false)
    }

    pub(crate) fn parse_type_expr(&mut self, recovery: TokenSet) -> Option<CompletedMarker> {
        self.parse_type_expr_inner(recovery, true)
    }

    fn parse_type_expr_inner(
        &mut self,
        recovery: TokenSet,
        allow_infix: bool,
    ) -> Option<CompletedMarker> {
        let kind = self.current();
        if kind == SK::EOF || recovery.contains(kind) {
            return None;
        }

        let lhs = match kind {
            T![forall] => {
                let m = self.start();
                self.bump_any();
                while !self.at_end() {
                    if self.at(T![.]) {
                        self.bump_any();
                        break;
                    }
                    self.bump_any();
                }
                self.parse_type_expr(recovery);
                Some(m.complete(self, SK::TYPE_FORALL))
            }
            T!['('] => {
                let m = self.start();
                self.bump_any();
                self.parse_comma_sep(T![')'], |p| {
                    p.parse_type_expr(recovery);
                });
                self.expect(T![')']);
                let cm = m.complete(self, SK::TYPE_TUPLE);
                if self.at(T![->]) || self.at(T![<->]) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_type_expr(recovery);
                    Some(m2.complete(self, SK::TYPE_ARROW))
                } else {
                    Some(cm)
                }
            }
            SK::IDENT | SK::KW_INT | SK::KW_BOOL | SK::KW_TYPE_UPPER => {
                let m = self.start();
                self.bump_any();
                let cm = m.complete(self, SK::TYPE_NAMED);
                let cm = if self.at(T!['(']) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_comma_sep(T![')'], |p| {
                        p.parse_type_expr(recovery);
                    });
                    self.expect(T![')']);
                    m2.complete(self, SK::TYPE_APP)
                } else {
                    cm
                };
                if self.at(T![->]) || self.at(T![<->]) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_type_expr(recovery);
                    Some(m2.complete(self, SK::TYPE_ARROW))
                } else {
                    Some(cm)
                }
            }
            SK::TY_VAR => {
                let m = self.start();
                self.bump_any();
                self.absorb_numeric_infix(recovery);
                let cm = m.complete(self, SK::TYPE_VAR);
                if self.at(T![->]) || self.at(T![<->]) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_type_expr(recovery);
                    Some(m2.complete(self, SK::TYPE_ARROW))
                } else {
                    Some(cm)
                }
            }
            // Type-level constants: literals, dec/inc order keywords.
            SK::NUM_LIT
            | SK::BIN_LIT
            | SK::HEX_LIT
            | T![_]
            | T![dec]
            | T![inc]
            | T![true]
            | T![false] => {
                let m = self.start();
                self.bump_any();
                self.absorb_numeric_infix(recovery);
                Some(m.complete(self, SK::TYPE_NAMED))
            }
            T![if] => {
                let m = self.start();
                self.bump_any(); // if
                while !self.at_end() && !self.at(T![then]) {
                    if recovery.contains(self.current())
                        && !matches!(self.current(), T![then] | T![else])
                    {
                        break;
                    }
                    self.bump_any();
                }
                if self.at(T![then]) {
                    self.bump_any();
                    let inner_recovery = TokenSet::new(&[T![else], T![')'], T!['}'], T![,], T![.]]);
                    self.parse_type_expr(inner_recovery);
                }
                if self.at(T![else]) {
                    self.bump_any();
                    self.parse_type_expr(recovery);
                }
                Some(m.complete(self, SK::TYPE_NAMED))
            }
            SK::KW_CONFIG => {
                let m = self.start();
                self.bump_any(); // config
                if self.at(SK::IDENT) {
                    self.bump_any();
                    while self.at(T![.]) {
                        self.bump_any();
                        if self.at(SK::IDENT) {
                            self.bump_any();
                        }
                    }
                }
                Some(m.complete(self, SK::TYPE_NAMED))
            }
            T![-] => {
                let m = self.start();
                self.bump_any();
                if self.at(T!['(']) {
                    let mut d: u32 = 0;
                    loop {
                        if self.at_end() {
                            break;
                        }
                        if self.at(T!['(']) {
                            d += 1;
                        }
                        if self.at(T![')']) {
                            d -= 1;
                            self.bump_any();
                            if d == 0 {
                                break;
                            }
                            continue;
                        }
                        self.bump_any();
                    }
                } else if matches!(self.current(), SK::NUM_LIT | SK::TY_VAR) {
                    self.bump_any();
                }
                self.absorb_numeric_infix(recovery);
                Some(m.complete(self, SK::TYPE_NAMED))
            }
            T!['{'] => {
                let m = self.start();
                self.bump_any();
                // Existential type: { kopt_list [, constraint] . type }.
                // Track brace depth so nested {1, 2, 4, 8} sets don't
                // prematurely close the existential.
                let mut depth = 0u32;
                while !self.at_end() {
                    if self.at(T!['{']) {
                        depth += 1;
                        self.bump_any();
                    } else if self.at(T!['}']) {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                        self.bump_any();
                    } else if self.at(T![.]) && depth == 0 {
                        self.bump_any();
                        self.parse_type_expr(recovery);
                        break;
                    } else {
                        self.bump_any();
                    }
                }
                self.expect(T!['}']);
                Some(m.complete(self, SK::TYPE_EXISTENTIAL))
            }
            // Deprecated `{| |}` syntax for numeric sets.
            SK::L_CURLY_BAR => {
                let m = self.start();
                self.bump_any(); // {|
                while !self.at_end() && !self.at(SK::R_CURLY_BAR) {
                    self.bump_any();
                }
                self.expect(SK::R_CURLY_BAR);
                Some(m.complete(self, SK::TYPE_EXISTENTIAL))
            }
            SK::KW_EFFECT => {
                let m = self.start();
                self.bump_any();
                if self.at(T!['{']) {
                    self.bump_any();
                    while !self.at_end() && !self.at(T!['}']) {
                        self.bump_any();
                    }
                    self.expect(T!['}']);
                }
                Some(m.complete(self, SK::TYPE_EFFECT))
            }
            T![register] => {
                let m = self.start();
                self.bump_any();
                if self.at(T!['(']) {
                    self.bump_any();
                    self.parse_type_expr(recovery);
                    self.expect(T![')']);
                }
                let cm = m.complete(self, SK::TYPE_APP);
                if self.at(T![->]) || self.at(T![<->]) {
                    let m2 = cm.precede(self);
                    self.bump_any();
                    self.parse_type_expr(recovery);
                    Some(m2.complete(self, SK::TYPE_ARROW))
                } else {
                    Some(cm)
                }
            }
            _ => {
                self.err_recover_grammar("expected type", recovery);
                return None;
            }
        }?;

        if !allow_infix {
            return Some(lhs);
        }

        // Infix type operators: +, -, *, /, ^, <, >, <=, >=, ==, !=, |, &, in, <->, -->
        // <--> is lexed as `<-` `->` — handle before general infix.
        if !recovery.contains(self.current()) && self.at(T![<-]) && self.nth(1) == T![->] {
            let m = lhs.precede(self);
            self.bump_any(); // <-
            self.bump_any(); // ->
            self.parse_type_expr(recovery);
            return Some(m.complete(self, SK::TYPE_APP));
        }
        if !recovery.contains(self.current())
            && matches!(
                self.current(),
                T![+]
                    | T![-]
                    | T![*]
                    | T![/]
                    | T![^]
                    | T![<]
                    | T![>]
                    | T![<=]
                    | T![>=]
                    | T![==]
                    | T![!=]
                    | T![|]
                    | T![&]
                    | T![in]
                    | T![<->]
            )
        {
            let m = lhs.precede(self);
            // --> is lexed as `-` `->` — absorb both tokens.
            if self.at(T![-]) && self.nth(1) == T![->] {
                self.bump_any(); // -
                self.bump_any(); // ->
            } else {
                self.bump_any();
            }
            self.parse_type_expr(recovery);
            return Some(m.complete(self, SK::TYPE_APP));
        }
        Some(lhs)
    }
}
