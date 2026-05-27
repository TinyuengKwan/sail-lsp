//! `destructure_struct_binding` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn destructure_struct_binding(
    _acc: &mut Assists,
    _ctx: &AssistContext<'_>,
) -> Option<()> {
    // TODO: implement destructure struct binding
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(destructure_struct_binding, "let x = foo\n", 0);
        let _ = labels;
    }
}
