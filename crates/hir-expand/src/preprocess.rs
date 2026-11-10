//! Pure preprocessing over loaded VFS inputs. No filesystem access in queries.
use base_db::{
    preprocess::{InstanceKey, WorkspaceSourceProject},
    FileId, FileText,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use syntax::preprocess::{include_candidates, normalize_path, Event};

/// Transferable preprocessing derived in a separate database. It contains no
/// Salsa handles or snapshots, so computing it never holds up host writes.
#[derive(Clone, Debug)]
pub struct PreparedWorkspaceData {
    pub project: base_db::preprocess::SourceProject,
    pub strict_bitvector: bool,
    pub projections: BTreeMap<FileId, SourceProjection>,
    pub plan: Plan,
}

#[derive(Clone, Debug)]
pub struct SourceProjection {
    pub text: Arc<str>,
    pub events: Vec<Event>,
    pub content: Vec<bool>,
}

#[salsa::input(singleton)]
pub struct PreparedWorkspace {
    #[returns(ref)]
    pub data: Option<Arc<PreparedWorkspaceData>>,
}

impl PreparedWorkspaceData {
    pub fn matches(&self, db: &dyn salsa::Database, input: WorkspaceSourceProject) -> bool {
        self.project == *input.project(db)
            && self.strict_bitvector == base_db::strict_bitvector(db)
            && self.projections.len() == input.files(db).len()
            && input.files(db).iter().all(|(id, file)| {
                self.projections.get(id).is_some_and(|p| p.text == *file.text(db))
            })
    }

    fn same_projections(&self, db: &dyn salsa::Database, input: WorkspaceSourceProject) -> bool {
        // Tracked plans depend on compact projections, not source text. A
        // body edit that backdates these queries must not rerun the walker.
        self.project == *input.project(db)
            && self.strict_bitvector == base_db::strict_bitvector(db)
            && self.projections.len() == input.files(db).len()
            && input.files(db).iter().all(|(id, file)| {
                self.projections.get(id).is_some_and(|source| {
                    source.events == *file_events(db, *file)
                        && source.content == *content_regions(db, *file)
                })
            })
    }
}

/// Called by a worker after all source inputs have arrived. The same production
/// walker computes the plan; compact projections seed subsequent host queries.
pub fn prepare_workspace(
    db: &dyn salsa::Database,
    input: WorkspaceSourceProject,
) -> PreparedWorkspaceData {
    let plan = workspace_plan(db, input).clone();
    let projections = input
        .files(db)
        .iter()
        .map(|(id, file)| {
            (
                *id,
                SourceProjection {
                    text: file.text(db).clone(),
                    events: file_events(db, *file).clone(),
                    content: content_regions(db, *file).clone(),
                },
            )
        })
        .collect();
    PreparedWorkspaceData {
        project: input.project(db).clone(),
        strict_bitvector: base_db::strict_bitvector(db),
        projections,
        plan,
    }
}

fn prepared_projection(db: &dyn salsa::Database, file: FileText) -> Option<&SourceProjection> {
    let prepared = PreparedWorkspace::try_get(db)?.data(db).as_ref()?;
    prepared.projections.get(&file.file_id(db)).filter(|p| p.text == *file.text(db))
}

#[salsa::tracked(returns(ref))]
pub fn file_events(db: &dyn salsa::Database, file: FileText) -> Vec<Event> {
    if let Some(projection) = prepared_projection(db, file) {
        return projection.events.clone();
    }
    let started = std::time::Instant::now();
    let text = file.text(db);
    if !may_have_events(text) {
        return Vec::new();
    }
    let events = syntax::parse_query::parse_file(db, file)
        .syntax_node()
        .into_iter()
        .flat_map(|root| root.children().filter_map(|n| Event::from_node(&n)).collect::<Vec<_>>())
        .collect();
    tracing::debug!(target: "sail_lsp::latency", "preprocess_events file={:?} bytes={} duration_us={}", file.file_id(db), text.len(), started.elapsed().as_micros());
    events
}

/// Structural projection: inactive guarded visits need no semantic view.
/// Function text and offsets deliberately do not participate in equality.
#[salsa::tracked(returns(ref))]
fn content_regions(db: &dyn salsa::Database, file: FileText) -> Vec<bool> {
    if let Some(projection) = prepared_projection(db, file) {
        return projection.content.clone();
    }
    let started = std::time::Instant::now();
    let text = file.text(db);
    if !may_have_events(text) && parser::has_tokens(text) {
        // With neither directive syntax nor the `default` keyword present,
        // all syntactic content belongs to the sole ordinary region. Keep
        // the CST path for tokenless malformed input and comment-only files.
        tracing::debug!(target: "sail_lsp::latency", "preprocess_content file={:?} bytes={} fast=true duration_us={}", file.file_id(db), text.len(), started.elapsed().as_micros());
        return vec![true];
    }
    let mut regions = vec![false];
    if let Some(root) = syntax::parse_query::parse_file(db, file).syntax_node() {
        for node in root.children() {
            let event = Event::from_node(&node);
            if event.as_ref().is_none_or(|event| !event.consumed()) {
                *regions.last_mut().unwrap() = true;
            }
            if event.is_some() {
                regions.push(false);
            }
        }
    }
    tracing::debug!(target: "sail_lsp::latency", "preprocess_content file={:?} bytes={} fast=false duration_us={}", file.file_id(db), text.len(), started.elapsed().as_micros());
    regions
}

fn may_have_events(text: &str) -> bool {
    // Necessary source spellings, deliberately conservative inside comments,
    // strings and longer identifiers. This can request an unnecessary parse,
    // but can never hide an actual preprocessing event.
    text.contains('$') || text.contains("default")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub key: InstanceKey,
    pub source: FileId,
    pub primary: bool,
    pub active: bool,
    pub regions: Vec<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub files: Vec<FileId>,
    pub instances: Vec<Instance>,
    pub regions: BTreeMap<FileId, Vec<bool>>,
    pub modules: base_db::module_graph::ModuleGraph,
    pub edges: Vec<(FileId, FileId)>,
    /// Each active `$include` that resolved: the including file, the
    /// directive's index among that file's preprocessor events, and the
    /// file it names. `missing` is the same for those that did not.
    pub includes: Vec<(FileId, usize, FileId)>,
    pub missing: Vec<(FileId, usize, PathBuf)>,
    pub cycles: Vec<FileId>,
    pub order: Vec<(InstanceKey, usize)>,
}

#[salsa::tracked(returns(ref))]
pub fn workspace_plan(db: &dyn salsa::Database, input: WorkspaceSourceProject) -> Plan {
    let _cancellation = syntax::cancellation_scope();
    if let Some(prepared) = PreparedWorkspace::try_get(db).and_then(|input| input.data(db).as_ref())
    {
        if prepared.same_projections(db, input) {
            return prepared.plan.clone();
        }
    }
    plan_with_overlay(db, input, None)
}

/// Read-only preview used by source edits. It shares the production walker;
/// callers never mutate a Salsa snapshot to test a proposed include.
pub fn preview_file(
    db: &dyn salsa::Database,
    input: WorkspaceSourceProject,
    source: FileId,
    root: &syntax::SyntaxNode,
) -> Plan {
    let mut content = vec![false];
    let mut events = Vec::new();
    for node in root.children() {
        let event = Event::from_node(&node);
        if event.as_ref().is_none_or(|e| !e.consumed()) {
            *content.last_mut().unwrap() = true;
        }
        if let Some(event) = event {
            events.push(event);
            content.push(false);
        }
    }
    plan_with_overlay(db, input, Some(&Overlay { source, events, content }))
}

struct Overlay {
    source: FileId,
    events: Vec<Event>,
    content: Vec<bool>,
}

fn plan_with_overlay(
    db: &dyn salsa::Database,
    input: WorkspaceSourceProject,
    overlay: Option<&Overlay>,
) -> Plan {
    walk_project(db, input, overlay, false).0
}

/// A read-only execution trace of the production walker. Region/event ordinals
/// are mapped to current source ranges by the IDE, never cached byte offsets.
#[derive(Clone, Debug)]
pub struct TraceStep {
    pub source: FileId,
    pub key: InstanceKey,
    pub kind: TraceKind,
}

#[derive(Clone, Debug)]
pub enum TraceKind {
    Enter {
        symbols: Vec<String>,
        target: Option<String>,
    },
    Region {
        ordinal: usize,
        active: bool,
    },
    Event {
        ordinal: usize,
        active: bool,
        following_active: bool,
        symbols_added: Vec<String>,
        include_target: Option<Result<FileId, PathBuf>>,
    },
    Cycle,
    Exit,
}

/// Explicit user request: replay precisely the same roots, environments,
/// conditionals and include resolution used to construct semantic instances.
pub fn trace_workspace(
    db: &dyn salsa::Database,
    input: WorkspaceSourceProject,
) -> (Plan, Vec<TraceStep>) {
    walk_project(db, input, None, true)
}

fn walk_project(
    db: &dyn salsa::Database,
    input: WorkspaceSourceProject,
    overlay: Option<&Overlay>,
    tracing: bool,
) -> (Plan, Vec<TraceStep>) {
    let project = input.project(db);
    let paths: HashMap<_, _> =
        project.paths.iter().map(|(p, id)| (normalize_path(p), *id)).collect();
    let reverse: HashMap<_, _> = paths.iter().map(|(p, id)| (*id, p.clone())).collect();
    let mut walk = Walker {
        db,
        input,
        overlay,
        paths,
        reverse,
        symbols: {
            let mut symbols = syntax::preprocess::default_symbols();
            if base_db::strict_bitvector(db) {
                symbols.insert("STRICT_BITVECTOR".into());
            }
            symbols
        },
        targets: HashMap::new(),
        stack: Vec::new(),
        owners: HashMap::new(),
        plan: Plan::default(),
        trace: tracing.then(Vec::new),
    };
    // Depend on compact event projections, not every body in the workspace.
    for (&id, &file) in input.files(db) {
        let len = overlay
            .filter(|o| o.source == id)
            .map_or_else(|| file_events(db, file).len(), |o| o.events.len());
        walk.plan.regions.insert(id, vec![false; len + 1]);
    }
    // Sail's frontend prepends Corelib_sail/Corelib_project even when no
    // source includes it. Its implicit module is available to every module.
    if let Some(core) = project
        .library
        .as_ref()
        .and_then(|p| walk.paths.get(&normalize_path(&p.join("core/corelib.sail"))))
        .copied()
    {
        walk.visit(
            core,
            InstanceKey { root: core, occurrence: usize::MAX, module: None, includes: Vec::new() },
        );
    }
    let mut occurrences = HashMap::new();
    let mut previous_options = None;
    let initial_symbols = walk.symbols.clone();
    let initial_targets = walk.targets.clone();
    for (index, &root) in project.roots.iter().enumerate() {
        let owner = project.root_modules.get(index).copied().unwrap_or_else(|| {
            project.modules.modules.iter().position(|m| m.files.contains(&root))
        });
        let options = owner.and_then(|owner| project.module_options.get(&owner));
        // Symbols flow in root order within one configuration. A different
        // project configuration begins its own preprocessor environment.
        if options != previous_options {
            walk.symbols = initial_symbols.clone();
            walk.targets = initial_targets.clone();
            if options.is_some_and(|o| o.strict_bitvector) {
                walk.symbols.insert("STRICT_BITVECTOR".into());
            } else if options.is_some() {
                walk.symbols.remove("STRICT_BITVECTOR");
            }
            previous_options = options;
        }
        let occurrence = occurrences.entry((root, owner)).or_insert(0);
        walk.visit(
            root,
            InstanceKey { root, occurrence: *occurrence, module: owner, includes: Vec::new() },
        );
        *occurrence += 1;
    }
    walk.plan.modules = project.modules.clone();
    for module in &mut walk.plan.modules.modules {
        module.files.clear();
    }
    walk.plan.modules.globals.clear();
    for &file in &walk.plan.files {
        match walk.owners[&file] {
            Some(owner) => walk.plan.modules.modules[owner].files.push(file),
            None => walk.plan.modules.globals.push(file),
        }
    }
    (walk.plan, walk.trace.unwrap_or_default())
}

struct Walker<'a> {
    db: &'a dyn salsa::Database,
    input: WorkspaceSourceProject,
    overlay: Option<&'a Overlay>,
    paths: HashMap<PathBuf, FileId>,
    reverse: HashMap<FileId, PathBuf>,
    symbols: HashSet<String>,
    targets: HashMap<String, Vec<String>>,
    stack: Vec<VisitState>,
    owners: HashMap<FileId, Option<usize>>,
    plan: Plan,
    trace: Option<Vec<TraceStep>>,
}

