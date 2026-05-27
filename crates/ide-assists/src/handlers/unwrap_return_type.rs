//! `unwrap_return_type` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn unwrap_return_type(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement unwrap return type
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(unwrap_return_type, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
