//! `add_turbo_fish` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn add_turbo_fish(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement add turbo fish
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(add_turbo_fish, "val x : int\n", 0);
        let _ = labels;
    }
}
