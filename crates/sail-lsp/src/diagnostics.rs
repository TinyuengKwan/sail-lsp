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
    /// Project loading context changed even when native findings did not.
    pub(crate) fn mark_all_changed(&mut self) {
        self.changes.extend(self.native_syntax.keys().copied());
        self.changes.extend(self.native_semantic.keys().copied());
        self.changes.extend(self.check.keys().copied());
    }

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
        // Every wave covers all subscriptions. Once another wave has been
        // requested, results from the previous snapshot must not be published,
        // even if the replacement has not finished yet.
        if generation < self.generation {
            return;
        }
        let target = match kind {
            NativeDiagnosticsFetchKind::Syntax => &mut self.native_syntax,
            NativeDiagnosticsFetchKind::Semantic => &mut self.native_semantic,
        };

        for (file_id, mut diags) in diagnostics {
            diags.sort_by_key(|it| (it.range.start, it.range.end));

            if let Some((old_gen, existing)) = target.get_mut(&file_id) {
                if generation < *old_gen {
                    continue;
                }
                *old_gen = generation;
                if *existing == diags {
                    continue;
                }
            }
            // Unlike macro-expansion batches in RA, our wave already aggregates
            // every include context for this physical file. Replace, never merge.
            target.insert(file_id, (generation, diags));
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
        let syntax = self.native_syntax.get(&file_id);
        let full = self.native_semantic.get(&file_id);
        // The second wave calls file_diagnostics: it already contains syntax
        // findings. Do not duplicate them, or mix new syntax with old types.
        let native = match (syntax, full) {
            (Some(syntax), Some(full)) if syntax.0 > full.0 => Some(syntax),
            (_, Some(full)) => Some(full),
            (syntax, None) => syntax,
        };
        let check = self.check.get(&file_id).into_iter().flatten();
        native.into_iter().flat_map(|(_, d)| d).chain(check)
    }

    pub(crate) fn native_diagnostics_are_current(&self, file_id: FileId) -> bool {
        !self.check.get(&file_id).is_some_and(|diags| !diags.is_empty())
            && [self.native_syntax.get(&file_id), self.native_semantic.get(&file_id)]
                .into_iter()
                .flatten()
                .any(|(generation, _)| *generation == self.generation)
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

/// Attach the actual load failure to the otherwise generic detached-file hint.
/// Called at the LSP boundary so project I/O does not enter semantic queries.
pub(crate) fn explain_unlinked_file(
    diagnostics: &mut [lsp_types::Diagnostic],
    project_error: Option<&str>,
) {
    let Some(error) = project_error else { return };
    for diagnostic in diagnostics {
        if matches!(&diagnostic.code, Some(lsp_types::NumberOrString::String(code)) if code == "unlinked-file")
        {
            diagnostic.message.push_str("\n\n");
            diagnostic.message.push_str(error);
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn diagnostic(message: &str) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic { message: message.into(), ..Default::default() }
    }

    #[test]
    fn full_results_replace_syntax_and_never_mix_revisions() {
        let mut collection = DiagnosticCollection::default();
        let file = FileId::from_raw(0);
        let first = collection.next_generation();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Syntax,
            first,
            vec![(file, vec![diagnostic("syntax")])],
        );
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            first,
            vec![(file, vec![diagnostic("syntax"), diagnostic("type")])],
        );
        assert_eq!(collection.diagnostics_for(file).count(), 2);
        assert!(collection.native_diagnostics_are_current(file));
        let second = collection.next_generation();
        assert!(!collection.native_diagnostics_are_current(file));
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Syntax,
            second,
            vec![(file, vec![diagnostic("new syntax")])],
        );
        let messages: Vec<_> =
            collection.diagnostics_for(file).map(|d| d.message.as_str()).collect();
        assert_eq!(messages, ["new syntax"]);
        assert!(collection.native_diagnostics_are_current(file));
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            second,
            vec![(file, vec![])],
        );
        assert_eq!(collection.diagnostics_for(file).count(), 0);
    }

    #[test]
    fn old_diagnostics_cannot_return_after_a_fix() {
        let mut collection = DiagnosticCollection::default();
        let file = FileId::from_raw(0);
        let old = collection.next_generation();
        let current = collection.next_generation();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            current,
            vec![(file, vec![])],
        );
        collection.take_changes();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            old,
            vec![(file, vec![diagnostic("already fixed")])],
        );
        assert_eq!(collection.diagnostics_for(file).count(), 0);
        assert!(collection.take_changes().is_none());
    }

    #[test]
    fn superseded_wave_is_ignored_before_replacement_finishes() {
        let mut collection = DiagnosticCollection::default();
        let file = FileId::from_raw(0);
        let old = collection.next_generation();
        collection.next_generation();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Syntax,
            old,
            vec![(file, vec![diagnostic("incomplete expression")])],
        );
        assert_eq!(collection.diagnostics_for(file).count(), 0);
        assert!(collection.take_changes().is_none());
    }

    #[test]
    fn identical_results_advance_generation_without_republishing() {
        let mut collection = DiagnosticCollection::default();
        let file = FileId::from_raw(0);
        for _ in 0..2 {
            let generation = collection.next_generation();
            collection.set_native_diagnostics(
                NativeDiagnosticsFetchKind::Semantic,
                generation,
                vec![(file, vec![diagnostic("unchanged")])],
            );
            if generation == 1 {
                assert!(collection.take_changes().is_some());
            } else {
                assert!(collection.take_changes().is_none());
            }
        }
        assert_eq!(collection.native_semantic[&file].0, 2);
    }

    #[test]
    fn changed_diagnostic_metadata_is_published() {
        let mut collection = DiagnosticCollection::default();
        let file = FileId::from_raw(0);
        let mut diag = diagnostic("unchanged message");
        let generation = collection.next_generation();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            generation,
            vec![(file, vec![diag.clone()])],
        );
        collection.take_changes();
        diag.code = Some(lsp_types::NumberOrString::String("new-fix-code".into()));
        let generation = collection.next_generation();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            generation,
            vec![(file, vec![diag.clone()])],
        );
        assert!(collection.take_changes().is_some());
        assert_eq!(collection.diagnostics_for(file).next(), Some(&diag));
    }
}
