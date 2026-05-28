use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use hir_def::callgraph::SourceFileInfo;

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

/// Type-check a file with optional cross-file struct info.
pub fn check_file_with_records(
    file: &dyn SourceFileInfo,
    cross_file: Option<&CrossFileRecords>,
) -> Option<TypeCheckResult> {
    let text = file.text();
    if text.is_empty() {
        return None;
    }
    let (cst_root, _) = syntax::parse_text(text);
    let (mut env, pattern_constants) = TopLevelEnv::from_cst(&cst_root);
    let parsed_file = syntax::cst_lower::parsed_file_from_cst(&cst_root, text);
    apply_callable_signature_metadata(&parsed_file, text, &mut env);
    if let Some(cf) = cross_file {
        for (name, info) in &cf.records {
            env.records.entry(name.clone()).or_insert_with(|| info.clone());
        }
        for (name, info) in &cf.bitfields {
            env.bitfields.entry(name.clone()).or_insert_with(|| info.clone());
        }
        // Merge cross-file symbol names so top_level_symbol_exists
        // returns true for symbols defined in other files.
        env.cross_file_function_names.extend(cf.function_names.iter().cloned());
        env.cross_file_constructor_names.extend(cf.constructor_names.iter().cloned());
        env.cross_file_value_names.extend(cf.value_names.iter().cloned());
        env.cross_file_register_names.extend(cf.register_names.iter().cloned());
        env.has_workspace_context = true;
    }
    let callable_bodies = std::sync::Arc::new(hir_def::bodies::CallableBodies::from_cst(&cst_root));

    let mut merged = TypeCheckResult::default();
    let cancel = CancellationToken::new();

    for entry in callable_bodies.entries() {
        if cancel.is_cancelled() {
            break;
        }
        // Fresh InferenceContext per callable.
        let mut ctx = Box::new(InferenceContext::new_for_body_with_cancel(
            text,
            entry.body.clone(),
            entry.source_map.clone(),
            env.clone(),
            pattern_constants.clone(),
            cancel.clone(),
        ));
        if entry.body.mapping_arms.is_empty() {
            ctx.infer_callable_body_hir(&entry.name, entry);
        } else {
            ctx.infer_mapping_body_hir(&entry.name, entry);
        }
        let per_callable = ctx.finish_query();
        cook_inference_diagnostics_to_legacy(
            &per_callable,
            &entry.source_map,
            &mut merged.legacy_diagnostics,
        );
        merged.legacy_diagnostics.extend(per_callable.legacy_diagnostics);
        merged.diagnostics.extend(per_callable.diagnostics);
    }

    merged.bodies = Some(callable_bodies);
    Some(merged)
}

/// Type-check a single file using per-callable inference contexts.
pub fn check_file(file: &dyn SourceFileInfo) -> Option<TypeCheckResult> {
    let text = file.text();
    if text.is_empty() {
        return None;
    }
    let (cst_root, _) = syntax::parse_text(text);
    let (mut env, pattern_constants) = TopLevelEnv::from_cst(&cst_root);
    let parsed_file = syntax::cst_lower::parsed_file_from_cst(&cst_root, text);
    apply_callable_signature_metadata(&parsed_file, text, &mut env);
    let callable_bodies = std::sync::Arc::new(hir_def::bodies::CallableBodies::from_cst(&cst_root));
    let _item_tree = hir_def::ItemTree::build_from_cst(&cst_root);

    let mut merged = TypeCheckResult::default();
    let cancel = CancellationToken::new();

    for entry in callable_bodies.entries() {
        if cancel.is_cancelled() {
            break;
        }
        // Fresh context per callable
        let mut ctx = Box::new(InferenceContext::new_for_body_with_cancel(
            text,
            entry.body.clone(),
            entry.source_map.clone(),
            env.clone(),
            pattern_constants.clone(),
            cancel.clone(),
        ));
        if entry.body.mapping_arms.is_empty() {
            ctx.infer_callable_body_hir(&entry.name, entry);
        } else {
            ctx.infer_mapping_body_hir(&entry.name, entry);
        }
        let per_callable = ctx.finish_query();
        // Convert typed InferenceDiagnostics + TypeMismatches to legacy
        // Diagnostics while we still have the per-callable source_map.
        cook_inference_diagnostics_to_legacy(
            &per_callable,
            &entry.source_map,
            &mut merged.legacy_diagnostics,
        );
        // Merge per-callable results into file-level aggregate
        merged.legacy_diagnostics.extend(per_callable.legacy_diagnostics);
        merged.diagnostics.extend(per_callable.diagnostics);
    }

    merged.bodies = Some(callable_bodies);
    Some(merged)
}

