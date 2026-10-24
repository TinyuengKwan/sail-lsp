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

    /// Parse an expression but stop before `<->`.
    pub(crate) fn parse_expr_no_bidi_arrow(&mut self) {
        self.expr_bp(R_NO_BIDI, 0);
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

            // Infix: an `infix`/`infixl`/`infixr` declaration wins over the
            // built-in level, except for the operators upstream hardcodes as
            // grammar productions. `=^` is declared `infix 8` in
            // lib/vector.sail and has to bind tighter than `@`, so consulting
            // the table only as a fallback — or only for IDENT, which no
            // symbolic operator ever lexes as — leaves the declaration inert.
            let kind = self.current();
            if r.forbid_bidi_arrow && kind == T![<->] {
                break;
            }
            let dynamic = (!super::fixity_is_fixed(kind))
                .then(|| self.fixities.get(self.current_text()).copied())
                .flatten();
            let bp = dynamic.or_else(|| infix_bp(kind));
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
            // Register dereference. `atomic_exp: Star atomic_exp`
            // (infix_parser.mly:421) — an atomic expression anywhere one is
            // allowed, not only inside parentheses: `*r = 0x4` and
            // `print_bits("*r = ", *r)` are both ordinary code.
            T![*] => {
                let m = self.start();
                self.bump_any();
                // Postfix access binds inside dereference: *registers[i]
                // dereferences the selected register, not the whole vector.
                self.expr_bp(r, PREFIX_BP);
                Some(m.complete(self, SK::PREFIX_EXPR))
            }
            T!['('] => {
                let m = self.start();
                self.bump_any();
                self.parse_comma_sep(T![')'], |p| {
                    // `E_assign` is an ordinary expression upstream, so a
                    // tuple element may be one: `let (_, j) = (i = i + 1, i)`.
                    p.parse_expr_or_assign();
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
                    self.parse_expr_or_assign();
                    if self.at(T![else]) {
                        self.bump_any();
                        self.parse_expr_or_assign();
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
                    // The body is an `exp`, and `lexp Eq exp` is one:
                    // `let v = e in X(rd) = v`.
                    self.parse_expr_or_assign();
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
                    self.parse_expr_or_assign();
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
                // `Sizeof Lparen typ Rparen` (parser.mly:800) — an *atomic*
                // expression over a type, exactly like `constraint` below it.
                // Calling `parse_expr` swallowed whatever followed the
                // brackets, so `sizeof(vlen_exp) >= 7` came out as one
                // `SIZEOF_EXPR` and typed as `atom(sizeof(vlen_exp) >= 7)`.
                // A comparison then had an `atom` where it wanted a `bool`,
                // which is the whole reason `atom(..) <: bool` was ever
                // accepted in `is_subtype` — a coercion covering for a parse.
                let m = self.start();
                self.bump_any();
                if self.at(T!['(']) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                    self.expect(T![')']);
                } else {
                    self.parse_expr();
                }
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
                // The header's bounds sit in the same position a condition
                // does: `foreach i from a to b { .. }` is `Foreach loop_exp
                // Lcurly block Rcurly` (parser.mly:681), so a `{` after `b`
                // opens the body, not a struct literal. Parsed plainly, `b {
                // g(i) }` became a struct expression and the definition ended
                // inside it.
                if !self.at_set(&foreach_stop) {
                    self.expr_bp(R_NO_STRUCT, 0);
                }
                if self.at(T![to]) || self.at(T![downto]) {
                    self.bump_any();
                }
                if !self.at_set(&foreach_stop) {
                    self.expr_bp(R_NO_STRUCT, 0);
                }
                if self.at(T![by]) {
                    self.bump_any();
                    self.expr_bp(R_NO_STRUCT, 0);
                }
                // `in <order>` belongs to the *header*: upstream's
                // `loop_exp` is `id From exp To exp By exp In order=typ`
                // (parser.mly:622), and the body forms that follow it are
                // `Lparen loop_exp Rparen Do? exp` and its siblings
                // (parser.mly:678-686). There is no `foreach .. in body`.
                //
                // Treating `in` as a body introducer ended the loop at the
                // `in`: `foreach (i from 0 to 32 by 1 in inc) { .. }` left
                // `inc`, the `)` and *the whole block* outside the loop as
                // siblings, so the loop body was not the loop's body and
                // every name the header bound was out of scope inside it.
                if self.at(T![in]) {
                    self.bump_any();
                    self.parse_type_expr(TYPE_RECOVERY);
                }
                if self.at(T![')']) {
                    self.bump_any();
                }
                if self.at(T![do]) {
                    self.bump_any();
                }
                // An unbraced loop body may be an assignment —
                // `foreach (j from 16 to 23) w[j] = f(w[j - 16])`. Parsing
                // it as an expression left `w[j]` as the whole body and the
                // `= f(..)` as a sibling of the loop, so every `j` on the
                // right was outside the loop's scope.
                self.parse_expr_or_assign();
                Some(m.complete(self, SK::FOREACH_EXPR))
            }
            T![while] => {
                let m = self.start();
                self.bump_any();
                // `while c { .. }` is a loop with a braced body
                // (`While exp in_place_loop_measure Lcurly block Rcurly`,
                // parser.mly:691), so the condition must not take the `{` for
                // a struct literal — `if` and a `match` scrutinee already say
                // so with `R_NO_STRUCT`, and this was the third such position
                // and the only one left plain. `c { g(2) }` parsed as
                // `c { .. }` and the definition ended mid-expression, so the
                // rest of the function was lost.
                self.expr_bp(R_NO_STRUCT, 0);
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
                    self.parse_expr_or_assign();
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
                // The loop body is an ordinary expression, and `E_assign` is
                // one: `repeat i = i + 1 until i < 100`.
                self.parse_expr_or_assign();
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
                // Optional struct name: upstream `E_struct of id option * ...`
                // covers both `struct { f = v }` and `struct S { f = v }`
                // (parse_ast.ml:246).
                if self.at(SK::IDENT) && self.nth(1) == T!['{'] {
                    let nm = self.start();
                    self.bump_any();
                    nm.complete(self, SK::NAME);
                }
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
                // `$[attr] exp` — upstream's `exp := attributes exp`
                // (`parser.mly:648`). The attribute *wraps* what follows; it
                // used to be completed as a `CONFIG_EXPR` and stop there, so
                // `$[complete] match e { .. }` left the match dangling and
                // nothing checked it.
                let m = self.start();
                self.bump_attribute_tokens();
                self.parse_lhs(r);
                Some(m.complete(self, SK::ATTRIBUTE))
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
                // `x.f()` and `x.f(args)` are the deprecated bitfield
                // accessor form, `_mod_f(x, args)` (parser.mly:778-783).
                // The lexer folds an empty argument list into one `UNIT`
                // token, so `sysreg.bits()` needs that case too — without it
                // the `()` was left to start the next match arm.
                let field = m.complete(self, SK::FIELD_ACCESS_EXPR);
                if self.at(SK::UNIT) {
                    let m = field.precede(self);
                    self.bump_any();
                    return Some(m.complete(self, SK::CALL_EXPR));
                }
                if self.at(T!['(']) {
                    let m = field.precede(self);
                    self.parse_arg_list();
                    return Some(m.complete(self, SK::CALL_EXPR));
                }
                Some(field)
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
                                 // `<->` terminates the annotation. A bidirectional type only
                                 // appears in a `val`/`mapping` signature, never in an
                                 // expression, so in `mapping clause f = x : bits(1) <-> x :
                                 // bits(2)` the arrow separates the clause's two sides.
                self.parse_atomic_type_expr(TYPE_RECOVERY.union(TokenSet::new(&[T![<->]])));
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

    /// Parse an expression, then an optional `= expr` assignment.
    ///
    /// Upstream's `exp` includes `E_assign`, so an assignment is an ordinary
    /// expression and may appear wherever one may — notably as a function
    /// argument, `test(i = i + 1, i)`.
    pub(crate) fn parse_expr_or_assign(&mut self) {
        let m = self.start();
        self.parse_expr();
        if self.at(T![=]) && self.nth(1) != T![=] && self.nth(1) != T![>] {
            self.bump_any(); // =
            self.parse_expr();
            m.complete(self, SK::ASSIGN_EXPR);
        } else {
            m.abandon(self);
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

    /// Parse a mapping body: `{ forwards P [if g] => E, backwards .., P <-> Q }`.
    ///
    /// A mapping clause has the same shape as a match arm, and until now it
    /// was parsed as neither: `parse_block` ran `parse_expr` over the whole
    /// thing, so `forwards Regidx(r) => zero_extend(r)` came out as two
    /// sibling `BLOCK_ITEM`s with `forwards` and `=>` as loose tokens, and
    /// the head was an *expression*. Nothing bound `r`, so every use of a
    /// mapping-clause variable resolved through no scope at all.
    pub(crate) fn parse_mapping_body(&mut self) -> Option<CompletedMarker> {
        let m = self.start();
        self.bump_any(); // {
        while !self.at_end() && !self.at(T!['}']) {
            if self.at(T![,]) {
                self.bump_any();
                continue;
            }
            let checkpoint = self.pos();
            let cm = self.start();
            while self.at_attribute() {
                self.bump_attribute();
            }
            if self.at(SK::KW_FORWARDS) || self.at(SK::KW_BACKWARDS) {
                self.bump_any();
            }
            let recovery = PAT_RECOVERY_SET.union(TokenSet::new(&[
                T![=>],
                T![<->],
                T![,],
                T!['}'],
                T![when],
                T![^],
            ]));
            // A mapping pattern is not an ordinary one: `sep() ^ "mf8"`
            // joins sub-patterns with `^`, which no other pattern position
            // allows (upstream's `mpat`, parser.mly).
            self.parse_mapping_pattern(recovery);
            // `encdec_reg(i) if get_config_use_abi_names() <-> ..` — a
            // guard is written between the head and *either* arrow.
            if self.at(T![if]) {
                self.bump_any();
                self.parse_expr_no_bidi_arrow();
            }
            if self.at(T![<->]) {
                // Bidirectional: both sides are patterns *and* expressions.
                self.bump_any();
                self.parse_mapping_pattern(recovery);
            } else {
                if self.at(T![=>]) {
                    self.bump_any();
                } else if self.at(T![=]) && self.nth(1) == T![>] {
                    self.bump_any();
                    self.bump_any();
                } else {
                    self.error("expected `=>` or `<->` in mapping clause".to_string());
                }
                self.parse_expr_or_assign();
            }
            // `.. <-> (ma, ta, sew, lmul) when sew[2] != 0b1` — a guard on
            // a bidirectional clause is spelled `when`, and comes last.
            if self.at(T![when]) {
                self.bump_any();
                self.parse_expr();
            }
            cm.complete(self, SK::MAPPING_CLAUSE);
            if self.at(T![,]) {
                self.bump_any();
            }
            if self.pos() == checkpoint {
                self.bump_any();
            }
        }
        self.expect(T!['}']);
        Some(m.complete(self, SK::BLOCK_EXPR))
    }

    /// A `^`-joined chain of patterns, as a mapping clause head allows.
    fn parse_mapping_pattern(&mut self, recovery: TokenSet) {
        let m = self.start();
        let mut joined = false;
        loop {
            let before = self.pos();
            self.parse_pattern(recovery);
            if self.pos() == before || !self.at(T![^]) {
                break;
            }
            self.bump_any(); // ^
            joined = true;
        }
        // Left unwrapped, the chain reaches the lowering as *separate*
        // children and the clause reads its second element as the other
        // side of the `<->`.
        if joined {
            m.complete(self, SK::BIN_PAT);
        } else {
            m.abandon(self);
        }
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
            self.parse_match_arm_contents();
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
    /// Attributes wrap a complete case, including its RHS, in parentheses.
    /// Keep the pattern and body directly in MATCH_ARM for HIR lowering.
    fn parse_match_arm_contents(&mut self) {
        let mut wrappers = 0;
        while self.at_attribute() {
            while self.at_attribute() {
                self.bump_attribute();
            }
            if self.eat(T!['(']) {
                wrappers += 1;
            } else {
                break;
            }
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
        // `case: pat EqGt exp` (parser.mly:722), and `lexp Eq exp` is an
        // `exp`: `match n { 32 => r = y @ y, .. }`.
        self.parse_expr_or_assign();
        for _ in 0..wrappers {
            self.expect(T![')']);
        }
    }
}
