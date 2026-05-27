//! Book keeping for keeping diagnostics easily in sync with the client.
//! Core types: `DiagnosticCollection` tracks per-file diagnostics with
//! generation numbers; `NativeDiagnosticsFetchKind` distinguishes
//! syntax-only from semantic diagnostics; `fetch_native_diagnostics`
//! computes diagnostics for a set of files.

use std::collections::{HashMap, HashSet};

use base_db::FileId;
use hir_def::callgraph::SourceFileInfo as _; // for .text() on SalsaFile

/// Generation counter for diagnostic updates.
/// Used to discard stale diagnostic results from cancelled workers.
pub(crate) type DiagnosticsGeneration = usize;

/// Distinguishes syntax-only from full semantic diagnostics.
/// Syntax diagnostics are fast (parse-only), semantic diagnostics
/// require type inference and are computed on worker threads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeDiagnosticsFetchKind {
    /// Fast parse-only diagnostics (no type inference).
    Syntax,
    /// Full semantic diagnostics (type inference + constraints).
    Semantic,
}

/// Tracks per-file diagnostic state with generation numbers.
/// Sail simplification: no flycheck/cargo diagnostics — only native
/// syntax + semantic diagnostics. The `changes` set tracks which
/// files need to have their diagnostics republished.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticCollection {
    /// Per-file native syntax diagnostics (parse errors).
    pub(crate) native_syntax: HashMap<FileId, (DiagnosticsGeneration, Vec<lsp_types::Diagnostic>)>,
    /// Per-file native semantic diagnostics (type errors).
    pub(crate) native_semantic:
        HashMap<FileId, (DiagnosticsGeneration, Vec<lsp_types::Diagnostic>)>,
    /// Files whose diagnostics changed since last `take_changes()`.
    changes: HashSet<FileId>,
    /// Counter for supplying new generation numbers.
    generation: DiagnosticsGeneration,
}

impl DiagnosticCollection {
    /// Clear native diagnostics for a specific file.
    /// Used when a file is closed (didClose notification).
    #[allow(dead_code)]
    pub(crate) fn clear_native_for(&mut self, file_id: FileId) {
        self.native_syntax.remove(&file_id);
        self.native_semantic.remove(&file_id);
        self.changes.insert(file_id);
    }

    /// Set native diagnostics (syntax or semantic) for a batch of files.
    /// Set native diagnostics for a batch of files.
    /// Accepts a batch of `(FileId, Vec<Diagnostic>)` from a single
    /// diagnostic wave. Only records a change if the diagnostics
    /// actually differ from the previously stored set.
    pub(crate) fn set_native_diagnostics(
        &mut self,
        kind: NativeDiagnosticsFetchKind,
        generation: DiagnosticsGeneration,
        diagnostics: Vec<(FileId, Vec<lsp_types::Diagnostic>)>,
    ) {
        let target = match kind {
            NativeDiagnosticsFetchKind::Syntax => &mut self.native_syntax,
            NativeDiagnosticsFetchKind::Semantic => &mut self.native_semantic,
        };

        for (file_id, mut diags) in diagnostics {
            diags.sort_by_key(|it| (it.range.start, it.range.end));

            if let Some((old_gen, existing)) = target.get(&file_id) {
                if existing.len() == diags.len()
                    && existing.iter().zip(&diags).all(|(a, b)| are_diagnostics_equal(a, b))
                {
                    continue;
                }
                if *old_gen < generation || generation == 0 {
                    target.insert(file_id, (generation, diags));
                } else {
                    // Same or older generation — merge
                    let existing = &mut target.get_mut(&file_id).unwrap().1;
                    existing.extend(diags);
                    existing.sort_by_key(|it| (it.range.start, it.range.end));
                }
            } else {
                target.insert(file_id, (generation, diags));
            }
            self.changes.insert(file_id);
        }
    }

    /// Get all diagnostics for a file (syntax + semantic combined).
    pub(crate) fn diagnostics_for(
        &self,
        file_id: FileId,
    ) -> impl Iterator<Item = &lsp_types::Diagnostic> {
        let syntax = self.native_syntax.get(&file_id).into_iter().flat_map(|(_, d)| d);
        let semantic = self.native_semantic.get(&file_id).into_iter().flat_map(|(_, d)| d);
        syntax.chain(semantic)
    }

    /// Take the set of files whose diagnostics changed.
    /// Returns `None` if no changes occurred.
    pub(crate) fn take_changes(&mut self) -> Option<HashSet<FileId>> {
        if self.changes.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.changes))
    }

    /// Bump and return the next generation number.
    pub(crate) fn next_generation(&mut self) -> DiagnosticsGeneration {
        self.generation += 1;
        self.generation
    }
}

/// Check if two LSP diagnostics are semantically equal.
fn are_diagnostics_equal(left: &lsp_types::Diagnostic, right: &lsp_types::Diagnostic) -> bool {
    left.source == right.source
        && left.severity == right.severity
        && left.range == right.range
        && left.message == right.message
}

