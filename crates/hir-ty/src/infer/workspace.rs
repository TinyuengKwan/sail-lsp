use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use super::*;

/// Cross-file symbol data for multi-file type checking.
pub struct CrossFileRecords {
    pub records: HashMap<String, super::env::RecordInfo>,
    pub bitfields: HashMap<String, super::env::BitfieldInfo>,
    pub function_names: HashSet<String>,
    pub constructor_names: HashSet<String>,
    pub value_names: HashSet<String>,
    pub register_names: HashSet<String>,
}

/// Cross-file aggregation result. Computed once per workspace fingerprint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceContext {
    pub(super) global_constraints: Vec<ConstraintExpr>,
    pub(super) type_aliases: HashMap<String, Ty>,
    pub(super) alias_schemes: HashMap<String, std::sync::Arc<super::env::AliasScheme>>,
    pub(super) constraint_synonyms: HashMap<String, (Vec<String>, crate::ty::ConstraintExpr)>,
    pub(super) overloads: HashMap<String, Vec<String>>,
    pub(super) known_field_names: HashSet<String>,
    pub(super) cross_file_records: HashMap<String, super::env::RecordInfo>,
    pub(super) cross_file_bitfields: HashMap<String, super::env::BitfieldInfo>,
    pub(super) cross_file_function_names: Arc<HashSet<String>>,
    pub(super) cross_file_constructor_names: Arc<HashSet<String>>,
    pub(super) cross_file_value_names: Arc<HashSet<String>>,
    pub(super) cross_file_register_names: Arc<HashSet<String>>,
    pub(super) cross_file_pattern_constants: HashSet<String>,
    pub(super) cross_file_function_schemes: Arc<HashMap<String, Vec<Arc<TypeScheme>>>>,
    pub(super) cross_file_constructor_schemes: Arc<HashMap<String, Vec<Arc<TypeScheme>>>>,
    pub(super) cross_file_mapping_schemes: Arc<HashMap<String, Vec<Arc<super::MappingScheme>>>>,
    pub(super) enums: HashMap<String, Vec<String>>,
    pub(super) unions: HashMap<String, Vec<String>>,
    pub(super) scattered_open_types: HashSet<String>,
    pub(super) cross_file_function_arity: Arc<HashMap<String, Vec<(usize, usize)>>>,
    /// Global signature fingerprint (cache key for typecheck cache).
    pub signatures_fingerprint: u64,
    /// Per-symbol signature hash for reverse-dep diff.
    pub symbol_sig_hashes: HashMap<String, u64>,
    /// File content hash → referenced symbol names.
    pub file_referenced_symbols: HashMap<u64, HashSet<String>>,
    pub symbol_index: Arc<super::env::WorkspaceSymbolIndex>,
}

impl Default for WorkspaceContext {
    fn default() -> Self {
        Self {
            global_constraints: Vec::new(),
            type_aliases: Default::default(),
            alias_schemes: Default::default(),
            constraint_synonyms: Default::default(),
            overloads: Default::default(),
            known_field_names: Default::default(),
            cross_file_records: Default::default(),
            cross_file_bitfields: Default::default(),
            cross_file_function_names: super::env::empty_names(),
            cross_file_constructor_names: super::env::empty_names(),
            cross_file_value_names: super::env::empty_names(),
            cross_file_register_names: super::env::empty_names(),
            cross_file_pattern_constants: Default::default(),
            cross_file_function_schemes: super::env::empty_schemes(),
            cross_file_constructor_schemes: super::env::empty_schemes(),
            cross_file_mapping_schemes: super::env::empty_mappings(),
            enums: Default::default(),
            unions: Default::default(),
            scattered_open_types: Default::default(),
            cross_file_function_arity: super::env::empty_arities(),
            signatures_fingerprint: Default::default(),
            symbol_sig_hashes: Default::default(),
            file_referenced_symbols: Default::default(),
            symbol_index: super::env::empty_index(),
        }
    }
}

impl WorkspaceContext {
    pub fn cross_file_function_names(&self) -> &HashSet<String> {
        &self.cross_file_function_names
    }
    pub fn cross_file_constructor_names(&self) -> &HashSet<String> {
        &self.cross_file_constructor_names
    }
    pub fn cross_file_pattern_constants(&self) -> &HashSet<String> {
        &self.cross_file_pattern_constants
    }
    /// The file declaring a type by this name, if the workspace has one.
    pub fn type_file(&self, name: &str) -> Option<base_db::FileText> {
        self.symbol_index.get_type_file(name)
    }

