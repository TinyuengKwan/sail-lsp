//! `generate_enum_variant` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn generate_enum_variant(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement generate enum variant
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(generate_enum_variant, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
