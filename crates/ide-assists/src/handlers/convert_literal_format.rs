//! `convert_literal_format` assist — `0x0F` ↔ `0b00001111`.
//!
//! It sat in the crate root as `convert_literal_format_edits`, returning
//! `Vec<(String, Vec<IdeTextEdit>)>` — a shape no caller consumed and `all()`
//! could not register, so it reached no editor. That is the same defect the
//! forty-nine unreachable assists were: implemented, and offered to nobody.
//!
//! A Sail `0x`/`0b` literal is a bitvector whose width is its digit count, and
//! a decimal is an `int`. So only hex and binary convert, digit for digit:
//! `0x0F` is `bits(8)` and so is `0b00001111`. This used to offer `0b1111`
//! (`bits(4)`) for `0x0F` and decimal for both, each a type change, and it
//! parsed through `i64`, so a 64-bit literal offered nothing.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn convert_literal_format(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let range = ctx.selection_trimmed();
    let text =
        ctx.source_text().get(base_db::range_start(range)..base_db::range_end(range))?.trim();

    let (name, new_text) = if let Some(hex) = text.strip_prefix("0x") {
        let digits: Vec<u32> =
            hex.chars().filter(|c| *c != '_').map(|c| c.to_digit(16)).collect::<Option<_>>()?;
        let bits: String = digits.iter().map(|d| format!("{d:04b}")).collect();
        ("binary", format!("0b{bits}"))
    } else {
        let bin = text.strip_prefix("0b")?;
        let bits: Vec<u32> =
            bin.chars().filter(|c| *c != '_').map(|c| c.to_digit(2)).collect::<Option<_>>()?;
        // A width that is not a whole number of hex digits has no hex form.
        if bits.is_empty() || !bits.len().is_multiple_of(4) {
            return None;
        }
        let hex: String = bits
            .chunks(4)
            .map(|nibble| nibble.iter().fold(0, |acc, b| acc * 2 + b))
            .map(|d| char::from_digit(d, 16).unwrap().to_ascii_uppercase())
            .collect();
        ("hex", format!("0x{hex}"))
    };
    if new_text.len() <= 2 {
        return None;
    }
    acc.add_with_edits(
        AssistId("convert_literal_format", AssistKind::RefactorRewrite),
        format!("Convert to {name}"),
        range,
        vec![TextEdit { range, new_text }],
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use crate::tests::check_assist;
    use expect_test::expect;

    /// The width is the digit count, so it is kept: `0x0F` is `bits(8)`.
    #[test]
    fn hex_becomes_binary_of_the_same_width() {
        check_assist(
            super::convert_literal_format,
            "function f() -> bits(8) = $00x0F$0",
            expect!["function f() -> bits(8) = 0b00001111"],
        );
    }

    #[test]
    fn binary_becomes_hex() {
        check_assist(
            super::convert_literal_format,
            "function f() -> bits(8) = $00b1010_0101$0",
            expect!["function f() -> bits(8) = 0xA5"],
        );
    }

    /// Past 63 bits, which the `i64` parse could not hold.
    #[test]
    fn a_wide_literal_converts() {
        check_assist(
            super::convert_literal_format,
            "function f() -> bits(68) = $00x8000000000000000F$0",
            expect![[
                r#"function f() -> bits(68) = 0b10000000000000000000000000000000000000000000000000000000000000001111"#
            ]],
        );
    }

    /// No conversion changes a literal's type: a decimal is an `int`, and
    /// three bits are no whole number of hex digits.
    #[test]
    fn nothing_changes_the_type() {
        for source in ["function f() -> int = $0255$0", "function f() -> bits(3) = $00b101$0"] {
            crate::tests::check_assist_not_applicable(super::convert_literal_format, source);
        }
    }
}
