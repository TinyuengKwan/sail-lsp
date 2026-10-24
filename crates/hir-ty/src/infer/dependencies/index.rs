//! Immutable file contributions and a persistent declaration directory.
//! Directory branches share unchanged subtrees across revisions/snapshots.
//! Bloom masks only prune candidates; exact leaf membership decides lookup.
use super::*;
use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Contribution {
    symbol: Arc<Symbol<WorkspaceContext>>,
    /// A val/outcome appends even if another file already supplies a signature.
    /// A bare function inserts only when absent. Keep both cases exactly.
    append_functions: Vec<Arc<TypeScheme>>,
}

#[salsa::tracked(returns(ref))]
fn file_contributions(db: &dyn salsa::Database, file: FileText) -> HashMap<String, Contribution> {
    let _span = tracing::info_span!("file_contributions").entered();
    let env = crate::query::top_level_env(db, file);
    let items = hir_def::def_query::file_item_tree(db, file);
    let mut context = WorkspaceContext::default();
    context.merge_file_from_queries(&env.0, items.as_ref(), file);
    context.compute_arities();
    let mut symbols = context.symbols();
    let local = file_symbols(db, file);
    for name in context.symbol_index.entries.keys() {
        if let Some(value) = local.get(name) {
            symbols
                .entry(name.clone())
                .or_default()
                .references
                .extend(value.references.iter().cloned());
        }
    }
    let mut out: HashMap<_, _> = symbols
        .into_iter()
        .map(|(name, symbol)| {
            (name, Contribution { symbol: Arc::new(symbol), append_functions: Vec::new() })
        })
        .collect();
    if let Some(items) = items {
        use hir_def::item_tree::DefKind;
        for &id in items.top_level_items() {
            if matches!(id.def_kind(items), DefKind::ValSpec | DefKind::Outcome) {
                let name = id.name(items).as_str();
                if let (Some(contribution), Some(schemes)) =
                    (out.get_mut(name), env.0.env.functions.get(name))
                {
                    contribution.append_functions.extend(schemes.iter().cloned());
                }
            }
        }
    }
    out
}

#[salsa::tracked(returns(ref))]
fn contribution(db: &dyn salsa::Database, file: FileText, name: Name) -> Option<Contribution> {
    file_contributions(db, file).get(name.as_str()).cloned()
}

