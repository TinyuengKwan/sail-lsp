//! `merge_imports` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn merge_imports(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement merge imports
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(merge_imports, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
