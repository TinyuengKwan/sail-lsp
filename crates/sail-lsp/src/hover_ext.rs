//! Hover post-processing helpers shared between backends.
//!
//! Not gated behind any feature flag so tests can use them.

use lsp_types::{Hover, HoverContents};

/// Append a callable-context footer to a hover result.
#[allow(dead_code)]
pub(crate) fn append_callable_context_to_hover(
    hover: &mut Hover,
    callable_name: &str,
    is_recursive: bool,
    effects: &[&str],
    callees: &[&str],
) {
    if let HoverContents::Markup(markup) = &mut hover.contents {
        if !markup.value.is_empty() {
            markup.value.push_str("\n\n___\n\n");
        }
        let suffix = if is_recursive { " (recursive)" } else { "" };
        stdx::format_to!(markup.value, "_in `{}`{}_", callable_name, suffix);
        if !effects.is_empty() {
            stdx::format_to!(markup.value, "\n\n_effects: {}_", effects.join(", "));
        }
        if !callees.is_empty() {
            const MAX_LISTED_CALLEES: usize = 5;
            let head: Vec<&&str> = callees.iter().take(MAX_LISTED_CALLEES).collect();
            let mut rendered =
                head.iter().map(|c| format!("`{}`", c)).collect::<Vec<_>>().join(", ");
            if callees.len() > MAX_LISTED_CALLEES {
                rendered.push_str(&format!(", … (+{} more)", callees.len() - MAX_LISTED_CALLEES));
            }
            markup.value.push_str(&format!("\n\n_calls: {}_", rendered));
        }
    }
}
