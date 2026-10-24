//! Grammar integration tests (moved from parser/src/grammar/mod.rs).
//!
//! These test the full lex -> parse -> tree pipeline via `parse_text`.

#[cfg(test)]
mod tests {
    use crate::parsing::parse_text;
    use crate::syntax_node::SyntaxNode;
    use parser::SyntaxKind as SK;

    fn assert_lossless(input: &str) {
        let (root, _) = parse_text(input);
        assert_eq!(root.text().to_string(), input, "lossless round-trip failed for: {input:?}");
    }

    fn find_kind(root: &SyntaxNode, kind: SK) -> Vec<SyntaxNode> {
        root.descendants().filter(|n| n.kind() == kind).collect()
    }

    #[test]
    fn attributed_match_cases_preserve_pattern_and_body() {
        for arm in ["$[foo] (1 => 2)", "$[foo] ($[bar] (1 if true => 2))"] {
            let source = format!("function f(x : int) -> int = match x {{ {arm}, _ => 3 }}\n");
            let (root, errors) = parse_text(&source);
            assert!(errors.is_empty(), "{errors:?}");
            assert_eq!(root.text().to_string(), source);
            let arms = find_kind(&root, SK::MATCH_ARM);
            assert_eq!(arms.len(), 2);
            assert!(arms[0].children().any(|n| n.kind() == SK::LITERAL_PAT));
            assert!(arms[0].children().any(|n| n.kind() == SK::LITERAL_EXPR));
        }
    }

    #[test]
    fn empty() {
        assert_lossless("");
    }
    #[test]
    fn val_spec() {
        assert_lossless("val x : int\n");
    }
    #[test]
    fn function_def() {
        assert_lossless("function f() = 42\n");
    }
    #[test]
    fn multi_defs() {
        assert_lossless("val x : int\nfunction f() = 42\ntype myint = int\n");
    }
    #[test]
    fn comments() {
        assert_lossless("// comment\nval x : int\n/* block */\nfunction f() = 42\n");
    }
    #[test]
    fn complex() {
        assert_lossless("val add : (int, int) -> int\nfunction add(x, y) = x + y\n");
    }

    /// `let 'cap_size = 32` names a value, so its initialiser is an
    /// expression like any other.
    ///
    /// `atomic_pat: | kid` binds `id_of_kid kid` (parser.mly:541-543,
    /// ast_util.ml:983). Without the type variable counting as a definition
    /// name, nothing consumed it, the `= expr` branch was never reached, and
    /// the initialiser arrived as a loose `NUM_LIT` — so the environment had
    /// no type to read off it.
    /// Every definition form upstream's `def_aux` lists parses as one
    /// definition, and does not disturb the next one.
    ///
    /// A sweep of `parser.mly` rather than of the corpora: a form no corpus
    /// file uses cannot move any of the four probes, so the thing to assert
    /// is the *shape* — the form parses without errors, and the `val zzz`
    /// after it is still its own definition. That is how `impl` was found
    /// (`Impl funcl`, parser.mly:1401): upstream type-checks it, this
    /// produced seven syntax errors, because `impl` was understood inside an
    /// `outcome` body and nowhere else.
    #[test]
    fn every_definition_form_parses_as_one_definition() {
        // Two forms upstream parses and this does not. Named rather than
        // dropped, each with what it needs:
        //
        // `mutual { fun_def_list }` (parser.mly:1409) — the functions
        // inside must end up in the item tree, which walks the source
        // file's *direct* children, so nesting them under a wrapper node
        // would hide them and flattening loses the grouping. It needs a
        // decision, not a parser tweak.
        //
        const NOT_PARSED: &[&str] = &["mutual { function a() = 1 function b() = 2 }"];
        for form in [
            "type t = int",
            "type t('n : Int) = bits('n)",
            "type t : Int = 8",
            "type constraint 'n > 0",
            "struct s = { f : int }",
            "struct s('n : Int) = { f : bits('n) }",
            "enum e = A | B",
            "enum e = { A, B }",
            "newtype n = N : int",
            "union u = { C : int }",
            "union u('n : Int) = { C : bits('n) }",
            "bitfield b : bits(8) = { hi : 7 .. 4 }",
            "register r : bits(8)",
            "let x : int = 1",
            "overload o = {f, g}",
            "default Order dec",
            "val v : int -> int",
            "function f() = 1",
            "mapping m : bits(1) <-> bool = { 0b0 <-> false }",
            "impl f(x) = x",
            "termination_measure f(x) = x",
            "infix 4 <+>",
            "scattered union su",
            "outcome o : unit -> unit",
            "mutual { function a() = 1 function b() = 2 }",
        ] {
            let src = format!("{form}\nval zzz : int\n");
            let (root, errs) = crate::parse_text(&src);
            let last = root.children().last();
            let sentinel_intact =
                last.as_ref().is_some_and(|n| n.text().to_string().trim() == "val zzz : int");
            if NOT_PARSED.contains(&form) {
                assert!(!errs.is_empty(), "{form:?} parses now: take it off NOT_PARSED");
                continue;
            }
            assert!(errs.is_empty(), "{form:?} does not parse: {} errors", errs.len());
            assert!(sentinel_intact, "{form:?} disturbed the definition after it");
        }
    }

