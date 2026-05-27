//! `number_representation` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn number_representation(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement number representation
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(number_representation, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
