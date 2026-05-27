//! SSR rule parsing.
//! Parses the `search ==>> replace` syntax into structured rules.
//! Supports placeholder syntax: `$name` or `${name:constraint}`.

use rustc_hash::FxHashMap;

use syntax::SyntaxNode;

use crate::errors::SsrError;
use crate::fragments;

/// A parsed SSR rule with pattern and optional template as syntax trees.
#[derive(Debug, Clone)]
pub(crate) struct ParsedRule {
    /// Placeholder metadata, keyed by stand-in name (e.g., `__ssr_0_a`).
    pub(crate) placeholders_by_stand_in: FxHashMap<String, Placeholder>,
    /// The parsed pattern as a syntax tree.
    pub(crate) pattern: SyntaxNode,
    /// The parsed template (replacement) as a syntax tree, if present.
    pub(crate) template: Option<SyntaxNode>,
}

/// A placeholder in an SSR pattern.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Placeholder {
    /// The user-visible variable (e.g., `Var("a")` for `$a`).
    pub(crate) ident: Var,
    /// The stand-in identifier used in the parsed tree (e.g., `__ssr_0_a`).
    pub(crate) stand_in_name: String,
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

/// Parse an SSR rule string into one or more `ParsedRule`s.
///
/// Returns all successful parses (expr, pat, typ, item).
pub(crate) fn parse_rules(input: &str) -> Result<Vec<ParsedRule>, SsrError> {
    let parts: Vec<&str> = input.splitn(2, "==>>").collect();
    if parts.len() != 2 {
        bail!("Expected `search ==>> replace` syntax");
    }
    let search_str = parts[0].trim();
    let replace_str = parts[1].trim();

    // Tokenize and replace placeholders with stand-in identifiers.
    let (search_text, placeholders) = replace_placeholders(search_str, 0)?;
    let (replace_text, _) = replace_placeholders(replace_str, 0)?;

    // Validate: all placeholders in replace exist in search.
    validate_replace_placeholders(replace_str, &placeholders)?;

    // Try parsing as each fragment kind.
    let mut rules = Vec::new();
    let fragment_parsers: &[fn(&str) -> Option<SyntaxNode>] =
        &[fragments::expr, fragments::typ, fragments::pat, fragments::item];

    for parse_fn in fragment_parsers {
        if let Some(pattern_node) = parse_fn(&search_text) {
            let template_node = parse_fn(&replace_text);
            rules.push(ParsedRule {
                placeholders_by_stand_in: placeholders.clone(),
                pattern: pattern_node,
                template: template_node,
            });
            break; // Use the first successful parse.
        }
    }

    if rules.is_empty() {
        // Fall back: parse as expression (wrapping in a dummy context).
        if let Some(pattern_node) = fragments::expr(&search_text) {
            let template_node = fragments::expr(&replace_text);
            rules.push(ParsedRule {
                placeholders_by_stand_in: placeholders,
                pattern: pattern_node,
                template: template_node,
            });
        } else {
            bail!("Failed to parse SSR pattern as any Sail fragment");
        }
    }

    Ok(rules)
}

/// Parse only the search side (for `SsrPattern` — no template).
pub(crate) fn parse_pattern_only(input: &str) -> Result<Vec<ParsedRule>, SsrError> {
    let (search_text, placeholders) = replace_placeholders(input.trim(), 0)?;

    let fragment_parsers: &[fn(&str) -> Option<SyntaxNode>] =
        &[fragments::expr, fragments::typ, fragments::pat, fragments::item];

    let mut rules = Vec::new();
    for parse_fn in fragment_parsers {
        if let Some(pattern_node) = parse_fn(&search_text) {
            rules.push(ParsedRule {
                placeholders_by_stand_in: placeholders.clone(),
                pattern: pattern_node,
                template: None,
            });
            break;
        }
    }

    if rules.is_empty() {
        bail!("Failed to parse SSR pattern as any Sail fragment");
    }
    Ok(rules)
}

/// Replace `$name` and `${name:constraint}` with stand-in identifiers.
///
/// Returns the modified text and a map of stand-in → Placeholder.
fn replace_placeholders(
    text: &str,
    _rule_index: usize,
) -> Result<(String, FxHashMap<String, Placeholder>), SsrError> {
    let mut result = String::new();
    let mut placeholders = FxHashMap::default();
    let mut chars = text.chars().peekable();
    let mut counter = 0;

    while let Some(ch) = chars.next() {
        if ch == '$' {
            if chars.peek() == Some(&'{') {
                // Extended syntax: ${name:constraint}
                chars.next(); // consume '{'
                let mut name = String::new();
                let mut constraint_str = String::new();
                let mut in_constraint = false;
                while let Some(&c) = chars.peek() {
                    if c == '}' {
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
                if name.is_empty() {
                    bail!("Empty placeholder name in ${{}}");
                }
                let stand_in = format!("{PLACEHOLDER_PREFIX}{counter}_{name}");
                counter += 1;
                let constraints = parse_constraint(&constraint_str)?;
                placeholders.insert(
                    stand_in.clone(),
                    Placeholder { ident: Var(name), stand_in_name: stand_in.clone(), constraints },
                );
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
                let stand_in = format!("{PLACEHOLDER_PREFIX}{counter}_{name}");
                counter += 1;
                placeholders.insert(
                    stand_in.clone(),
                    Placeholder {
                        ident: Var(name),
                        stand_in_name: stand_in.clone(),
                        constraints: Vec::new(),
                    },
                );
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

/// Validate that all placeholders referenced in the replacement exist in the search.
fn validate_replace_placeholders(
    replace_text: &str,
    search_placeholders: &FxHashMap<String, Placeholder>,
) -> Result<(), SsrError> {
    let search_names: Vec<&str> =
        search_placeholders.values().map(|p| p.ident.0.as_str()).collect();

    let mut chars = replace_text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            let mut name = String::new();
            if chars.peek() == Some(&'{') {
                chars.next();
                while let Some(&c) = chars.peek() {
                    if c == '}' || c == ':' {
                        break;
                    }
                    name.push(c);
                    chars.next();
                }
                // consume rest until '}'
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c == '}' {
                        break;
                    }
                }
            } else {
                while let Some(&c) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
            }
            if !name.is_empty() && !search_names.contains(&name.as_str()) {
                bail!("Placeholder ${} in replace not found in search", name);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_rule() {
        let rules = parse_rules("foo($a) ==>> bar($a)").unwrap();
        assert!(!rules.is_empty());
        let rule = &rules[0];
        assert!(rule.template.is_some());
        // Should have one placeholder
        assert_eq!(rule.placeholders_by_stand_in.len(), 1);
    }

    #[test]
    fn parse_error_missing_separator() {
        let result = parse_rules("foo bar");
        assert!(result.is_err());
    }

    #[test]
    fn parse_error_undefined_placeholder() {
        let result = parse_rules("foo($a) ==>> bar($b)");
        assert!(result.is_err());
    }

    #[test]
    fn parse_constraint_syntax() {
        let rules = parse_rules("${x:kind(literal)} ==>> $x").unwrap();
        assert!(!rules.is_empty());
        let rule = &rules[0];
        let ph = rule.placeholders_by_stand_in.values().next().unwrap();
        assert!(!ph.constraints.is_empty());
    }
}
