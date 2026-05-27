//! `convert_integer_literal` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn convert_integer_literal(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement convert integer literal
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(convert_integer_literal, "let x = 42\n", 0);
        let _ = labels;
    }
}