struct VisitState {
    file: FileId,
    symbols: HashSet<String>,
    targets: HashMap<String, Vec<String>>,
}

impl Walker<'_> {
    fn record(&mut self, source: FileId, key: &InstanceKey, kind: TraceKind) {
        if let Some(trace) = &mut self.trace {
            trace.push(TraceStep { source, key: key.clone(), kind });
        }
    }

    fn visit(&mut self, id: FileId, key: InstanceKey) {
        self.db.unwind_if_revision_cancelled();
        let Some(&file) = self.input.files(self.db).get(&id) else { return };
        if self.stack.len() >= 128
            || self.stack.iter().any(|state| {
                state.file == id && state.symbols == self.symbols && state.targets == self.targets
            })
        {
            self.record(id, &key, TraceKind::Cycle);
            let start = self.stack.iter().position(|state| state.file == id).unwrap_or(0);
            for file in self.stack[start..].iter().map(|state| state.file).chain([id]) {
                if !self.plan.cycles.contains(&file) {
                    self.plan.cycles.push(file);
                }
            }
            return;
        }
        self.stack.push(VisitState {
            file: id,
            symbols: self.symbols.clone(),
            targets: self.targets.clone(),
        });
        if self.trace.is_some() {
            let mut symbols: Vec<_> = self.symbols.iter().cloned().collect();
            symbols.sort();
            let project = self.input.project(self.db);
            let target = key
                .module
                .and_then(|owner| project.module_options.get(&owner))
                .map_or_else(|| project.target.clone(), |options| options.target.clone());
            self.record(id, &key, TraceKind::Enter { symbols, target });
        }
        let primary = !self.owners.contains_key(&id);
        self.owners.entry(id).or_insert(key.module);
        let mut include_ordinal = 0;
        let mut conditions = Vec::new();
        let mut regions = vec![true];
        self.plan.order.push((key.clone(), 0));
        self.record(id, &key, TraceKind::Region { ordinal: 0, active: true });
        let overlay = self.overlay.filter(|o| o.source == id);
        let events =
            overlay.map_or_else(|| file_events(self.db, file).as_slice(), |o| o.events.as_slice());
        for (event_index, event) in events.iter().enumerate() {
            let taking = conditions.iter().all(|&b| b);
            let before = self.trace.as_ref().map(|_| self.symbols.clone());
            let trace_index = self.trace.as_ref().map(Vec::len);
            self.record(
                id,
                &key,
                TraceKind::Event {
                    ordinal: event_index,
                    active: taking,
                    following_active: taking,
                    symbols_added: Vec::new(),
                    include_target: None,
                },
            );
            if matches!(event, Event::Include { .. }) {
                include_ordinal += 1;
            }
            match event {
                Event::Ifdef(name, negative) => {
                    conditions.push(self.symbols.contains(name) != *negative)
                }
                Event::IfTarget(names) => {
                    let project = self.input.project(self.db);
                    let target = key
                        .module
                        .and_then(|owner| project.module_options.get(&owner))
                        .map_or(project.target.as_deref(), |options| options.target.as_deref());
                    conditions.push(target.is_some_and(|t| {
                        names.iter().any(|n| {
                            self.targets.get(n).map_or(n == t, |set| set.iter().any(|n| n == t))
                        })
                    }));
                }
                Event::Else => {
                    if let Some(last) = conditions.last_mut() {
                        *last = !*last;
                    }
                }
                Event::Endif => {
                    conditions.pop();
                }
                Event::Define(name) if taking => {
                    self.symbols.insert(name.clone());
                }
                Event::TargetSet(name, values) if taking => {
                    self.targets.insert(name.clone(), values.clone());
                }
                Event::Order(order) if taking => order.apply(&mut self.symbols),
                Event::Include { path, library } if taking => {
                    let project = self.input.project(self.db);
                    let from = &self.reverse[&id];
                    let candidates = include_candidates(
                        from,
                        path,
                        *library,
                        project.library.as_deref(),
                        key.module
                            .and_then(|owner| project.module_options.get(&owner))
                            .map_or(&project.include_paths, |options| &options.include_paths),
                    );
                    if let Some(target) = candidates.iter().find_map(|p| self.paths.get(p)).copied()
                    {
                        if let (Some(index), Some(trace)) = (trace_index, &mut self.trace) {
                            if let TraceKind::Event { include_target, .. } = &mut trace[index].kind
                            {
                                *include_target = Some(Ok(target));
                            }
                        }
                        if !self.plan.includes.contains(&(id, event_index, target)) {
                            self.plan.includes.push((id, event_index, target));
                        }
                        let recursive = self.stack.iter().any(|state| state.file == target);
                        let cycles = self.plan.cycles.len();
                        let mut child = key.clone();
                        child.includes.push(include_ordinal);
                        self.visit(target, child);
                        if (!recursive || self.plan.cycles.len() > cycles)
                            && !self.plan.edges.contains(&(id, target))
                        {
                            self.plan.edges.push((id, target));
                        }
                    } else {
                        if let (Some(index), Some(trace)) = (trace_index, &mut self.trace) {
                            if let TraceKind::Event { include_target, .. } = &mut trace[index].kind
                            {
                                *include_target = Some(Err(candidates
                                    .first()
                                    .cloned()
                                    .unwrap_or_else(|| path.into())));
                            }
                        }
                        self.plan.missing.push((
                            id,
                            event_index,
                            candidates.first().cloned().unwrap_or_else(|| path.into()),
                        ));
                    }
                }
                _ => {}
            }
            regions.push(conditions.iter().all(|&b| b));
            if let (Some(before), Some(index), Some(trace)) = (before, trace_index, &mut self.trace)
            {
                let mut added: Vec<_> = self.symbols.difference(&before).cloned().collect();
                added.sort();
                if let TraceKind::Event { symbols_added, following_active, .. } =
                    &mut trace[index].kind
                {
                    *symbols_added = added;
                    *following_active = *regions.last().unwrap();
                }
            }
            self.plan.order.push((key.clone(), event_index + 1));
            self.record(
                id,
                &key,
                TraceKind::Region { ordinal: event_index + 1, active: *regions.last().unwrap() },
            );
        }
        if primary {
            self.plan.regions.insert(id, regions.clone());
        }
        let content = overlay
            .map_or_else(|| content_regions(self.db, file).as_slice(), |o| o.content.as_slice());
        let active = content.iter().zip(&regions).any(|(content, taken)| *content && *taken);
        self.record(id, &key, TraceKind::Exit);
        self.plan.instances.push(Instance { key, source: id, primary, active, regions });
        if !self.plan.files.contains(&id) {
            self.plan.files.push(id);
        }
        self.stack.pop();
    }
}

