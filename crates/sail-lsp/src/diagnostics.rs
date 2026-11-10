//! Book keeping for keeping diagnostics easily in sync with the client.
//! Core types: `DiagnosticCollection` tracks per-file diagnostics with
//! generation numbers; `NativeDiagnosticsFetchKind` distinguishes
//! syntax-only from semantic diagnostics; `fetch_native_diagnostics`
//! computes diagnostics for a set of files.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

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

/// Immutable findings shared with requests. Presentation is derived for the
/// requested physical file; policy changes never alter compiler evidence.
#[derive(Clone, Debug, Default)]
pub(crate) struct CompilerDiagnostics {
    raw: BTreeMap<crate::flycheck::CheckTarget, Arc<BTreeMap<FileId, Vec<lsp_types::Diagnostic>>>>,
    policies:
        Arc<BTreeMap<crate::flycheck::CheckTarget, crate::flycheck::CompilerDiagnosticPolicy>>,
}
impl CompilerDiagnostics {
    pub(crate) fn for_file(
        &self,
        file: FileId,
    ) -> impl Iterator<Item = lsp_types::Diagnostic> + '_ {
        self.raw.iter().flat_map(move |(target, files)| {
            let policy = self.policies.get(target);
            files.get(&file).into_iter().flatten().filter_map(move |diagnostic| match policy {
                Some(policy) => policy.project(diagnostic),
                None => Some(diagnostic.clone()),
            })
        })
    }
}