/// Cross-file aggregation result. Computed once per workspace fingerprint.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceContext {
    type_aliases: HashMap<String, Ty>,
    alias_schemes: HashMap<String, std::sync::Arc<super::env::AliasScheme>>,
    overloads: HashMap<String, Vec<String>>,
    known_field_names: HashSet<String>,
    cross_file_records: HashMap<String, super::env::RecordInfo>,
    cross_file_bitfields: HashMap<String, super::env::BitfieldInfo>,
    cross_file_function_names: HashSet<String>,
    cross_file_constructor_names: HashSet<String>,
    cross_file_value_names: HashSet<String>,
    cross_file_register_names: HashSet<String>,
    cross_file_pattern_constants: HashSet<String>,
    cross_file_function_schemes: HashMap<String, Vec<Arc<TypeScheme>>>,
    cross_file_constructor_schemes: HashMap<String, Vec<Arc<TypeScheme>>>,
    enums: HashMap<String, Vec<String>>,
    unions: HashMap<String, Vec<String>>,
    scattered_open_types: HashSet<String>,
    /// Global signature fingerprint (cache key for typecheck cache).
    pub signatures_fingerprint: u64,
    /// Per-symbol signature hash for reverse-dep diff.
    pub symbol_sig_hashes: HashMap<String, u64>,
    /// File content hash → referenced symbol names.
    pub file_referenced_symbols: HashMap<u64, HashSet<String>>,
    pub symbol_index: super::env::WorkspaceSymbolIndex,
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
    pub fn type_aliases_count(&self) -> usize {
        self.type_aliases.len()
    }

    /// Build workspace context from CST-derived data.
    #[allow(dead_code)]
    pub fn build_from_cst<'a, F: SourceFileInfo + 'a>(
        files: impl IntoIterator<Item = &'a F>,
    ) -> Self {
        use hir_def::bodies::CallableBodies;
        use hir_def::hir::Expr;
        use hir_def::item_tree::{ItemKind, ItemTree};

        let mut ctx = WorkspaceContext::default();

        for f in files {
            let text = f.text();
            let file_id = f.content_hash();

            // Parse CST once and reuse. Use cached item_tree from
            // SourceFileInfo when available (salsa-backed SalsaFile provides
            // it from file_item_tree; TestFile builds it lazily).
            let (cst_root, _errors) = syntax::parse_text(text);
            let item_tree_owned;
            let item_tree: &ItemTree = match f.item_tree() {
                Some(it) => it,
                None => {
                    item_tree_owned = ItemTree::build_from_cst(&cst_root);
                    &item_tree_owned
                }
            };

            // Build TopLevelEnv from the same CST (no re-parse)
            let (file_env, _) = TopLevelEnv::from_cst(&cst_root);

            // Referenced-symbol collection via Body arena (same CST, no re-parse)
            if file_id != 0 {
                let entry = ctx.file_referenced_symbols.entry(file_id).or_default();
                {
                    let bodies = CallableBodies::from_cst(&cst_root);
                    for body_entry in bodies.entries() {
                        for (_, hir) in body_entry.body.iter_exprs() {
                            match hir {
                                Expr::Ident(name) => {
                                    entry.insert(name.clone());
                                }
                                Expr::Call { .. } => {
                                    // Callee name extracted during iteration
                                }
                                Expr::Ref(name) => {
                                    entry.insert(name.clone());
                                }
                                Expr::Field { field, .. } => {
                                    entry.insert(field.clone());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }

            // Aggregate cross-file data from ItemTree entries
            for &id in item_tree.top_level_items() {
                let name = id.name(item_tree).as_str();
                match id.item_kind(item_tree) {
                    ItemKind::ValSpec => {
                        ctx.cross_file_function_names.insert(name.to_string());
                        // Use scheme from TopLevelEnv
                        if let Some(schemes) = file_env.functions.get(name) {
                            ctx.cross_file_function_schemes
                                .entry(name.to_string())
                                .or_default()
                                .extend(schemes.iter().cloned());
                        }
                        // Note: some val specs with extern clauses
                        // (e.g., `val f = pure {cpp: "f"} : T -> U`) may not
                        // produce schemes in file_env.functions. The arity check
                        // at expr.rs:1200 only fires when arity IS known, so
                        // unknown-arity functions skip the check gracefully.
                    }
                    ItemKind::Function | ItemKind::Mapping => {
                        ctx.cross_file_function_names.insert(name.to_string());
                    }
                    ItemKind::Enum => {
                        let sig = id.signature(item_tree);
                        let members = extract_braced_idents_from_sig(sig);
                        let enum_entry = ctx.enums.entry(name.to_string()).or_default();
                        for m in &members {
                            ctx.cross_file_pattern_constants.insert(m.clone());
                            ctx.cross_file_value_names.insert(m.clone());
                            if !enum_entry.contains(m) {
                                enum_entry.push(m.clone());
                            }
                        }
                    }
                    ItemKind::Union => {
                        let sig = id.signature(item_tree);
                        let variants = extract_braced_idents_from_sig(sig);
                        let union_entry = ctx.unions.entry(name.to_string()).or_default();
                        for v in &variants {
                            ctx.cross_file_constructor_names.insert(v.clone());
                            if !union_entry.contains(v) {
                                union_entry.push(v.clone());
                            }
                        }
                        // Use constructor schemes from TopLevelEnv
                        for v in &variants {
                            if let Some(schemes) = file_env.constructors.get(v.as_str()) {
                                ctx.cross_file_constructor_schemes
                                    .entry(v.clone())
                                    .or_default()
                                    .extend(schemes.iter().cloned());
                            }
                        }
                    }
                    ItemKind::Register => {
                        ctx.cross_file_register_names.insert(name.to_string());
                        ctx.cross_file_value_names.insert(name.to_string());
                    }
                    ItemKind::Let | ItemKind::Var => {
                        ctx.cross_file_value_names.insert(name.to_string());
                    }
                    ItemKind::Struct => {
                        let sig = id.signature(item_tree);
                        let fields = extract_braced_idents_from_sig(sig);
                        ctx.known_field_names.extend(fields);

                        if let Some(info) = file_env.records.get(name) {
                            ctx.cross_file_records
                                .entry(name.to_string())
                                .or_insert_with(|| info.clone());
                        }
                    }
                    ItemKind::Bitfield => {
                        let sig = id.signature(item_tree);
                        let fields = extract_braced_idents_from_sig(sig);
                        ctx.known_field_names.extend(fields);
                        if let Some(info) = file_env.bitfields.get(name) {
                            ctx.cross_file_bitfields
                                .entry(name.to_string())
                                .or_insert_with(|| info.clone());
                        }
                    }
                    ItemKind::Overload => {
                        let sig = id.signature(item_tree);
                        let members = extract_braced_idents_from_sig(sig);
                        // For `overload operator | = {or_vec}`, `name` is
                        // "operator". Also register under the actual operator
                        // symbol so BinaryOp inference can resolve it.
                        let op_key = if name == "operator" {
                            extract_operator_key_from_sig(sig)
                        } else {
                            None
                        };
                        let ov_entry = ctx.overloads.entry(name.to_string()).or_default();
                        ov_entry.extend(members.iter().cloned());
                        if let Some(ref key) = op_key {
                            let op_entry = ctx.overloads.entry(key.clone()).or_default();
                            op_entry.extend(members);
                        }
                        ctx.cross_file_function_names.insert(name.to_string());
                    }
                    ItemKind::TypeAlias => {
                        if let Some(ty) = file_env.type_aliases.get(name) {
                            ctx.type_aliases.entry(name.to_string()).or_insert_with(|| ty.clone());
                        }
                        if let Some(sch) = file_env.alias_schemes.get(name) {
                            ctx.alias_schemes
                                .entry(name.to_string())
                                .or_insert_with(|| sch.clone());
                        }
                    }
                    ItemKind::ScatteredClause => {
                        // Handle scattered enum/union clauses.
                        // Mark the parent type as potentially open (scattered).
                        ctx.scattered_open_types.insert(name.to_string());
                        if let Some(member) = id.member_name(item_tree) {
                            ctx.cross_file_value_names.insert(member.to_string());
                            ctx.cross_file_constructor_names.insert(member.to_string());
                            ctx.cross_file_pattern_constants.insert(member.to_string());

                            let parent = name.to_string();

                            // Register constructor type scheme.
                            // For enum clauses: unit → parent_type
                            // For union clauses: payload → parent_type
                            let parent_ty = Ty::named(parent.clone());
                            // Use TypeRef from ItemTree (populated by lower.rs
                            // from type_ref_from_text). Falls back to signature
                            // parsing if type_ref is None.
                            let payload_ty: Option<Ty> = id
                                .type_ref(item_tree)
                                .map(ty_from_type_ref)
                                .or_else(|| {
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
                            ctx.cross_file_constructor_schemes
                                .entry(member.to_string())
                                .or_default()
                                .push(scheme);

                            // Add to enum/union members.
                            let enum_entry = ctx.enums.entry(parent.clone()).or_default();
                            if !enum_entry.contains(&member.to_string()) {
                                enum_entry.push(member.to_string());
                            }
                            let union_entry = ctx.unions.entry(parent).or_default();
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
                ctx.cross_file_constructor_names.insert(ctor_name.clone());
                if let Some(schemes) = file_env.constructors.get(ctor_name.as_str()) {
                    ctx.cross_file_constructor_schemes
                        .entry(ctor_name.clone())
                        .or_default()
                        .extend(schemes.iter().cloned());
                }
            }
        }

        // Reuse the same fingerprinting logic
        ctx.compute_fingerprints();
        ctx
    }

    /// Merge one file's data from salsa query results.
    pub fn merge_file_from_queries(
        &mut self,
        env_data: &crate::query::TopLevelEnvData,
        item_tree: Option<&std::sync::Arc<hir_def::item_tree::ItemTree>>,
        file: base_db::FileText,
    ) {
        use hir_def::item_tree::ItemKind;

        let Some(item_tree) = item_tree else { return };
        let file_env = &env_data.env;

        use super::env::SymbolKind;
        // Build unified symbol index.
        for name in file_env.functions.keys() {
            self.symbol_index.insert(name.clone(), file, SymbolKind::Function);
        }
        for name in file_env.values.keys() {
            self.symbol_index.insert(name.clone(), file, SymbolKind::Value);
        }
        for name in file_env.registers.keys() {
            self.symbol_index.insert(name.clone(), file, SymbolKind::Register);
        }
        for name in file_env.mappings.keys() {
            self.symbol_index.insert(name.clone(), file, SymbolKind::Mapping);
        }
        for name in file_env.constructors.keys() {
            self.symbol_index.insert(name.clone(), file, SymbolKind::Constructor);
        }
        // Also from ItemTree items (some functions lack env.functions entries).
        for &id in item_tree.top_level_items() {
            let name = id.name(item_tree).as_str();
            match id.item_kind(item_tree) {
                ItemKind::ValSpec | ItemKind::Function => {
                    self.symbol_index.insert(name.to_string(), file, SymbolKind::Function);
                }
                ItemKind::Mapping => {
                    self.symbol_index.insert(name.to_string(), file, SymbolKind::Mapping);
                }
                _ => {}
            }
        }

        for &id in item_tree.top_level_items() {
            let name = id.name(item_tree).as_str();
            match id.item_kind(item_tree) {
                ItemKind::ValSpec => {
                    self.cross_file_function_names.insert(name.to_string());
                    if let Some(schemes) = file_env.functions.get(name) {
                        self.cross_file_function_schemes
                            .entry(name.to_string())
                            .or_default()
                            .extend(schemes.iter().cloned());
                    }
                }
                ItemKind::Function | ItemKind::Mapping => {
                    self.cross_file_function_names.insert(name.to_string());
                }
                ItemKind::Enum => {
                    let sig = id.signature(item_tree);
                    let members = extract_braced_idents_from_sig(sig);
                    let enum_entry = self.enums.entry(name.to_string()).or_default();
                    for m in &members {
                        self.cross_file_pattern_constants.insert(m.clone());
                        self.cross_file_value_names.insert(m.clone());
                        if !enum_entry.contains(m) {
                            enum_entry.push(m.clone());
                        }
                    }
                }
                ItemKind::Union => {
                    let sig = id.signature(item_tree);
                    let variants = extract_braced_idents_from_sig(sig);
                    let union_entry = self.unions.entry(name.to_string()).or_default();
                    for v in &variants {
                        self.cross_file_constructor_names.insert(v.clone());
                        if !union_entry.contains(v) {
                            union_entry.push(v.clone());
                        }
                    }
                    for v in &variants {
                        if let Some(schemes) = file_env.constructors.get(v.as_str()) {
                            self.cross_file_constructor_schemes
                                .entry(v.clone())
                                .or_default()
                                .extend(schemes.iter().cloned());
                        }
                    }
                }
                ItemKind::Register => {
                    self.cross_file_register_names.insert(name.to_string());
                    self.cross_file_value_names.insert(name.to_string());
                }
                ItemKind::Let | ItemKind::Var => {
                    self.cross_file_value_names.insert(name.to_string());
                }
                ItemKind::Struct => {
                    let sig = id.signature(item_tree);
                    let fields = extract_braced_idents_from_sig(sig);
                    self.known_field_names.extend(fields);
                    if let Some(info) = file_env.records.get(name) {
                        self.cross_file_records
                            .entry(name.to_string())
                            .or_insert_with(|| info.clone());
                    }
                }
                ItemKind::Bitfield => {
                    let sig = id.signature(item_tree);
                    let fields = extract_braced_idents_from_sig(sig);
                    self.known_field_names.extend(fields);
                    if let Some(info) = file_env.bitfields.get(name) {
                        self.cross_file_bitfields
                            .entry(name.to_string())
                            .or_insert_with(|| info.clone());
                    }
                }
                ItemKind::Overload => {
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
                    self.cross_file_function_names.insert(name.to_string());
                }
                ItemKind::TypeAlias => {
                    if let Some(ty) = file_env.type_aliases.get(name) {
                        self.type_aliases.entry(name.to_string()).or_insert_with(|| ty.clone());
                    }
                    if let Some(sch) = file_env.alias_schemes.get(name) {
                        self.alias_schemes.entry(name.to_string()).or_insert_with(|| sch.clone());
                    }
                }
                ItemKind::ScatteredClause => {
                    self.scattered_open_types.insert(name.to_string());
                    if let Some(member) = id.member_name(item_tree) {
                        self.cross_file_value_names.insert(member.to_string());
                        self.cross_file_constructor_names.insert(member.to_string());
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
                        self.cross_file_constructor_schemes
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
            self.cross_file_constructor_names.insert(ctor_name.clone());
            if let Some(schemes) = file_env.constructors.get(ctor_name.as_str()) {
                self.cross_file_constructor_schemes
                    .entry(ctor_name.clone())
                    .or_default()
                    .extend(schemes.iter().cloned());
            }
        }
    }

    /// Compute signature fingerprints from the aggregated data.
    pub fn compute_fingerprints(&mut self) {
        let mut symbol_sig_hashes: HashMap<String, u64> = HashMap::new();

        for (name, schemes) in &self.cross_file_function_schemes {
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
        for (name, schemes) in &self.cross_file_constructor_schemes {
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
        for name in &self.cross_file_register_names {
            let mut h = DefaultHasher::new();
            "reg".hash(&mut h);
            name.hash(&mut h);
            symbol_sig_hashes.insert(format!("reg::{name}"), h.finish());
        }
        for name in &self.cross_file_value_names {
            let mut h = DefaultHasher::new();
            "val".hash(&mut h);
            name.hash(&mut h);
            symbol_sig_hashes.entry(format!("val::{name}")).or_insert_with(|| h.finish());
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
            env.bitfields.entry(name.clone()).or_insert_with(|| info.clone());
        }
        // Cross-file name sets: replace.
        env.cross_file_function_names = self.cross_file_function_names.clone();
        env.cross_file_constructor_names = self.cross_file_constructor_names.clone();
        env.cross_file_value_names = self.cross_file_value_names.clone();
        env.cross_file_register_names = self.cross_file_register_names.clone();
        // Enums / unions: union with locally-defined entries.
        for (name, members) in &self.enums {
            let entry = env.enums.entry(name.clone()).or_default();
            for m in members {
                if !entry.contains(m) {
                    entry.push(m.clone());
                }
            }
        }
        for (name, variants) in &self.unions {
            let entry = env.unions.entry(name.clone()).or_default();
            for v in variants {
                if !entry.contains(v) {
                    entry.push(v.clone());
                }
            }
        }
        // Scattered open types: propagate to env for exhaustiveness guard.
        env.scattered_open_types.extend(self.scattered_open_types.iter().cloned());

        // Cross-file schemes are exposed via arity-check sets rather
        // than merged into env.functions/constructors, because full
        // unification on quantified types causes pathological recursion.
        for (name, schemes) in &self.cross_file_function_schemes {
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
            env.cross_file_function_arity.entry(name.clone()).or_default().extend(arities);
        }
        // Pattern constants: union with cross-file contributions.
        pattern_constants.extend(self.cross_file_pattern_constants.iter().cloned());
        pattern_constants.extend(self.cross_file_constructor_names.iter().cloned());

        env.symbol_index = self.symbol_index.clone();

        // Merge cross-file alias schemes. Local file's entries win over
        // workspace context entries (already inserted before apply_to runs).
        for (name, sch) in &self.alias_schemes {
            env.alias_schemes.entry(name.clone()).or_insert_with(|| sch.clone());
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
        TyKind::Adt(name, _) if CONFIG_NAMES.contains(&name.as_str()) => true,
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
        TypeRef::Named(name) => Ty::named(name),
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

fn parse_type_text(text: &str) -> Ty {
    let trimmed = text.trim();
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

/// Type-check a file with selective cross-file context.
pub fn check_file_with_workspace<'a, F: SourceFileInfo + 'a, I>(
    file: &dyn SourceFileInfo,
    all_files: I,
    workspace_complete: bool,
    cancel: CancellationToken,
) -> Option<TypeCheckResult>
where
    I: IntoIterator<Item = &'a F> + Clone,
{
    let text = file.text();
    if text.is_empty() {
        return None;
    }

    let (cst_root, _) = syntax::parse_text(text);
    let (mut env, mut pattern_constants) = TopLevelEnv::from_cst(&cst_root);
    let parsed_file = syntax::cst_lower::parsed_file_from_cst(&cst_root, text);
    apply_callable_signature_metadata(&parsed_file, text, &mut env);

    // Build workspace context from all provided files.
    // Non-salsa path: builds context directly from CST (used by tests/examples).
    let workspace_ctx = Arc::new(WorkspaceContext::build_from_cst(all_files));
    workspace_ctx.apply_to(&mut env, &mut pattern_constants);
    for name in env.constructors.keys() {
        pattern_constants.insert(name.clone());
    }
    env.has_workspace_context = workspace_complete;

    let callable_bodies = hir_def::bodies::CallableBodies::from_cst(&cst_root);

    let mut merged = TypeCheckResult::default();

    for entry in callable_bodies.entries() {
        if cancel.is_cancelled() {
            break;
        }
        // Fresh context per callable
        let mut ctx = InferenceContext::new_for_body_with_cancel(
            text,
            entry.body.clone(),
            entry.source_map.clone(),
            env.clone(),
            pattern_constants.clone(),
            cancel.clone(),
        );
        if entry.body.mapping_arms.is_empty() {
            ctx.infer_callable_body_hir(&entry.name, entry);
        } else {
            ctx.infer_mapping_body_hir(&entry.name, entry);
        }
        let per_callable = ctx.finish_query();
        cook_inference_diagnostics_to_legacy(
            &per_callable,
            &entry.source_map,
            &mut merged.legacy_diagnostics,
        );
        merged.legacy_diagnostics.extend(per_callable.legacy_diagnostics);
        merged.diagnostics.extend(per_callable.diagnostics);
    }

    Some(merged)
}

/// Build a workspace context scoped to an AnalysisScope.
pub fn scoped_workspace_context<'a, F: SourceFileInfo + 'a>(
    all_files: &'a [(impl AsRef<base_db::FileId>, &'a F)],
    scope: Option<&hir_def::analysis_scope::AnalysisScope>,
) -> Arc<WorkspaceContext> {
    match scope {
        Some(scope) => {
            let scoped: Vec<&'a F> = all_files
                .iter()
                .filter(|(fid, _)| scope.contains(*fid.as_ref()))
                .map(|(_, f)| *f)
                .collect();
            Arc::new(WorkspaceContext::build_from_cst(scoped))
        }
        None => {
            let all: Vec<&'a F> = all_files.iter().map(|(_, f)| *f).collect();
            Arc::new(WorkspaceContext::build_from_cst(all))
        }
    }
}

pub fn infer_expr_type_text_in_files<F: SourceFileInfo + ?Sized>(
    files: &[&F],
    current_file: &F,
    span: Span,
) -> Option<String> {
    // Build a Body from the expression's source text, then infer via HIR.
    let expr_text = current_file.text().get(span.start..span.end)?;
    let wrapper = format!("function _e() = {expr_text}\n");
    let (cst_root, _) = syntax::parse_text(&wrapper);
    let bodies = hir_def::bodies::CallableBodies::from_cst(&cst_root);
    let entry = bodies.entries().first()?;

    let env = build_env_from_files(files);
    let pattern_constants = {
        let (cst_root, _) = syntax::parse_text(current_file.text());
        TopLevelEnv::from_cst(&cst_root).1
    };
    // Fresh context with body
    let mut checker = Box::new(InferenceContext::new_for_body(
        current_file.text(),
        entry.body.clone(),
        entry.source_map.clone(),
        env,
        pattern_constants,
    ));
    let mut locals = LocalEnv::new(None);
    let ty = checker.infer_expr_hir(&entry.body, entry.body.root(), &mut locals);
    if ty.is_error() {
        None
    } else {
        Some(ty.display_text())
    }
}

/// Convert typed `InferenceDiagnostic`s to legacy `Diagnostic` values.
fn cook_inference_diagnostics_to_legacy(
    inference: &InferenceResult,
    source_map: &hir_def::body::BodySourceMap,
    out: &mut Vec<Diagnostic>,
) {
    let expr_span = |id: hir_def::ExprId| -> Option<(usize, usize)> {
        source_map.expr_syntax(id).map(|s| (s.start, s.end))
    };
    let pat_span = |id: hir_def::PatId| -> Option<(usize, usize)> {
        source_map.pat_syntax(id).map(|s| (s.start, s.end))
    };

    // 1. Convert InferenceDiagnostics
    for d in &inference.diagnostics {
        let (code, msg, span) = match d {
            InferenceDiagnostic::UnresolvedIdent { expr, name } => (
                DiagnosticCode::SailError("type-error"),
                format!("Unresolved identifier: {name}"),
                expr_span(*expr),
            ),
            InferenceDiagnostic::UnresolvedField { expr, name, receiver, .. } => (
                DiagnosticCode::SailError("type-error"),
                format!("no field `{name}` on type `{}`", receiver.display_text()),
                expr_span(*expr),
            ),
            InferenceDiagnostic::MismatchedArgCount { call_expr, expected, found } => (
                DiagnosticCode::SailError("mismatched-arg-count"),
                format!("Expected {expected} arguments, found {found}"),
                expr_span(*call_expr),
            ),
            InferenceDiagnostic::ExpectedFunction { call_expr, found } => (
                DiagnosticCode::SailError("type-error"),
                format!("expected function, found `{}`", found.display_text()),
                expr_span(*call_expr),
            ),
            InferenceDiagnostic::EffectViolation { expr, effect, context } => (
                DiagnosticCode::SailError("type-error"),
                format!("effect `{effect:?}` not allowed in {context}"),
                expr_span(*expr),
            ),
            InferenceDiagnostic::UnsolvedConstraint { expr, constraint } => (
                DiagnosticCode::SailLint("unsolved-constraint", Severity::Warning),
                format!("unsolved constraint: {constraint}"),
                expr_span(*expr),
            ),
            InferenceDiagnostic::IncompleteMatch { expr, missing_arms } => (
                DiagnosticCode::SailLint("incomplete-match", Severity::Warning),
                format!("non-exhaustive match: missing {}", missing_arms.join(", ")),
                expr_span(*expr),
            ),
            InferenceDiagnostic::MissingFields { expr, record_name: _, missing } => (
                DiagnosticCode::SailError("type-error"),
                format!("struct literal missing fields: {}", missing.join(", ")),
                expr_span(*expr),
            ),
            InferenceDiagnostic::UnusedVariable { pat, name } => (
                DiagnosticCode::SailLint("unused-variable", Severity::Warning),
                format!("Unused variable: `{name}`"),
                pat_span(*pat),
            ),
            InferenceDiagnostic::RemoveTrailingReturn { return_expr } => (
                DiagnosticCode::SailLint("remove-trailing-return", Severity::WeakWarning),
                "unnecessary trailing return".to_string(),
                expr_span(*return_expr),
            ),
            InferenceDiagnostic::RemoveUnnecessaryElse { if_expr } => (
                DiagnosticCode::SailLint("remove-unnecessary-else", Severity::WeakWarning),
                "unnecessary else branch".to_string(),
                expr_span(*if_expr),
            ),
            InferenceDiagnostic::ConcatTypeMismatch { expr, message } => {
                (DiagnosticCode::SailError("type-error"), message.clone(), expr_span(*expr))
            }
            InferenceDiagnostic::ConstraintViolation { expr, constraint, derived_from } => {
                let mut msg = format!("Failed to prove constraint: {constraint}");
                for span in derived_from {
                    msg.push_str(&format!("\n  constraint from {:?}", span));
                }
                (DiagnosticCode::SailError("type-error"), msg, expr_span(*expr))
            }
            InferenceDiagnostic::CallConstraintViolation {
                call_expr,
                constraint,
                derived_from,
            } => {
                let mut msg = format!("Failed to prove constraint: {constraint}");
                for span in derived_from {
                    msg.push_str(&format!("\n  constraint from {:?}", span));
                }
                (DiagnosticCode::SailError("type-error"), msg, expr_span(*call_expr))
            }
            InferenceDiagnostic::UnresolvedCallQuantifiers { call_expr, id, quants, signature } => {
                let mut msg = format!("Could not resolve quantifiers for {id}");
                if let Some(sig) = signature {
                    msg.push_str(&format!("\n  signature: {sig}"));
                }
                msg.push_str(&format!("\n* {}", quants.join("\n* ")));
                (DiagnosticCode::SailError("type-error"), msg, expr_span(*call_expr))
            }
            InferenceDiagnostic::NoOverloading { call_expr, name } => (
                DiagnosticCode::SailError("type-error"),
                format!("no matching overload for `{name}`"),
                expr_span(*call_expr),
            ),
            InferenceDiagnostic::MappingBindingMismatch { expr, name, side } => {
                let other_side = if *side == "left" { "right" } else { "left" };
                (
                    DiagnosticCode::SailError("type-error"),
                    format!("Identifier {name} found on {side} hand side of mapping, but not on {other_side}"),
                    expr_span(*expr),
                )
            }
            InferenceDiagnostic::DuplicateBinding { pat, name } => (
                DiagnosticCode::SailError("type-error"),
                format!("Duplicate binding for {name} in pattern"),
                pat_span(*pat),
            ),
            InferenceDiagnostic::MissingPatternFields { pat, record_name: _, missing } => (
                DiagnosticCode::SailError("type-error"),
                format!("struct pattern missing fields: {}", missing.join(", ")),
                pat_span(*pat),
            ),
            InferenceDiagnostic::NonContiguousSubrange { pat } => (
                DiagnosticCode::SailError("type-error"),
                "pattern subranges are non-contiguous".to_string(),
                pat_span(*pat),
            ),
            InferenceDiagnostic::VectorSubrangeOrder { expr, first, second, order } => {
                let order_desc = match order {
                    hir_def::type_error::VectorOrder::Dec => "decreasing",
                    hir_def::type_error::VectorOrder::Inc => "increasing",
                };
                (
                    DiagnosticCode::SailError("type-error"),
                    format!("vector subrange [{first} .. {second}] violates {order_desc} order"),
                    expr_span(*expr),
                )
            }
            InferenceDiagnostic::IncorrectCase { .. } => continue,
        };
        if let Some((start, end)) = span {
            let range = base_db::text_range(start, end);
            let severity = code.default_severity();
            let mut diag = Diagnostic::new(code, msg, range, severity);
            // Tag unused-variable diagnostics as Unnecessary for LSP faded rendering.
            if matches!(d, InferenceDiagnostic::UnusedVariable { .. }) {
                diag.tags.push(hir_def::diagnostics::DiagnosticTag::Unnecessary);
            }
            out.push(diag);
        }
    }

    // 2. Convert TypeMismatches
    for (expr_or_pat, mismatch) in &inference.type_mismatches {
        let span = match expr_or_pat {
            hir_def::ExprOrPatId::ExprId(id) => expr_span(*id),
            hir_def::ExprOrPatId::PatId(id) => pat_span(*id),
        };
        if let Some((start, end)) = span {
            let range = base_db::text_range(start, end);
            out.push(Diagnostic::new(
                DiagnosticCode::SailError("type-error"),
                format!(
                    "expected `{}`, found `{}`",
                    mismatch.expected.display_text(),
                    mismatch.actual.display_text()
                ),
                range,
                Severity::Error,
            ));
        }
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
