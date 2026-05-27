//! `reorder_fields` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn reorder_fields(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement reorder fields
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(reorder_fields, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
