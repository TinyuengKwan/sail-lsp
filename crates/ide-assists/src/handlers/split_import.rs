//! `split_import` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn split_import(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement split import
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(split_import, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
