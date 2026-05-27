//! `wrap_return_type` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn wrap_return_type(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement wrap return type
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(wrap_return_type, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
