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
