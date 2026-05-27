//! Shape and Indent — formatting space constraints.
//!
//! Shape tracks the remaining available width on the current line
//! plus the indentation context. Indent separates block-level
//! indentation (tabs) from visual alignment (spaces).

use std::borrow::Cow;

use super::FormatOptions;

/// Two-component indentation: block indent (multiples of tab_spaces)
/// plus alignment (extra spaces for visual continuation).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Indent {
    /// Block-level indentation in spaces (multiple of tab_spaces).
    pub(crate) block_indent: usize,
    /// Visual alignment spaces (for continuation lines).
    pub(crate) alignment: usize,
}

impl Indent {
    pub(crate) fn new(block_indent: usize, alignment: usize) -> Self {
        Self { block_indent, alignment }
    }

    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn block_only(&self) -> Self {
        Self { block_indent: self.block_indent, alignment: 0 }
    }

    /// Total width in columns.
    pub(crate) fn width(&self) -> usize {
        self.block_indent + self.alignment
    }

    /// Increase block indent by one level.
    pub(crate) fn block_indent(mut self, config: &FormatOptions) -> Self {
        self.block_indent += config.tab_spaces();
        self
    }

    /// Decrease block indent by one level.
    pub(crate) fn block_unindent(mut self, config: &FormatOptions) -> Self {
        self.block_indent = self.block_indent.saturating_sub(config.tab_spaces());
        self
    }

    /// Render as a whitespace string.
    pub(crate) fn to_string_inner(&self, config: &FormatOptions) -> Cow<'static, str> {
        let mut s = String::with_capacity(self.width());
        if config.hard_tabs() {
            let tabs = self.block_indent / config.tab_spaces();
            let spaces = self.block_indent % config.tab_spaces();
            s.extend(std::iter::repeat('\t').take(tabs));
            s.extend(std::iter::repeat(' ').take(spaces));
        } else {
            s.extend(std::iter::repeat(' ').take(self.block_indent));
        }
        s.extend(std::iter::repeat(' ').take(self.alignment));
        Cow::Owned(s)
    }

    /// Render with a leading newline.
    pub(crate) fn to_string_with_newline(&self, config: &FormatOptions) -> Cow<'static, str> {
        let mut s = String::from("\n");
        s.push_str(&self.to_string_inner(config));
        Cow::Owned(s)
    }
}

impl std::ops::Add for Indent {
    type Output = Indent;
    fn add(self, rhs: Indent) -> Indent {
        Indent::new(self.block_indent + rhs.block_indent, self.alignment + rhs.alignment)
    }
}

impl std::ops::Sub for Indent {
    type Output = Indent;
    fn sub(self, rhs: Indent) -> Indent {
        Indent::new(
            self.block_indent.saturating_sub(rhs.block_indent),
            self.alignment.saturating_sub(rhs.alignment),
        )
    }
}

/// Tracks available space for formatting an element.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Shape {
    /// Maximum characters available on the current line.
    pub(crate) width: usize,
    /// Current indentation state.
    pub(crate) indent: Indent,
    /// Characters already consumed (indent + placed text).
    pub(crate) offset: usize,
}

impl Shape {
    /// Create a shape for a new indented block.
    pub(crate) fn indented(indent: Indent, config: &FormatOptions) -> Self {
        Self {
            width: config.max_width().saturating_sub(indent.width()),
            indent,
            offset: indent.alignment,
        }
    }

    /// Shape with maximum available width (for root level).
    pub(crate) fn with_max_width(config: &FormatOptions) -> Self {
        Self { width: config.max_width(), indent: Indent::empty(), offset: 0 }
    }

    /// Visual indent: increase offset for continuation lines.
    pub(crate) fn visual_indent(&self, extra: usize) -> Self {
        let alignment = self.offset + extra;
        let indent = Indent::new(self.indent.block_indent, alignment);
        let width = self.width.saturating_sub(extra);
        Self { width, indent, offset: alignment }
    }

    /// Block indent: increase block component by one level.
    pub(crate) fn block_indent(&self, config: &FormatOptions) -> Self {
        let indent = self.indent.block_indent(config);
        Self::indented(indent, config)
    }

    /// Reset to block-only (no alignment), keeping block_indent.
    pub(crate) fn block(&self) -> Self {
        Self { width: self.width, indent: self.indent.block_only(), offset: 0 }
    }

    /// Reduce available width.
    pub(crate) fn sub_width(&self, delta: usize) -> Option<Self> {
        if delta > self.width {
            None
        } else {
            Some(Self { width: self.width - delta, ..*self })
        }
    }

    /// Account for text already placed on this line.
    pub(crate) fn offset_left(&self, delta: usize) -> Option<Self> {
        self.sub_width(delta).map(|s| Self { offset: s.offset + delta, ..s })
    }

    /// Characters consumed so far on this line.
    pub(crate) fn used_width(&self) -> usize {
        self.indent.width() + self.offset
    }

    /// Remaining budget after used_width.
    pub(crate) fn budget(&self, used: usize) -> usize {
        self.width.saturating_sub(used)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> FormatOptions {
        FormatOptions::default()
    }

    #[test]
    fn indent_width() {
        let i = Indent::new(4, 2);
        assert_eq!(i.width(), 6);
    }

    #[test]
    fn indent_block_indent() {
        let config = default_config();
        let i = Indent::empty().block_indent(&config);
        assert_eq!(i.block_indent, config.tab_spaces());
    }

    #[test]
    fn shape_indented() {
        let config = default_config();
        let indent = Indent::new(4, 0);
        let s = Shape::indented(indent, &config);
        assert_eq!(s.width, config.max_width() - 4);
        assert_eq!(s.indent.block_indent, 4);
    }

    #[test]
    fn shape_visual_indent() {
        let config = default_config();
        let s = Shape::with_max_width(&config);
        let vis = s.visual_indent(10);
        assert_eq!(vis.offset, 10);
        assert_eq!(vis.indent.alignment, 10);
    }

    #[test]
    fn shape_sub_width() {
        let config = default_config();
        let s = Shape::with_max_width(&config);
        let narrower = s.sub_width(20).unwrap();
        assert_eq!(narrower.width, s.width - 20);
    }
}
