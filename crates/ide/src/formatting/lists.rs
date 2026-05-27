//! List formatting — comma-separated items with layout tactics.
//!
//! Provides three-phase list formatting:
//! 1. `itemize_list()` — extract items with pre/post comments
//! 2. `definitive_tactic()` — choose Horizontal/Vertical/Mixed
//! 3. `write_list()` — produce formatted output

use super::rewrite::RewriteResult;
use super::shape::Shape;
use super::FormatOptions;

/// Layout strategy for a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DefinitiveListTactic {
    /// One item per line.
    Vertical,
    /// All items on one line.
    Horizontal,
    /// Pack multiple items per line.
    Mixed,
}

/// When to include trailing separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SeparatorTactic {
    /// Always include trailing separator.
    Always,
    /// Never include trailing separator.
    Never,
    /// Only on vertical layouts.
    Vertical,
}

/// A single list item with optional surrounding comments.
#[derive(Debug, Clone)]
pub(crate) struct ListItem {
    /// Comment before this item.
    pub(crate) pre_comment: Option<String>,
    /// The formatted item text.
    pub(crate) item: RewriteResult,
    /// Comment after this item.
    pub(crate) post_comment: Option<String>,
}

impl ListItem {
    /// Create from a formatted string.
    pub(crate) fn from_str<S: Into<String>>(s: S) -> Self {
        Self { pre_comment: None, item: Ok(s.into()), post_comment: None }
    }

    /// Get the item text (empty string on error).
    pub(crate) fn inner_as_ref(&self) -> &str {
        match &self.item {
            Ok(s) => s.as_str(),
            Err(_) => "",
        }
    }

    /// Is this item multiline?
    pub(crate) fn is_multiline(&self) -> bool {
        self.inner_as_ref().contains('\n')
    }

    /// Does this item have a comment?
    pub(crate) fn has_comment(&self) -> bool {
        self.pre_comment.is_some() || self.post_comment.is_some()
    }
}

/// Configuration for list formatting.
pub(crate) struct ListFormatting<'a> {
    pub(crate) tactic: DefinitiveListTactic,
    pub(crate) separator: &'a str,
    pub(crate) trailing_separator: SeparatorTactic,
    pub(crate) shape: Shape,
    pub(crate) config: &'a FormatOptions,
}

impl<'a> ListFormatting<'a> {
    /// Create with defaults.
    pub(crate) fn new(shape: Shape, config: &'a FormatOptions) -> Self {
        Self {
            tactic: DefinitiveListTactic::Vertical,
            separator: ",",
            trailing_separator: SeparatorTactic::Vertical,
            shape,
            config,
        }
    }

    /// Builder: set tactic.
    pub(crate) fn tactic(mut self, tactic: DefinitiveListTactic) -> Self {
        self.tactic = tactic;
        self
    }

    /// Builder: set separator.
    pub(crate) fn separator(mut self, separator: &'a str) -> Self {
        self.separator = separator;
        self
    }

    /// Builder: set trailing separator.
    pub(crate) fn trailing_separator(mut self, trailing: SeparatorTactic) -> Self {
        self.trailing_separator = trailing;
        self
    }

    /// Should the last item have a trailing separator?
    pub(crate) fn needs_trailing_separator(&self) -> bool {
        match self.trailing_separator {
            SeparatorTactic::Always => true,
            SeparatorTactic::Never => false,
            SeparatorTactic::Vertical => self.tactic == DefinitiveListTactic::Vertical,
        }
    }
}

/// Determine layout tactic based on item widths and available space.
pub(crate) fn definitive_tactic(
    items: &[ListItem],
    width: usize,
    separator_len: usize,
) -> DefinitiveListTactic {
    if items.is_empty() {
        return DefinitiveListTactic::Horizontal;
    }

    // If any item is multiline or has a comment, force vertical.
    if items.iter().any(|i| i.is_multiline() || i.has_comment()) {
        return DefinitiveListTactic::Vertical;
    }

    // Try horizontal: total width of all items + separators.
    let total: usize = items.iter().map(|i| i.inner_as_ref().len()).sum::<usize>()
        + (items.len().saturating_sub(1)) * (separator_len + 1); // sep + space

    if total <= width {
        DefinitiveListTactic::Horizontal
    } else {
        // If there are 3+ items and each individual item fits on one line,
        // pack multiple items per line instead of one-per-line vertical.
        let max_item_width = items.iter().map(|i| i.inner_as_ref().len()).max().unwrap_or(0);
        if items.len() >= 3 && max_item_width + separator_len + 1 < width {
            DefinitiveListTactic::Mixed
        } else {
            DefinitiveListTactic::Vertical
        }
    }
}

