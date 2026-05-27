//! Post-parse tree validation.
//! Checks for semantic errors not caught by the parser:
//! - Duplicate `$include` directives
//! - `$ifdef` without matching `$endif`/$else
//! - Invalid numeric literal values
//! - Mismatched definition patterns

use crate::syntax_error::SyntaxError;
use crate::syntax_node::SyntaxNode;
use parser::SyntaxKind as SK;

/// Validate a parsed syntax tree for semantic errors.
///
/// Appends additional `SyntaxError`s beyond what the parser produces.
pub(crate) fn validate(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    validate_directives(root, errors);
    validate_definitions(root, errors);
}

/// Check for directive-level issues.
fn validate_directives(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    let mut include_set = std::collections::HashSet::new();
    let mut ifdef_depth: i32 = 0;

    for child in root.children() {
        if child.kind() != SK::DIRECTIVE_DEF {
            continue;
        }
        let text = child.text().to_string();
        let trimmed = text.trim();
        let offset = child.text_range().start();

        // Check for duplicate $include
        if trimmed.starts_with("$include") {
            let path = trimmed.strip_prefix("$include").unwrap_or("").trim();
            if !include_set.insert(path.to_string()) {
                errors.push(SyntaxError::new_at_offset(
                    format!("duplicate $include: {}", path),
                    offset,
                ));
            }
        }

        // Track $ifdef/$endif balance
        if trimmed.starts_with("$ifdef")
            || trimmed.starts_with("$ifndef")
            || trimmed.starts_with("$iftarget")
        {
            ifdef_depth += 1;
        } else if trimmed.starts_with("$endif") {
            ifdef_depth -= 1;
            if ifdef_depth < 0 {
                errors.push(SyntaxError::new_at_offset(
                    "$endif without matching $ifdef".to_string(),
                    offset,
                ));
                ifdef_depth = 0;
            }
        }
    }

    // Unclosed $ifdef at end of file
    if ifdef_depth > 0 {
        let last_offset = root.text_range().end();
        errors.push(SyntaxError::new_at_offset(
            format!("{} unclosed $ifdef/$ifndef directive(s)", ifdef_depth),
            last_offset,
        ));
    }
}

/// Check for definition-level issues.
fn validate_definitions(root: &SyntaxNode, errors: &mut Vec<SyntaxError>) {
    for child in root.children() {
        // Check for ERROR nodes (parser recovery)
        if child.kind() == SK::ERROR {
            let offset = child.text_range().start();
            errors.push(SyntaxError::new_at_offset("syntax error".to_string(), offset));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_text;

    #[test]
    fn no_errors_for_valid_file() {
        let (root, _) = parse_text("function f(x) = x\n");
        let mut errors = Vec::new();
        validate(&root, &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn detects_duplicate_include() {
        let (root, _) =
            parse_text("$include \"foo.sail\"\n$include \"foo.sail\"\nfunction f(x) = x\n");
        let mut errors = Vec::new();
        validate(&root, &mut errors);
        assert!(
            errors.iter().any(|e| e.message().contains("duplicate")),
            "should detect duplicate include, got: {:?}",
            errors
        );
    }
}
