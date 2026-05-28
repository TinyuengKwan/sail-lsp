//! Top-level type environment.
//!
//! Aggregates type information from a file's ItemTree (val specs, type
//! definitions, registers, etc.) into a lookup table used by the
//! inference engine.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::ty::{ConstraintExpr, Ty, TyArg};

#[allow(unused_imports)]
use super::*;

#[derive(Clone, Debug)]
pub struct TopLevelEnv {
    pub(crate) functions: HashMap<String, Vec<Arc<TypeScheme>>>,
    pub(crate) mappings: HashMap<String, Vec<Arc<MappingScheme>>>,
    pub(crate) overloads: HashMap<String, Vec<String>>,
    pub(crate) values: HashMap<String, Ty>,
    pub(crate) registers: HashMap<String, Ty>,
    pub records: HashMap<String, RecordInfo>,
    pub bitfields: HashMap<String, BitfieldInfo>,
    pub(crate) constructors: HashMap<String, Vec<Arc<TypeScheme>>>,
    pub(crate) enums: HashMap<String, Vec<String>>,
    pub(crate) unions: HashMap<String, Vec<String>>,
    /// Types with scattered clauses; match exhaustiveness is skipped.
    pub(crate) scattered_open_types: HashSet<String>,
    pub(crate) type_aliases: HashMap<String, Ty>,
    /// Parameterised alias schemes for on-demand substitution.
    pub(crate) alias_schemes: HashMap<String, Arc<AliasScheme>>,
    pub(crate) cross_file_function_names: HashSet<String>,
    pub(crate) cross_file_constructor_names: HashSet<String>,
    pub(crate) cross_file_value_names: HashSet<String>,
    pub(crate) cross_file_register_names: HashSet<String>,
    /// Cross-file callable arities for arity-only checking on cross-file calls.
    pub(crate) cross_file_function_arity: HashMap<String, Vec<(usize, usize)>>,
    /// Cross-file schemes for return type propagation (not full unification).
    pub(crate) cross_file_schemes: Option<Arc<HashMap<String, Vec<Arc<TypeScheme>>>>>,
    pub(crate) known_field_names: HashSet<String>,
    /// Whether cross_file_* sets reflect the full workspace.
    pub(crate) has_workspace_context: bool,
    pub(crate) global_constraints: Vec<ConstraintExpr>,
    pub(crate) vector_order: VectorOrder,
    pub(crate) symbol_index: WorkspaceSymbolIndex,
}

/// Unified index mapping symbol names to their defining file and kind.
#[derive(Clone, Debug, Default)]
pub struct WorkspaceSymbolIndex {
    entries: HashMap<String, (base_db::FileText, SymbolKind)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Value,
    Register,
    Constructor,
    Mapping,
}

impl WorkspaceSymbolIndex {
    pub fn insert(&mut self, name: String, file: base_db::FileText, kind: SymbolKind) {
        self.entries.entry(name).or_insert((file, kind));
    }

    pub fn get(&self, name: &str) -> Option<(base_db::FileText, SymbolKind)> {
        self.entries.get(name).copied()
    }

    pub fn get_file(&self, name: &str) -> Option<base_db::FileText> {
        self.entries.get(name).map(|(f, _)| *f)
    }
}

/// Parameterised alias scheme for on-demand substitution.
#[derive(Clone, Debug)]
pub struct AliasScheme {
    pub params: Vec<String>,
    pub body: Ty,
    pub config_dependent: bool,
}

impl AliasScheme {
    /// Substitute actual args into the alias body. Returns `None` on arity mismatch.
    pub fn substitute(&self, args: &[crate::ty::TyArg]) -> Option<Ty> {
        use crate::ty::TyArg;
        if args.len() != self.params.len() {
            return None;
        }
        let mut type_subst: HashMap<String, Ty> = HashMap::new();
        let mut value_subst: HashMap<String, String> = HashMap::new();
        for (name, arg) in self.params.iter().zip(args.iter()) {
            let bare = name.strip_prefix('\'').unwrap_or(name).to_string();
            match arg {
                TyArg::Type(t) => {
                    type_subst.insert(name.clone(), t.clone());
                    type_subst.insert(bare, t.clone());
                }
                TyArg::Value(s) => {
                    value_subst.insert(name.clone(), s.clone());
                    value_subst.insert(bare, s.clone());
                }
                TyArg::Nexp(n) => {
                    let s = n.to_string_repr();
                    value_subst.insert(name.clone(), s.clone());
                    value_subst.insert(bare, s);
                }
            }
        }
        Some(substitute_alias_body(&self.body, &type_subst, &value_subst))
    }
}

