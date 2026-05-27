//! `replace_string_with_char` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn replace_string_with_char(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement replace string with char
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(replace_string_with_char, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
