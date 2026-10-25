//! Parameter list and argument list parsing.

use super::*;

impl<'t> Parser<'t> {
    /// Parse a parenthesized parameter list as a PARAM_LIST node.
    ///
    /// Handles: `(x, y)`, `(x : bits(3), y : int)`, `()` (UNIT token),
    /// and nested parentheses.
    pub(crate) fn parse_param_list(&mut self, end_pos: usize) {
        // Handle `()` (UNIT token) as empty param list.
        if self.at(SK::UNIT) && self.pos() < end_pos {
            self.bump_any(); // ()
        }
        // An unparenthesised type-variable parameter: `function a 'n = ..`.
        // `atomic_pat: | kid` binds the value `n` and constrains it to
        // `atom('n)` (parser.mly:541-543), so this is one parameter.
        if self.at(SK::TY_VAR) && self.pos() < end_pos {
            let pm = self.start();
            self.bump_any();
            pm.complete(self, SK::PARAM_LIST);
        }
        // Constructor heads contain full patterns, including nested tuples
        // and `as` type binders. Keep their CST instead of flattening tokens.
        if self.at(SK::IDENT) && self.nth(1) == T!['('] && self.pos() < end_pos {
            let pm = self.start();
            self.parse_pattern(PARAM_RECOVERY_SET.remove(T!['{']));
            pm.complete(self, SK::PARAM_LIST);
            return;
        }
        // Handle bare IDENT (or IDENT(args)) as a param.
        // Covers: `function f value_name = { ... }`
        //         `function f BAR(x) = x`
        //         `function f flag = if flag then 0 else 1`
        if self.at(SK::IDENT) && self.pos() < end_pos {
            let pm = self.start();
            self.bump_any(); // ident (param name or constructor)
                             // If followed by `(`, consume the constructor args
            if self.at(T!['(']) && self.pos() < end_pos {
                let mut depth = 0u32;
                loop {
                    if self.at_end() || self.pos() >= end_pos {
                        break;
                    }
                    if self.at(T!['(']) {
                        depth += 1;
                    }
                    if self.at(T![')']) {
                        depth -= 1;
                        self.bump_any();
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    self.bump_any();
                }
            }
            // Vector-subrange and vector-concat pattern parameters:
            //   `function foo imm[19] @ imm[9..0] @ imm[18..11] = ...`
            // Upstream's `funcl_patexp: pat Eq exp` (parser.mly:921-923)
            // accepts an arbitrary pattern here, including `P_vector_subrange`
            // and `P_vector_concat`, with no parentheses required.
            loop {
                if self.at(T!['[']) && self.pos() < end_pos {
                    let mut depth = 0u32;
                    loop {
                        if self.at_end() || self.pos() >= end_pos {
                            break;
                        }
                        if self.at(T!['[']) {
                            depth += 1;
                        }
                        if self.at(T![']']) {
                            depth -= 1;
                            self.bump_any();
                            if depth == 0 {
                                break;
                            }
                            continue;
                        }
                        self.bump_any();
                    }
                } else if self.at(T![@]) && self.nth(1) == SK::IDENT && self.pos() < end_pos {
                    self.bump_any(); // @
                    self.bump_any(); // ident
                } else {
                    break;
                }
            }
            // Optional type annotation: `: type`
            //
            // `->` terminates it. In the unparenthesised clause form
            // `function f x : atom('n) -> bool = ...` the arrow separates the
            // parameter pattern from the *return* type (`funcl_patexp_typ:
            // pat MinusGt typ Eq exp`, parser.mly:930); without stopping here
            // the parameter would take the function type `atom('n) -> bool`.
            if self.at(T![:]) && self.pos() < end_pos {
                self.bump_any();
                let recovery =
                    super::TYPE_RECOVERY.union(TokenSet::new(&[T![as], T![match], T![->]]));
                self.parse_type_expr(recovery);
            }
            // Optional `as id` binding
            if self.at(T![as]) && self.pos() < end_pos {
                self.bump_any();
                if self.at(SK::IDENT) && self.pos() < end_pos {
                    self.bump_any();
                }
            }
            // `@ <part> [: type]` continues the *same* parameter.
            //
            // The loop above reads `@ ident` only where no annotation
            // follows, and the marker used to close here — so
            // `function clause decode splitting : bits(4) @ vop : bits(2) @
            // rest : bits(2)` put `splitting` in the `PARAM_LIST` and left
            // the other two parts outside it, in the `CALLABLE_DEF`. They
            // were lowered by nothing, so every use of `vop` and `rest` was
            // an unresolved identifier — which a text scan for "this name
            // looks bound somewhere" then hid.
            while self.at(T![@]) && self.pos() < end_pos {
                self.bump_any(); // @
                let part_start = self.pos();
                if matches!(self.current(), SK::IDENT | SK::HEX_LIT | SK::BIN_LIT | SK::NUM_LIT)
                    || self.at(T![_])
                {
                    self.bump_any();
                }
                // `[..]` — a vector pattern, or a subrange suffix.
                while self.at(T!['[']) && self.pos() < end_pos {
                    let mut depth = 0u32;
                    loop {
                        if self.at_end() || self.pos() >= end_pos {
                            break;
                        }
                        if self.at(T!['[']) {
                            depth += 1;
                        }
                        if self.at(T![']']) {
                            depth -= 1;
                            self.bump_any();
                            if depth == 0 {
                                break;
                            }
                            continue;
                        }
                        self.bump_any();
                    }
                }
                if self.at(T![:]) && self.pos() < end_pos {
                    self.bump_any();
                    let recovery = super::TYPE_RECOVERY.union(TokenSet::new(&[
                        T![as],
                        T![match],
                        T![->],
                        T![@],
                    ]));
                    self.parse_type_expr(recovery);
                }
                // Nothing consumed — stop rather than spin.
                if self.pos() == part_start {
                    break;
                }
            }
            pm.complete(self, SK::PARAM_LIST);
        }
        // Handle bare `_` as a single wildcard param.
        // (funcl_patexp: `function test _ : int as x = x`)
        if self.at(T![_]) && self.pos() < end_pos {
            let pm = self.start();
            self.bump_any(); // _
                             // Optional type annotation: `_ : type`
            if self.at(T![:]) && self.pos() < end_pos {
                self.bump_any(); // :
                                 // Parse type, stopping at `=` or `as`
                let recovery = super::TYPE_RECOVERY.union(TokenSet::new(&[T![as], T![match]]));
                self.parse_type_expr(recovery);
            }
            // Optional `as id` binding
            if self.at(T![as]) && self.pos() < end_pos {
                self.bump_any(); // as
                if self.at(SK::IDENT) && self.pos() < end_pos {
                    self.bump_any(); // id
                }
            }
            pm.complete(self, SK::PARAM_LIST);
        }
        while self.at(T!['(']) && self.pos() < end_pos {
            let pm = self.start();
            let mut depth: u32 = 0;
            loop {
                if self.at_end() || self.pos() >= end_pos {
                    break;
                }
                if self.at(T!['(']) {
                    depth += 1;
                }
                if self.at(T![')']) {
                    depth -= 1;
                    if depth == 0 {
                        self.bump_any();
                        break;
                    }
                }
                if depth == 1 && self.at(SK::IDENT) && self.nth(1) == T!['('] {
                    self.parse_pattern(PARAM_RECOVERY_SET.remove(T!['{']));
                    continue;
                }
                // At depth 1: `:` signals a param type annotation.
                // Remove `{` from recovery so existential
                // types like Quux({'n, ... . int('n)}) can parse.
                if self.at(T![:]) && depth == 1 {
                    self.bump_any(); // :
                    let recovery = PARAM_RECOVERY_SET.remove(T!['{']);
                    self.parse_type_expr(recovery);
                    continue;
                }
                self.bump_any();
            }
            pm.complete(self, SK::PARAM_LIST);
        }
        // Handle @ (concat) between pattern groups in funcl.
        // E.g., `function foo imm[19] @ (imm[9..0] as imm) @ imm[10] = {`
        // After parsing one param group, consume remaining `@ token...`
        // fragments up to `=` or `->` or `:`.
        while self.at(T![@]) && self.pos() < end_pos {
            self.bump_any(); // @
                             // Consume the next pattern fragment (ident, ident[...], or (...))
            if self.at(T!['(']) {
                let mut depth = 0u32;
                loop {
                    if self.at_end() || self.pos() >= end_pos {
                        break;
                    }
                    if self.at(T!['(']) {
                        depth += 1;
                    }
                    if self.at(T![')']) {
                        depth -= 1;
                        self.bump_any();
                        if depth == 0 {
                            break;
                        }
                        continue;
                    }
                    self.bump_any();
                }
            } else {
                // Bare ident or ident[index] fragment
                while self.pos() < end_pos
                    && !self.at_end()
                    && !matches!(self.current(), T![@] | T![=] | T![->] | T![:] | T!['{'])
                {
                    self.bump_any();
                }
            }
        }
    }

    /// Parse argument list for function calls: `(expr, expr, ...)`.
    pub(crate) fn parse_arg_list(&mut self) -> CompletedMarker {
        let am = self.start();
        self.bump_any(); // (
        self.parse_comma_sep(T![')'], |p| {
            p.parse_expr_or_assign();
        });
        self.expect(T![')']);
        am.complete(self, SK::ARG_LIST)
    }

    /// Parse a UNIT token as an empty arg list.
    pub(crate) fn parse_unit_arg_list(&mut self) -> CompletedMarker {
        let am = self.start();
        self.bump_any(); // UNIT
        am.complete(self, SK::ARG_LIST)
    }
}