    /// A `{` after a header opens a body, and the definition survives it.
    ///
    /// `While exp measure Lcurly block Rcurly` (parser.mly:691) and
    /// `Foreach loop_exp Lcurly block Rcurly` (:681) both let a brace follow
    /// a header whose last token is a *name* — exactly where the expression
    /// parser would otherwise read a struct literal. `if` and a `match`
    /// scrutinee guard that with `R_NO_STRUCT`; `while` and `foreach` did
    /// not, and `while c { .. }` ended the enclosing definition inside the
    /// struct expression, spilling the rest of the function out as top-level
    /// tokens.
    ///
    /// The property, not the guard: each form must leave exactly one
    /// definition covering the whole source.
    #[test]
    fn a_braced_body_after_a_header_does_not_end_the_definition() {
        for body in [
            "while c { g(1) }",
            "while c do { g(1) }",
            "foreach i from a to b { g(i) }",
            "foreach i from a to b by s { g(i) }",
            "foreach (i from a to b by s in inc) { g(i) }",
            "match c { _ => g(1) }",
            "if c then { g(1) } else { g(2) }",
            "repeat { g(1) } until c",
        ] {
            let src = format!("function t() = {{ {body} }}\n");
            assert_lossless(&src);
            let (root, _) = crate::parse_text(&src);
            let defs: Vec<_> = root.children().filter(|c| c.kind() == SK::CALLABLE_DEF).collect();
            assert_eq!(defs.len(), 1, "{body:?} did not leave one definition");
            let covered = usize::from(defs[0].text_range().end());
            assert_eq!(
                covered,
                src.trim_end().len(),
                "{body:?} ended its definition early, losing the rest of the function"
            );
        }
    }

    #[test]
    fn a_let_may_name_a_type_variable() {
        let src = "let 'cap_size = 32\n";
        assert_lossless(src);
        let (root, _) = crate::parse_text(src);
        assert_eq!(
            find_kind(&root, SK::NAME).len(),
            1,
            "the type variable is the definition's name"
        );
        assert_eq!(find_kind(&root, SK::LITERAL_EXPR).len(), 1, "the initialiser is an expression");
    }