/// Candidate discovery reads all syntactically named includes, including
/// inactive ones. Only `workspace_plan` decides which loaded files are semantic
/// inputs. Missing inactive files are harmless and can appear in a later scan.
pub fn discover_candidates(
    roots: &[PathBuf],
    library: Option<&std::path::Path>,
    search: &[PathBuf],
    mut read: impl FnMut(&std::path::Path) -> Option<String>,
) -> Vec<PathBuf> {
    let mut pending: Vec<_> = roots.iter().rev().map(|p| normalize_path(p)).collect();
    if let Some(library) = library {
        pending.push(normalize_path(&library.join("core/corelib.sail")));
    }
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    while let Some(path) = pending.pop() {
        if !seen.insert(path.clone()) {
            continue;
        }
        let Some(text) = read(&path) else { continue };
        let mut children = Vec::new();
        // Discovery is a conservative I/O candidate pass, not semantic
        // preprocessing. Lexing excludes comments/strings and keeps malformed
        // or inactive source available without building full generated CSTs.
        for (token, span) in parser::tokenize(&text) {
            if !matches!(token, parser::Token::Directive { .. }) {
                continue;
            }
            if let Some(Event::Include { path: include, library: is_lib }) =
                Event::from_directive(&text[span.start..span.end])
            {
                if let Some(child) = include_candidates(&path, &include, is_lib, library, search)
                    .into_iter()
                    .find(|p| read(p).is_some())
                {
                    children.push(child);
                }
            }
        }
        result.push(path);
        pending.extend(children.into_iter().rev());
    }
    result
}

