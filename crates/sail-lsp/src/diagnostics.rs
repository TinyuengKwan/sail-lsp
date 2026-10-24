//! Book keeping for keeping diagnostics easily in sync with the client.
//! Core types: `DiagnosticCollection` tracks per-file diagnostics with
//! generation numbers; `NativeDiagnosticsFetchKind` distinguishes
//! syntax-only from semantic diagnostics; `fetch_native_diagnostics`
//! computes diagnostics for a set of files.

use std::collections::{HashMap, HashSet};

use base_db::FileId;
use hir::callgraph::SourceFileInfo as _; // for .text() on SalsaFile

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
///
/// Three sources, as rust-analyzer has: native syntax, native semantic, and
/// what the real compiler said. The third was absent and this comment used
/// to record its absence as a "Sail simplification" — but the IDE's analysis
/// is not the compiler's, and the classes it provably cannot decide are
/// exactly the ones `sail --just-check` decides. The `changes` set tracks
/// which files need to have their diagnostics republished.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticCollection {
    /// Per-file native syntax diagnostics (parse errors).
    pub(crate) native_syntax: HashMap<FileId, (DiagnosticsGeneration, Vec<lsp_types::Diagnostic>)>,
    /// Per-file native semantic diagnostics (type errors).
    pub(crate) native_semantic:
        HashMap<FileId, (DiagnosticsGeneration, Vec<lsp_types::Diagnostic>)>,
    /// Per-file diagnostics from `sail --just-check`. Not generation-keyed:
    /// a run replaces the previous run wholesale, because upstream stops at
    /// the first error in a file and a partial merge would keep a finding it
    /// no longer makes.
    pub(crate) check: HashMap<FileId, Vec<lsp_types::Diagnostic>>,
    /// Files whose diagnostics changed since last `take_changes()`.
    changes: HashSet<FileId>,
    /// Counter for supplying new generation numbers.
    generation: DiagnosticsGeneration,
}

impl DiagnosticCollection {
    /// Clear native diagnostics for a specific file.
    ///
    /// Called when a file *outside the workspace* is closed. A workspace
    /// file keeps its diagnostics — that is what the problems panel is
    /// for — but a scratch file the editor has stopped showing has no
    /// reason to keep reporting.
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

    /// Replace everything the last `sail --just-check` run said.
    pub(crate) fn set_check_diagnostics(
        &mut self,
        diagnostics: Vec<(FileId, Vec<lsp_types::Diagnostic>)>,
    ) {
        // Every file that had findings before is a change, whether or not it
        // has any now — clearing is a change too.
        self.changes.extend(self.check.keys().copied());
        self.check.clear();
        for (file_id, diags) in diagnostics {
            self.changes.insert(file_id);
            self.check.entry(file_id).or_default().extend(diags);
        }
    }

    /// Get all diagnostics for a file (syntax + semantic + check).
    pub(crate) fn diagnostics_for(
        &self,
        file_id: FileId,
    ) -> impl Iterator<Item = &lsp_types::Diagnostic> {
        let syntax = self.native_syntax.get(&file_id).into_iter().flat_map(|(_, d)| d);
        let semantic = self.native_semantic.get(&file_id).into_iter().flat_map(|(_, d)| d);
        let check = self.check.get(&file_id).into_iter().flatten();
        syntax.chain(semantic).chain(check)
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
        .filter_map(|(url, file_id)| {
            let diagnostics = match kind {
                NativeDiagnosticsFetchKind::Syntax => {
                    match analysis.syntax_diagnostics(*file_id, config) {
                        Ok(diags) => diags,
                        Err(_) => return None,
                    }
                }
                NativeDiagnosticsFetchKind::Semantic => {
                    match analysis.file_diagnostics(*file_id, config) {
                        Ok(diags) => diags,
                        // Cancelled: the file changed under us, and the
                        // fetch that replaces this one is already queued.
                        Err(_) => return None,
                    }
                }
            };
            let lsp_diags: Vec<lsp_types::Diagnostic> = if let Some(sf) = analysis.file(url) {
                let line_index = ide_db::line_index::LineIndex::new(sf.text());
                diagnostics
                    .iter()
                    .map(|d| {
                        crate::to_proto::diagnostic_with_analysis(analysis, &line_index, url, d)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Some((*file_id, lsp_diags))
        })
        .collect()
}