/// Tracks per-file diagnostic state with generation numbers.
///
/// Native waves use generation numbers. Compiler findings stay isolated by
/// check target and are projected through presentation policy. `changes`
/// records physical files requiring fresh publication.
#[derive(Debug, Default)]
pub(crate) struct DiagnosticCollection {
    /// Per-file native syntax diagnostics (parse errors).
    pub(crate) native_syntax: HashMap<FileId, (DiagnosticsGeneration, Vec<lsp_types::Diagnostic>)>,
    /// Per-file native semantic diagnostics (type errors).
    pub(crate) native_semantic:
        HashMap<FileId, (DiagnosticsGeneration, Vec<lsp_types::Diagnostic>)>,
    /// Completed compiler runs and their presentation settings. Replacing one
    /// target never discards another target's findings.
    pub(crate) check: CompilerDiagnostics,
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
        self.changes
            .extend(self.check.raw.values().flat_map(|files| files.iter()).map(|(file, _)| *file));
    }

    pub(crate) fn is_current_generation(&self, generation: DiagnosticsGeneration) -> bool {
        self.generation == generation
    }

    pub(crate) fn generation(&self) -> DiagnosticsGeneration {
        self.generation
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

    fn native_for_file(
        &self,
        file_id: FileId,
    ) -> Option<(NativeDiagnosticsFetchKind, &(DiagnosticsGeneration, Vec<lsp_types::Diagnostic>))>
    {
        match (self.native_syntax.get(&file_id), self.native_semantic.get(&file_id)) {
            (Some(syntax), Some(full)) if syntax.0 > full.0 => {
                Some((NativeDiagnosticsFetchKind::Syntax, syntax))
            }
            (_, Some(full)) => Some((NativeDiagnosticsFetchKind::Semantic, full)),
            (Some(syntax), None) => Some((NativeDiagnosticsFetchKind::Syntax, syntax)),
            (None, None) => None,
        }
    }

    /// Set native diagnostics for a batch of files.
    /// Accepts a batch of `(FileId, Vec<Diagnostic>)` from a single
    /// diagnostic wave. Republish when its payload or the selected wave changes.
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
        for (file_id, mut diags) in diagnostics {
            diags.sort_by_key(|it| (it.range.start, it.range.end));
            let selected_before = self.native_for_file(file_id).map(|(kind, _)| kind);
            let target = match kind {
                NativeDiagnosticsFetchKind::Syntax => &mut self.native_syntax,
                NativeDiagnosticsFetchKind::Semantic => &mut self.native_semantic,
            };

            if let Some((old_gen, existing)) = target.get_mut(&file_id) {
                if generation < *old_gen {
                    continue;
                }
                *old_gen = generation;
                if *existing == diags {
                    // Equal cached payloads can still replace the other wave:
                    // new empty syntax clears old types; completed semantics
                    // must then restore findings even if they are unchanged.
                    if selected_before != self.native_for_file(file_id).map(|(kind, _)| kind) {
                        self.changes.insert(file_id);
                    }
                    continue;
                }
            }
            // Unlike macro-expansion batches in RA, our wave already aggregates
            // every include context for this physical file. Replace, never merge.
            target.insert(file_id, (generation, diags));
            self.changes.insert(file_id);
        }
    }

    /// Replace everything the last `sail --just-check` run of `target` said.
    pub(crate) fn set_check_diagnostics(
        &mut self,
        target: &crate::flycheck::CheckTarget,
        diagnostics: Vec<(FileId, Vec<lsp_types::Diagnostic>)>,
    ) {
        self.mark_check_files(target);
        let mut by_file = BTreeMap::<FileId, Vec<lsp_types::Diagnostic>>::new();
        for (file, diagnostics) in diagnostics {
            by_file.entry(file).or_default().extend(diagnostics);
        }
        self.check.raw.insert(target.clone(), Arc::new(by_file));
        self.mark_check_files(target);
    }

    fn mark_check_files(&mut self, target: &crate::flycheck::CheckTarget) {
        self.changes.extend(
            self.check
                .raw
                .get(target)
                .into_iter()
                .flat_map(|files| files.iter())
                .map(|(file, _)| *file),
        );
    }

    pub(crate) fn set_check_policy(
        &mut self,
        target: &crate::flycheck::CheckTarget,
        policy: crate::flycheck::CompilerDiagnosticPolicy,
    ) {
        if self.check.policies.get(target) == Some(&policy) {
            return;
        }
        Arc::make_mut(&mut self.check.policies).insert(target.clone(), policy);
        self.mark_check_files(target);
    }

    pub(crate) fn check_targets(&self) -> Vec<crate::flycheck::CheckTarget> {
        self.check.raw.keys().cloned().collect()
    }

    pub(crate) fn check_counts(&self, target: &crate::flycheck::CheckTarget) -> (usize, usize) {
        let diagnostics = self
            .check
            .raw
            .get(target)
            .into_iter()
            .flat_map(|files| files.iter())
            .flat_map(|(_, diagnostics)| diagnostics);
        let policy = self.check.policies.get(target);
        let (mut reported, mut visible) = (0, 0);
        for diagnostic in diagnostics {
            reported += 1;
            visible += usize::from(policy.is_none_or(|policy| !policy.is_ignored(diagnostic)));
        }
        (reported, visible)
    }

    /// Forget the runs of targets that no longer exist. Clearing is a change.
    pub(crate) fn retain_check_targets(
        &mut self,
        mut keep: impl FnMut(&crate::flycheck::CheckTarget) -> bool,
    ) {
        let targets: std::collections::BTreeSet<_> =
            self.check.raw.keys().chain(self.check.policies.keys()).cloned().collect();
        for target in targets.into_iter().filter(|target| !keep(target)) {
            self.mark_check_files(&target);
            self.check.raw.remove(&target);
            Arc::make_mut(&mut self.check.policies).remove(&target);
        }
    }

    /// Get all diagnostics for a file (syntax + semantic + check).
    pub(crate) fn diagnostics_for(
        &self,
        file_id: FileId,
    ) -> impl Iterator<Item = lsp_types::Diagnostic> {
        // The second wave calls file_diagnostics: it already contains syntax
        // findings. Do not duplicate them, or mix new syntax with old types.
        let native = self.native_for_file(file_id).map(|(_, diagnostics)| diagnostics);
        let mut out = Vec::new();
        for diagnostic in
            native.into_iter().flat_map(|(_, d)| d).cloned().chain(self.check.for_file(file_id))
        {
            if !out.contains(&diagnostic) {
                out.push(diagnostic);
            }
        }
        out.into_iter()
    }

    pub(crate) fn native_diagnostics_are_current(&self, file_id: FileId) -> bool {
        self.check.for_file(file_id).next().is_none()
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

/// Deduplicate identical compiler findings from shared projects without
/// guessing that different messages/codes or native fixes mean the same thing.
pub(crate) fn extend_unique(
    items: &mut Vec<lsp_types::Diagnostic>,
    checks: impl IntoIterator<Item = lsp_types::Diagnostic>,
) {
    for diagnostic in checks {
        if !items.contains(&diagnostic) {
            items.push(diagnostic);
        }
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
    config: impl Fn(&lsp_types::Url) -> ide_diagnostics::DiagnosticsConfig,
) -> ide::Cancellable<Vec<(base_db::FileId, Vec<lsp_types::Diagnostic>)>> {
    subscriptions
        .iter()
        .map(|(url, file_id)| {
            let config = config(url);
            let diagnostics = match kind {
                NativeDiagnosticsFetchKind::Syntax => {
                    analysis.syntax_diagnostics(*file_id, &config)?
                }
                NativeDiagnosticsFetchKind::Semantic => {
                    analysis.file_diagnostics(*file_id, &config)?
                }
            };
            let lsp_diags = if let Some(sf) = analysis.file(url) {
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
            Ok((*file_id, lsp_diags))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_presentation_is_reversible_and_never_filters_native_or_other_projects() {
        let file = FileId::from_raw(0);
        let a = crate::flycheck::CheckTarget::Project("a.sail_project".into());
        let b = crate::flycheck::CheckTarget::Project("b.sail_project".into());
        let compiler = lsp_types::Diagnostic {
            source: Some("sail".into()),
            severity: Some(lsp_types::DiagnosticSeverity::WARNING),
            message: "compiler".into(),
            ..Default::default()
        };
        let native = lsp_types::Diagnostic {
            source: Some("sail-lsp".into()),
            severity: Some(lsp_types::DiagnosticSeverity::ERROR),
            message: "native".into(),
            ..Default::default()
        };
        let mut collection = DiagnosticCollection::default();
        collection.native_semantic.insert(file, (0, vec![native.clone()]));
        collection.set_check_diagnostics(&a, vec![(file, vec![compiler.clone()])]);
        collection.set_check_diagnostics(&b, vec![(file, vec![compiler.clone()])]);
        let mut config =
            crate::config::FlycheckConfig { ignore: vec!["*".into()], ..Default::default() };
        collection.set_check_policy(&a, (&config).into());
        assert_eq!(collection.check_counts(&a), (1, 0));
        assert_eq!(collection.check_counts(&b), (1, 1));
        assert_eq!(collection.diagnostics_for(file).count(), 2);
        assert_eq!(collection.diagnostics_for(file).next(), Some(native.clone()));
        config.ignore.clear();
        config.warnings_as_hint = vec!["severity:warning".into()];
        collection.set_check_policy(&a, (&config).into());
        assert_eq!(collection.check_counts(&a), (1, 1));
        let shown: Vec<_> = collection.diagnostics_for(file).collect();
        assert!(shown.iter().any(|d| d.source.as_deref() == Some("sail")
            && d.severity == Some(lsp_types::DiagnosticSeverity::HINT)));
        assert!(shown.iter().any(|d| d == &compiler));
        collection.set_check_policy(&a, Default::default());
        assert_eq!(collection.diagnostics_for(file).collect::<Vec<_>>(), [native, compiler]);
        config.ignore = vec!["*".into()];
        collection.set_check_policy(&a, (&config).into());
        collection.retain_check_targets(|target| target != &a);
        collection.set_check_policy(&a, Default::default());
        assert_eq!(
            collection.check_counts(&a),
            (0, 0),
            "cleared raw findings cannot reappear after un-hiding"
        );
    }

    fn diagnostic(message: &str) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic { message: message.into(), ..Default::default() }
    }

    #[test]
    fn shared_project_duplicates_merge_but_sources_and_fix_metadata_survive() {
        let file = FileId::from_raw(0);
        let a = crate::flycheck::CheckTarget::Project("a.sail_project".into());
        let b = crate::flycheck::CheckTarget::Project("b.sail_project".into());
        let mut compiler = diagnostic("type mismatch");
        compiler.source = Some("sail".into());
        let mut native = compiler.clone();
        native.source = Some("sail-lsp".into());
        native.data = Some(serde_json::json!({"fix": "unsigned"}));
        let mut collection = DiagnosticCollection::default();
        collection.native_semantic.insert(file, (0, vec![native.clone()]));
        collection.set_check_diagnostics(&a, vec![(file, vec![compiler.clone()])]);
        collection.set_check_diagnostics(&b, vec![(file, vec![compiler.clone()])]);
        let pushed: Vec<_> = collection.diagnostics_for(file).collect();
        let mut pulled = vec![native];
        extend_unique(&mut pulled, [compiler.clone(), compiler]);
        assert_eq!(pushed, pulled);
        assert_eq!(pushed.len(), 2);
        assert_eq!(pushed[0].data.as_ref().unwrap()["fix"], "unsigned");
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
        let messages: Vec<_> = collection.diagnostics_for(file).map(|d| d.message).collect();
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
    fn unchanged_native_payloads_publish_when_the_selected_wave_changes() {
        let mut collection = DiagnosticCollection::default();
        let file = FileId::from_raw(0);
        let first = collection.next_generation();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Syntax,
            first,
            vec![(file, vec![])],
        );
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            first,
            vec![(file, vec![diagnostic("semantic")])],
        );
        collection.take_changes();
        let second = collection.next_generation();
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Syntax,
            second,
            vec![(file, vec![])],
        );
        assert_eq!(collection.diagnostics_for(file).count(), 0);
        assert_eq!(
            collection.take_changes(),
            Some(HashSet::from([file])),
            "clear the previous semantic wave even when the syntax payload is unchanged"
        );
        collection.set_native_diagnostics(
            NativeDiagnosticsFetchKind::Semantic,
            second,
            vec![(file, vec![diagnostic("semantic")])],
        );
        assert_eq!(collection.diagnostics_for(file).count(), 1);
        assert_eq!(
            collection.take_changes(),
            Some(HashSet::from([file])),
            "restore semantic findings even when their payload matches the previous generation"
        );
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
        assert_eq!(collection.diagnostics_for(file).next(), Some(diag.clone()));
    }
}
