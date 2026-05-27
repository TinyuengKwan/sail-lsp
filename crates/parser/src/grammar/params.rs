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
                    if self.at_end() || self.pos() >= end_pos { break; }
                    if self.at(T!['(']) { depth += 1; }
                    if self.at(T![')']) {
                        depth -= 1;
                        self.bump_any();
                        if depth == 0 { break; }
                        continue;
                    }
                    self.bump_any();
                }
            }
            // Optional type annotation: `: type`
            if self.at(T![:]) && self.pos() < end_pos {
                self.bump_any();
                let recovery =
                    super::TYPE_RECOVERY.union(TokenSet::new(&[T![as], T![match]]));
                self.parse_type_expr(recovery);
            }
            // Optional `as id` binding
            if self.at(T![as]) && self.pos() < end_pos {
                self.bump_any();
                if self.at(SK::IDENT) && self.pos() < end_pos {
                    self.bump_any();
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
                let recovery =
                    super::TYPE_RECOVERY.union(TokenSet::new(&[T![as], T![match]]));
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
                    && !matches!(
                        self.current(),
                        T![@] | T![=] | T![->] | T![:] | T!['{']
                    )
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
            p.parse_expr();
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
