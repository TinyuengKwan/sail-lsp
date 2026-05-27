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

    #[test]
    fn smoke() {
        let labels = check_assist(fill_record_fields, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