    #[test]
    fn val_spec_node() {
        let (root, _) = parse_text("val x : int\n");
        assert!(!find_kind(&root, SK::CALLABLE_SPEC).is_empty());
    }
    #[test]
    fn function_def_node() {
        let (root, _) = parse_text("function f() = 42\n");
        assert!(!find_kind(&root, SK::CALLABLE_DEF).is_empty());
    }
    #[test]
    fn enum_node() {
        let (root, _) = parse_text("enum color = { Red, Green, Blue }\n");
        assert!(!find_kind(&root, SK::NAMED_DEF).is_empty());
    }
    #[test]
    fn infix() {
        let (root, _) = parse_text("function f(x, y) = x + y\n");
        assert!(!find_kind(&root, SK::BIN_EXPR).is_empty());
    }
    #[test]
    fn call() {
        let (root, _) = parse_text("function f() = add(1, 2)\n");
        assert!(!find_kind(&root, SK::CALL_EXPR).is_empty());
    }
    #[test]
    fn if_expr() {
        let (root, _) = parse_text("function f(x) = if x == 0 then 1 else 2\n");
        assert!(!find_kind(&root, SK::IF_EXPR).is_empty());
    }
    #[test]
    fn literal() {
        let (root, _) = parse_text("function f() = 42\n");
        assert!(!find_kind(&root, SK::LITERAL_EXPR).is_empty());
    }
    #[test]
    fn block() {
        let (root, _) = parse_text("function f() = { let x = 1; x + 2 }\n");
        assert!(!find_kind(&root, SK::BLOCK_EXPR).is_empty());
    }
    #[test]
    fn precedence() {
        let (root, _) = parse_text("function f(x, y, z) = x + y * z\n");
        assert!(find_kind(&root, SK::BIN_EXPR).len() >= 2);
    }
    #[test]
    fn match_expr() {
        let input = "function f(x) = match x { 0 => 10, _ => 20 }\n";
        assert_lossless(input);
        let (root, _) = parse_text(input);
        assert!(!find_kind(&root, SK::MATCH_EXPR).is_empty());
        assert_eq!(find_kind(&root, SK::MATCH_ARM).len(), 2);
    }
    #[test]
    fn wildcard_pattern() {
        let (root, _) = parse_text("function f(x) = match x { _ => 0 }\n");
        assert!(!find_kind(&root, SK::WILD_PAT).is_empty());
    }
    #[test]
    fn constructor_pattern() {
        let (root, _) = parse_text("function f(x) = match x { Some(v) => v }\n");
        assert!(!find_kind(&root, SK::APP_PAT).is_empty());
    }
    #[test]
    fn scattered_def() {
        let (root, _) = parse_text("scattered function foo\n");
        assert!(!find_kind(&root, SK::SCATTERED_DEF).is_empty());
    }
    #[test]
    fn lossless_complex_file() {
        assert_lossless(
            "\
// Sail example
val add : (int, int) -> int
function add(x, y) = x + y

enum color = { Red, Green, Blue }

type myint = int
",
        );
    }

    #[test]
    fn missing_eq_unit_param() {
        let (root, errors) = parse_text("function f() 42\n");
        assert_eq!(root.text().to_string(), "function f() 42\n");
        assert!(
            !errors.is_empty(),
            "expected parse error for missing `=`, got 0 errors. CST:\n{root:#?}"
        );
    }

    #[test]
    fn missing_eq_real_param() {
        let (root, errors) = parse_text("function f(x) 42\n");
        assert_eq!(root.text().to_string(), "function f(x) 42\n");
        assert!(!errors.is_empty(), "expected parse error for missing `=`, got 0 errors");
    }
}

#[test]
fn duplicate_block_documentation_and_visibility_are_rejected() {
    for source in [
        "/*! a */ /*! b */ function f() -> unit = ()",
        "val f : int -> int\nfunction /*! a */ /*! b */ f(x) = x",
        "union U = { /*! a */ /*! b */ A : int }",
        "private private function f() -> unit = ()",
        "union U = { private private A : int }",
        "outcome f : unit -> unit with 'n : Int = { impl $[a] f() = () }",
    ] {
        let parse = crate::SourceFile::parse(source);
        assert_eq!(parse.syntax_node().text().to_string(), source);
        assert!(!parse.errors().is_empty(), "expected a declaration annotation error for {source}");
    }
    for source in [
        "/*! a */ function f() -> unit = ()",
        "union U = { /*! a */ A : int, /*! b */ B : bool }",
        "val f : int -> int\nfunction $[foo] f(x) = x",
        "val f : nat -> nat\nfunction {x => x} f(x) = if x == 0 then 0 else f(x - 1)",
    ] {
        let (tree, errors) = crate::parse_text(source);
        assert_eq!(tree.text().to_string(), source);
        assert!(errors.is_empty(), "{source}: {errors:?}");
        if source.contains("val f") {
            let def =
                tree.children().find(|n| n.kind() == crate::SyntaxKind::CALLABLE_DEF).unwrap();
            assert!(def
                .children_with_tokens()
                .any(|e| e.kind() == crate::SyntaxKind::IDENT && e.to_string() == "f"));
            assert!(def.children().any(|n| n.kind() == crate::SyntaxKind::PARAM_LIST));
        }
    }
}

#[test]
fn dereference_binds_after_postfix_but_before_infix() {
    for expr in ["*refs[0]", "(*refs[0])", "(*refs[0] + 1)"] {
        let source = format!("function f() -> int = {expr}");
        let (tree, errors) = crate::parse_text(&source);
        assert!(errors.is_empty(), "{source}: {errors:?}");
        let deref =
            tree.descendants().find(|n| n.kind() == crate::SyntaxKind::PREFIX_EXPR).unwrap();
        assert_eq!(deref.text().to_string(), "*refs[0]");
    }
}