/// Compute native diagnostics for a slice of files.
/// Returns `Vec<(FileId, Vec<lsp_types::Diagnostic>)>` for batch
/// processing by `set_native_diagnostics`.
pub(crate) fn fetch_native_diagnostics(
    analysis: &ide::analysis::Analysis,
    subscriptions: &[(lsp_types::Url, base_db::FileId)],
    kind: NativeDiagnosticsFetchKind,
    config: &ide_diagnostics::DiagnosticsConfig,
) -> Vec<(base_db::FileId, Vec<lsp_types::Diagnostic>)> {
    subscriptions
        .iter()
        .map(|(url, file_id)| {
            let diagnostics = match kind {
                NativeDiagnosticsFetchKind::Syntax => analysis.syntax_diagnostics(*file_id, config),
                NativeDiagnosticsFetchKind::Semantic => analysis.file_diagnostics(*file_id, config),
            };
            let lsp_diags: Vec<lsp_types::Diagnostic> = if let Some(sf) = analysis.file(url) {
                let line_index = ide_db::line_index::LineIndex::new(sf.text());
                diagnostics.iter().map(|d| crate::to_proto::diagnostic(&line_index, d)).collect()
            } else {
                Vec::new()
            };
            (*file_id, lsp_diags)
        })
        .collect()
}

use ide_db::FileDb;
use std::hash::{Hash, Hasher};

/// Compute parse + semantic + type-check diagnostics for a file.
/// Replaces the removed `FileDb::diagnostics()` trait method.
fn compute_file_diagnostics(file: &dyn FileDb) -> Vec<ide_diagnostics::Diagnostic> {
    let parse_diags = ide_diagnostics::compute_parse_diagnostics(file, &[]);
    let semantic_diags = ide_diagnostics::compute_semantic_diagnostics(file);

    let type_diags: Vec<hir_def::diagnostics::Diagnostic> =
        hir_ty::infer::check_file(file).map(|tc| tc.diagnostics().to_vec()).unwrap_or_default();

    parse_diags
        .into_iter()
        .chain(semantic_diags)
        .chain(type_diags.iter().cloned())
        .map(|d| {
            let has_unnecessary = d
                .tags
                .iter()
                .any(|t| matches!(t, hir_def::diagnostics::DiagnosticTag::Unnecessary));
            ide_diagnostics::Diagnostic::new(d.code, d.message, d.range)
                .with_severity(d.severity)
                .with_unused(has_unnecessary)
        })
        .collect()
}

fn file_diagnostic_result_id(file: &dyn FileDb) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let diags = compute_file_diagnostics(file);
    file.text().len().hash(&mut hasher);
    diags.len().hash(&mut hasher);
    for d in &diags {
        d.range.range.start().hash(&mut hasher);
        d.range.range.end().hash(&mut hasher);
        d.code.hash(&mut hasher);
        d.message.hash(&mut hasher);
    }
    format!("{:x}", hasher.finish())
}

/// Compute LSP diagnostics for a file via FileDb trait.
pub fn compute_lsp_diagnostics_for_file(file: &dyn FileDb) -> Vec<lsp_types::Diagnostic> {
    let line_index = ide_db::line_index::LineIndex::new(file.text());
    compute_file_diagnostics(file)
        .iter()
        .map(|d| crate::to_proto::diagnostic(&line_index, d))
        .collect()
}

#[allow(dead_code)]
pub fn document_diagnostic_report_for_file(
    file: &dyn FileDb,
    previous_result_id: Option<&str>,
) -> lsp_types::DocumentDiagnosticReportResult {
    let result_id = file_diagnostic_result_id(file);
    if previous_result_id == Some(result_id.as_str()) {
        return lsp_types::DocumentDiagnosticReport::Unchanged(
            lsp_types::RelatedUnchangedDocumentDiagnosticReport {
                related_documents: None,
                unchanged_document_diagnostic_report:
                    lsp_types::UnchangedDocumentDiagnosticReport { result_id },
            },
        )
        .into();
    }
    lsp_types::DocumentDiagnosticReport::Full(lsp_types::RelatedFullDocumentDiagnosticReport {
        related_documents: None,
        full_document_diagnostic_report: lsp_types::FullDocumentDiagnosticReport {
            result_id: Some(result_id),
            items: compute_lsp_diagnostics_for_file(file),
        },
    })
    .into()
}

#[allow(dead_code)]
pub fn workspace_diagnostic_report<'a, F, I>(
    files: I,
    versions: &HashMap<lsp_types::Url, i32>,
    previous_result_ids: &HashMap<lsp_types::Url, String>,
) -> lsp_types::WorkspaceDiagnosticReportResult
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a lsp_types::Url, &'a F)>,
{
    let mut items = Vec::new();
    for (uri, file) in files {
        let result_id = file_diagnostic_result_id(file);
        let version = versions.get(uri).copied().map(i64::from);
        if previous_result_ids.get(uri).map(String::as_str) == Some(result_id.as_str()) {
            items.push(lsp_types::WorkspaceDocumentDiagnosticReport::Unchanged(
                lsp_types::WorkspaceUnchangedDocumentDiagnosticReport {
                    uri: uri.clone(),
                    version,
                    unchanged_document_diagnostic_report:
                        lsp_types::UnchangedDocumentDiagnosticReport { result_id },
                },
            ));
        } else {
            items.push(lsp_types::WorkspaceDocumentDiagnosticReport::Full(
                lsp_types::WorkspaceFullDocumentDiagnosticReport {
                    uri: uri.clone(),
                    version,
                    full_document_diagnostic_report: lsp_types::FullDocumentDiagnosticReport {
                        result_id: Some(result_id),
                        items: compute_lsp_diagnostics_for_file(file),
                    },
                },
            ));
        }
    }
    lsp_types::WorkspaceDiagnosticReport { items }.into()
}