#[derive(Debug)]
struct Directory {
    mask: [u64; 4],
    node: Node,
    // Pure memoization over this immutable directory, never database state.
    // New directory contents get a fresh cache; snapshots retain their own.
    lookups: std::sync::RwLock<HashMap<Name, Arc<[FileText]>>>,
}
impl PartialEq for Directory {
    fn eq(&self, other: &Self) -> bool {
        self.mask == other.mask && self.node == other.node
    }
}
impl Eq for Directory {}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Leaf(FileText, HashSet<String>),
    Branch(Arc<Directory>, Arc<Directory>),
    Batch(Vec<Arc<Directory>>),
    Empty,
}
fn name_bits(name: &str) -> [u64; 4] {
    let mut hash = rustc_hash::FxHasher::default();
    name.hash(&mut hash);
    let h = hash.finish();
    std::array::from_fn(|i| 1u64 << ((h >> (i * 16)) & 63))
}
impl Directory {
    fn lookup(&self, name: &Name) -> Arc<[FileText]> {
        if let Some(files) = self.lookups.read().unwrap().get(name) {
            return files.clone();
        }
        let mut files = Vec::new();
        self.locations(name, &name_bits(name), &mut files);
        let files: Arc<[FileText]> = files.into();
        let mut cache = self.lookups.write().unwrap();
        // Bound caches for names introduced by repeated edits.
        if cache.len() < 16384 {
            cache.insert(name.clone(), files.clone());
        }
        files
    }
    fn locations(&self, name: &str, bits: &[u64; 4], out: &mut Vec<FileText>) {
        if self.mask.iter().zip(bits).any(|(mask, bit)| mask & bit == 0) {
            return;
        }
        match &self.node {
            Node::Leaf(file, names) => {
                if names.contains(name) {
                    out.push(*file);
                }
            }
            Node::Branch(left, right) => {
                left.locations(name, bits, out);
                right.locations(name, bits, out);
            }
            Node::Batch(files) => {
                for file in files {
                    file.locations(name, bits, out);
                }
            }
            Node::Empty => {}
        }
    }
}
#[salsa::tracked(returns(ref))]
fn file_directory(db: &dyn salsa::Database, file: FileText) -> Arc<Directory> {
    let names: HashSet<_> = file_contributions(db, file).keys().cloned().collect();
    let mut mask = [0; 4];
    for name in &names {
        for (dest, bit) in mask.iter_mut().zip(name_bits(name)) {
            *dest |= bit;
        }
    }
    Arc::new(Directory { mask, node: Node::Leaf(file, names), lookups: Default::default() })
}
#[salsa::tracked(returns(ref))]
fn directory(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    start: usize,
    len: usize,
) -> Arc<Directory> {
    match len {
        0 => Arc::new(Directory { mask: [0; 4], node: Node::Empty, lookups: Default::default() }),
        1 => file_directory(db, ws.file_texts(db)[start]).clone(),
        2..=32 => {
            let leaves: Vec<_> = ws.file_texts(db)[start..start + len]
                .iter()
                .map(|&f| file_directory(db, f).clone())
                .collect();
            let mut mask = [0; 4];
            for leaf in &leaves {
                for (m, b) in mask.iter_mut().zip(leaf.mask) {
                    *m |= b;
                }
            }
            Arc::new(Directory { mask, node: Node::Batch(leaves), lookups: Default::default() })
        }
        _ => {
            let mid = len / 2;
            let left = directory(db, ws, start, mid).clone();
            let right = directory(db, ws, start + mid, len - mid).clone();
            let mask = std::array::from_fn(|i| left.mask[i] | right.mask[i]);
            Arc::new(Directory {
                mask,
                node: Node::Branch(left, right),
                lookups: Default::default(),
            })
        }
    }
}
#[salsa::tracked(returns(ref))]
fn symbol_locations(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    name: Name,
) -> Vec<FileText> {
    let _span = tracing::info_span!("symbol_locations").entered();
    let root = directory(db, ws, 0, ws.file_texts(db).len());
    root.lookup(&name).to_vec()
}

#[salsa::tracked(returns(ref))]
pub(super) fn workspace_symbol(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    name: Name,
) -> Option<Arc<Symbol<WorkspaceContext>>> {
    scoped_workspace_symbol(db, ws, None, name).clone()
}

#[salsa::tracked(returns(ref))]
pub(super) fn scoped_workspace_symbol(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    from: Option<base_db::FileId>,
    name: Name,
) -> Option<Arc<Symbol<WorkspaceContext>>> {
    let _span = tracing::info_span!("workspace_symbol").entered();
    let files: Vec<_> = symbol_locations(db, ws, name.clone())
        .iter()
        .copied()
        .filter(|&file| {
            from.is_none_or(|from| {
                hir_def::visibility::symbol_visible_from(db, from, file, name.as_str())
            })
        })
        .collect();
    let (&first, rest) = files.split_first()?;
    let first = contribution(db, first, name.clone()).as_ref()?;
    if rest.is_empty() {
        return Some(first.symbol.clone());
    }
    let mut symbol = (*first.symbol).clone();
    for &file in rest {
        db.unwind_if_revision_cancelled();
        if let Some(next) = contribution(db, file, name.clone()) {
            merge(&mut symbol.data, next);
            symbol.references.extend(next.symbol.references.iter().cloned());
        }
    }
    symbol.data.compute_arities();
    Some(Arc::new(symbol))
}