/// Format a list of items according to the given formatting config.
pub(crate) fn write_list(items: &[ListItem], formatting: &ListFormatting<'_>) -> RewriteResult {
    if items.is_empty() {
        return Ok(String::new());
    }

    let indent_str = formatting.shape.indent.to_string_inner(formatting.config);

    match formatting.tactic {
        DefinitiveListTactic::Horizontal => {
            let mut result = String::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    result.push_str(formatting.separator);
                    result.push(' ');
                }
                result.push_str(item.inner_as_ref());
            }
            Ok(result)
        }

        DefinitiveListTactic::Vertical => {
            let mut result = String::new();
            let last_idx = items.len() - 1;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    result.push('\n');
                }
                result.push_str(&indent_str);
                // Pre-comment
                if let Some(ref pre) = item.pre_comment {
                    result.push_str(pre);
                    result.push('\n');
                    result.push_str(&indent_str);
                }

                result.push_str(item.inner_as_ref());

                // Separator
                if i < last_idx || formatting.needs_trailing_separator() {
                    result.push_str(formatting.separator);
                }

                // Post-comment
                if let Some(ref post) = item.post_comment {
                    result.push(' ');
                    result.push_str(post);
                }
            }
            Ok(result)
        }

        DefinitiveListTactic::Mixed => {
            // Pack items into lines, respecting width.
            let max_width = formatting.shape.width;
            let sep_width = formatting.separator.len() + 1; // sep + space
            let mut result = String::new();
            let mut line_width = 0usize;
            let last_idx = items.len() - 1;

            for (i, item) in items.iter().enumerate() {
                let item_str = item.inner_as_ref();
                let item_width = item_str.len();

                if i > 0 {
                    if line_width + sep_width + item_width > max_width {
                        // New line
                        result.push_str(formatting.separator);
                        result.push('\n');
                        result.push_str(&indent_str);
                        line_width = indent_str.len();
                    } else {
                        result.push_str(formatting.separator);
                        result.push(' ');
                        line_width += sep_width;
                    }
                }

                result.push_str(item_str);
                line_width += item_width;

                if i == last_idx && formatting.needs_trailing_separator() {
                    result.push_str(formatting.separator);
                }
            }
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::ide_types::FormatOptions;

    fn make_items(strs: &[&str]) -> Vec<ListItem> {
        strs.iter().map(|s| ListItem::from_str(*s)).collect()
    }

    #[test]
    fn tactic_horizontal_fits() {
        let items = make_items(&["A", "B", "C"]);
        assert_eq!(definitive_tactic(&items, 80, 1), DefinitiveListTactic::Horizontal);
    }

    #[test]
    fn tactic_mixed_when_items_fit_individually() {
        // 3 items, each fits individually (16 + 1 + 1 < 20) but total doesn't.
        let items = make_items(&["very_long_name_a", "very_long_name_b", "very_long_name_c"]);
        assert_eq!(definitive_tactic(&items, 20, 1), DefinitiveListTactic::Mixed);
    }

    #[test]
    fn tactic_vertical_when_item_too_wide() {
        // Each item is wider than available width — must go Vertical.
        let items =
            make_items(&["extremely_long_item_name_alpha", "extremely_long_item_name_beta"]);
        assert_eq!(definitive_tactic(&items, 20, 1), DefinitiveListTactic::Vertical);
    }

    #[test]
    fn write_horizontal() {
        let items = make_items(&["A", "B", "C"]);
        let config = FormatOptions::default();
        let shape = super::super::shape::Shape::with_max_width(&config);
        let fmt = ListFormatting::new(shape, &config)
            .tactic(DefinitiveListTactic::Horizontal)
            .separator(",");
        let result = write_list(&items, &fmt).unwrap();
        assert_eq!(result, "A, B, C");
    }

    #[test]
    fn write_vertical_trailing_comma() {
        let items = make_items(&["x : int", "y : bits(32)"]);
        let config = FormatOptions::default();
        let shape = super::super::shape::Shape::with_max_width(&config);
        let fmt = ListFormatting::new(shape, &config)
            .tactic(DefinitiveListTactic::Vertical)
            .separator(",")
            .trailing_separator(SeparatorTactic::Always);
        let result = write_list(&items, &fmt).unwrap();
        assert!(result.contains("x : int,"), "got: {result}");
        assert!(result.contains("y : bits(32),"), "got: {result}");
    }

    #[test]
    fn write_empty() {
        let items: Vec<ListItem> = vec![];
        let config = FormatOptions::default();
        let shape = super::super::shape::Shape::with_max_width(&config);
        let fmt = ListFormatting::new(shape, &config);
        assert_eq!(write_list(&items, &fmt).unwrap(), "");
    }
}
