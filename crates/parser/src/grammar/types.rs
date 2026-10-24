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

    /// `kopt_list` — the variables a `forall` or an existential binds.
    ///
    /// One `TYPE_PARAM` per `kopt`, which is upstream's unit: `('n 'm : Int)`
    /// binds two variables at one kind and is one `kopt` (`parser.mly:430`),
    /// so it is one node holding both names and the kind. A bare `'n` is a
    /// `kopt` too, with no kind written — Sail defaults it to `Int`.
    pub(crate) fn parse_type_param_list(&mut self) {
        let list = self.start();
        loop {
            if self.at(T!['(']) {
                // `( 'a 'b : Kind )`, or `( constant 'n : Int )`. Only a
                // group that names a type variable is a `kopt`; anything
                // else parenthesised here is not ours to consume.
                if !self.nth_at(1, SK::TY_VAR) && !self.nth_at(1, SK::IDENT) {
                    break;
                }
                let m = self.start();
                self.bump_any();
                let mut depth = 0usize;
                while !self.at_end() {
                    if self.at(T!['(']) {
                        depth += 1;
                    } else if self.at(T![')']) {
                        if depth == 0 {
                            break;
                        }
                        depth -= 1;
                    } else if self.at(T![.]) && depth == 0 {
                        break;
                    }
                    self.bump_any();
                }
                self.expect(T![')']);
                m.complete(self, SK::TYPE_PARAM);
            } else if self.at(SK::TY_VAR) {
                let m = self.start();
                self.bump_any();
                m.complete(self, SK::TYPE_PARAM);
            } else {
                break;
            }
        }
        list.complete(self, SK::TYPE_PARAM_LIST);
    }

    /// `type ds('n: Int, 'm: Int)` — a synonym's parameters.
    ///
    /// The same `kopt`s a `forall` binds, written as one comma-separated
    /// group rather than a sequence of parenthesised ones.
    pub(crate) fn parse_type_synonym_params(&mut self) {
        let list = self.start();
        self.bump_any(); // (
        while !self.at_end() && !self.at(T![')']) {
            if self.at(SK::TY_VAR) || self.at(SK::IDENT) {
                let m = self.start();
                self.bump_any();
                if self.at(T![:]) {
                    self.bump_any();
                    // `Int`, `Bool` and `Type` are keywords, not identifiers
                    // — taking only `IDENT` here left `type ds('n: Int)`
                    // with its kind outside the node, so the kind check saw
                    // a parameter that declared nothing.
                    if matches!(
                        self.current(),
                        SK::IDENT | SK::KW_INT | SK::KW_BOOL | SK::KW_TYPE_UPPER | SK::KW_ORDER
                    ) {
                        self.bump_any();
                    }
                }
                m.complete(self, SK::TYPE_PARAM);
            } else {
                self.bump_any();
            }
        }
        self.expect(T![')']);
        list.complete(self, SK::TYPE_PARAM_LIST);
    }

    /// Wrap `lhs` in a `TYPE_ARROW` if a `->` or `<->` follows.
    ///
    /// Every atomic-type arm has to do this itself. A numeric set or an
    /// existential is an ordinary `typ`, so `{4, 8} -> unit` and
    /// `{'n, 'n > 0. int('n)} -> unit` are function types -- and a `val`
    /// whose arrow was never built looks like it takes no parameters.
    fn type_arrow_tail(&mut self, lhs: CompletedMarker, recovery: TokenSet) -> CompletedMarker {
        if !(self.at(T![->]) || self.at(T![<->])) || recovery.contains(self.current()) {
            return lhs;
        }
        let m = lhs.precede(self);
        self.bump_any();
        self.parse_type_expr(recovery);
        m.complete(self, SK::TYPE_ARROW)
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
                // Upstream's `typquant := kopt_list [',' typ]`
                // (`parser.mly:444`). This used to `bump_any()` until the
                // `.`, leaving the quantifier list as loose tokens under
                // `TYPE_FORALL` — so `TypeParamList` and `TypeParam` were
                // declared in `sail.ungram`, generated into the AST, and
                // built by nobody, while three consumers re-derived the
                // list from the token stream with three different
                // heuristics.
                self.parse_type_param_list();
                if self.at(T![,]) {
                    self.bump_any();
                    self.parse_type_expr(recovery.union(TokenSet::new(&[SK::DOT])));
                }
                self.expect(T![.]);
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
                if (self.at(T![->]) || self.at(T![<->])) && !recovery.contains(self.current()) {
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
                if (self.at(T![->]) || self.at(T![<->])) && !recovery.contains(self.current()) {
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
                if (self.at(T![->]) || self.at(T![<->])) && !recovery.contains(self.current()) {
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
                // `{ kopt_list [',' constraint] '.' typ }` — upstream's
                // `typquant` again (`parser.mly:444`), inside braces. The
                // same braces also spell a numeric set, `{1, 2, 4, 8}`,
                // which binds nothing; a `kopt` is the difference, and a
                // `kopt` starts with a type variable or a `(`.
                //
                // Built as a tree for the same reason the `forall` is:
                // `hir-ty` carried a second copy of the token heuristics —
                // find the first comparison operator, walk back to the comma
                // before it — to recover the variables and the constraint.
                if matches!(self.current(), SK::TY_VAR) || self.at(T!['(']) {
                    self.parse_type_param_list();
                    if self.at(T![,]) {
                        self.bump_any();
                        self.parse_type_expr(recovery.union(TokenSet::new(&[SK::DOT])));
                    }
                    self.expect(T![.]);
                    self.parse_type_expr(recovery.union(TokenSet::new(&[SK::R_CURLY])));
                    self.expect(T!['}']);
                    let cm = m.complete(self, SK::TYPE_EXISTENTIAL);
                    return Some(self.type_arrow_tail(cm, recovery));
                }
                // A numeric set. Track brace depth so nested `{1, 2, 4, 8}`
                // sets don't prematurely close it.
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
                let cm = m.complete(self, SK::TYPE_EXISTENTIAL);
                Some(self.type_arrow_tail(cm, recovery))
            }
            // Deprecated `{| |}` syntax for numeric sets.
            SK::L_CURLY_BAR => {
                let m = self.start();
                self.bump_any(); // {|
                while !self.at_end() && !self.at(SK::R_CURLY_BAR) {
                    self.bump_any();
                }
                self.expect(SK::R_CURLY_BAR);
                let cm = m.complete(self, SK::TYPE_EXISTENTIAL);
                Some(self.type_arrow_tail(cm, recovery))
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
                if (self.at(T![->]) || self.at(T![<->])) && !recovery.contains(self.current()) {
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
                    // User-defined infix operators are legal in type and
                    // constraint position: upstream collects them as
                    // `ATyp_infix` and re-associates them in a second pass
                    // using the declared fixities (parse_ast.ml:143,
                    // infix_parser.mly). Without this, `constraint('p --> 'q)`
                    // stops at the operator and reports a spurious `expected
                    // R_PAREN`.
                    | SK::OP_IDENT
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
