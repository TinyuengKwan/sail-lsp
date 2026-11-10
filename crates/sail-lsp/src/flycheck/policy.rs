//! Compiler presentation is a projection of retained, unmodified findings.
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CompilerDiagnosticPolicy {
    ignore: HashSet<String>,
    hints: HashSet<String>,
    information: HashSet<String>,
}

impl From<&crate::config::FlycheckConfig> for CompilerDiagnosticPolicy {
    fn from(config: &crate::config::FlycheckConfig) -> Self {
        Self {
            ignore: config.ignore.iter().cloned().collect(),
            hints: config.warnings_as_hint.iter().cloned().collect(),
            information: config.warnings_as_info.iter().cloned().collect(),
        }
    }
}

impl CompilerDiagnosticPolicy {
    fn matches(selectors: &HashSet<String>, diagnostic: &Diagnostic) -> bool {
        if selectors.contains("*") {
            return true;
        }
        let severity = match diagnostic.severity {
            Some(DiagnosticSeverity::ERROR) => "severity:error",
            Some(DiagnosticSeverity::WARNING) => "severity:warning",
            Some(DiagnosticSeverity::INFORMATION) => "severity:information",
            Some(DiagnosticSeverity::HINT) => "severity:hint",
            _ => "",
        };
        if !severity.is_empty() && selectors.contains(severity) {
            return true;
        }
        match &diagnostic.code {
            Some(NumberOrString::String(code)) => selectors.contains(code),
            Some(NumberOrString::Number(code)) => selectors.contains(&code.to_string()),
            None => false,
        }
    }

    pub fn is_ignored(&self, diagnostic: &Diagnostic) -> bool {
        Self::matches(&self.ignore, diagnostic)
    }

    pub fn project(&self, diagnostic: &Diagnostic) -> Option<Diagnostic> {
        if self.is_ignored(diagnostic) {
            return None;
        }
        let mut projected = diagnostic.clone();
        if Self::matches(&self.hints, diagnostic) {
            projected.severity = Some(DiagnosticSeverity::HINT);
        } else if Self::matches(&self.information, diagnostic) {
            projected.severity = Some(DiagnosticSeverity::INFORMATION);
        }
        Some(projected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn code_and_severity_policies_preserve_original_findings_and_fix_metadata() {
        let original = Diagnostic {
            code: Some(NumberOrString::String("width".into())),
            severity: Some(DiagnosticSeverity::ERROR),
            data: Some(serde_json::json!({"sourceHash":"unchanged","fix":"safe edit"})),
            ..Default::default()
        };
        let mut config = crate::config::FlycheckConfig::default();
        config.warnings_as_hint.push("width".into());
        let projected = CompilerDiagnosticPolicy::from(&config).project(&original).unwrap();
        assert_eq!(projected.severity, Some(DiagnosticSeverity::HINT));
        assert_eq!(projected.data, original.data);
        assert_eq!(original.severity, Some(DiagnosticSeverity::ERROR));
        config.ignore.push("severity:error".into());
        assert!(CompilerDiagnosticPolicy::from(&config).project(&original).is_none());
        assert!(config.same_invocation(&crate::config::FlycheckConfig::default()));
        config.extra_args.push("--strict-var".into());
        assert!(!config.same_invocation(&crate::config::FlycheckConfig::default()));
    }
}