fn substitute_alias_body(
    ty: &Ty,
    type_subst: &HashMap<String, Ty>,
    value_subst: &HashMap<String, String>,
) -> Ty {
    use crate::ty::{TyArg, TyKind};
    match ty.kind() {
        TyKind::Param(name) => {
            if let Some(r) = type_subst.get(name.as_str()) {
                return r.clone();
            }
            let bare = name.strip_prefix('\'').unwrap_or(name);
            type_subst.get(bare).cloned().unwrap_or_else(|| ty.clone())
        }
        TyKind::App { name, args, text } => {
            let new_args: Vec<TyArg> = args
                .iter()
                .map(|a| match a {
                    TyArg::Type(t) => {
                        TyArg::Type(substitute_alias_body(t, type_subst, value_subst))
                    }
                    TyArg::Value(s) => {
                        // Replace tick-prefixed param names inside the
                        // textual numeric expression. Conservative: only
                        // substitute whole-tick-token occurrences.
                        let mut out = s.clone();
                        for (k, v) in value_subst {
                            // Only handle apostrophe-prefixed keys here.
                            if k.starts_with('\'') {
                                out = out.replace(k, v);
                            }
                        }
                        TyArg::numeric(out)
                    }
                    TyArg::Nexp(n) => {
                        // Re-serialise via Value path so substitution applies.
                        let s = n.to_string_repr();
                        let mut out = s;
                        for (k, v) in value_subst {
                            if k.starts_with('\'') {
                                out = out.replace(k, v);
                            }
                        }
                        TyArg::numeric(out)
                    }
                })
                .collect();
            Ty::app(name.clone(), new_args, text.clone())
        }
        TyKind::Adt(name, args) => {
            let new_args: Vec<TyArg> = args
                .iter()
                .map(|a| match a {
                    TyArg::Type(t) => {
                        TyArg::Type(substitute_alias_body(t, type_subst, value_subst))
                    }
                    other => other.clone(),
                })
                .collect();
            Ty::adt_with_args(name.clone(), new_args)
        }
        TyKind::Tuple(items) => Ty::tuple(
            items.iter().map(|t| substitute_alias_body(t, type_subst, value_subst)).collect(),
        ),
        TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
            let p =
                params.iter().map(|t| substitute_alias_body(t, type_subst, value_subst)).collect();
            let r = substitute_alias_body(ret, type_subst, value_subst);
            Ty::function(p, r)
        }
        TyKind::Bidir { lhs, rhs } => Ty::bidir(
            substitute_alias_body(lhs, type_subst, value_subst),
            substitute_alias_body(rhs, type_subst, value_subst),
        ),
        TyKind::Exist { vars, constraint, inner } => {
            // Don't substitute into shadowed vars.
            let filtered_type: HashMap<String, Ty> = type_subst
                .iter()
                .filter(|(k, _)| !vars.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let filtered_value: HashMap<String, String> = value_subst
                .iter()
                .filter(|(k, _)| !vars.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Ty::exist(
                vars.clone(),
                constraint.clone(),
                substitute_alias_body(inner, &filtered_type, &filtered_value),
            )
        }
        _ => ty.clone(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct RecordInfo {
    pub(crate) params: Vec<String>,
    pub(crate) fields: HashMap<String, Ty>,
}

#[derive(Clone, Debug)]
pub struct BitfieldInfo {
    pub(crate) underlying: Ty,
    pub(crate) fields: HashMap<String, Ty>,
}

impl Default for TopLevelEnv {
    fn default() -> Self {
        Self {
            functions: HashMap::new(),
            mappings: HashMap::new(),
            overloads: HashMap::new(),
            values: HashMap::new(),
            registers: HashMap::new(),
            records: HashMap::new(),
            bitfields: HashMap::new(),
            constructors: HashMap::new(),
            enums: HashMap::new(),
            unions: HashMap::new(),
            scattered_open_types: HashSet::new(),
            type_aliases: HashMap::new(),
            alias_schemes: HashMap::new(),
            cross_file_function_names: HashSet::new(),
            cross_file_constructor_names: HashSet::new(),
            cross_file_value_names: HashSet::new(),
            cross_file_register_names: HashSet::new(),
            cross_file_function_arity: HashMap::new(),
            cross_file_schemes: None,
            known_field_names: HashSet::new(),
            has_workspace_context: false,
            global_constraints: Vec::new(),
            vector_order: VectorOrder::Dec,
            symbol_index: WorkspaceSymbolIndex::default(),
        }
    }
}

impl TopLevelEnv {
    pub fn function_names(&self) -> impl Iterator<Item = &String> {
        self.functions.keys()
    }

    pub fn constructor_names(&self) -> impl Iterator<Item = &String> {
        self.constructors.keys()
    }

    pub fn value_names(&self) -> impl Iterator<Item = &String> {
        self.values.keys()
    }

    pub fn register_names(&self) -> impl Iterator<Item = &String> {
        self.registers.keys()
    }

    pub fn record_names(&self) -> impl Iterator<Item = &String> {
        self.records.keys()
    }

    /// Build a `TopLevelEnv` from a rowan CST root node.
    pub fn from_cst(root: &syntax::SyntaxNode) -> (Self, HashSet<String>) {
        use parser::SyntaxKind as SK;

        let mut env = Self::default();
        let mut pattern_constants = HashSet::new();

        for def_node in root.children() {
            let kind = def_node.kind();
            // Extract name (first IDENT descendant)
            let name = match cst_ident_text(&def_node) {
                Some(n) => n,
                None => {
                    // For let/var definitions with complex patterns like
                    // `let (size as 'size) : ... = ...`, extract ALL ident
                    // names from the definition and register them as values.
                    let first_kw = first_keyword_text(&def_node);
                    if matches!(first_kw.as_deref(), Some("let") | Some("var")) {
                        let ty = find_type_child(&def_node)
                            .map(|n| type_from_cst_node(&n))
                            .unwrap_or(Ty::error());
                        for tok in def_node.descendants_with_tokens() {
                            if let Some(t) = tok.as_token() {
                                if t.kind() == SK::EQ {
                                    break;
                                }
                                if t.kind() == SK::IDENT {
                                    env.values.insert(t.text().to_string(), ty.clone());
                                }
                            }
                        }
                    }
                    continue;
                }
            };

            match kind {
                SK::CALLABLE_SPEC => {
                    // val name : type_signature
                    let first_kw = first_keyword_text(&def_node);
                    if first_kw.as_deref() == Some("mapping") {
                        if let Some(type_node) = find_type_child(&def_node) {
                            if let Some(mapping) = mapping_scheme_from_cst(&type_node) {
                                env.mappings.entry(name).or_default().push(Arc::new(mapping));
                            }
                        }
                    } else {
                        if let Some(type_node) = find_type_child(&def_node) {
                            let scheme = scheme_from_cst_node(&type_node);
                            env.functions.entry(name).or_default().push(Arc::new(scheme));
                        }
                    }
                }
                SK::CALLABLE_DEF => {
                    // function/mapping name(...) [: sig | -> ret] = body
                    let first_kw = first_keyword_text(&def_node);
                    if first_kw.as_deref() == Some("mapping") {
                        if let Some(type_node) = find_type_child(&def_node) {
                            if let Some(mapping) = mapping_scheme_from_cst(&type_node) {
                                env.mappings.entry(name).or_default().push(Arc::new(mapping));
                            }
                        }
                    } else if !env.functions.contains_key(&name) {
                        // Build scheme from CST structure.
                        // 1. Direct child TYPE_* = ret type or inline sig
                        // 2. PARAM_LIST child TYPE_* nodes = param types
                        let direct_type = find_type_child(&def_node);
                        let param_types = extract_param_types_from_param_list(&def_node);

                        match direct_type {
                            Some(ref tn) if has_preceding_arrow(&def_node, tn) => {
                                // `-> RetType` with param types from PARAM_LIST
                                let ret = type_from_cst_node(tn);
                                let params = if param_types.is_empty() {
                                    let n = count_params_in_param_list(&def_node);
                                    if n == 0 {
                                        // Sail: `f()` is `f : unit -> R`
                                        vec![Ty::named("unit".to_string())]
                                    } else {
                                        vec![Ty::error(); n]
                                    }
                                } else {
                                    param_types
                                };
                                let implicit = vec![false; params.len()];
                                env.functions.entry(name).or_default().push(Arc::new(TypeScheme {
                                    quantifiers: Vec::new(),
                                    kind_bounds: HashMap::new(),
                                    constraints: Vec::new(),
                                    params,
                                    implicit_params: implicit,
                                    ret,
                                    declared_effects: Vec::new(),
                                    is_declared_pure: false,
                                }));
                            }
                            Some(ref tn) => {
                                // Full inline sig (`: (T) -> R`)
                                let scheme = scheme_from_cst_node(tn);
                                env.functions.entry(name).or_default().push(Arc::new(scheme));
                            }
                            None if !param_types.is_empty() => {
                                // No ret type, only param types from `: Type`
                                let implicit = vec![false; param_types.len()];
                                env.functions.entry(name).or_default().push(Arc::new(TypeScheme {
                                    quantifiers: Vec::new(),
                                    kind_bounds: HashMap::new(),
                                    constraints: Vec::new(),
                                    params: param_types,
                                    implicit_params: implicit,
                                    ret: Ty::error(),
                                    declared_effects: Vec::new(),
                                    is_declared_pure: false,
                                }));
                            }
                            _ => {}
                        }
                    }
                }
                SK::NAMED_DEF => {
                    let first_kw = first_keyword_text(&def_node);
                    match first_kw.as_deref() {
                        Some("overload") => {
                            // Collect member names (all IDENTs after the overload name)
                            let idents = cst_ident_texts(&def_node);
                            // For `overload operator | = {or_vec}`, the name is
                            // "operator" but we also need to register the actual
                            // operator symbol ("|", "&", "-", etc.) so BinaryOp
                            // inference can find the overloaded functions.
                            let overload_key = if name == "operator" {
                                extract_operator_overload_key(&def_node)
                                    .unwrap_or_else(|| name.clone())
                            } else {
                                name.clone()
                            };
                            if idents.len() > 1 {
                                env.overloads
                                    .entry(overload_key.clone())
                                    .or_default()
                                    .extend(idents.iter().skip(1).cloned());
                                // Also keep the original "operator" entry so
                                // existing call-based overload resolution still
                                // works for `operator(...)` patterns.
                                if overload_key != name {
                                    env.overloads
                                        .entry(name)
                                        .or_default()
                                        .extend(idents.into_iter().skip(1));
                                }
                            }
                        }
                        Some("struct") => {
                            // Extract field names and types
                            let fields = extract_struct_fields_from_cst(&def_node);
                            for field_name in fields.keys() {
                                env.known_field_names.insert(field_name.clone());
                            }
                            let params = extract_type_params_from_cst(&def_node);
                            // Register Mk_{name} constructor (takes all fields,
                            // returns the struct type).
                            let ctor_name = format!("Mk_{name}");
                            let field_tys: Vec<Ty> = fields.values().cloned().collect();
                            let param_ty = if field_tys.len() == 1 {
                                field_tys.into_iter().next().unwrap()
                            } else {
                                Ty::tuple(field_tys)
                            };
                            let ret_ty = Ty::named(name.clone());
                            env.constructors.entry(ctor_name).or_default().push(Arc::new(
                                TypeScheme {
                                    quantifiers: Vec::new(),
                                    kind_bounds: HashMap::new(),
                                    constraints: Vec::new(),
                                    params: vec![param_ty],
                                    implicit_params: vec![false],
                                    ret: ret_ty,
                                    declared_effects: Vec::new(),
                                    is_declared_pure: false,
                                },
                            ));
                            env.records.insert(name, RecordInfo { params, fields });
                        }
                        Some("enum") => {
                            // Enum members: all IDENTs inside { }
                            let members = extract_braced_idents(&def_node);
                            let entry = env.enums.entry(name.clone()).or_default();
                            for member in &members {
                                pattern_constants.insert(member.clone());
                                env.values.insert(member.clone(), Ty::named(name.clone()));
                                if !entry.contains(member) {
                                    entry.push(member.clone());
                                }
                            }
                            // Sail compiler auto-generates `num_of_{name} : name -> nat`
                            // for each enum definition. Register it so callers resolve.
                            let num_of_name = format!("num_of_{name}");
                            env.functions.entry(num_of_name).or_default().push(Arc::new(
                                plain_scheme(
                                    vec![Ty::named(name.clone())],
                                    Ty::named("nat".to_string()),
                                ),
                            ));
                        }
                        Some("union") => {
                            // Union variants: Name : Type inside { }
                            let type_params = extract_type_params_from_cst(&def_node);
                            let variants = extract_union_variants_from_cst(&def_node);

                            // Build return type: bare name or App with type params
                            let ret = if type_params.is_empty() {
                                Ty::named(name.clone())
                            } else {
                                let args: Vec<TyArg> = type_params
                                    .iter()
                                    .map(|p| TyArg::Type(Ty::param(p.clone())))
                                    .collect();
                                let text = format!(
                                    "{}({})",
                                    name,
                                    type_params
                                        .iter()
                                        .map(|p| p.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                );
                                Ty::app(name.clone(), args, text)
                            };

                            let mut variant_names = Vec::new();
                            for (vname, vty) in &variants {
                                // ALL union constructors take an argument
                                // (even unit-payload ones) — mirrors from_ast.
                                let scheme = TypeScheme {
                                    quantifiers: type_params.clone(),
                                    kind_bounds: HashMap::new(),
                                    constraints: Vec::new(),
                                    params: vec![vty.clone()],
                                    implicit_params: vec![false],
                                    ret: ret.clone(),
                                    declared_effects: Vec::new(),
                                    is_declared_pure: false,
                                };
                                env.constructors
                                    .entry(vname.clone())
                                    .or_default()
                                    .push(Arc::new(scheme));
                                variant_names.push(vname.clone());
                            }
                            env.unions.entry(name).or_default().extend(variant_names);
                        }
                        Some("newtype") => {
                            // Newtypes desugar to a union with a single arm.
                            // Constructor is `arg_typ -> variant`.
                            let idents = cst_ident_texts(&def_node);
                            if idents.len() >= 2 {
                                let ctor_name = idents[1].clone();
                                let inner_ty = find_type_child(&def_node)
                                    .map(|n| type_from_cst_node(&n))
                                    .unwrap_or(Ty::error());
                                let outer_ty = Ty::named(name.clone());
                                // Register constructor: Ctor(inner) -> outer
                                env.constructors.entry(ctor_name.clone()).or_default().push(
                                    Arc::new(TypeScheme {
                                        quantifiers: Vec::new(),
                                        kind_bounds: HashMap::new(),
                                        constraints: Vec::new(),
                                        params: vec![inner_ty],
                                        implicit_params: vec![false],
                                        ret: outer_ty,
                                        declared_effects: Vec::new(),
                                        is_declared_pure: false,
                                    }),
                                );
                                pattern_constants.insert(ctor_name);
                            }
                        }
                        Some("register") => {
                            // Extract register type. Full initializer type
                            // checking is deferred to the wf.rs pass.
                            if let Some(type_node) = find_type_child(&def_node) {
                                let ty = type_from_cst_node(&type_node);
                                env.values.insert(name.clone(), ty.clone());
                                env.registers.insert(name, ty);
                            }
                        }
                        Some("let") | Some("var") => {
                            let ty = find_type_child(&def_node)
                                .map(|n| type_from_cst_node(&n))
                                .unwrap_or(Ty::error());
                            env.values.insert(name, ty);
                        }
                        Some("bitfield") => {
                            if let Some(info) = bitfield_info_from_cst(&def_node) {
                                for field_name in info.fields.keys() {
                                    env.known_field_names.insert(field_name.clone());
                                }
                                // Register Mk_{name} constructor (takes
                                // underlying bitvector, returns bitfield type).
                                let ctor_name = format!("Mk_{name}");
                                let ret_ty = Ty::named(name.clone());
                                env.constructors.entry(ctor_name).or_default().push(Arc::new(
                                    TypeScheme {
                                        quantifiers: Vec::new(),
                                        kind_bounds: HashMap::new(),
                                        constraints: Vec::new(),
                                        params: vec![info.underlying.clone()],
                                        implicit_params: vec![false],
                                        ret: ret_ty,
                                        declared_effects: Vec::new(),
                                        is_declared_pure: false,
                                    },
                                ));
                                let field_entries: Vec<_> = info
                                    .fields
                                    .iter()
                                    .map(|(n, t)| (n.clone(), t.clone()))
                                    .collect();
                                // Add synthetic `bits` field containing the full bitvector.
                                let mut bf_info = info.clone();
                                bf_info.fields.insert("bits".to_string(), info.underlying.clone());
                                env.known_field_names.insert("bits".to_string());
                                env.bitfields.insert(name.clone(), bf_info);
                                for (field_name, field_ty) in field_entries {
                                    synthesize_bitfield_accessors(
                                        &mut env,
                                        &name,
                                        &field_name,
                                        field_ty,
                                    );
                                }
                                synthesize_bitfield_accessors(
                                    &mut env,
                                    &name,
                                    "bits",
                                    info.underlying,
                                );
                            } else {
                                // Fallback: at least extract field names
                                let fields = extract_braced_idents(&def_node);
                                for field_name in &fields {
                                    env.known_field_names.insert(field_name.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                SK::SCATTERED_CLAUSE_DEF => {
                    // Detect clause type from the FIRST keyword token.
                    let first_kw = def_node
                        .descendants_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| !t.kind().is_trivia())
                        .map(|t| t.kind());
                    if first_kw == Some(SK::KW_ENUM) {
                        // Enum clause → register member.
                        let idents = cst_ident_texts(&def_node);
                        if idents.len() >= 2 {
                            let parent_name = &idents[0];
                            let member_name = &idents[1];
                            pattern_constants.insert(member_name.clone());
                            env.values.insert(member_name.clone(), Ty::named(parent_name.clone()));
                            let entry = env.enums.entry(parent_name.clone()).or_default();
                            if !entry.contains(member_name) {
                                entry.push(member_name.clone());
                            }
                        }
                    } else if first_kw == Some(SK::KW_UNION) {
                        // Union clause → register constructor with correct tuple type.
                        // E.g., "union clause instruction = FVVTYPE : (fvvfunct6, ...)"
                        let idents = cst_ident_texts(&def_node);
                        if idents.len() >= 2 {
                            let parent_name = &idents[0];
                            let ctor_name = &idents[1];
                            let parent_ty = Ty::named(parent_name.clone());

                            // Extract payload type: try CST type node first,
                            // then fall back to text parsing after `:`.
                            let payload_ty = find_type_child(&def_node)
                                .map(|n| type_from_cst_node(&n))
                                .or_else(|| {
                                    let text = def_node.text().to_string();
                                    text.find('=')
                                        .and_then(|eq| text[eq + 1..].find(':').map(|c| eq + 1 + c))
                                        .map(|colon| {
                                            let type_text = text[colon + 1..].trim();
                                            let tr = hir_def::hir::type_ref::type_ref_from_text(
                                                type_text,
                                            );
                                            super::workspace::ty_from_type_ref_pub(&tr)
                                        })
                                })
                                .unwrap_or(Ty::named("unit"));

                            let scheme = Arc::new(TypeScheme {
                                quantifiers: Vec::new(),
                                kind_bounds: HashMap::new(),
                                constraints: Vec::new(),
                                params: vec![payload_ty],
                                implicit_params: vec![false],
                                ret: parent_ty,
                                declared_effects: Vec::new(),
                                is_declared_pure: false,
                            });
                            env.constructors.entry(ctor_name.clone()).or_default().push(scheme);
                            pattern_constants.insert(ctor_name.clone());
                            let entry = env.unions.entry(parent_name.clone()).or_default();
                            if !entry.contains(ctor_name) {
                                entry.push(ctor_name.clone());
                            }
                        }
                    }
                }
                SK::TYPE_ALIAS_DEF => {
                    // Two parser shapes:
                    //   1) `type X = Body` — Body is a proper TYPE node
                    //      (find_type_after_eq returns it).
                    //   2) `type X('a) = Body` — parameterised; the parser
                    //      flattens `('a)` and Body as raw tokens (no
                    //      TYPE_APP node). Falls back to text-parsing.
                    let underlying = if let Some(type_node) = find_type_after_eq(&def_node) {
                        Some(type_from_cst_node(&type_node))
                    } else {
                        let text = def_node.text().to_string();
                        text.find('=').and_then(|eq| {
                            // Skip `==` and `=>` (won't occur at top level
                            // of a TYPE_ALIAS_DEF but defensive).
                            let bytes = text.as_bytes();
                            if bytes.get(eq + 1) == Some(&b'=') || bytes.get(eq + 1) == Some(&b'>')
                            {
                                None
                            } else {
                                let body_text = text[eq + 1..].trim();
                                if body_text.is_empty() {
                                    None
                                } else {
                                    let tr = hir_def::hir::type_ref::type_ref_from_text(body_text);
                                    Some(super::workspace::ty_from_type_ref_pub(&tr))
                                }
                            }
                        })
                    };
                    if let Some(underlying) = underlying {
                        // Extract formal parameter names from text between
                        // the alias name and `=`. Looks for `'<word>` tokens.
                        let mut params: Vec<String> = Vec::new();
                        let text = def_node.text().to_string();
                        if let Some(eq_idx) = text.find('=') {
                            // Skip `==`/`=>`.
                            let bytes = text.as_bytes();
                            let real_eq = if bytes.get(eq_idx + 1) == Some(&b'=')
                                || bytes.get(eq_idx + 1) == Some(&b'>')
                            {
                                None
                            } else {
                                Some(eq_idx)
                            };
                            if let Some(eq) = real_eq {
                                let head = &text[..eq];
                                let mut chars = head.char_indices().peekable();
                                while let Some((i, c)) = chars.next() {
                                    if c == '\'' {
                                        let start = i + 1;
                                        let mut end = start;
                                        while let Some(&(j, nc)) = chars.peek() {
                                            if nc.is_alphanumeric() || nc == '_' {
                                                end = j + nc.len_utf8();
                                                chars.next();
                                            } else {
                                                break;
                                            }
                                        }
                                        if end > start {
                                            let pname = format!("'{}", &head[start..end]);
                                            if !params.contains(&pname) {
                                                params.push(pname);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        let config_dependent =
                            super::workspace::contains_config_dependent(&underlying);
                        env.alias_schemes.insert(
                            name.clone(),
                            Arc::new(AliasScheme {
                                params,
                                body: underlying.clone(),
                                config_dependent,
                            }),
                        );
                        env.type_aliases.insert(name, underlying);
                    }
                }
                SK::CONSTRAINT_DEF => {
                    let def_text_dbg = def_node.text().to_string();
                    let _body_dbg = def_text_dbg
                        .trim()
                        .strip_prefix("constraint")
                        .unwrap_or(&def_text_dbg)
                        .trim();
                    let _has_type = find_type_child(&def_node).is_some();
                    // Global constraint — may be a TYPE node or expression
                    if let Some(type_node) = find_type_child(&def_node) {
                        let constraint = constraint_from_cst_node(&type_node);
                        env.global_constraints.push(constraint);
                    } else {
                        // Expression-based constraint: extract from source text
                        let def_text = def_node.text().to_string();
                        let body =
                            def_text.trim().strip_prefix("constraint").unwrap_or(&def_text).trim();
                        let result = constraint_expr_from_expr_text(body);
                        if let Some(constraint) = result {
                            env.global_constraints.push(constraint);
                        }
                    }
                }
                SK::DEFAULT_DEF => {
                    // `default Order dec` or `default Order inc`
                    let tokens: Vec<String> = def_node
                        .descendants_with_tokens()
                        .filter_map(|el| el.into_token())
                        .filter(|t| !t.kind().is_trivia())
                        .map(|t| t.text().to_string())
                        .collect();
                    if tokens.len() >= 3 && tokens[1].eq_ignore_ascii_case("Order") {
                        if tokens[2].eq_ignore_ascii_case("dec") {
                            env.vector_order = VectorOrder::Dec;
                        } else if tokens[2].eq_ignore_ascii_case("inc") {
                            env.vector_order = VectorOrder::Inc;
                        }
                    }
                }
                _ => {}
            }
        }

        (env, pattern_constants)
    }

    // resolve_alias removed
    // through InferenceTable.normalize_alias_ty(), not TopLevelEnv.
    // Callers in checker.rs now use self.table.normalize_alias_ty().

    /// Returns true if `name` is a plausible top-level symbol (intentionally permissive).
    pub(crate) fn top_level_symbol_exists(&self, name: &str) -> bool {
        if is_prelude_value(name) {
            return true;
        }
        self.values.contains_key(name)
            || self.functions.contains_key(name)
            || self.constructors.contains_key(name)
            || self.registers.contains_key(name)
            || self.records.contains_key(name)
            || self.bitfields.contains_key(name)
            || self.type_aliases.contains_key(name)
            || self.overloads.contains_key(name)
            || self.cross_file_function_names.contains(name)
            || self.cross_file_constructor_names.contains(name)
            || self.cross_file_value_names.contains(name)
            || self.cross_file_register_names.contains(name)
            || self.known_field_names.contains(name)
    }

    pub(crate) fn lookup_value(&self, locals: &LocalEnv, name: &str) -> Option<Ty> {
        locals
            .lookup(name)
            .cloned()
            .or_else(|| self.values.get(name).cloned())
            // Registers are accessed via bare identifiers (e.g., `x25 = v`).
            .or_else(|| self.registers.get(name).cloned())
            // Bitfield field names used as vector indices return `int`
            // so the identifier is recognized as a valid index.
            .or_else(|| {
                if self.known_field_names.contains(name)
                    && !self.values.contains_key(name)
                    && !self.registers.contains_key(name)
                {
                    Some(Ty::named("int"))
                } else {
                    None
                }
            })
            .or_else(|| {
                let schemes = self.functions.get(name)?;
                if schemes.len() == 1 {
                    Some(Ty::function(schemes[0].params.clone(), schemes[0].ret.clone()))
                } else {
                    None
                }
            })
            .or_else(|| {
                let schemes = self.constructors.get(name)?;
                if schemes.len() == 1 {
                    Some(Ty::function(schemes[0].params.clone(), schemes[0].ret.clone()))
                } else {
                    None
                }
            })
    }

    pub(crate) fn lookup_functions(&self, name: &str) -> SmallVec<[Arc<TypeScheme>; 4]> {
        let mut out: SmallVec<[Arc<TypeScheme>; 4]> = SmallVec::new();
        if let Some(members) = self.overloads.get(name) {
            for member in members {
                // Look up in local functions first
                if let Some(schemes) = self.functions.get(member) {
                    out.extend(schemes.iter().cloned());
                }
                // Also look up in cross-file schemes (stdlib, other workspace files).
                // This is needed for overloaded operators whose implementations
                // (e.g., or_vec, shift_bits_right) are defined in the Sail stdlib.
                if let Some(ref cf) = self.cross_file_schemes {
                    if let Some(schemes) = cf.get(member) {
                        out.extend(schemes.iter().cloned());
                    }
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
        if let Some(schemes) = self.functions.get(name) {
            out.extend(schemes.iter().cloned());
        }
        // Cross-file fallback for direct function lookup
        if out.is_empty() {
            if let Some(ref cf) = self.cross_file_schemes {
                if let Some(schemes) = cf.get(name) {
                    out.extend(schemes.iter().cloned());
                }
            }
        }
        if let Some(schemes) = self.constructors.get(name) {
            out.extend(schemes.iter().cloned());
        }
        out
    }

    pub(crate) fn lookup_mappings(&self, name: &str) -> SmallVec<[Arc<MappingScheme>; 4]> {
        let mut out: SmallVec<[Arc<MappingScheme>; 4]> = SmallVec::new();
        if let Some(schemes) = self.mappings.get(name) {
            out.extend(schemes.iter().cloned());
        }
        out
    }
}

fn synthesize_bitfield_accessors(
    env: &mut TopLevelEnv,
    bitfield_name: &str,
    field_name: &str,
    field_ty: Ty,
) {
    let bitfield_ty = Ty::named(bitfield_name.to_string());
    let getter_name = format!("_get_{bitfield_name}_{field_name}");
    let setter_name = format!("_set_{bitfield_name}_{field_name}");
    let updater_name = format!("_update_{bitfield_name}_{field_name}");
    let overload_name = format!("_mod_{field_name}");

    env.functions
        .entry(getter_name.clone())
        .or_default()
        .push(Arc::new(plain_scheme(vec![bitfield_ty.clone()], field_ty.clone())));
    env.functions.entry(setter_name.clone()).or_default().push(Arc::new(plain_scheme(
        vec![register_ty(bitfield_ty.clone()), field_ty.clone()],
        Ty::named("unit".to_string()),
    )));
    env.functions
        .entry(updater_name)
        .or_default()
        .push(Arc::new(plain_scheme(vec![bitfield_ty.clone(), field_ty.clone()], bitfield_ty)));
    env.overloads.entry(overload_name).or_default().extend([getter_name, setter_name]);
}

pub(crate) fn apply_callable_signature_metadata(
    parsed: &syntax::parser_lower::ParsedFile,
    text: &str,
    env: &mut TopLevelEnv,
) {
    let mut best_signatures =
        HashMap::<(String, usize), (usize, Vec<bool>, Vec<Ty>, Option<String>)>::new();

    for signature in hir_def::callable_info::collect_callable_signatures_from(parsed, text) {
        let implicit_params =
            signature.params.iter().map(|param| param.is_implicit).collect::<Vec<_>>();
        let signature_params = signature
            .params
            .iter()
            .map(|param| {
                param
                    .name
                    .split_once(':')
                    .map(|(_, ty)| Ty::named(ty.trim().to_string()))
                    .unwrap_or(Ty::error())
            })
            .collect::<Vec<_>>();
        let score = signature
            .params
            .iter()
            .filter(|param| param.is_implicit || param.name.contains(':'))
            .count()
            + usize::from(signature.return_type.is_some());
        let key = (signature.name.clone(), signature.params.len());
        match best_signatures.get(&key) {
            Some((best_score, _, _, _)) if *best_score >= score => {}
            _ => {
                best_signatures
                    .insert(key, (score, implicit_params, signature_params, signature.return_type));
            }
        }
    }

    // Sort by key so iteration order is stable — otherwise HashMap's
    // randomized hasher can cause `check_file_with_workspace` to observe
    // a different scheme shape on different runs (e.g. tuple-flattened
    // vs. not) and emit different `Unresolved identifier` diagnostics.
    let mut best_signatures: Vec<_> = best_signatures.into_iter().collect();
    best_signatures.sort_by(|a, b| a.0.cmp(&b.0));
    for ((name, _param_count), (_, implicit_params, signature_params, return_type)) in
        best_signatures
    {
        let Some(schemes) = env.functions.get_mut(&name) else {
            continue;
        };
        // Find a matching scheme by index. We can't keep an `&mut Arc<...>`
        // borrow across `Arc::make_mut` (which needs exclusive access to
        // the slot), so we resolve the index first and mutate after.
        let matched_idx = schemes.iter().position(|scheme| {
            if scheme.params.len() == implicit_params.len() {
                return true;
            }
            if scheme.params.len() == 1 {
                if let TyKind::Tuple(items) = scheme.params[0].kind() {
                    return items.len() == implicit_params.len();
                }
            }
            false
        });
        if let Some(idx) = matched_idx {
            let scheme = Arc::make_mut(&mut schemes[idx]);
            if scheme.params.len() == 1 {
                if let TyKind::Tuple(items) = scheme.params[0].kind() {
                    if items.len() == implicit_params.len() {
                        let new_params = items.clone();
                        scheme.params = new_params;
                    }
                }
            }
            scheme.implicit_params = implicit_params.clone();
            if scheme.ret.is_unknown() {
                if let Some(ret) = return_type.as_deref() {
                    scheme.ret = Ty::named(ret.to_string());
                }
            }
            continue;
        }

        if schemes.len() == 1 {
            let scheme = Arc::make_mut(&mut schemes[0]);
            // Only override params if the scheme has error types (no type info).
            // Don't override correct params from from_cst with metadata that
            // may miscount due to pattern destructuring (e.g., `Physaddr(addr)`
            // counted as 2 params instead of 1).
            // Only apply metadata when it's consistent with the existing
            // scheme. Pattern destructuring may cause metadata to miscount.
            if signature_params.len() == scheme.params.len() {
                // Same count — safe to apply both params and implicit flags
                scheme.params = signature_params;
                scheme.implicit_params = implicit_params;
            } else if scheme.params.iter().all(|p| p.is_error()) {
                // Scheme has no type info — use metadata as best guess
                scheme.params = signature_params;
                scheme.implicit_params = implicit_params;
            }
            // Otherwise: keep existing scheme params, don't override
            // with inconsistent metadata count
            if scheme.ret.is_unknown() {
                if let Some(ret) = return_type.as_deref() {
                    scheme.ret = Ty::named(ret.to_string());
                }
            }
        }
    }
}

pub(crate) fn infer_literal_type(literal: &Literal) -> Ty {
    match literal {
        Literal::Bool(_) => Ty::named("bool".to_string()),
        Literal::Unit => Ty::named("unit".to_string()),
        Literal::Number(text) => {
            if text.contains('.') {
                Ty::named("real".to_string())
            } else {
                Ty::named("int".to_string())
            }
        }
        Literal::Binary(text) => {
            let bits = text.trim_start_matches("0b").chars().filter(|ch| *ch != '_').count();
            Ty::app("bits", vec![TyArg::numeric(bits.to_string())], format!("bits({bits})"))
        }
        Literal::Hex(text) => {
            let bits = text.trim_start_matches("0x").chars().filter(|ch| *ch != '_').count() * 4;
            Ty::app("bits", vec![TyArg::numeric(bits.to_string())], format!("bits({bits})"))
        }
        Literal::String(_) => Ty::named("string".to_string()),
        Literal::BitZero | Literal::BitOne => Ty::named("bit".to_string()),
        Literal::Undefined => Ty::error(),
    }
}

pub(crate) fn bits_ty(width: impl ToString) -> Ty {
    let width = width.to_string();
    Ty::app("bits", vec![TyArg::numeric(width.clone())], format!("bits({width})"))
}

pub(crate) fn register_ty(inner: Ty) -> Ty {
    let inner_text = inner.display_text();
    Ty::app("register", vec![TyArg::Type(inner)], format!("register({inner_text})"))
}

pub(crate) fn vector_ty(width: impl ToString, elem: Ty) -> Ty {
    let width = width.to_string();
    let elem_text = elem.display_text();
    Ty::app(
        "vector",
        vec![TyArg::numeric(width.clone()), TyArg::Type(elem)],
        format!("vector({width}, {elem_text})"),
    )
}

fn plain_scheme(params: Vec<Ty>, ret: Ty) -> TypeScheme {
    let param_count = params.len();
    TypeScheme {
        declared_effects: Vec::new(),
        is_declared_pure: false,
        quantifiers: Vec::new(),
        kind_bounds: HashMap::new(),
        constraints: Vec::new(),
        params,
        implicit_params: vec![false; param_count],
        ret,
    }
}

pub(crate) fn parse_int_literal(text: &str) -> Option<i64> {
    let text = text.trim().replace('_', "");
    if let Some(rest) = text.strip_prefix("0b") {
        i64::from_str_radix(rest, 2).ok()
    } else if let Some(rest) = text.strip_prefix("0x") {
        i64::from_str_radix(rest, 16).ok()
    } else {
        text.parse().ok()
    }
}

/// Convert `Ty` into `MatchTy` for the pattern usefulness algorithm.
pub(crate) fn ty_to_match_ty(ty: &Ty) -> MatchTy {
    match ty.kind() {
        TyKind::Scalar(crate::ty::Scalar::Bool) => MatchTy::Bool,
        TyKind::Scalar(s) => MatchTy::Named(s.name().to_string(), Vec::new()),
        TyKind::Adt(name, _) if name == "bool" => MatchTy::Bool,
        TyKind::Adt(name, _) if name == "unit" => MatchTy::Named("unit".to_string(), Vec::new()),
        TyKind::Adt(name, _) => MatchTy::Named(name.clone(), Vec::new()),
        TyKind::App { name, args, .. } if name == "list" => match args.first() {
            Some(TyArg::Type(elem)) => MatchTy::List(Box::new(ty_to_match_ty(elem))),
            _ => MatchTy::List(Box::new(MatchTy::Unknown)),
        },
        TyKind::App { name, args, .. } if name == "bits" => {
            // bits(N) — try to extract the width from the Value/Nexp arg.
            if let Some(width) =
                args.iter().find_map(|arg| arg.as_value_str().and_then(|v| v.parse::<usize>().ok()))
            {
                MatchTy::Bits(width)
            } else {
                MatchTy::Named(name.clone(), Vec::new())
            }
        }
        TyKind::App { name, args, .. } if name == "vector" => {
            // Vector type-args are `(length, order, elem)`; the element
            // type is the last positional type-arg. Fall back to Unknown
            // when we can't pick it out.
            let elem = args
                .iter()
                .filter_map(|arg| match arg {
                    TyArg::Type(t) => Some(t),
                    _ => None,
                })
                .next_back()
                .map(ty_to_match_ty)
                .unwrap_or(MatchTy::Unknown);
            MatchTy::Vector(Box::new(elem))
        }
        TyKind::App { name, args, .. } => {
            let lowered: Vec<MatchTy> = args
                .iter()
                .filter_map(|arg| match arg {
                    TyArg::Type(t) => Some(ty_to_match_ty(t)),
                    TyArg::Nexp(_) | TyArg::Value(_) => None,
                })
                .collect();
            MatchTy::Named(name.clone(), lowered)
        }
        TyKind::Tuple(items) => MatchTy::Tuple(items.iter().map(ty_to_match_ty).collect()),
        TyKind::Error
        | TyKind::Infer(crate::ty::InferTy(_))
        | TyKind::Param(_)
        | TyKind::FnPtr { .. } => MatchTy::Unknown,
        TyKind::Exist { inner, .. } => ty_to_match_ty(inner),
        TyKind::Bidir { .. } => MatchTy::Unknown,
        TyKind::Abstract { name, .. } => MatchTy::Named(name.clone(), Vec::new()),
    }
}

/// Like `ty_to_match_ty` but applies a quantifier substitution.
pub(crate) fn ty_to_match_ty_with_subst(
    ty: &Ty,
    subst: &HashMap<String, MatchTy>,
    records: &HashMap<String, RecordInfo>,
) -> MatchTy {
    match ty.kind() {
        TyKind::Param(name) => subst.get(name).cloned().unwrap_or(MatchTy::Unknown),
        TyKind::Scalar(crate::ty::Scalar::Bool) => MatchTy::Bool,
        TyKind::Scalar(s) => MatchTy::Named(s.name().to_string(), Vec::new()),
        TyKind::Adt(name, _) if name == "bool" => MatchTy::Bool,
        TyKind::Adt(name, _) => {
            let mt = MatchTy::Named(name.clone(), Vec::new());
            promote_record_ty(&mt, records)
        }
        TyKind::App { name, args, .. } if name == "list" => {
            let elem = match args.first() {
                Some(TyArg::Type(t)) => ty_to_match_ty_with_subst(t, subst, records),
                _ => MatchTy::Unknown,
            };
            MatchTy::List(Box::new(elem))
        }
        TyKind::App { name, args, .. } if name == "bits" => {
            if let Some(width) =
                args.iter().find_map(|arg| arg.as_value_str().and_then(|v| v.parse::<usize>().ok()))
            {
                MatchTy::Bits(width)
            } else {
                MatchTy::Named(name.clone(), Vec::new())
            }
        }
        TyKind::App { name, args, .. } if name == "vector" => {
            let elem = args
                .iter()
                .filter_map(|arg| match arg {
                    TyArg::Type(t) => Some(ty_to_match_ty_with_subst(t, subst, records)),
                    _ => None,
                })
                .next_back()
                .unwrap_or(MatchTy::Unknown);
            MatchTy::Vector(Box::new(elem))
        }
        TyKind::App { name, args, .. } => {
            let lowered: Vec<MatchTy> = args
                .iter()
                .filter_map(|arg| match arg {
                    TyArg::Type(t) => Some(ty_to_match_ty_with_subst(t, subst, records)),
                    TyArg::Nexp(_) | TyArg::Value(_) => None,
                })
                .collect();
            let mt = MatchTy::Named(name.clone(), lowered);
            promote_record_ty(&mt, records)
        }
        TyKind::Tuple(items) => MatchTy::Tuple(
            items.iter().map(|i| ty_to_match_ty_with_subst(i, subst, records)).collect(),
        ),
        TyKind::Error | TyKind::Infer(crate::ty::InferTy(_)) | TyKind::FnPtr { .. } => {
            MatchTy::Unknown
        }
        TyKind::Exist { inner, .. } => ty_to_match_ty_with_subst(inner, subst, records),
        TyKind::Bidir { .. } => MatchTy::Unknown,
        TyKind::Abstract { name, .. } => MatchTy::Named(name.clone(), Vec::new()),
    }
}

/// Promote `Named(name)` to `Record(name)` for known structs.
pub(crate) fn promote_record_ty<V>(ty: &MatchTy, records: &HashMap<String, V>) -> MatchTy {
    match ty {
        MatchTy::Named(name, _args) if records.contains_key(name) => {
            // Records lose their type args in the matrix today; the
            // record code path doesn't thread them through.
            MatchTy::Record(name.clone())
        }
        MatchTy::Named(name, args) => MatchTy::Named(
            name.clone(),
            args.iter().map(|a| promote_record_ty(a, records)).collect(),
        ),
        MatchTy::Tuple(items) => {
            MatchTy::Tuple(items.iter().map(|item| promote_record_ty(item, records)).collect())
        }
        MatchTy::List(elem) => MatchTy::List(Box::new(promote_record_ty(elem, records))),
        MatchTy::Vector(elem) => MatchTy::Vector(Box::new(promote_record_ty(elem, records))),
        other => other.clone(),
    }
}

pub(crate) fn build_env_from_files<F: hir_def::callgraph::SourceFileInfo + ?Sized>(
    files: &[&F],
) -> TopLevelEnv {
    let mut env = TopLevelEnv::default();
    for &file in files {
        let text = file.text();
        let (cst_root, _) = syntax::parse_text(text);
        let (mut file_env, _) = TopLevelEnv::from_cst(&cst_root);
        let parsed_file = syntax::cst_lower::parsed_file_from_cst(&cst_root, text);
        apply_callable_signature_metadata(&parsed_file, text, &mut file_env);
        for (name, schemes) in file_env.functions {
            env.functions.entry(name).or_default().extend(schemes);
        }
        for (name, schemes) in file_env.mappings {
            env.mappings.entry(name).or_default().extend(schemes);
        }
        for (name, members) in file_env.overloads {
            env.overloads.entry(name).or_default().extend(members);
        }
        env.values.extend(file_env.values);
        env.registers.extend(file_env.registers);
        env.records.extend(file_env.records);
        env.bitfields.extend(file_env.bitfields);
        env.type_aliases.extend(file_env.type_aliases);
        env.global_constraints.extend(file_env.global_constraints);
        for (name, schemes) in file_env.constructors {
            env.constructors.entry(name).or_default().extend(schemes);
        }
    }
    env
}