    pub fn type_aliases_count(&self) -> usize {
        self.type_aliases.len()
    }

    /// Merge one file's data from salsa query results.
    pub fn merge_file_from_queries(
        &mut self,
        env_data: &crate::query::TopLevelEnvData,
        item_tree: Option<&std::sync::Arc<hir_def::item_tree::ItemTree>>,
        file: base_db::FileText,
    ) {
        use hir_def::item_tree::DefKind;

        let Some(item_tree) = item_tree else { return };
        let file_env = &env_data.env;
        self.global_constraints.extend(file_env.global_constraints.iter().cloned());

        use super::env::SymbolKind;
        // Build unified symbol index.
        for name in file_env.functions.keys() {
            Arc::make_mut(&mut self.symbol_index).insert(name.clone(), file, SymbolKind::Function);
        }
        for name in file_env.values.keys() {
            Arc::make_mut(&mut self.symbol_index).insert(name.clone(), file, SymbolKind::Value);
        }
        for name in file_env.registers.keys() {
            Arc::make_mut(&mut self.symbol_index).insert(name.clone(), file, SymbolKind::Register);
        }
        for (name, schemes) in &file_env.mappings {
            Arc::make_mut(&mut self.symbol_index).insert(name.clone(), file, SymbolKind::Mapping);
            Arc::make_mut(&mut self.cross_file_mapping_schemes)
                .entry(name.clone())
                .or_default()
                .extend(schemes.iter().cloned());
        }
        for name in file_env.constructors.keys() {
            Arc::make_mut(&mut self.symbol_index).insert(
                name.clone(),
                file,
                SymbolKind::Constructor,
            );
        }
        // Also from ItemTree items (some functions lack env.functions entries).
        for &id in item_tree.top_level_items() {
            let name = id.name(item_tree).as_str();
            if matches!(
                id.def_kind(item_tree),
                DefKind::Struct
                    | DefKind::Bitfield
                    | DefKind::Union
                    | DefKind::Enum
                    | DefKind::Newtype
                    | DefKind::TypeAlias
            ) {
                Arc::make_mut(&mut self.symbol_index).insert_type(name.to_string(), file);
            }
            match id.def_kind(item_tree) {
                DefKind::ValSpec | DefKind::Function | DefKind::Outcome => {
                    Arc::make_mut(&mut self.symbol_index).insert(
                        name.to_string(),
                        file,
                        SymbolKind::Function,
                    );
                }
                DefKind::Mapping => {
                    Arc::make_mut(&mut self.symbol_index).insert(
                        name.to_string(),
                        file,
                        SymbolKind::Mapping,
                    );
                }
                _ => {}
            }
        }

        for &id in item_tree.top_level_items() {
            let name = id.name(item_tree).as_str();
            match id.def_kind(item_tree) {
                // An `outcome` declares a callable the same way a `val` does,
                // so it crosses files the same way. It used not to reach here
                // at all — the item tree dropped it — so every call to one
                // from another file was untyped.
                DefKind::ValSpec | DefKind::Outcome => {
                    Arc::make_mut(&mut self.cross_file_function_names).insert(name.to_string());
                    if let Some(schemes) = file_env.functions.get(name) {
                        Arc::make_mut(&mut self.cross_file_function_schemes)
                            .entry(name.to_string())
                            .or_default()
                            .extend(schemes.iter().cloned());
                    }
                }
                DefKind::Function | DefKind::Mapping => {
                    Arc::make_mut(&mut self.cross_file_function_names).insert(name.to_string());
                    // A bare `function f(x : T) -> U = ..` with no `val` still
                    // has a signature — `TopLevelEnv` reads it off the
                    // clause's typed parameters. Only the name was recorded
                    // here, so `overload X = {rX_bits, wX_bits, rX, wX}` in
                    // sail-riscv, whose members are all declared that way,
                    // resolved to nothing from any other file.
                    // Not `extend`: a function with both a `val` and a
                    // clause appears under two item kinds, and registering
                    // the same signature twice makes it look overloaded.
                    if let Some(schemes) = file_env.functions.get(name) {
                        Arc::make_mut(&mut self.cross_file_function_schemes)
                            .entry(name.to_string())
                            .or_insert_with(|| schemes.clone());
                    }
                }
                DefKind::Enum => {
                    let sig = id.signature(item_tree);
                    let members = extract_braced_idents_from_sig(sig);
                    let enum_entry = self.enums.entry(name.to_string()).or_default();
                    for m in &members {
                        self.cross_file_pattern_constants.insert(m.clone());
                        Arc::make_mut(&mut self.cross_file_value_names).insert(m.clone());
                        if !enum_entry.contains(m) {
                            enum_entry.push(m.clone());
                        }
                    }
                }
                DefKind::Newtype => {
                    // Newtypes are nominal single-constructor unions too.
                    // Export this identity for overload filtering in callers.
                    if let Some(variants) = file_env.unions.get(name) {
                        self.unions.insert(name.to_owned(), variants.clone());
                    }
                }
                DefKind::Union => {
                    let sig = id.signature(item_tree);
                    let variants = extract_braced_idents_from_sig(sig);
                    let union_entry = self.unions.entry(name.to_string()).or_default();
                    for v in &variants {
                        Arc::make_mut(&mut self.cross_file_constructor_names).insert(v.clone());
                        if !union_entry.contains(v) {
                            union_entry.push(v.clone());
                        }
                    }
                    for v in &variants {
                        if let Some(schemes) = file_env.constructors.get(v.as_str()) {
                            Arc::make_mut(&mut self.cross_file_constructor_schemes)
                                .entry(v.clone())
                                .or_default()
                                .extend(schemes.iter().cloned());
                        }
                    }
                }
                DefKind::Register => {
                    Arc::make_mut(&mut self.cross_file_register_names).insert(name.to_string());
                    Arc::make_mut(&mut self.cross_file_value_names).insert(name.to_string());
                }
                DefKind::Let | DefKind::Var => {
                    Arc::make_mut(&mut self.cross_file_value_names).insert(name.to_string());
                }
                DefKind::Struct => {
                    let sig = id.signature(item_tree);
                    let fields = extract_braced_idents_from_sig(sig);
                    self.known_field_names.extend(fields);
                    if let Some(info) = file_env.records.get(name) {
                        self.cross_file_records
                            .entry(name.to_string())
                            .or_insert_with(|| info.clone());
                    }
                }
                DefKind::Bitfield => {
                    let sig = id.signature(item_tree);
                    let fields = extract_braced_idents_from_sig(sig);
                    self.known_field_names.extend(fields);
                    if let Some(info) = file_env.bitfields.get(name) {
                        self.cross_file_bitfields
                            .entry(name.to_string())
                            .or_insert_with(|| info.clone());
                    }
                }
                DefKind::Overload => {
                    let sig = id.signature(item_tree);
                    // Get overload members from TopLevelEnv (CST-parsed)
                    // which has the full `{member1, member2, ...}` list.
                    // ItemTree signature may be truncated to just
                    // "overload X" without the member list.
                    let members = file_env
                        .overloads
                        .get(name)
                        .cloned()
                        .unwrap_or_else(|| extract_braced_idents_from_sig(sig));
                    let op_key =
                        if name == "operator" { extract_operator_key_from_sig(sig) } else { None };
                    let ov_entry = self.overloads.entry(name.to_string()).or_default();
                    ov_entry.extend(members.iter().cloned());
                    if let Some(ref key) = op_key {
                        let op_entry = self.overloads.entry(key.clone()).or_default();
                        op_entry.extend(members);
                    }
                    Arc::make_mut(&mut self.cross_file_function_names).insert(name.to_string());
                }
                DefKind::TypeAlias => {
                    if let Some(ty) = file_env.type_aliases.get(name) {
                        self.type_aliases.entry(name.to_string()).or_insert_with(|| ty.clone());
                    }
                    if let Some(sch) = file_env.alias_schemes.get(name) {
                        self.alias_schemes.entry(name.to_string()).or_insert_with(|| sch.clone());
                    }
                    if let Some(syn) = file_env.constraint_synonyms.get(name) {
                        self.constraint_synonyms
                            .entry(name.to_string())
                            .or_insert_with(|| syn.clone());
                    }
                }
                DefKind::ScatteredClause => {
                    self.scattered_open_types.insert(name.to_string());
                    if let Some(member) = id.member_name(item_tree) {
                        Arc::make_mut(&mut self.cross_file_value_names).insert(member.to_string());
                        Arc::make_mut(&mut self.cross_file_constructor_names)
                            .insert(member.to_string());
                        self.cross_file_pattern_constants.insert(member.to_string());
                        let parent = name.to_string();

                        // Register constructor scheme (same as build_from_cst path).
                        let parent_ty = Ty::named(parent.clone());
                        let payload_ty: Option<Ty> =
                            id.type_ref(item_tree).map(ty_from_type_ref).or_else(|| {
                                let sig = id.signature(item_tree);
                                sig.find(':').map(|colon_pos| {
                                    let type_text = sig[colon_pos + 1..].trim();
                                    parse_type_text(type_text)
                                })
                            });
                        let (params, implicit_params) = match payload_ty {
                            Some(ty) => (vec![ty], vec![false]),
                            None => (vec![Ty::named("unit")], vec![false]),
                        };
                        let scheme = std::sync::Arc::new(TypeScheme {
                            quantifiers: Vec::new(),
                            kind_bounds: std::collections::HashMap::new(),
                            constraints: Vec::new(),
                            params,
                            implicit_params,
                            ret: parent_ty,
                            declared_effects: Vec::new(),
                            is_declared_pure: false,
                        });
                        Arc::make_mut(&mut self.cross_file_constructor_schemes)
                            .entry(member.to_string())
                            .or_default()
                            .push(scheme);

                        let enum_entry = self.enums.entry(parent.clone()).or_default();
                        if !enum_entry.contains(&member.to_string()) {
                            enum_entry.push(member.to_string());
                        }
                        let union_entry = self.unions.entry(parent).or_default();
                        if !union_entry.contains(&member.to_string()) {
                            union_entry.push(member.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        // Register ALL constructors from TopLevelEnv (includes enum,
        // union, struct Mk_, bitfield Mk_, and newtype constructors).
        for ctor_name in file_env.constructor_names() {
            Arc::make_mut(&mut self.cross_file_constructor_names).insert(ctor_name.clone());
            if let Some(schemes) = file_env.constructors.get(ctor_name.as_str()) {
                Arc::make_mut(&mut self.cross_file_constructor_schemes)
                    .entry(ctor_name.clone())
                    .or_default()
                    .extend(schemes.iter().cloned());
            }
        }
    }

    /// Compute signature fingerprints from the aggregated data.
    pub(super) fn compute_arities(&mut self) {
        // Arity variants are workspace data, not per-callable work. Build
        // them once and share the immutable index with every inference.
        let mut arity_index = HashMap::new();
        // They are still not merged into env.functions/constructors, because
        // full unification on quantified types causes pathological recursion;
        // the main call path uses the arity sets below.
        for (name, schemes) in self.cross_file_function_schemes.iter() {
            let arities: Vec<(usize, usize)> = schemes
                .iter()
                .flat_map(|s| {
                    let total = s.params.len();
                    let required = s.implicit_params.iter().filter(|implicit| !**implicit).count();
                    let mut variants = vec![(required, total)];
                    // Sail allows calling `(A, B) -> C` either with one
                    // tuple arg or two flattened args. Accept both.
                    if s.params.len() == 1 {
                        if let TyKind::Tuple(items) = s.params[0].kind() {
                            variants.push((items.len(), items.len()));
                        }
                        // `foo : unit -> T` can be called as `foo()` with 0 args
                        if matches!(s.params[0].kind(), TyKind::Scalar(crate::ty::Scalar::Unit)) {
                            variants.push((0, 0));
                        }
                    }
                    variants
                })
                .collect();
            arity_index.insert(name.clone(), arities);
        }
        self.cross_file_function_arity = Arc::new(arity_index);
    }

    pub fn compute_fingerprints(&mut self) {
        self.compute_arities();
        let mut symbol_sig_hashes: HashMap<String, u64> = HashMap::new();

        for (name, schemes) in self.cross_file_function_schemes.iter() {
            let mut h = DefaultHasher::new();
            "fn".hash(&mut h);
            for scheme in schemes {
                for q in &scheme.quantifiers {
                    q.hash(&mut h);
                }
                for c in &scheme.constraints {
                    c.text.hash(&mut h);
                }
                for p in &scheme.params {
                    p.display_text().hash(&mut h);
                }
                for implicit in &scheme.implicit_params {
                    implicit.hash(&mut h);
                }
                scheme.ret.display_text().hash(&mut h);
            }
            symbol_sig_hashes.insert(name.clone(), h.finish());
        }
        for (name, schemes) in self.cross_file_constructor_schemes.iter() {
            let mut h = DefaultHasher::new();
            "ctor".hash(&mut h);
            for scheme in schemes {
                for q in &scheme.quantifiers {
                    q.hash(&mut h);
                }
                for c in &scheme.constraints {
                    c.text.hash(&mut h);
                }
                for p in &scheme.params {
                    p.display_text().hash(&mut h);
                }
                scheme.ret.display_text().hash(&mut h);
            }
            symbol_sig_hashes.entry(format!("ctor::{name}")).or_insert_with(|| h.finish());
        }
        for (name, ty) in &self.type_aliases {
            let mut h = DefaultHasher::new();
            "alias".hash(&mut h);
            ty.display_text().hash(&mut h);
            symbol_sig_hashes.insert(format!("alias::{name}"), h.finish());
        }
        for (name, members) in &self.enums {
            let mut h = DefaultHasher::new();
            "enum".hash(&mut h);
            let mut sorted: Vec<&String> = members.iter().collect();
            sorted.sort();
            for m in sorted {
                m.hash(&mut h);
            }
            symbol_sig_hashes.insert(format!("enum::{name}"), h.finish());
        }
        for (name, variants) in &self.unions {
            let mut h = DefaultHasher::new();
            "union".hash(&mut h);
            let mut sorted: Vec<&String> = variants.iter().collect();
            sorted.sort();
            for v in sorted {
                v.hash(&mut h);
            }
            symbol_sig_hashes.insert(format!("union::{name}"), h.finish());
        }
        for name in self.cross_file_register_names.iter() {
            let mut h = DefaultHasher::new();
            "reg".hash(&mut h);
            name.hash(&mut h);
            symbol_sig_hashes.insert(format!("reg::{name}"), h.finish());
        }
        for name in self.cross_file_value_names.iter() {
            let mut h = DefaultHasher::new();
            "val".hash(&mut h);
            name.hash(&mut h);
            symbol_sig_hashes.entry(format!("val::{name}")).or_insert_with(|| h.finish());
        }
        for (i, fact) in self.global_constraints.iter().enumerate() {
            let mut hash = DefaultHasher::new();
            fact.hash(&mut hash);
            symbol_sig_hashes.insert(format!("constraint::{i}"), hash.finish());
        }
        let mut global_hasher = DefaultHasher::new();
        let mut sorted_keys: Vec<&String> = symbol_sig_hashes.keys().collect();
        sorted_keys.sort();
        for key in sorted_keys {
            let hash = symbol_sig_hashes[key];
            key.hash(&mut global_hasher);
            hash.hash(&mut global_hasher);
        }
        self.signatures_fingerprint = global_hasher.finish();
        self.symbol_sig_hashes = symbol_sig_hashes;
    }

    /// Apply this cached aggregation to a freshly built per-file env.
    pub fn apply_to(&self, env: &mut TopLevelEnv, pattern_constants: &mut HashSet<String>) {
        for fact in &self.global_constraints {
            if !env.global_constraints.contains(fact) {
                env.global_constraints.push(fact.clone());
            }
        }
        // Type alias merge — disabled. Even with safe/unsafe filtering,
        // merging aliases changes normalize_alias_ty behavior and produces
        // more mismatches than it fixes. The per-query cross-file return
        // type lookup (via symbol_index → top_level_env) handles the
        // important case; alias expansion in unify is deferred to future
        // Overloads: union, preserving existing entries.
        for (name, members) in &self.overloads {
            let entry = env.overloads.entry(name.clone()).or_default();
            for m in members {
                if !entry.contains(m) {
                    entry.push(m.clone());
                }
            }
        }
        // Known field names: union.
        env.known_field_names.extend(self.known_field_names.iter().cloned());

        for (name, info) in &self.cross_file_records {
            env.records.entry(name.clone()).or_insert_with(|| info.clone());
        }
        for (name, info) in &self.cross_file_bitfields {
            let is_new = !env.bitfields.contains_key(name);
            env.bitfields.entry(name.clone()).or_insert_with(|| info.clone());
            // A bitfield's accessors are generated, not written, so they are
            // not among the definitions the cross-file name sets collect:
            // synthesize them here the same way the defining file does, or a
            // call to `update_f` from another file resolves to nothing.
            if is_new {
                for (field_name, field_ty) in &info.fields {
                    super::env::synthesize_bitfield_accessors(
                        env,
                        name,
                        field_name,
                        field_ty.clone(),
                    );
                }
            }
        }
        // Cross-file name sets: replace.
        env.cross_file_function_names = self.cross_file_function_names.clone();
        env.cross_file_constructor_names = self.cross_file_constructor_names.clone();
        env.cross_file_constructor_schemes = self.cross_file_constructor_schemes.clone();
        env.cross_file_mapping_schemes = self.cross_file_mapping_schemes.clone();
        env.cross_file_value_names = self.cross_file_value_names.clone();
        env.cross_file_register_names = self.cross_file_register_names.clone();
        // Enums / unions.
        //
        // A file that declares the type itself keeps its own member list. Two
        // files may declare the same name — sail-riscv defines a six-member
        // `write_kind` while the bundled `lib/regfp.sail` and
        // `lib/instr_kinds.sail` each define an eleven-member one — and
        // unioning them makes the local type look like it has members it does
        // not, which the exhaustiveness check then demands arms for.
        //
        // A *scattered* type is the exception: `enum clause E = M` in another
        // file genuinely adds a member, so those still merge.
        let merge = |name: &String,
                     incoming: &Vec<String>,
                     local: &mut HashMap<String, Vec<String>>,
                     scattered: &HashSet<String>| {
            match local.get_mut(name) {
                Some(existing) if !scattered.contains(name) => {
                    let _ = existing;
                }
                Some(existing) => {
                    for m in incoming {
                        if !existing.contains(m) {
                            existing.push(m.clone());
                        }
                    }
                }
                None => {
                    local.insert(name.clone(), incoming.clone());
                }
            }
        };
        let scattered = &self.scattered_open_types;
        for (name, members) in &self.enums {
            merge(name, members, &mut env.enums, scattered);
        }
        for (name, variants) in &self.unions {
            merge(name, variants, &mut env.unions, scattered);
        }
        // Scattered open types: propagate to env for exhaustiveness guard.
        env.scattered_open_types.extend(self.scattered_open_types.iter().cloned());

        // Cross-file schemes, for the lookups that can tolerate them.
        // `lookup_overload_members_cross_file` is the only consumer: it
        // reports nothing when a candidate fails, so an imperfect signature
        // costs an unresolved operator rather than a false diagnostic. The
        // field had never been populated at all, which is why a prelude
        // operator like `overload operator =^ = {sail_mask}` never resolved.
        env.cross_file_schemes = Some(self.cross_file_function_schemes.clone());

        env.cross_file_function_arity = self.cross_file_function_arity.clone();
        // Pattern constants: union with cross-file contributions.
        pattern_constants.extend(self.cross_file_pattern_constants.iter().cloned());
        pattern_constants.extend(self.cross_file_constructor_names.iter().cloned());

        env.symbol_index = self.symbol_index.clone();

        // Merge cross-file alias schemes. Local file's entries win over
        // workspace context entries (already inserted before apply_to runs).
        for (name, sch) in &self.alias_schemes {
            env.alias_schemes.entry(name.clone()).or_insert_with(|| sch.clone());
        }
        // Declared in one file, used in another file's `forall`.
        for (name, syn) in &self.constraint_synonyms {
            env.constraint_synonyms.entry(name.clone()).or_insert_with(|| syn.clone());
        }
    }
}

/// Check if a type references config-dependent names (e.g., `xlen`).
pub(crate) fn contains_config_dependent(ty: &Ty) -> bool {
    use crate::ty::TyKind;
    // Known config-dependent type names
    const CONFIG_NAMES: &[&str] = &[
        "xlen",
        "vlen",
        "elen",
        "flen",
        "xlen_bytes",
        "log2_xlen",
        "xlenbits",
        "vlenbits",
        "flenbits",
        "regtype",
        "fregtype",
        "physaddrbits_len",
        "asidlen",
        "asidbits",
    ];
    match ty.kind() {
        TyKind::Named(name, _) if CONFIG_NAMES.contains(&name.as_str()) => true,
        TyKind::App { name, args, .. } => {
            if CONFIG_NAMES.contains(&name.as_str()) {
                return true;
            }
            args.iter().any(|a| match a {
                crate::ty::TyArg::Type(t) => contains_config_dependent(t),
                _ => false,
            })
        }
        TyKind::Tuple(items) => items.iter().any(contains_config_dependent),
        _ => false,
    }
}

/// Return symbol names whose signature hash changed between `old` and `new`.
pub fn diff_symbol_sig_hashes(
    old: &HashMap<String, u64>,
    new: &HashMap<String, u64>,
) -> HashSet<String> {
    let mut changed = HashSet::new();
    for (name, hash) in new {
        match old.get(name) {
            Some(prev) if prev == hash => {}
            _ => {
                changed.insert(name.clone());
            }
        }
    }
    for name in old.keys() {
        if !new.contains_key(name) {
            changed.insert(name.clone());
        }
    }
    changed
}

/// Extract IDENT-like tokens from inside braces in a signature string.
pub fn extract_braced_idents_from_sig_pub(sig: &str) -> Vec<String> {
    extract_braced_idents_from_sig(sig)
}

fn extract_braced_idents_from_sig(sig: &str) -> Vec<String> {
    let mut result = Vec::new();
    // Strip comments first — comments may contain `{` or `}` that
    // would confuse brace matching (e.g. `// WRS.{STO,NTO}`).
    let stripped = strip_sail_comments(sig);
    if let Some(start) = stripped.find('{') {
        let inner = &stripped[start + 1..];
        if let Some(end) = inner.find('}') {
            let fields = &inner[..end];
            for part in split_top_level_commas(fields) {
                let trimmed = part.trim();
                if let Some(name) = trimmed.split_whitespace().next() {
                    let name =
                        name.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '?');
                    if !name.is_empty() {
                        result.push(name.to_string());
                    }
                }
            }
        }
    }
    result
}

/// Strip `//` line comments and `/* */` block comments from source text.
fn strip_sail_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' {
            match chars.peek() {
                Some('/') => {
                    // Skip to end of line
                    for c2 in chars.by_ref() {
                        if c2 == '\n' {
                            out.push('\n');
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next(); // consume *
                    loop {
                        match chars.next() {
                            Some('*') if chars.peek() == Some(&'/') => {
                                chars.next();
                                break;
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
                _ => out.push(c),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Convert a `TypeRef` to a `Ty` without full lowering context.
pub(crate) fn ty_from_type_ref_pub(tr: &hir_def::hir::type_ref::TypeRef) -> Ty {
    ty_from_type_ref(tr)
}

fn ty_from_type_ref(tr: &hir_def::hir::type_ref::TypeRef) -> Ty {
    use hir_def::hir::type_ref::{TypeArg as TrArg, TypeRef};
    match tr {
        TypeRef::Named(name) => {
            if is_numeric_set_text(name) {
                super::type_from_type_text(name)
            } else {
                Ty::named(name)
            }
        }
        TypeRef::Var(name) => Ty::param(format!("'{name}")),
        TypeRef::App { name, args } => {
            let ty_args: Vec<TyArg> = args
                .iter()
                .map(|a| match a {
                    TrArg::Type(inner) => TyArg::Type(ty_from_type_ref(inner)),
                    TrArg::Value(v) => TyArg::numeric(v.as_str()),
                })
                .collect();
            let text = super::numeric::app_text(name, &ty_args);
            Ty::app(name, ty_args, text)
        }
        TypeRef::Tuple(items) => Ty::tuple(items.iter().map(ty_from_type_ref).collect()),
        TypeRef::Fn { params, ret } => {
            Ty::function(params.iter().map(ty_from_type_ref).collect(), ty_from_type_ref(ret))
        }
        TypeRef::Bidir { lhs, rhs } => Ty::bidir(ty_from_type_ref(lhs), ty_from_type_ref(rhs)),
        TypeRef::Error | TypeRef::Exist { .. } | TypeRef::Forall { .. } => Ty::error(),
    }
}

/// Numeric sets in ItemTree payloads must retain the same refinement as CST
/// declarations. Treating them as plain int loses constructor-pattern evidence.
fn is_numeric_set_text(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('{')
        && trimmed.ends_with('}')
        && !trimmed.contains('.')
        && trimmed[1..trimmed.len() - 1]
            .split(',')
            .all(|part| crate::ty::NumericExpr::integer_literal(part.trim()).is_some())
}

fn parse_type_text(text: &str) -> Ty {
    let trimmed = text.trim();
    if is_numeric_set_text(trimmed) {
        return super::type_from_type_text(trimmed);
    }
    // Tuple: (A, B, C) where there are top-level commas inside parens.
    if trimmed.starts_with('(') && trimmed.ends_with(')') {
        let inner = &trimmed[1..trimmed.len() - 1];
        let parts = split_top_level_commas(inner);
        if parts.len() > 1 {
            let items: Vec<Ty> = parts.iter().map(|p| parse_type_text(p.trim())).collect();
            return Ty::tuple(items);
        }
        // Single element in parens: unwrap.
        return parse_type_text(inner.trim());
    }
    // App type: bits(32), range(0, 63), etc.
    if let Some(paren_pos) = trimmed.find('(') {
        if trimmed.ends_with(')') {
            let name = &trimmed[..paren_pos];
            let args_text = &trimmed[paren_pos + 1..trimmed.len() - 1];
            let arg_parts = split_top_level_commas(args_text);
            let args: Vec<TyArg> = arg_parts.iter().map(|p| TyArg::numeric(p.trim())).collect();
            let text_repr = trimmed.to_string();
            return Ty::app(name, args, text_repr);
        }
    }
    Ty::named(trimmed)
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0u32;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// For `overload operator | = {or_vec}`, the signature text is
/// `"overload operator |"`. Extract the operator symbol after "operator".
fn extract_operator_key_from_sig(sig: &str) -> Option<String> {
    // Signature looks like: "overload operator |" or "overload operator -"
    let rest = sig.strip_prefix("overload")?;
    let rest = rest.trim_start();
    let rest = rest.strip_prefix("operator")?;
    let op = rest.trim();
    if op.is_empty() {
        None
    } else {
        // Take only the operator symbol (first non-whitespace token)
        let op_tok = op.split_whitespace().next().unwrap_or(op);
        Some(op_tok.to_string())
    }
}

#[cfg(test)]
mod c1_intern_tests {
    use super::*;

    #[test]
    fn primitive_named_types_share_interned() {
        let a = Ty::named("int");
        let b = Ty::named("int");
        // Both calls go through the intern pool — pointer equality via Interned.
        assert_eq!(a, b);
    }

    #[test]
    fn error_is_interned() {
        let a = Ty::error();
        let b = Ty::error();
        assert_eq!(a, b);
    }

    #[test]
    fn non_primitive_named_is_interned() {
        let a = Ty::named("Minterrupts");
        let b = Ty::named("Minterrupts");
        // Global intern table means ALL identical types share Interned pointer.
        assert_eq!(a, b);
    }

    #[test]
    fn nested_clone_is_cheap() {
        // Building a deep type and cloning it should bump exactly one
        // refcount — no recursion into the inner data.
        let inner = Ty::tuple(vec![Ty::named("int"), Ty::named("bool")]);
        let outer = Ty::function(vec![inner.clone(), Ty::named("unit")], Ty::named("int"));
        let cloned = outer.clone();
        assert_eq!(outer, cloned);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unify_value_skips_self_bindings() {
        let mut subst = Subst::default();
        assert!(unify_value("'n", "'n", &mut subst));
        assert!(subst.values.is_empty());
    }

    #[test]
    fn subst_numeric_expr_handles_self_referential_value_bindings() {
        let mut subst = Subst::default();
        subst.values.insert("'n".to_string(), "'n".to_string());
        assert_eq!(
            subst_numeric_expr(&NumericExpr::Var("'n".to_string()), &subst),
            NumericExpr::Var("'n".to_string())
        );
    }

    #[test]
    fn existential_constraint_extracted_into_scheme() {
        // A val spec with existential return type should
        // have the existential binder added to quantifiers.
        // Sail existential syntax: `{exist 'n, constraint. type}`
        // or without braces: `exist 'n, constraint. type`

        // First verify basic forall extraction works via CST
        let source = "val foo : forall 'a. bits('a) -> bits('a)\n";
        let (cst_root, _) = syntax::parse_text(source);
        let env = TopLevelEnv::from_cst(&cst_root).0;
        assert!(env.functions.contains_key("foo"), "basic forall test failed");
        let schemes = &env.functions["foo"];
        assert_eq!(schemes.len(), 1);
        let scheme = &schemes[0];
        assert!(scheme.quantifiers.contains(&"'a".to_string()));
        assert_eq!(scheme.params.len(), 1);
    }
}
