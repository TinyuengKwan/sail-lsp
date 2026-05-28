//! Name resolution context with scope chain.
//! Resolves a bare identifier by walking a chain of scopes from
//! innermost to outermost:
//!
//!   1. Expression scopes (let/var/match/foreach bindings)
//!   2. Block/module scope (file-level definitions via [`DefMap`])
//!   3. Workspace scope (cross-file definitions)

use base_db::FileId;

use crate::item_id::{FunctionId, LetId, MappingId, RegisterId, TypeDefId, ValSpecId};
use crate::item_tree::ItemKind;
use crate::name::Name;
use crate::nameres::{DefId, DefMap};
use crate::per_ns::PerNs;
use crate::workspace_def_map::WorkspaceDefMap;
use std::collections::HashMap;

/// Resolution in the type namespace.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeNs {
    /// A struct, union, enum, bitfield, or newtype definition.
    AdtId(TypeDefId),
    /// A type alias definition.
    TypeAliasId(TypeDefId),
    /// A built-in type (int, bool, bits, etc.).
    BuiltinType(Name),
    /// Cross-file resolution (name only, not yet resolved).
    Workspace(Name),
}

/// Result of resolving a path in the value namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveValueResult {
    ValueNs(ValueNs),
}

/// Resolution in the value namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueNs {
    /// Local variable binding (from let/var/match/foreach).
    /// Keeps DefId since locals are expression-scoped, not module-level.
    LocalBinding(DefId),
    /// A function definition (or function clause).
    FunctionId(FunctionId),
    /// Multiple functions/clauses with the same name (overloading).
    FunctionIds(Vec<FunctionId>),
    /// A val spec (type signature only).
    ValSpecId(ValSpecId),
    /// A register definition.
    RegisterId(RegisterId),
    /// A mapping definition.
    MappingId(MappingId),
    /// A let/var binding at top level.
    LetId(LetId),
    /// An enum/union variant (constructor).
    EnumVariantId(Name),
    /// Cross-file workspace resolution (name only).
    Workspace(Name),
}

/// Combined-namespace resolution result (legacy API).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Resolved to a single local binding (let/var/match arm).
    Def(DefId),
    /// Resolved to one or more module-level definitions.
    Defs(Vec<crate::item_id::ModuleDefId>),
    /// Resolved at workspace level (cross-file).
    Workspace(String),
    /// The name is not known in this resolver's scope.
    Unresolved,
}

/// A single layer in the scope chain.
#[derive(Debug)]
enum Scope<'db> {
    /// All items and imported names of a module/file.
    Block { def_map: &'db DefMap },
    /// Local bindings from let/var/match/foreach.
    Expr { bindings: HashMap<Name, DefId> },
    /// Workspace-wide definitions from all files.
    Workspace { def_map: &'db WorkspaceDefMap },
}

/// Check if a name is a Sail built-in type.
fn is_builtin_type(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "nat"
            | "bool"
            | "unit"
            | "string"
            | "real"
            | "bit"
            | "bits"
            | "vector"
            | "list"
            | "option"
            | "result"
            | "range"
            | "atom"
            | "atom_bool"
            | "implicit"
    )
}

/// Scope-chain resolver: Expr > Block > Workspace.
#[derive(Debug)]
pub struct Resolver<'db> {
    scopes: Vec<Scope<'db>>,
    /// Source file for `@private` visibility filtering.
    from_file: Option<FileId>,
}

impl<'db> Resolver<'db> {
    /// Create a resolver with a single block/module scope.
    pub fn new(def_map: &'db DefMap) -> Self {
        Self { scopes: vec![Scope::Block { def_map }], from_file: None }
    }

    /// Create a resolver with a module scope. Alias for `new`.
    pub fn for_file(def_map: &'db DefMap) -> Self {
        Self::new(def_map)
    }

