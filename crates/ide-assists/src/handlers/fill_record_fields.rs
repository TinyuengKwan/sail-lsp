//! `fill_record_fields` assist.
//! When the cursor is on a struct literal with missing fields,
//! adds all missing fields with default values `()`.
//!
//! This is the RA-named entry point — internally delegates to
//! `add_missing_fields` which does the actual work.

use crate::assist_context::{AssistContext, Assists};

/// Fill all missing struct fields with default values.
/// Trigger: cursor inside a struct literal `struct { x = 1 }` where
/// the struct definition has more fields than are initialized.
///
/// Action: inserts `field = ()` for each missing field.
pub(crate) fn fill_record_fields(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    // Delegate to add_missing_fields — same logic, name.
    super::add_missing_fields::add_missing_fields(acc, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    /// This declined on a struct literal missing a field — the case it
    /// exists for — because `add_missing_fields`, which it delegates to,
    /// took the literal's first `IDENT` for the struct's name. In Sail
    /// that is the first *field*.
    #[test]
    fn applies() {
        check_assist(
            fill_record_fields,
            "struct S = { a : int, b : int }\nval f : unit -> S\nfunction f() = struct { $0a = 1 }\n",
            expect![[r#"
                struct S = { a : int, b : int }
                val f : unit -> S
                function f() = struct { a = 1, b = () }
            "#]],
        );
    }
}
