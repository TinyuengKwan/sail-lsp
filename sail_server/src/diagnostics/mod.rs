pub mod parse;
pub(crate) mod reporting;
pub mod semantic;
pub(crate) mod type_error;

pub(crate) use parse::compute_parse_diagnostics;
pub(crate) use semantic::compute_semantic_diagnostics;

use crate::state::File;
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};
use tower_lsp::lsp_types::{
    Diagnostic as LspDiagnostic, DiagnosticSeverity, DocumentDiagnosticReport,
    DocumentDiagnosticReportResult, FullDocumentDiagnosticReport, Range,
    RelatedFullDocumentDiagnosticReport, RelatedUnchangedDocumentDiagnosticReport,
    UnchangedDocumentDiagnosticReport, Url, WorkspaceDiagnosticReport,
    WorkspaceDiagnosticReportResult, WorkspaceDocumentDiagnosticReport,
    WorkspaceFullDocumentDiagnosticReport,
};

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Severity {
    Error,
    Warning,
    WeakWarning,
    Information,
    Hint,
}

impl From<Severity> for DiagnosticSeverity {
    fn from(severity: Severity) -> Self {
        match severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::WeakWarning => DiagnosticSeverity::INFORMATION,
            Severity::Information => DiagnosticSeverity::INFORMATION,
            Severity::Hint => DiagnosticSeverity::HINT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DiagnosticCode {
    DuplicateDefinition,
    UnusedVariable,
    UnreachableCode,
    MismatchedArgCount,
    TypeError,
    LexicalError,
    SyntaxError,
    DeprecatedEffectAnnotation,
    DeprecatedCastAnnotation,
    MissingExternPurity,
    UnmodifiedMutableVariable,
    OptionRegisterNoDefault,
    UnionConstructorInPattern,
    RedundantTypeAnnotation,
}

impl DiagnosticCode {
    pub fn as_str(&self) -> &str {
        match self {
            DiagnosticCode::DuplicateDefinition => "duplicate-definition",
            DiagnosticCode::UnusedVariable => "unused-variable",
            DiagnosticCode::UnreachableCode => "unreachable-code",
            DiagnosticCode::MismatchedArgCount => "mismatched-arg-count",
            DiagnosticCode::TypeError => "type-error",
            DiagnosticCode::LexicalError => "lexical-error",
            DiagnosticCode::SyntaxError => "syntax-error",
            DiagnosticCode::DeprecatedEffectAnnotation => "deprecated-effect-annotation",
            DiagnosticCode::DeprecatedCastAnnotation => "deprecated-cast-annotation",
            DiagnosticCode::MissingExternPurity => "missing-extern-purity",
            DiagnosticCode::UnmodifiedMutableVariable => "unmodified-mutable-variable",
            DiagnosticCode::OptionRegisterNoDefault => "option-register-no-default",
            DiagnosticCode::UnionConstructorInPattern => "union-constructor-in-pattern",
            DiagnosticCode::RedundantTypeAnnotation => "redundant-type-annotation",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub message: String,
    pub range: Range,
    pub severity: Severity,
    pub tags: Option<Vec<tower_lsp::lsp_types::DiagnosticTag>>,
}

impl Diagnostic {
    pub fn new(code: DiagnosticCode, message: String, range: Range, severity: Severity) -> Self {
        Self {
            code,
            message,
            range,
            severity,
            tags: None,
        }
    }

    pub fn with_tags(mut self, tags: Vec<tower_lsp::lsp_types::DiagnosticTag>) -> Self {
        self.tags = Some(tags);
        self
    }

    pub fn to_proto(&self) -> LspDiagnostic {
        LspDiagnostic {
            range: self.range,
            severity: Some(self.severity.into()),
            code: Some(tower_lsp::lsp_types::NumberOrString::String(
                self.code.as_str().to_string(),
            )),
            source: Some("Sail".to_string()),
            message: self.message.clone(),
            tags: self.tags.clone(),
            ..Default::default()
        }
    }
}

fn file_diagnostic_result_id(file: &File) -> String {
    let mut hasher = DefaultHasher::new();
    let lsp_diags = file.lsp_diagnostics();
    file.source.text().len().hash(&mut hasher);
    lsp_diags.len().hash(&mut hasher);
    for diagnostic in &lsp_diags {
        diagnostic.range.start.line.hash(&mut hasher);
        diagnostic.range.start.character.hash(&mut hasher);
        diagnostic.range.end.line.hash(&mut hasher);
        diagnostic.range.end.character.hash(&mut hasher);
        format!("{:?}", diagnostic.severity).hash(&mut hasher);
        diagnostic.code.hash(&mut hasher);
        diagnostic.source.hash(&mut hasher);
        diagnostic.message.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

pub(crate) fn document_diagnostic_report_for_file(
    file: &File,
    previous_result_id: Option<&str>,
) -> DocumentDiagnosticReportResult {
    let result_id = file_diagnostic_result_id(file);
    if previous_result_id == Some(result_id.as_str()) {
        return DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
            related_documents: None,
            unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport { result_id },
        })
        .into();
    }

    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
        related_documents: None,
        full_document_diagnostic_report: FullDocumentDiagnosticReport {
            result_id: Some(result_id),
            items: file.lsp_diagnostics(),
        },
    })
    .into()
}

pub(crate) fn workspace_diagnostic_report<'a, I>(
    files: I,
    versions: &HashMap<Url, i32>,
    previous_result_ids: &HashMap<Url, String>,
) -> WorkspaceDiagnosticReportResult
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut items = Vec::new();
    for (uri, file) in files {
        let result_id = file_diagnostic_result_id(file);
        let version = versions.get(uri).copied().map(i64::from);
        if previous_result_ids.get(uri).map(String::as_str) == Some(result_id.as_str()) {
            items.push(WorkspaceDocumentDiagnosticReport::Unchanged(
                tower_lsp::lsp_types::WorkspaceUnchangedDocumentDiagnosticReport {
                    uri: uri.clone(),
                    version,
                    unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                        result_id,
                    },
                },
            ));
        } else {
            items.push(WorkspaceDocumentDiagnosticReport::Full(
                WorkspaceFullDocumentDiagnosticReport {
                    uri: uri.clone(),
                    version,
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id: Some(result_id),
                        items: file.lsp_diagnostics(),
                    },
                },
            ));
        }
    }
    WorkspaceDiagnosticReport { items }.into()
}
