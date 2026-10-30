//! SSR rule parsing.
//! Parses the `search ==>> replace` syntax into structured rules.
//! Supports placeholder syntax: `$name` or `${name:constraint}`.

use rustc_hash::FxHashMap;

use syntax::SyntaxNode;

use crate::errors::SsrError;
use crate::fragments;

/// A parsed SSR rule with pattern and template as syntax trees.
#[derive(Debug, Clone)]
pub(crate) struct ParsedRule {
    /// Placeholder metadata, keyed by stand-in name (e.g., `__ssr_0_a`).
    pub(crate) placeholders_by_stand_in: FxHashMap<String, Placeholder>,
    /// The parsed pattern as a syntax tree.
    pub(crate) pattern: SyntaxNode,
    /// The parsed template (replacement) as a syntax tree.
    pub(crate) template: SyntaxNode,
}

/// A placeholder in an SSR pattern.
#[derive(Debug, Clone)]
pub struct Placeholder {
    /// The user-visible variable (e.g., `Var("a")` for `$a`).
    pub(crate) ident: Var,
    /// Optional constraints on what the placeholder can match.
    pub(crate) constraints: Vec<Constraint>,
}

/// A named variable in an SSR pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Var(pub(crate) String);

/// Constraint on a placeholder binding.
#[derive(Debug, Clone)]
pub(crate) enum Constraint {
    /// The matched node must be of a specific kind.
    Kind(NodeKind),
    /// Negation of a constraint.
    Not(Box<Constraint>),
}

/// Kinds of nodes that a constraint can require.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NodeKind {
    Literal,
}

/// Prefix used to create stand-in identifiers from placeholders.
pub(crate) const PLACEHOLDER_PREFIX: &str = "__ssr_";

/// Split only outside strings and comments, using the Sail lexer.
pub(crate) fn split_rule(input: &str) -> Result<(&str, &str), SsrError> {
    // A rule placeholder is not a Sail pragma. Mask its sigil without
    // changing byte offsets before asking the lexer about trivia/literals.
    let lexical_input = input.replace('$', "_");
    let lexed = parser::LexedStr::new(&lexical_input);
    let mut masked = input.as_bytes().to_vec();
    let mut offset = 0;
    for i in 0..lexed.len() {
        let len = lexed.text(i).len();
        if matches!(
            lexed.kind(i),
            parser::SyntaxKind::STRING_LIT
                | parser::SyntaxKind::MULTILINE_STRING_LIT
                | parser::SyntaxKind::LINE_COMMENT
                | parser::SyntaxKind::BLOCK_COMMENT
                | parser::SyntaxKind::DOC_COMMENT
        ) {
            masked[offset..offset + len].fill(b' ');
        } else if lexed.kind(i) == parser::SyntaxKind::IDENT
            && lexed.text(i).starts_with(PLACEHOLDER_PREFIX)
        {
            bail!("Identifiers beginning with __ssr_ are reserved in SSR rules");
        }
        offset += len;
    }
    let masked = String::from_utf8(masked).expect("mask preserves UTF-8");
    let separators: Vec<_> = masked.match_indices("==>>").map(|(i, _)| i).collect();
    if separators.len() != 1 {
        bail!("Expected exactly one `search ==>> replace` separator");
    }
    let i = separators[0];
    let (search, replace) = (input[..i].trim(), input[i + 4..].trim());
    if search.is_empty() || replace.is_empty() {
        bail!("Search and replacement must not be empty");
    }
    Ok((search, replace))
}

/// Parse the expression pair accepted by the SSR request.
pub(crate) fn parse_rule(input: &str) -> Result<ParsedRule, SsrError> {
    let (search, replace) = split_rule(input)?;
    let (search, placeholders) = replace_placeholders(search)?;
    let (replace, replacement_placeholders) = replace_placeholders(replace)?;
    for placeholder in replacement_placeholders.values() {
        if !placeholders.values().any(|p| p.ident == placeholder.ident) {
            bail!("Placeholder ${} in replace not found in search", placeholder.ident.0);
        }
    }
    let (Some(pattern), Some(template)) = (fragments::expr(&search), fragments::expr(&replace))
    else {
        bail!("Search and replacement must be valid Sail expressions; item, type and binding-pattern rules are unsupported");
    };
    Ok(ParsedRule { placeholders_by_stand_in: placeholders, pattern, template })
}

