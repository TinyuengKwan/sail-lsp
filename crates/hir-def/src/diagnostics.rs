//! Core diagnostic data types shared by hir-ty and ide-diagnostics.

use base_db::TextRange;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    WeakWarning,
    Information,
    Hint,
}

/// Structured diagnostic code (kebab-case slug).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    /// Hard errors from the Sail language.
    SailError(&'static str),
    /// Syntax/parse errors.
    SyntaxError,
    /// Warnings and hints.
    SailLint(&'static str, Severity),
}

impl DiagnosticCode {
    /// Default severity for this diagnostic code.
    pub fn default_severity(&self) -> Severity {
        match self {
            DiagnosticCode::SailError(_) => Severity::Error,
            DiagnosticCode::SyntaxError => Severity::Error,
            DiagnosticCode::SailLint(_, severity) => *severity,
        }
    }

    /// Extract the slug string from the variant.
    pub fn as_str(&self) -> &'static str {
        match self {
            DiagnosticCode::SailError(s) => s,
            DiagnosticCode::SyntaxError => "syntax-error",
            DiagnosticCode::SailLint(s, _) => s,
        }
    }

    /// Documentation URL for this diagnostic code.
    pub fn url(&self) -> String {
        format!("https://sail-lsp.dev/diagnostics/{}", self.as_str())
    }

    /// Parse from string, falling back to `SyntaxError`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> DiagnosticCode {
        match s {
            "duplicate-definition" => DiagnosticCode::SailError("duplicate-definition"),
            "mismatched-arg-count" => DiagnosticCode::SailError("mismatched-arg-count"),
            "type-error" => DiagnosticCode::SailError("type-error"),
            "lexical-error" => DiagnosticCode::SailError("lexical-error"),
            "private-access" => DiagnosticCode::SailError("private-access"),
            "effect-mismatch" => DiagnosticCode::SailError("effect-mismatch"),
            "unresolved-include" => DiagnosticCode::SailError("unresolved-include"),
            "undeclared-effect" => DiagnosticCode::SailError("undeclared-effect"),
            "return-outside-function" => DiagnosticCode::SailError("return-outside-function"),
            "invalid-vector-concat" => DiagnosticCode::SailError("invalid-vector-concat"),
            "invalid-list-pattern" => DiagnosticCode::SailError("invalid-list-pattern"),
            "invalid-string-pattern" => DiagnosticCode::SailError("invalid-string-pattern"),
            "impossible-constraint" => DiagnosticCode::SailError("impossible-constraint"),
            "unresolved-quants" => DiagnosticCode::SailError("unresolved-quants"),
            "invalid-slice-assign" => DiagnosticCode::SailError("invalid-slice-assign"),
            "undeclared-mapping-type" => DiagnosticCode::SailError("undeclared-mapping-type"),
            "circular-include" => DiagnosticCode::SailError("circular-include"),
            "unresolved-ident" => DiagnosticCode::SailError("unresolved-ident"),
            "unresolved-field" => DiagnosticCode::SailError("unresolved-field"),
            "expected-function" => DiagnosticCode::SailError("expected-function"),
            "missing-fields" => DiagnosticCode::SailError("missing-fields"),
            "effect-violation" => DiagnosticCode::SailError("effect-violation"),
            "syntax-error" => DiagnosticCode::SyntaxError,
            "unused-variable" => DiagnosticCode::SailLint("unused-variable", Severity::Warning),
            "deprecated-effect-annotation" => {
                DiagnosticCode::SailLint("deprecated-effect-annotation", Severity::Warning)
            }
            "missing-extern-purity" => {
                DiagnosticCode::SailLint("missing-extern-purity", Severity::Warning)
            }
            "unmodified-mutable-variable" => {
                DiagnosticCode::SailLint("unmodified-mutable-variable", Severity::Warning)
            }
            "option-register-no-default" => {
                DiagnosticCode::SailLint("option-register-no-default", Severity::Warning)
            }
            "union-constructor-in-pattern" => {
                DiagnosticCode::SailLint("union-constructor-in-pattern", Severity::Warning)
            }
            "redundant-type-annotation" => {
                DiagnosticCode::SailLint("redundant-type-annotation", Severity::Warning)
            }
            "incomplete-match" => DiagnosticCode::SailLint("incomplete-match", Severity::Warning),
            "redundant-match-arm" => {
                DiagnosticCode::SailLint("redundant-match-arm", Severity::Warning)
            }
            "incomplete-scattered" => {
                DiagnosticCode::SailLint("incomplete-scattered", Severity::Warning)
            }
            "config-bitvector-truncation" => {
                DiagnosticCode::SailLint("config-bitvector-truncation", Severity::Warning)
            }
            "unknown-directive" => DiagnosticCode::SailLint("unknown-directive", Severity::Warning),
            "unclosed-directive" => {
                DiagnosticCode::SailLint("unclosed-directive", Severity::Warning)
            }
            "unsupported-register-type" => {
                DiagnosticCode::SailLint("unsupported-register-type", Severity::Warning)
            }
            "unverified-constraint" => {
                DiagnosticCode::SailLint("unverified-constraint", Severity::Warning)
            }
            "recursive-without-termination-measure" => {
                DiagnosticCode::SailLint("recursive-without-termination-measure", Severity::Warning)
            }
            "unused-function" => DiagnosticCode::SailLint("unused-function", Severity::Warning),
            "shadowed-binding" => DiagnosticCode::SailLint("shadowed-binding", Severity::Warning),
            "missing-effect-annotation" => {
                DiagnosticCode::SailLint("missing-effect-annotation", Severity::Warning)
            }
            "deprecated-syntax" => DiagnosticCode::SailLint("deprecated-syntax", Severity::Warning),
            "inconsistent-hex-casing" => {
                DiagnosticCode::SailLint("inconsistent-hex-casing", Severity::Warning)
            }
            "unused-import" => DiagnosticCode::SailLint("unused-import", Severity::Warning),
            "unsolved-constraint" => {
                DiagnosticCode::SailLint("unsolved-constraint", Severity::Warning)
            }
            "unreachable-code" => DiagnosticCode::SailLint("unreachable-code", Severity::Hint),
            "unreachable-after-escape" => {
                DiagnosticCode::SailLint("unreachable-after-escape", Severity::Hint)
            }
            _ => DiagnosticCode::SyntaxError,
        }
    }
}

/// Internal diagnostic tag (framework-independent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticTag {
    Unnecessary,
    Deprecated,
}

/// A diagnostic with byte-offset range (framework-independent).
///
/// Conversion to `lsp_types::Diagnostic` happens in to_proto.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub range: TextRange,
    pub severity: Severity,
    pub tags: Vec<DiagnosticTag>,
}

impl Diagnostic {
    pub fn new(
        code: DiagnosticCode,
        message: String,
        range: TextRange,
        severity: Severity,
    ) -> Self {
        Self { code, message, range, severity, tags: Vec::new() }
    }

    pub fn with_tags(mut self, tags: Vec<DiagnosticTag>) -> Self {
        self.tags = tags;
        self
    }
}