/// Signatures retain the meaning of aliases resolved in their declaring
/// module. This support data is not exported as visible names or functions.
#[salsa::tracked(returns(ref))]
fn signature_support(db: &dyn salsa::Database, owner: FileText, name: Name) -> WorkspaceContext {
    let mut support = WorkspaceContext::default();
    let Some(ws) = base_db::WorkspaceFiles::try_get(db) else { return support };
    let Some(root) = contribution(db, owner, name) else { return support };
    let mut pending: Vec<_> = root.symbol.references.iter().cloned().map(|n| (owner, n)).collect();
    let mut visited = HashSet::new();
    while let Some((from, name)) = pending.pop() {
        if !visited.insert((from, name.clone())) {
            continue;
        }
        db.unwind_if_revision_cancelled();
        for &target in symbol_locations(db, ws, name.clone()) {
            if !hir_def::visibility::symbol_visible_from(
                db,
                from.file_id(db),
                target,
                name.as_str(),
            ) {
                continue;
            }
            let Some(value) = contribution(db, target, name.clone()) else { continue };
            let data = &value.symbol.data;
            if data.type_aliases.is_empty()
                && data.alias_schemes.is_empty()
                && data.constraint_synonyms.is_empty()
                && data.cross_file_records.is_empty()
                && data.cross_file_bitfields.is_empty()
            {
                continue;
            }
            macro_rules! copy_types { ($($field:ident),*) => {$(
                for (key, value) in &data.$field { support.$field.entry(key.clone()).or_insert_with(|| value.clone()); }
            )*}; }
            copy_types!(
                type_aliases,
                alias_schemes,
                constraint_synonyms,
                cross_file_records,
                cross_file_bitfields
            );
            pending.extend(value.symbol.references.iter().cloned().map(|n| (target, n)));
        }
    }
    support
}

#[salsa::tracked(returns(ref))]
fn contribution_selection(
    db: &dyn salsa::Database,
    file: FileText,
    names: Names,
    project_scope: bool,
) -> Vec<(Name, Contribution)> {
    let symbols = file_contributions(db, file);
    names
        .into_iter()
        .filter_map(|name| symbols.get(name.as_str()).cloned().map(|mut symbol| {
            if project_scope {
                let support = signature_support(db, file, name.clone());
                if !support.type_aliases.is_empty() || !support.alias_schemes.is_empty()
                    || !support.constraint_synonyms.is_empty() || !support.cross_file_records.is_empty()
                    || !support.cross_file_bitfields.is_empty() {
                    let data = &mut Arc::make_mut(&mut symbol.symbol).data;
                    macro_rules! copy_types { ($($field:ident),*) => {$(
                        for (key, value) in &support.$field { data.$field.entry(key.clone()).or_insert_with(|| value.clone()); }
                    )*}; }
                    copy_types!(type_aliases, alias_schemes, constraint_synonyms, cross_file_records, cross_file_bitfields);
                }
            }
            (name, symbol)
        }))
        .collect()
}

/// Resolve a batch against the immutable directory, then depend on one
/// semantic projection per contributing file. Missing names depend on the
/// directory; unrelated signature changes leave both dependencies unchanged.
pub(super) fn select_workspace(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    roots: Names,
    from: base_db::FileId,
) -> Selection<WorkspaceContext> {
    select_workspace_excluding(db, ws, roots, from, None)
}

/// An inline program supplies its own declaration-point local contributions.
/// Its whole-file summaries must never re-enter through the workspace fallback.
pub(super) fn select_workspace_excluding(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    roots: Names,
    from: base_db::FileId,
    excluded: Option<FileText>,
) -> Selection<WorkspaceContext> {
    let _span = tracing::info_span!("workspace_selection").entered();
    let root = directory(db, ws, 0, ws.file_texts(db).len());
    let mut selected = Selection { symbols: Vec::new(), names: Names::new() };
    let mut pending = roots;
    while !pending.is_empty() {
        let mut located = Vec::new();
        let mut groups: HashMap<FileText, Names> = HashMap::new();
        for name in std::mem::take(&mut pending) {
            if !selected.names.insert(name.clone()) {
                continue;
            }
            db.unwind_if_revision_cancelled();
            mapping_base(&name, &mut pending);
            let files: Arc<[FileText]> = root
                .lookup(&name)
                .iter()
                .copied()
                .filter(|&file| {
                    Some(file) != excluded
                        && hir_def::visibility::symbol_visible_from(db, from, file, name.as_str())
                })
                .collect::<Vec<_>>()
                .into();
            for &file in files.iter() {
                groups.entry(file).or_default().insert(name.clone());
            }
            if !files.is_empty() {
                located.push((name, files));
            }
        }
        let mut contributions = HashMap::new();
        for (file, names) in groups {
            for (name, symbol) in contribution_selection(
                db,
                file,
                names,
                base_db::module_graph::file_scope(db, from).is_some(),
            ) {
                contributions.insert((file, name.clone()), symbol);
            }
        }
        for (name, files) in located {
            let first = contributions[&(files[0], name.clone())];
            let value = if files.len() == 1 {
                first.symbol.clone()
            } else {
                let mut symbol = (*first.symbol).clone();
                for &file in &files[1..] {
                    let next = contributions[&(file, name.clone())];
                    merge(&mut symbol.data, next);
                    symbol.references.extend(next.symbol.references.iter().cloned());
                }
                symbol.data.compute_arities();
                Arc::new(symbol)
            };
            pending
                .extend(value.references.iter().filter(|n| !selected.names.contains(*n)).cloned());
            selected.symbols.push((name, value));
        }
    }
    selected.symbols.sort_by(|a, b| a.0.cmp(&b.0));
    selected
}

