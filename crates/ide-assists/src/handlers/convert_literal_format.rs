//! `convert_literal_format` assist — `0xFF` ↔ `255` ↔ `0b11111111`.
//!
//! It sat in the crate root as `convert_literal_format_edits`, returning
//! `Vec<(String, Vec<IdeTextEdit>)>` — a shape no caller consumed and `all()`
//! could not register, so it reached no editor. That is the same defect the
//! forty-nine unreachable assists were: implemented, and offered to nobody.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn convert_literal_format(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let range = ctx.selection_trimmed();
    let text =
        ctx.source_text().get(base_db::range_start(range)..base_db::range_end(range))?.trim();

    // The two other spellings of whatever this literal is.
    let (value, alternatives): (i64, [(&str, String); 2]) =
        if let Some(hex) = text.strip_prefix("0x") {
            let value = i64::from_str_radix(hex, 16).ok()?;
            (value, [("decimal", format!("{value}")), ("binary", format!("0b{value:b}"))])
        } else if let Some(bin) = text.strip_prefix("0b") {
            let value = i64::from_str_radix(bin, 2).ok()?;
            (value, [("decimal", format!("{value}")), ("hex", format!("0x{value:X}"))])
        } else {
            let value = text.parse::<i64>().ok()?;
            // `-1` has no unsigned spelling to offer.
            if value < 0 {
                return None;
            }
            (value, [("hex", format!("0x{value:X}")), ("binary", format!("0b{value:b}"))])
        };
    let _ = value;

    for (name, new_text) in alternatives {
        acc.add_with_edits(
            AssistId("convert_literal_format", AssistKind::RefactorRewrite),
            format!("Convert to {name}"),
            range,
            vec![TextEdit { range, new_text }],
        );
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn hex_becomes_decimal() {
        check_assist(
            super::convert_literal_format,
            "function f() -> int = $00xFF$0",
            expect!["function f() -> int = 255"],
        );
    }

    #[test]
    fn decimal_becomes_hex() {
        check_assist(
            super::convert_literal_format,
            "function f() -> int = $0255$0",
            expect!["function f() -> int = 0xFF"],
        );
    }
}
