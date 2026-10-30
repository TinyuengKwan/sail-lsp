//! Pure preprocessing over loaded VFS inputs. No filesystem access in queries.
use base_db::{
    preprocess::{InstanceKey, WorkspaceSourceProject},
    FileId, FileText,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use syntax::preprocess::{include_candidates, normalize_path, Event};

#[salsa::tracked(returns(ref))]
pub fn file_events(db: &dyn salsa::Database, file: FileText) -> Vec<Event> {
    syntax::parse_query::parse_file(db, file)
        .syntax_node()
        .into_iter()
        .flat_map(|root| root.children().filter_map(|n| Event::from_node(&n)).collect::<Vec<_>>())
        .collect()
}

/// Structural projection: inactive guarded visits need no semantic view.
/// Function text and offsets deliberately do not participate in equality.
#[salsa::tracked(returns(ref))]
fn content_regions(db: &dyn salsa::Database, file: FileText) -> Vec<bool> {
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
    regions
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
    for (index, &root) in project.roots.iter().enumerate() {
        let owner = project.root_modules.get(index).copied().unwrap_or_else(|| {
            project.modules.modules.iter().position(|m| m.files.contains(&root))
        });
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
    walk.plan
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
}

struct VisitState {
    file: FileId,
    symbols: HashSet<String>,
    targets: HashMap<String, Vec<String>>,
}

impl Walker<'_> {
    fn visit(&mut self, id: FileId, key: InstanceKey) {
        self.db.unwind_if_revision_cancelled();
        let Some(&file) = self.input.files(self.db).get(&id) else { return };
        if self.stack.len() >= 128
            || self.stack.iter().any(|state| {
                state.file == id && state.symbols == self.symbols && state.targets == self.targets
            })
        {
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
        let primary = !self.owners.contains_key(&id);
        self.owners.entry(id).or_insert(key.module);
        let mut include_ordinal = 0;
        let mut conditions = Vec::new();
        let mut regions = vec![true];
        self.plan.order.push((key.clone(), 0));
        let overlay = self.overlay.filter(|o| o.source == id);
        let events =
            overlay.map_or_else(|| file_events(self.db, file).as_slice(), |o| o.events.as_slice());
        for (event_index, event) in events.iter().enumerate() {
            let taking = conditions.iter().all(|&b| b);
            if matches!(event, Event::Include { .. }) {
                include_ordinal += 1;
            }
            match event {
                Event::Ifdef(name, negative) => {
                    conditions.push(self.symbols.contains(name) != *negative)
                }
                Event::IfTarget(names) => {
                    let target = self.input.project(self.db).target.as_deref();
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
                        &project.include_paths,
                    );
                    if let Some(target) = candidates.iter().find_map(|p| self.paths.get(p)).copied()
                    {
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
            self.plan.order.push((key.clone(), event_index + 1));
        }
        if primary {
            self.plan.regions.insert(id, regions.clone());
        }
        let content = overlay
            .map_or_else(|| content_regions(self.db, file).as_slice(), |o| o.content.as_slice());
        let active = content.iter().zip(&regions).any(|(content, taken)| *content && *taken);
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
        let (root, _) = syntax::parse_text(&text);
        let mut children = Vec::new();
        for node in root.children() {
            if let Some(Event::Include { path: include, library: is_lib }) = Event::from_node(&node)
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