fn merge(target: &mut WorkspaceContext, next: &Contribution) {
    let src = &next.symbol.data;
    macro_rules! first { ($($field:ident),*) => {$(
        for (k,v) in &src.$field { target.$field.entry(k.clone()).or_insert_with(|| v.clone()); }
    )*}; }
    first!(
        type_aliases,
        alias_schemes,
        constraint_synonyms,
        cross_file_records,
        cross_file_bitfields
    );
    for (k, v) in &src.overloads {
        target.overloads.entry(k.clone()).or_default().extend(v.iter().cloned());
    }
    for (k, v) in src.cross_file_function_schemes.iter() {
        match Arc::make_mut(&mut target.cross_file_function_schemes).entry(k.clone()) {
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(v.clone());
            }
            std::collections::hash_map::Entry::Occupied(mut e) => {
                e.get_mut().extend(next.append_functions.iter().cloned());
            }
        }
    }
    macro_rules! append { ($($field:ident),*) => {$(
        for (k,v) in src.$field.iter() { Arc::make_mut(&mut target.$field).entry(k.clone()).or_default().extend(v.iter().cloned()); }
    )*}; }
    append!(cross_file_constructor_schemes, cross_file_mapping_schemes);
    macro_rules! members { ($($field:ident),*) => {$(
        for (k,v) in &src.$field {
            let dest = target.$field.entry(k.clone()).or_default();
            for value in v { if !dest.contains(value) { dest.push(value.clone()); } }
        }
    )*}; }
    members!(enums, unions);
    merge_maps!(target, src, known_field_names, cross_file_pattern_constants, scattered_open_types);
    merge_arc_sets!(
        target,
        src,
        cross_file_function_names,
        cross_file_constructor_names,
        cross_file_value_names,
        cross_file_register_names
    );
    if !src.symbol_index.entries.is_empty() || !src.symbol_index.types.is_empty() {
        let index = Arc::make_mut(&mut target.symbol_index);
        for (k, v) in &src.symbol_index.entries {
            index.entries.entry(k.clone()).or_insert(*v);
        }
        for (k, v) in &src.symbol_index.types {
            index.types.entry(k.clone()).or_insert(*v);
        }
    }
}

