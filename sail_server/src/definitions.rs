use std::collections::HashMap;

use sail_parser::{DeclKind, DeclRole, Scope, Span, Token};

pub fn add_definitions(
    tokens: &[(Token, Span)],
    _text: &str,
    definitions: &mut HashMap<String, usize>,
) {
    let parsed = sail_parser::parse_tokens(tokens);
    for decl in parsed.decls {
        if decl.role != DeclRole::Definition {
            continue;
        }
        let include = match decl.kind {
            DeclKind::Let | DeclKind::Var => decl.scope == Scope::TopLevel,
            _ => true,
        };
        if !include {
            continue;
        }

        definitions
            .entry(decl.name.clone())
            .or_insert(decl.span.start);
        if decl.kind == DeclKind::Bitfield {
            definitions
                .entry(format!("Mk_{}", decl.name))
                .or_insert(decl.span.start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chumsky::Parser;

    #[test]
    fn captures_only_top_level_bindings() {
        let source = r#"
function foo() = {
  let x = 1;
  var y = 2;
  x + y
}
let z = 3
"#;
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();
        let mut definitions = HashMap::new();
        add_definitions(&tokens, source, &mut definitions);

        assert!(definitions.contains_key("foo"));
        assert!(definitions.contains_key("z"));
        assert!(!definitions.contains_key("x"));
        assert!(!definitions.contains_key("y"));
    }

    #[test]
    fn excludes_value_declarations_from_definitions() {
        let source = "val foo : int -> int\nfunction foo(x) = x";
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();
        let mut definitions = HashMap::new();
        add_definitions(&tokens, source, &mut definitions);
        assert!(definitions.contains_key("foo"));
    }
}
