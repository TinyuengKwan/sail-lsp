//! Bitfield layout visualization.
//!
//! Sail-specific feature: renders bitfield definitions as visual
//! tables showing bit ranges, field widths, and masks. Displayed
//! in hover or as a custom command.
//!
//! Example:
//! ```text
//! bitfield Instruction = bits(32) {
//!     opcode : 31 .. 26,
//!     rs     : 25 .. 21,
//!     rt     : 20 .. 16,
//!     imm    : 15 .. 0,
//! }
//! ```
//! →
//! ```text
//! ┌──────┬────┬────┬────────────────┐
//! │opcode│ rs │ rt │      imm       │
//! │31..26│25.2│20.1│    15..0       │
//! │ 6 bit│5bit│5bit│   16 bits      │
//! └──────┴────┴────┴────────────────┘
//! ```

/// A single field within a bitfield.
#[derive(Debug, Clone)]
pub struct BitfieldField {
    pub name: String,
    pub hi: u32,
    pub lo: u32,
}

impl BitfieldField {
    pub fn width(&self) -> u32 {
        self.hi.saturating_sub(self.lo) + 1
    }
}

/// Parse bitfield fields from a signature text.
///
/// Input: `"bitfield Foo = bits(32) { opcode : 31 .. 26, rs : 25 .. 21 }"`
/// Returns: Vec of BitfieldField
pub fn parse_bitfield_fields(signature: &str) -> Vec<BitfieldField> {
    let Some(brace_start) = signature.find('{') else {
        return Vec::new();
    };
    let Some(brace_end) = signature.rfind('}') else {
        return Vec::new();
    };
    let inner = &signature[brace_start + 1..brace_end];

    // Strip line comments before splitting by comma.
    // Bitfield definitions often have `// ...` comments between fields.
    // Without stripping, comments bleed into field names.
    let stripped: String = inner
        .lines()
        .map(|line| if let Some(pos) = line.find("//") { &line[..pos] } else { line })
        .collect::<Vec<_>>()
        .join(" ");

    let mut fields = Vec::new();
    for part in stripped.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some(colon_pos) = part.find(':') else {
            continue;
        };
        let name = part[..colon_pos].trim().to_string();
        if name.is_empty() {
            continue;
        }
        let range_str = part[colon_pos + 1..].trim();

        // Parse "hi .. lo" or "hi..lo"
        let nums: Vec<u32> = range_str.split("..").filter_map(|s| s.trim().parse().ok()).collect();

        if nums.len() == 2 {
            let (hi, lo) = if nums[0] >= nums[1] { (nums[0], nums[1]) } else { (nums[1], nums[0]) };
            fields.push(BitfieldField { name, hi, lo });
        } else if nums.len() == 1 {
            // Single bit field
            fields.push(BitfieldField { name, hi: nums[0], lo: nums[0] });
        }
    }
    // Sort by hi bit descending (MSB first)
    fields.sort_by(|a, b| b.hi.cmp(&a.hi));
    fields
}

/// Render bitfield fields as an ASCII box-drawing table for hover display.
///
/// Produces a three-column table (Field, Bits, Width) inside a code block
/// for proper monospace alignment in the editor hover popup.
pub fn render_bitfield_layout(fields: &[BitfieldField]) -> String {
    if fields.is_empty() {
        return String::new();
    }

    let total_width: u32 = fields.iter().map(|f| f.width()).sum();

    // Compute column widths
    let field_col = fields.iter().map(|f| f.name.len()).max().unwrap_or(5).max(5);
    let bits_col = fields
        .iter()
        .map(|f| {
            if f.hi == f.lo {
                format!("{}", f.hi).len()
            } else {
                format!("{}..{}", f.hi, f.lo).len()
            }
        })
        .max()
        .unwrap_or(4)
        .max(4);
    let width_col = fields.iter().map(|f| format!("{}", f.width()).len()).max().unwrap_or(5).max(5);

    let mut lines = Vec::new();
    lines.push("```".to_string());
    lines.push(format!("Total: {} bits", total_width));
    lines.push(String::new());

    // Top border
    lines.push(format!(
        "┌{f}┬{b}┬{w}┐",
        f = "─".repeat(field_col + 2),
        b = "─".repeat(bits_col + 2),
        w = "─".repeat(width_col + 2),
    ));
    // Header
    lines.push(format!(
        "│ {:<fw$} │ {:<bw$} │ {:<ww$} │",
        "Field",
        "Bits",
        "Width",
        fw = field_col,
        bw = bits_col,
        ww = width_col,
    ));
    // Header separator
    lines.push(format!(
        "├{f}┼{b}┼{w}┤",
        f = "─".repeat(field_col + 2),
        b = "─".repeat(bits_col + 2),
        w = "─".repeat(width_col + 2),
    ));
    // Data rows
    for field in fields {
        let bits = if field.hi == field.lo {
            format!("{}", field.hi)
        } else {
            format!("{}..{}", field.hi, field.lo)
        };
        lines.push(format!(
            "│ {:<fw$} │ {:<bw$} │ {:<ww$} │",
            field.name,
            bits,
            field.width(),
            fw = field_col,
            bw = bits_col,
            ww = width_col,
        ));
    }
    // Bottom border
    lines.push(format!(
        "└{f}┴{b}┴{w}┘",
        f = "─".repeat(field_col + 2),
        b = "─".repeat(bits_col + 2),
        w = "─".repeat(width_col + 2),
    ));
    lines.push("```".to_string());

    lines.join("\n")
}