    /// Create a resolver for a specific file, setting `from_file` for
    /// visibility filtering.
    pub fn new_for_file(def_map: &'db DefMap, file_id: base_db::FileId) -> Self {
        Self { scopes: vec![Scope::Block { def_map }], from_file: Some(file_id) }
    }

    /// Try to access the underlying DefMap, if any block scope exists.
    pub fn try_def_map(&self) -> Option<&'db DefMap> {
        self.scopes.iter().find_map(|s| match s {
            Scope::Block { def_map } => Some(*def_map),
            _ => None,
        })
    }

    /// Create a resolver with block scope + workspace scope.
    /// Names not found in the current file fall back to the workspace.
    pub fn for_file_in_workspace(def_map: &'db DefMap, workspace: &'db WorkspaceDefMap) -> Self {
        Self {
            scopes: vec![Scope::Workspace { def_map: workspace }, Scope::Block { def_map }],
            from_file: None,
        }
    }

    /// Create a resolver scoped to an `$include` analysis scope.
    pub fn for_file_in_scope(def_map: &'db DefMap, scoped_workspace: &'db WorkspaceDefMap) -> Self {
        Self {
            scopes: vec![Scope::Workspace { def_map: scoped_workspace }, Scope::Block { def_map }],
            from_file: None,
        }
    }

    /// Set the file from which this resolver is operating.
    /// Enables `@private` visibility filtering.
    pub fn with_from_file(mut self, file_id: FileId) -> Self {
        self.from_file = Some(file_id);
        self
    }

    fn scopes(&self) -> impl Iterator<Item = &Scope<'db>> {
        self.scopes.iter().rev()
    }

    /// Push a fresh expression scope.
    pub fn push_expr_scope(&mut self) {
        self.scopes.push(Scope::Expr { bindings: HashMap::new() });
    }

    /// Pop the innermost scope. Panics if only the block scope remains.
    pub fn pop_scope(&mut self) {
        assert!(self.scopes.len() > 1, "cannot pop the module scope");
        self.scopes.pop();
    }

    /// Add a binding to the innermost expression scope.
    pub fn add_binding(&mut self, name: Name, def: DefId) {
        match self.scopes.last_mut() {
            Some(Scope::Expr { bindings }) => {
                bindings.insert(name, def);
            }
            _ => panic!("add_binding called without an active expression scope"),
        }
    }

    /// Resolve `name` by walking scopes from innermost to outermost.
    /// Priority: Expr > Block (current file) > Workspace.
    ///
    /// This is the combined-namespace legacy API. New code should prefer
    /// `resolve_path_in_type_ns` / `resolve_path_in_value_ns`.
    pub fn resolve_name(&self, name: &str) -> Resolution {
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::Expr { bindings } => {
                    if let Some(&def) = bindings.get(name) {
                        return Resolution::Def(def);
                    }
                }
                Scope::Block { def_map } => {
                    let ids = def_map.lookup_name(name);
                    if !ids.is_empty() {
                        return Resolution::Defs(ids.to_vec());
                    }
                }
                Scope::Workspace { def_map } => {
                    let visible = match self.from_file {
                        Some(fid) => def_map.contains_visible_from(name, fid),
                        None => def_map.contains(name),
                    };
                    if visible {
                        return Resolution::Workspace(name.to_string());
                    }
                }
            }
        }
        Resolution::Unresolved
    }

    /// Resolve in type namespace only.
    /// Returns typed `TypeNs` distinguishing ADT from TypeAlias.
    /// Resolve in type namespace only.
    ///
    /// Takes `db` parameter`).
    pub fn resolve_path_in_type_ns(
        &self,
        _db: &dyn crate::db::DefDatabase,
        name: &str,
    ) -> Option<TypeNs> {
        // Check builtin types first.
        if is_builtin_type(name) {
            return Some(TypeNs::BuiltinType(Name::from(name)));
        }
        let name_key = Name::from(name);
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::Block { def_map } => {
                    let per_ns = def_map.root_scope().get(&name_key);
                    if let Some(item) = per_ns.types {
                        let type_def_id = TypeDefId(item.def.as_raw());
                        let raw_id = crate::nameres::DefId(item.def.as_raw());
                        let type_ns = match def_map.kind_of(raw_id) {
                            Some(ItemKind::TypeAlias) => TypeNs::TypeAliasId(type_def_id),
                            _ => TypeNs::AdtId(type_def_id),
                        };
                        return Some(type_ns);
                    }
                }
                Scope::Workspace { def_map } => {
                    let visible = match self.from_file {
                        Some(fid) => def_map.contains_visible_from(name, fid),
                        None => def_map.contains(name),
                    };
                    if visible {
                        return Some(TypeNs::Workspace(Name::from(name)));
                    }
                }
                Scope::Expr { .. } => {} // locals are values, not types
            }
        }
        None
    }

    /// Resolve in value namespace only.
    ///
    /// Takes `db` parameter`).
    pub fn resolve_path_in_value_ns(
        &self,
        _db: &dyn crate::db::DefDatabase,
        name: &str,
    ) -> Option<ValueNs> {
        let name_key = Name::from(name);
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::Expr { bindings } => {
                    if let Some(&def) = bindings.get(name) {
                        return Some(ValueNs::LocalBinding(def));
                    }
                }
                Scope::Block { def_map } => {
                    let ids = def_map.root_scope().get_values(&name_key);
                    if !ids.is_empty() {
                        let first_raw = crate::nameres::DefId(ids[0].as_raw());
                        let first_kind = def_map.kind_of(first_raw);
                        match first_kind {
                            Some(ItemKind::Register) => {
                                return Some(ValueNs::RegisterId(RegisterId(first_raw.0)));
                            }
                            Some(ItemKind::Mapping | ItemKind::MappingSpec) => {
                                return Some(ValueNs::MappingId(MappingId(first_raw.0)));
                            }
                            Some(ItemKind::ValSpec) => {
                                return Some(ValueNs::ValSpecId(ValSpecId(first_raw.0)));
                            }
                            Some(ItemKind::Let | ItemKind::Var) => {
                                return Some(ValueNs::LetId(LetId(first_raw.0)));
                            }
                            Some(ItemKind::Function) => {
                                if ids.len() == 1 {
                                    return Some(ValueNs::FunctionId(FunctionId(first_raw.0)));
                                } else {
                                    let fn_ids: Vec<FunctionId> =
                                        ids.iter().map(|mid| FunctionId(mid.as_raw())).collect();
                                    return Some(ValueNs::FunctionIds(fn_ids));
                                }
                            }
                            _ => {
                                if ids.len() == 1 {
                                    return Some(ValueNs::FunctionId(FunctionId(first_raw.0)));
                                } else {
                                    let fn_ids: Vec<FunctionId> =
                                        ids.iter().map(|mid| FunctionId(mid.as_raw())).collect();
                                    return Some(ValueNs::FunctionIds(fn_ids));
                                }
                            }
                        }
                    }
                }
                Scope::Workspace { def_map } => {
                    let visible = match self.from_file {
                        Some(fid) => def_map.contains_visible_from(name, fid),
                        None => def_map.contains(name),
                    };
                    if visible {
                        return Some(ValueNs::Workspace(Name::from(name)));
                    }
                }
            }
        }
        None
    }

    /// Resolve in both namespaces, returning `PerNs`.
    pub fn resolve_per_ns(&self, db: &dyn crate::db::DefDatabase, name: &str) -> PerNs {
        use crate::item_id::ModuleDefId;
        use crate::per_ns::ItemInNs;
        use crate::visibility::Visibility;

        // Helper: wrap a raw DefId into a ModuleDefId using kind lookup.
        let def_map = self.try_def_map();
        let to_module_def = |raw_id: crate::nameres::DefId| -> ModuleDefId {
            let kind = def_map
                .and_then(|dm| dm.kind_of(raw_id))
                .unwrap_or(crate::item_tree::ItemKind::Function);
            ModuleDefId::from_def_id(raw_id, kind)
        };

        let types = match self.resolve_path_in_type_ns(db, name) {
            Some(TypeNs::AdtId(id)) => {
                Some(ItemInNs { def: ModuleDefId::TypeDefId(id), vis: Visibility::Public })
            }
            Some(TypeNs::TypeAliasId(id)) => {
                Some(ItemInNs { def: ModuleDefId::TypeDefId(id), vis: Visibility::Public })
            }
            _ => None,
        };
        let values = match self.resolve_path_in_value_ns(db, name) {
            Some(ValueNs::LocalBinding(id)) => {
                Some(ItemInNs { def: to_module_def(id), vis: Visibility::Public })
            }
            Some(ValueNs::FunctionId(id)) => {
                Some(ItemInNs { def: ModuleDefId::FunctionId(id), vis: Visibility::Public })
            }
            Some(ValueNs::ValSpecId(id)) => {
                Some(ItemInNs { def: ModuleDefId::ValSpecId(id), vis: Visibility::Public })
            }
            Some(ValueNs::RegisterId(id)) => {
                Some(ItemInNs { def: ModuleDefId::RegisterId(id), vis: Visibility::Public })
            }
            Some(ValueNs::MappingId(id)) => {
                Some(ItemInNs { def: ModuleDefId::MappingId(id), vis: Visibility::Public })
            }
            Some(ValueNs::LetId(id)) => {
                Some(ItemInNs { def: ModuleDefId::LetId(id), vis: Visibility::Public })
            }
            Some(ValueNs::FunctionIds(ref ids)) => ids
                .first()
                .copied()
                .map(|id| ItemInNs { def: ModuleDefId::FunctionId(id), vis: Visibility::Public }),
            _ => None,
        };
        PerNs { types, values }
    }

    /// Used by completion engine to enumerate candidates.
    pub fn names_in_scope(&self) -> Vec<Name> {
        let mut names = Vec::new();
        // Collect module-level names via item_scope() (
        // RA-aligned item_scope accessor instead of manual Block match).
        if let Some(item_scope) = self.item_scope() {
            for (name, _) in item_scope.entries() {
                names.push(name.clone());
            }
        }
        for scope in self.scopes() {
            match scope {
                Scope::Expr { bindings } => {
                    names.extend(bindings.keys().cloned());
                }
                Scope::Block { .. } => {
                    // Module-level names already collected via item_scope() above.
                }
                Scope::Workspace { def_map } => {
                    for name in def_map.defs.keys() {
                        names.push(name.clone());
                    }
                }
            }
        }
        names
    }

    pub fn item_scope(&self) -> Option<&crate::item_scope::ItemScope> {
        for scope in self.scopes() {
            if let Scope::Block { def_map } = scope {
                return Some(def_map.root_scope());
            }
        }
        None
    }

    /// Access the underlying [`DefMap`] (the block/module scope).
    pub fn def_map(&self) -> &'db DefMap {
        for scope in &self.scopes {
            if let Scope::Block { def_map } = scope {
                return def_map;
            }
        }
        unreachable!("Resolver always has a block scope")
    }

    /// Current scope depth (1 = module only, 2+ = has local scopes).
    pub fn depth(&self) -> usize {
        self.scopes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item_tree::ItemTree;
    use std::sync::Arc;

    #[salsa::db]
    #[derive(Default, Clone)]
    struct TestDb {
        storage: salsa::Storage<Self>,
    }
    #[salsa::db]
    impl salsa::Database for TestDb {}
    impl base_db::SourceDatabase for TestDb {
        fn file_text(&self, _: base_db::FileId) -> base_db::FileText {
            unimplemented!()
        }
        fn all_file_ids(&self) -> Vec<base_db::FileId> {
            unimplemented!()
        }
        fn set_file_text(&mut self, _: base_db::FileId, _: &str) {
            unimplemented!()
        }
        fn set_file_text_with_durability(
            &mut self,
            _: base_db::FileId,
            _: &str,
            _: base_db::Durability,
        ) {
            unimplemented!()
        }
        fn source_root(&self, _: base_db::SourceRootId) -> base_db::SourceRootInput {
            unimplemented!()
        }
        fn file_source_root(&self, _: base_db::FileId) -> base_db::FileSourceRootInput {
            unimplemented!()
        }
        fn set_file_source_root_with_durability(
            &mut self,
            _: base_db::FileId,
            _: base_db::SourceRootId,
            _: base_db::Durability,
        ) {
            unimplemented!()
        }
        fn set_source_root_with_durability(
            &mut self,
            _: base_db::SourceRootId,
            _: std::sync::Arc<base_db::SourceRoot>,
            _: base_db::Durability,
        ) {
            unimplemented!()
        }
    }
    impl hir_expand::db::ExpandDatabase for TestDb {
        fn include_paths(&self, input: base_db::FileText) -> &[String] {
            crate::def_query::include_paths(self, input)
        }
    }

    impl crate::db::DefDatabase for TestDb {
        fn file_item_tree(&self, input: base_db::FileText) -> Option<&std::sync::Arc<ItemTree>> {
            crate::def_query::file_item_tree(self, input).as_ref()
        }
        fn callable_bodies(
            &self,
            input: base_db::FileText,
        ) -> Option<&crate::bodies::CallableBodies> {
            crate::def_query::callable_bodies(self, input).as_ref().map(|b| b.0.as_ref())
        }
        fn def_map(&self, input: base_db::FileText) -> Option<&DefMap> {
            crate::def_query::crate_def_map(self, input).as_ref().map(|d| d.0.as_ref())
        }
        fn callgraph(&self, input: base_db::FileText) -> Option<&crate::callgraph::CallGraph> {
            crate::def_query::callgraph(self, input).as_ref().map(|cg| cg.0.as_ref())
        }
        fn file_def_with_body_ids<'db>(
            &'db self,
            input: base_db::FileText,
        ) -> &'db [crate::def_query::DefWithBodyId<'db>] {
            crate::def_query::file_def_with_body_ids(self, input)
        }
        fn body_with_source_map<'db>(
            &'db self,
            id: crate::def_query::DefWithBodyId<'db>,
        ) -> &'db crate::def_query::ArcBodyWithSourceMap {
            crate::def_query::body_with_source_map(self, id)
        }
    }

    fn test_db() -> TestDb {
        TestDb::default()
    }

    fn build_def_map(source: &str) -> Arc<DefMap> {
        let (root, _) = syntax::parse_text(source);
        let it = Arc::new(ItemTree::build_from_cst(&root));
        Arc::new(DefMap::build(&it))
    }

    #[test]
    fn unknown_name_is_unresolved() {
        let dm = build_def_map("function f() = 0\n");
        let r = Resolver::new(&dm);
        assert_eq!(r.resolve_name("g"), Resolution::Unresolved);
    }

    #[test]
    fn known_function_resolves_to_defs() {
        let dm = build_def_map("function f() = 0\n");
        let r = Resolver::new(&dm);
        match r.resolve_name("f") {
            Resolution::Defs(ids) => assert_eq!(ids.len(), 1),
            other => panic!("expected Defs, got {:?}", other),
        }
    }

    #[test]
    fn function_clauses_resolve_to_multiple_def_ids() {
        let dm = build_def_map(
            "\
val pick : int -> int
function clause pick(0) = 1
function clause pick(_) = 0
",
        );
        let r = Resolver::new(&dm);
        match r.resolve_name("pick") {
            Resolution::Defs(ids) => assert_eq!(ids.len(), 3),
            other => panic!("expected Defs, got {:?}", other),
        }
    }

    #[test]
    fn local_binding_shadows_module() {
        let dm = build_def_map("function f() = 0\n");
        let mut r = Resolver::new(&dm);
        r.push_expr_scope();
        let shadow_def = DefId(999);
        r.add_binding(Name::new("f"), shadow_def);

        match r.resolve_name("f") {
            Resolution::Def(id) => assert_eq!(id, shadow_def),
            other => panic!("expected Def(999), got {:?}", other),
        }
    }

    #[test]
    fn pop_scope_restores_module_resolution() {
        let dm = build_def_map("function f() = 0\n");
        let mut r = Resolver::new(&dm);
        r.push_expr_scope();
        r.add_binding(Name::new("f"), DefId(999));
        r.pop_scope();

        match r.resolve_name("f") {
            Resolution::Defs(ids) => assert_eq!(ids.len(), 1),
            other => panic!("expected Defs, got {:?}", other),
        }
    }

    #[test]
    fn nested_scopes_shadow_correctly() {
        let dm = build_def_map("function f() = 0\n");
        let mut r = Resolver::new(&dm);
        r.push_expr_scope();
        r.add_binding(Name::new("x"), DefId(10));
        r.push_expr_scope();
        r.add_binding(Name::new("x"), DefId(20));

        match r.resolve_name("x") {
            Resolution::Def(id) => assert_eq!(id, DefId(20)),
            other => panic!("expected Def(20), got {:?}", other),
        }

        r.pop_scope();
        match r.resolve_name("x") {
            Resolution::Def(id) => assert_eq!(id, DefId(10)),
            other => panic!("expected Def(10), got {:?}", other),
        }

        r.pop_scope();
        assert_eq!(r.resolve_name("x"), Resolution::Unresolved);
    }

    #[test]
    fn local_scope_does_not_affect_other_names() {
        let dm = build_def_map("function f() = 0\n");
        let mut r = Resolver::new(&dm);
        r.push_expr_scope();
        r.add_binding(Name::new("x"), DefId(42));
        match r.resolve_name("f") {
            Resolution::Defs(ids) => assert_eq!(ids.len(), 1),
            other => panic!("expected Defs, got {:?}", other),
        }
    }

    #[test]
    fn depth_tracking() {
        let dm = build_def_map("");
        let mut r = Resolver::new(&dm);
        assert_eq!(r.depth(), 1);
        r.push_expr_scope();
        assert_eq!(r.depth(), 2);
        r.push_expr_scope();
        assert_eq!(r.depth(), 3);
        r.pop_scope();
        assert_eq!(r.depth(), 2);
    }

    #[test]
    #[should_panic(expected = "cannot pop the module scope")]
    fn cannot_pop_module_scope() {
        let dm = build_def_map("");
        let mut r = Resolver::new(&dm);
        r.pop_scope();
    }

    #[test]
    fn resolve_path_in_type_ns_finds_enum_as_adt() {
        let dm = build_def_map("enum Foo = { A, B }\n");
        let r = Resolver::new(&dm);
        match r.resolve_path_in_type_ns(&test_db(), "Foo") {
            Some(TypeNs::AdtId(_)) => {} // correct
            other => panic!("expected AdtId, got {:?}", other),
        }
        assert!(r.resolve_path_in_type_ns(&test_db(), "nonexistent").is_none());
    }

    #[test]
    fn resolve_path_in_type_ns_finds_type_alias() {
        let dm = build_def_map("type myint = int\n");
        let r = Resolver::new(&dm);
        match r.resolve_path_in_type_ns(&test_db(), "myint") {
            Some(TypeNs::TypeAliasId(_)) => {} // correct
            other => panic!("expected TypeAliasId, got {:?}", other),
        }
    }

    #[test]
    fn resolve_path_in_value_ns_finds_function() {
        let dm = build_def_map("function f() = 0\n");
        let r = Resolver::new(&dm);
        match r.resolve_path_in_value_ns(&test_db(), "f") {
            Some(ValueNs::FunctionId(_)) => {} // correct: single function
            other => panic!("expected FunctionId, got {:?}", other),
        }
        assert!(r.resolve_path_in_value_ns(&test_db(), "g").is_none());
    }

    #[test]
    fn resolve_path_in_value_ns_finds_function_clauses() {
        let dm = build_def_map("function clause f(0) = 1\nfunction clause f(_) = 0\n");
        let r = Resolver::new(&dm);
        match r.resolve_path_in_value_ns(&test_db(), "f") {
            Some(ValueNs::FunctionIds(ids)) => assert_eq!(ids.len(), 2),
            other => panic!("expected FunctionIds, got {:?}", other),
        }
    }

    #[test]
    fn resolve_path_in_value_ns_finds_register() {
        let dm = build_def_map("register PC : bits(64)\n");
        let r = Resolver::new(&dm);
        match r.resolve_path_in_value_ns(&test_db(), "PC") {
            Some(ValueNs::RegisterId(_)) => {} // correct
            other => panic!("expected RegisterId, got {:?}", other),
        }
    }

    #[test]
    fn resolve_path_in_value_ns_finds_local_binding() {
        let dm = build_def_map("function f() = 0\n");
        let mut r = Resolver::new(&dm);
        r.push_expr_scope();
        r.add_binding(Name::new("x"), DefId(42));
        match r.resolve_path_in_value_ns(&test_db(), "x") {
            Some(ValueNs::LocalBinding(id)) => assert_eq!(id, DefId(42)),
            other => panic!("expected LocalBinding, got {:?}", other),
        }
    }

    #[test]
    fn resolve_path_in_value_ns_finds_val_spec() {
        let dm = build_def_map("val add : (int, int) -> int\n");
        let r = Resolver::new(&dm);
        match r.resolve_path_in_value_ns(&test_db(), "add") {
            Some(ValueNs::ValSpecId(_)) => {} // correct
            other => panic!("expected ValSpecId, got {:?}", other),
        }
    }

    #[test]
    fn workspace_private_item_not_visible_from_other_file() {
        let (root0, _) =
            syntax::parse_text("$[private] function secret() = 42\nfunction public_fn() = 0\n");
        let tree0 = ItemTree::build_from_cst(&root0);

        let (root1, _) = syntax::parse_text("function other() = 1\n");
        let tree1 = ItemTree::build_from_cst(&root1);

        let workspace = Arc::new(WorkspaceDefMap::build(&[(0, &tree0), (1, &tree1)]));

        let dm1 = Arc::new(DefMap::build(&Arc::new(tree1)));
        let resolver = Resolver::for_file_in_workspace(&dm1, &workspace)
            .with_from_file(base_db::FileId::from_raw(1));

        assert!(matches!(resolver.resolve_name("public_fn"), Resolution::Workspace(_)));
        assert_eq!(resolver.resolve_name("secret"), Resolution::Unresolved);
        assert!(matches!(resolver.resolve_name("other"), Resolution::Defs(_)));
    }

    #[test]
    fn workspace_private_item_visible_from_same_file() {
        let (root0, _) = syntax::parse_text("$[private] function secret() = 42\n");
        let tree0 = ItemTree::build_from_cst(&root0);

        let workspace = Arc::new(WorkspaceDefMap::build(&[(0, &tree0)]));
        let dm0 = Arc::new(DefMap::build(&Arc::new(tree0)));
        let resolver = Resolver::for_file_in_workspace(&dm0, &workspace)
            .with_from_file(base_db::FileId::from_raw(0));

        assert!(matches!(resolver.resolve_name("secret"), Resolution::Defs(_)));
    }

    #[test]
    fn without_from_file_all_workspace_items_visible() {
        let (root0, _) = syntax::parse_text("$[private] function secret() = 42\n");
        let tree0 = ItemTree::build_from_cst(&root0);

        let workspace = Arc::new(WorkspaceDefMap::build(&[(0, &tree0)]));
        let dm_empty = Arc::new(DefMap::default());
        let resolver = Resolver::for_file_in_workspace(&dm_empty, &workspace);

        assert!(matches!(resolver.resolve_name("secret"), Resolution::Workspace(_)));
    }
}