#[cfg(test)]
mod discovery_tests {
    use super::*;

    #[test]
    fn ordinary_content_fast_path_agrees_with_cst_projection() {
        for text in [
            "function f(x : int) -> int = x\n",
            "type T = struct { field : bits(8) }\n",
            "function unfinished(\n",
            "/* comment */ val x : int\n",
            "let text = \"🙂\"\r\n",
        ] {
            assert!(!may_have_events(text));
            assert!(!parser::tokenize(text).is_empty());
            let root = syntax::parse_text(text).0;
            assert!(root.children().any(|n| Event::from_node(&n).is_none()));
        }
        for text in ["$include \"a.sail\"", "default Order dec", "let s = \"$include\""] {
            assert!(may_have_events(text));
        }
    }

    #[test]
    fn discovery_lexes_real_includes_without_parsing_generated_function_bodies() {
        let root = std::env::temp_dir().join("sail-discovery-lexical");
        let main = root.join("main.sail");
        let child = root.join("child.sail");
        let sources = HashMap::from([
            (main.clone(), "/* $include \"comment.sail\" */\nlet s = \"$include fake\"\n$ifdef DISABLED\n$include \"child.sail\" // discover inactive candidates too\n$endif\nfunction unfinished(\n".to_owned()),
            (child.clone(), "val child : int\n".to_owned()),
        ]);
        let found = discover_candidates(std::slice::from_ref(&main), None, &[], |path| {
            sources.get(path).cloned()
        });
        assert_eq!(found, [main, child]);
    }
}