#[salsa::tracked(returns(ref))]
fn file_function_names(db: &dyn salsa::Database, file: FileText) -> Names {
    file_contributions(db, file)
        .values()
        .flat_map(|c| c.symbol.data.cross_file_function_names.iter().map(|n| Name::new(n)))
        .collect()
}
#[salsa::tracked(returns(ref))]
pub(super) fn function_names(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    from: base_db::FileId,
) -> Names {
    ws.file_texts(db)
        .iter()
        .flat_map(|&file| {
            file_function_names(db, file)
                .iter()
                .filter(move |n| {
                    hir_def::visibility::symbol_visible_from(db, from, file, n.as_str())
                })
                .cloned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use salsa::Setter;
    #[salsa::db]
    #[derive(Default, Clone)]
    struct Db {
        storage: salsa::Storage<Self>,
    }
    #[salsa::db]
    impl salsa::Database for Db {}
    fn file(db: &Db, id: u32, text: &str) -> FileText {
        FileText::new(db, Arc::from(text), base_db::FileId::from_raw(id))
    }
    fn compare_sequential(db: &Db, ws: base_db::WorkspaceFiles) {
        let expected = crate::query::workspace_context(db, ws).0.symbols();
        let mut names: HashSet<String> = expected.keys().cloned().collect();
        for &file in ws.file_texts(db) {
            names.extend(file_contributions(db, file).keys().cloned());
        }
        for name in names {
            let actual = workspace_symbol(db, ws, Name::new(&name));
            assert_eq!(
                actual.as_ref().map(|s| &s.data).cloned().unwrap_or_default(),
                expected.get(&name).map(|s| s.data.clone()).unwrap_or_default(),
                "merge changed {name}"
            );
        }
    }
    #[test]
    fn contributions_preserve_ordered_workspace_merging() {
        let mut db = Db::default();
        let a = file(&db, 0, "val f : int -> int\nstruct R = { field : int }\nenum E = { A }\nunion U = { C : int }\noverload choose = {f}\n");
        let b = file(&db, 1, "function f(x : int) -> int = x\nval g : bool -> bool\noverload choose = {g}\nstruct R = { field : bool }\n");
        // Body before val is intentional: appending a file summary must not
        // append the insert-if-absent signature when f already exists.
        let c = file(&db, 2, "function f(x : bool) -> bool = x\nval f : bool -> bool\nscattered union U\nunion clause U = D : bool\nscattered enum E\nenum clause E = B\nval m : int <-> bool\n");
        for files in [vec![a, b, c], vec![c, b, a], vec![a, c], vec![c, a, b]] {
            hir_def::def_query::set_workspace_files(&mut db, files);
            let ws = base_db::WorkspaceFiles::try_get(&db).unwrap();
            compare_sequential(&db, ws);
        }
        b.set_text(&mut db).to(Arc::from("val g : int -> bool\noverload choose = {g,f}\nstruct R = { other : string }\nval m : string <-> bool\n"));
        compare_sequential(&db, base_db::WorkspaceFiles::try_get(&db).unwrap());
    }
    #[test]
    fn directory_and_symbol_storage_survive_unrelated_edits() {
        let mut db = Db::default();
        let files: Vec<_> = ["f", "g", "h", "j"]
            .iter()
            .enumerate()
            .map(|(i, n)| file(&db, i as u32, &format!("val {n} : unit -> int")))
            .collect();
        hir_def::def_query::set_workspace_files(&mut db, files.clone());
        let ws = base_db::WorkspaceFiles::try_get(&db).unwrap();
        let old_root = directory(&db, ws, 0, 4).clone();
        let old_h = workspace_symbol(&db, ws, Name::new("h")).clone().unwrap();
        let old_f = workspace_symbol(&db, ws, Name::new("f")).clone().unwrap();
        assert!(workspace_symbol(&db, ws, Name::new("renamed")).is_none());
        files[0].set_text(&mut db).to(Arc::from("val f : unit -> bool"));
        assert!(
            Arc::ptr_eq(&old_root, directory(&db, ws, 0, 4)),
            "signature edit rebuilt the name directory"
        );
        assert!(Arc::ptr_eq(&old_h, workspace_symbol(&db, ws, Name::new("h")).as_ref().unwrap()));
        assert!(!Arc::ptr_eq(&old_f, workspace_symbol(&db, ws, Name::new("f")).as_ref().unwrap()));
        files[0].set_text(&mut db).to(Arc::from("val renamed : unit -> bool"));
        let root = directory(&db, ws, 0, 4);
        let (Node::Batch(old_leaves), Node::Batch(leaves)) = (&old_root.node, &root.node) else {
            panic!("missing batch")
        };
        assert!(Arc::ptr_eq(&old_leaves[2], &leaves[2]), "rename rebuilt an untouched subtree");
        let mut old_locations = Vec::new();
        old_root.locations("f", &name_bits("f"), &mut old_locations);
        assert_eq!(old_locations, vec![files[0]], "retained immutable directory was changed");
        assert!(workspace_symbol(&db, ws, Name::new("f")).is_none());
        assert!(workspace_symbol(&db, ws, Name::new("renamed")).is_some());
        assert!(Arc::ptr_eq(&old_h, workspace_symbol(&db, ws, Name::new("h")).as_ref().unwrap()));
        // A saturated mask must still consult exact leaf membership.
        let saturated = Directory {
            mask: [u64::MAX; 4],
            node: Node::Leaf(files[0], HashSet::from(["present".to_owned()])),
            lookups: Default::default(),
        };
        let mut absent = Vec::new();
        saturated.locations("absent", &name_bits("absent"), &mut absent);
        assert!(absent.is_empty());
    }
}