/// Get bitfield layout for a named bitfield from the ItemTree.
pub fn bitfield_layout_for_name(file: &dyn ide_db::FileDb, name: &str) -> Option<String> {
    let tree = file.item_tree()?;
    let id = tree.top_level_items().iter().find(|id| {
        id.item_kind(&tree) == hir_def::item_tree::ItemKind::Bitfield
            && id.name(&tree).as_str() == name
    })?;

    let fields = parse_bitfield_fields(id.signature(&tree));
    if fields.is_empty() {
        return None;
    }
    Some(render_bitfield_layout(&fields))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_bitfield() {
        let fields = parse_bitfield_fields(
            "bitfield Instr = bits(32) { opcode : 31 .. 26, rs : 25 .. 21, imm : 15 .. 0 }",
        );
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[0].name, "opcode");
        assert_eq!(fields[0].width(), 6);
        assert_eq!(fields[1].name, "rs");
        assert_eq!(fields[1].width(), 5);
        assert_eq!(fields[2].name, "imm");
        assert_eq!(fields[2].width(), 16);
    }

    #[test]
    fn render_layout() {
        let fields = parse_bitfield_fields("bitfield X = bits(16) { hi : 15 .. 8, lo : 7 .. 0 }");
        let layout = render_bitfield_layout(&fields);
        // ASCII box-drawing table with Field/Bits/Width columns
        assert!(layout.contains("┌"), "should have top border");
        assert!(layout.contains("Field"), "should have Field header");
        assert!(layout.contains("Bits"), "should have Bits header");
        assert!(layout.contains("Width"), "should have Width header");
        assert!(layout.contains("│ hi"), "should contain field 'hi'");
        assert!(layout.contains("│ lo"), "should contain field 'lo'");
        assert!(layout.contains("15..8"), "should contain range 15..8");
        assert!(layout.contains("7..0"), "should contain range 7..0");
        assert!(layout.contains("Total: 16 bits"), "should show total");
        assert!(layout.contains("```"), "should be in code block");
    }

    #[test]
    fn single_bit_field() {
        let fields =
            parse_bitfield_fields("bitfield Flags = bits(8) { carry : 0 .. 0, zero : 1 .. 1 }");
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].width(), 1);
        assert_eq!(fields[1].width(), 1);
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn render_layout_alignment() {
        // Test with varying field name and range lengths to verify
        // all rows align correctly (dynamic column widths).
        let fields = vec![
            BitfieldField { name: "rvfi_order".to_string(), hi: 63, lo: 0 },
            BitfieldField { name: "rvfi_insn".to_string(), hi: 127, lo: 64 },
            BitfieldField { name: "rvfi_trap".to_string(), hi: 135, lo: 128 },
            BitfieldField { name: "padding".to_string(), hi: 191, lo: 176 },
        ];
        let layout = render_bitfield_layout(&fields);
        // All lines between ``` markers should have the same byte length
        // (monospace alignment). Check by comparing border line lengths.
        let content_lines: Vec<&str> = layout
            .lines()
            .filter(|l| {
                l.starts_with('│') || l.starts_with('┌') || l.starts_with('├') || l.starts_with('└')
            })
            .collect();
        assert!(!content_lines.is_empty());
        let first_len = content_lines[0].chars().count();
        for (i, line) in content_lines.iter().enumerate() {
            assert_eq!(
                line.chars().count(),
                first_len,
                "line {} has different char width: {:?}\nvs first: {:?}",
                i,
                line,
                content_lines[0]
            );
        }
        // Verify content
        assert!(layout.contains("rvfi_order"), "should contain field name");
        assert!(layout.contains("191..176"), "should contain bit range");
        assert!(layout.contains("Total: 152 bits"), "should show total: {layout}");
        eprintln!("{layout}");
    }

    #[test]
    fn comments_stripped_from_fields() {
        let fields = parse_bitfield_fields(
            "bitfield X : bits(128) = {\n\
             // This is a long comment about the pc field\n\
             // that spans multiple lines.\n\
             pc_rdata  : 63 .. 0,\n\
             pc_wdata  : 127 ..  64,\n\
             }",
        );
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "pc_wdata");
        assert_eq!(fields[1].name, "pc_rdata");
        // No comment text in field names
        assert!(!fields[0].name.contains("//"));
        assert!(!fields[1].name.contains("comment"));
    }
}
