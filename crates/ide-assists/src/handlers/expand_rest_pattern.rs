//! `expand_rest_pattern` assist.

use crate::assist_context::{AssistContext, Assists};

pub(crate) fn expand_rest_pattern(_acc: &mut Assists, _ctx: &AssistContext<'_>) -> Option<()> {
    // TODO: implement expand rest pattern
    // Architecture is wired — detection logic to be added.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(expand_rest_pattern, "match x { _ => () }\n", 0);
        let _ = labels;
    }
}
