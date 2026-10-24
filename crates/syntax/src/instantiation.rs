//! Reference roles in an outcome instantiation. The parser represents both
//! kinds of replacement RHS as type syntax; their LHS determines the meaning.
use crate::{SyntaxKind as SK, SyntaxNode, SyntaxToken, TextRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstantiationReference {
    Outcome,
    Type,
    Callable,
}

/// Classify an identifier position. Substitution keys belong to the outcome's
/// parameter/member scope, not to the surrounding global scope.
pub fn instantiation_reference(node: &SyntaxNode, offset: usize) -> Option<InstantiationReference> {
    if node.kind() != SK::INSTANTIATION_DEF {
        return None;
    }
    if crate::definition_name_span(node).is_some_and(|s| s.start <= offset && offset < s.end) {
        return Some(InstantiationReference::Outcome);
    }
    substitutions(node).into_iter().find_map(|(key, value)| {
        value.contains((offset as u32).into()).then_some(if key.kind() == SK::TY_VAR {
            InstantiationReference::Type
        } else {
            InstantiationReference::Callable
        })
    })
}

/// Complete substitutions. Nested commas and comments retain their syntax
/// meaning; comments become whitespace without changing substitution boundaries.
pub fn instantiation_substitutions(node: &SyntaxNode) -> Vec<(String, String)> {
    substitutions(node)
        .into_iter()
        .filter_map(|(key, range)| {
            let value = node
                .descendants_with_tokens()
                .filter_map(|t| t.into_token())
                .filter(|t| range.contains_range(t.text_range()))
                .map(|t| {
                    if matches!(t.kind(), SK::LINE_COMMENT | SK::BLOCK_COMMENT) {
                        " ".to_owned()
                    } else {
                        t.text().to_owned()
                    }
                })
                .collect::<String>();
            (!value.trim().is_empty()).then(|| (key.text().to_owned(), value.trim().to_owned()))
        })
        .collect()
}

fn substitutions(node: &SyntaxNode) -> Vec<(SyntaxToken, TextRange)> {
    if node.kind() != SK::INSTANTIATION_DEF {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut with = false;
    let mut key = None;
    let mut rhs_start = None;
    let mut depth = 0usize;
    for token in node
        .descendants_with_tokens()
        .filter_map(|t| t.into_token())
        .filter(|t| !t.kind().is_trivia())
    {
        if token.kind() == SK::KW_WITH && !with {
            with = true;
            continue;
        }
        if !with {
            continue;
        }
        if rhs_start.is_none() {
            match token.kind() {
                SK::TY_VAR | SK::IDENT => key = Some(token),
                SK::EQ => rhs_start = Some(token.text_range().end()),
                _ => {}
            }
        } else {
            match token.text() {
                "(" | "[" | "{" => depth += 1,
                ")" | "]" | "}" => depth = depth.saturating_sub(1),
                "," if depth == 0 => {
                    if let Some((key, start)) = key.take().zip(rhs_start.take()) {
                        result.push((key, TextRange::new(start, token.text_range().start())));
                    }
                }
                _ => {}
            }
        }
    }
    if let Some((key, start)) = key.zip(rhs_start) {
        result.push((key, TextRange::new(start, node.text_range().end())));
    }
    result
}
