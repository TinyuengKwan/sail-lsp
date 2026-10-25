//! Completion is now in the `ide-completion` crate.
//! This module re-exports everything for backward compatibility.
pub use ide_completion::*;

#[cfg(test)]
mod tests {
    use super::CompletionConfig;

    #[test]
    fn disabling_templates_keeps_plain_keywords() {
        let (analysis, file, position) = crate::fixture::position("//- /main.sail\nfun$0\n");
        let enabled = analysis.completions(file, position, &Default::default()).unwrap();
        assert!(enabled.iter().any(|item| item.label == "function"
            && item.insert_text.as_deref().is_some_and(|text| text.contains('$'))));
        let config = CompletionConfig { enable_snippets: false, ..Default::default() };
        let disabled = analysis.completions(file, position, &config).unwrap();
        let function = disabled.iter().find(|item| item.label == "function").unwrap();
        assert_eq!(function.insert_text.as_deref(), Some("function"));
        assert!(!disabled
            .iter()
            .any(|item| item.kind == ide_db::defs::CompletionItemKind::Snippet));
    }

    #[test]
    fn postfix_setting_controls_provider() {
        let (analysis, file, position) =
            crate::fixture::position("//- /main.sail\nfunction f(x : bool) -> bool = x.i$0\n");
        let enabled = analysis.completions(file, position, &Default::default()).unwrap();
        assert!(enabled.iter().any(|item| item.label == ".if"), "{enabled:?}");
        let item = enabled.iter().find(|item| item.label == ".if").unwrap();
        assert_eq!(item.insert_text.as_deref(), Some("if x then {\n\t$0\n}"));
        let config = CompletionConfig { enable_postfix_completions: false, ..Default::default() };
        let disabled = analysis.completions(file, position, &config).unwrap();
        assert!(!disabled.iter().any(|item| item.label == ".if"));
    }
}