/// Replace `$name` and `${name:constraint}` with stand-in identifiers.
///
/// Returns the modified text and a map of stand-in → Placeholder.
fn replace_placeholders(text: &str) -> Result<(String, FxHashMap<String, Placeholder>), SsrError> {
    let mut result = String::new();
    let mut placeholders: FxHashMap<String, Placeholder> = FxHashMap::default();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        // A dollar inside a string or comment is literal text, not a capture.
        if ch == '"' {
            result.push(ch);
            let mut escaped = false;
            for next in chars.by_ref() {
                result.push(next);
                if next == '"' && !escaped {
                    break;
                }
                escaped = next == '\\' && !escaped;
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            result.push(ch);
            for next in chars.by_ref() {
                result.push(next);
                if next == '\n' {
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            result.push(ch);
            result.push(chars.next().unwrap());
            let mut depth = 1;
            while let Some(next) = chars.next() {
                result.push(next);
                if next == '/' && chars.peek() == Some(&'*') {
                    result.push(chars.next().unwrap());
                    depth += 1;
                } else if next == '*' && chars.peek() == Some(&'/') {
                    result.push(chars.next().unwrap());
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
            }
            continue;
        }
        if ch == '$' {
            if chars.peek() == Some(&'{') {
                // Extended syntax: ${name:constraint}
                chars.next(); // consume '{'
                let mut name = String::new();
                let mut constraint_str = String::new();
                let mut in_constraint = false;
                let mut closed = false;
                while let Some(&c) = chars.peek() {
                    if c == '}' {
                        closed = true;
                        chars.next();
                        break;
                    } else if c == ':' && !in_constraint {
                        in_constraint = true;
                        chars.next();
                    } else if in_constraint {
                        constraint_str.push(c);
                        chars.next();
                    } else {
                        name.push(c);
                        chars.next();
                    }
                }
                if !closed {
                    bail!("Unclosed placeholder");
                }
                if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    bail!("Invalid placeholder name");
                }
                let stand_in = format!("{PLACEHOLDER_PREFIX}{name}");
                let constraints = parse_constraint(&constraint_str)?;
                placeholders
                    .entry(stand_in.clone())
                    .or_insert_with(|| Placeholder { ident: Var(name), constraints: Vec::new() })
                    .constraints
                    .extend(constraints);
                result.push_str(&stand_in);
            } else {
                // Simple syntax: $name
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() {
                    // Lone '$' — pass through.
                    result.push('$');
                    continue;
                }
                let stand_in = format!("{PLACEHOLDER_PREFIX}{name}");
                placeholders
                    .entry(stand_in.clone())
                    .or_insert(Placeholder { ident: Var(name), constraints: Vec::new() });
                result.push_str(&stand_in);
            }
        } else {
            result.push(ch);
        }
    }

    Ok((result, placeholders))
}

/// Parse a constraint string like `kind(literal)`.
fn parse_constraint(s: &str) -> Result<Vec<Constraint>, SsrError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut constraints = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.starts_with("not(") && part.ends_with(')') {
            let inner = &part[4..part.len() - 1];
            let inner_constraints = parse_constraint(inner)?;
            for c in inner_constraints {
                constraints.push(Constraint::Not(Box::new(c)));
            }
        } else if part.starts_with("kind(") && part.ends_with(')') {
            let kind_str = &part[5..part.len() - 1];
            let kind = match kind_str {
                "literal" => NodeKind::Literal,
                _ => bail!("Unknown node kind: {}", kind_str),
            };
            constraints.push(Constraint::Kind(kind));
        } else if !part.is_empty() {
            bail!("Unknown constraint syntax: {}", part);
        }
    }
    Ok(constraints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_rule() {
        let rule = parse_rule("foo($a) ==>> bar($a)").unwrap();
        // Should have one placeholder
        assert_eq!(rule.placeholders_by_stand_in.len(), 1);
    }

    #[test]
    fn parse_error_missing_separator() {
        let result = parse_rule("foo bar");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_undefined_placeholder() {
        let result = parse_rule("foo($a) ==>> bar($b)");
        assert!(result.is_err());
    }

    #[test]
    fn parse_constraint_syntax() {
        let rule = parse_rule("${x:kind(literal)} ==>> $x").unwrap();
        let ph = rule.placeholders_by_stand_in.values().next().unwrap();
        assert!(!ph.constraints.is_empty());
    }
}

#[cfg(test)]
mod rule_safety_tests {
    use super::*;
    #[test]
    fn literals_and_comments_are_not_rule_syntax() {
        let (search, replace) = split_rule("\"$a ==>> literal\" ==>> \"$b\"").unwrap();
        assert_eq!(search, "\"$a ==>> literal\"");
        assert_eq!(replace, "\"$b\"");
        assert!(parse_rule("\"$a ==>> literal\" ==>> \"$b\"").is_ok());
        assert!(
            parse_rule("foo($a /* $ignored ==>> */) ==>> $a").is_ok(),
            "{:?}",
            parse_rule("foo($a /* $ignored ==>> */) ==>> $a")
        );
        assert!(parse_rule("foo($a) ==>> $missing").is_err());
        assert!(parse_rule("foo(${a:kind(literal)) ==>> $a").is_err());
        assert!(parse_rule("__ssr_a ==>> $a").is_err());
        assert!(parse_rule("foo($a) ==>> bar($a) ==>> baz($a)").is_err());
    }
}
