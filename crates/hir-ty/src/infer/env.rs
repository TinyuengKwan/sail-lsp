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

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// `type is_mem_width('w) = 'w in {1, 2, 4, 8}` — a type alias whose
    /// body is a *constraint*. Upstream expands these before proving
    /// anything (`Env.expand_constraint_synonyms`, from `prove`).
    pub(crate) constraint_synonyms: HashMap<String, (Vec<String>, ConstraintExpr)>,
    /// Parameterised alias schemes for on-demand substitution.
    pub(crate) alias_schemes: HashMap<String, Arc<AliasScheme>>,
    pub(crate) cross_file_function_names: Arc<HashSet<String>>,
    pub(crate) cross_file_constructor_names: Arc<HashSet<String>>,
    /// Payload signatures for the constructors above. Separate from
    /// `constructors` so nothing that walks the local ones picks these up.
    pub(crate) cross_file_constructor_schemes: Arc<HashMap<String, Vec<Arc<TypeScheme>>>>,
    /// Signatures for mappings declared in another file. `val encdec : ast
    /// <-> bits(32)` and its `mapping clause`es routinely live apart.
    pub(crate) cross_file_mapping_schemes: Arc<HashMap<String, Vec<Arc<MappingScheme>>>>,
    pub(crate) cross_file_value_names: Arc<HashSet<String>>,
    pub(crate) cross_file_register_names: Arc<HashSet<String>>,
    /// Cross-file callable arities for arity-only checking on cross-file calls.
    pub(crate) cross_file_function_arity: Arc<HashMap<String, Vec<(usize, usize)>>>,
    /// Cross-file schemes for return type propagation (not full unification).
    pub(crate) cross_file_schemes: Option<Arc<HashMap<String, Vec<Arc<TypeScheme>>>>>,
    pub(crate) known_field_names: HashSet<String>,
    /// Whether cross_file_* sets reflect the full workspace.
    pub(crate) has_workspace_context: bool,
    /// Whether to report a call whose callee resolves to nothing. Off until
    /// union constructors with existential payloads and `outcome`
    /// declarations are registered as callables; without them this fires on
    /// valid code.
    pub(crate) report_unknown_calls: bool,
    pub(crate) global_constraints: Vec<ConstraintExpr>,
    pub(crate) vector_order: VectorOrder,
    pub(crate) symbol_index: Arc<WorkspaceSymbolIndex>,
}

/// Unified index mapping symbol names to their defining file and kind.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceSymbolIndex {
    pub(super) entries: HashMap<String, (base_db::FileText, SymbolKind)>,
    /// Types, kept apart from the value namespace above. Sail's are
    /// separate — a `struct S` and a `let S` can coexist — and folding them
    /// together would let a type entry win the `or_insert` race and answer
    /// a question about the value.
    pub(super) types: HashMap<String, base_db::FileText>,
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

    pub fn insert_type(&mut self, name: String, file: base_db::FileText) {
        self.types.entry(name).or_insert(file);
    }

    /// The file declaring a type by this name.
    pub fn get_type_file(&self, name: &str) -> Option<base_db::FileText> {
        self.types.get(name).copied()
    }
}

/// Parameterised alias scheme for on-demand substitution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AliasScheme {
    pub params: Vec<String>,
    pub body: Ty,
    pub config_dependent: bool,
    pub kind_bounds: HashMap<String, Kind>,
}

impl AliasScheme {
    /// Substitute actual args into the alias body. Returns `None` on arity mismatch.
    pub fn substitute(&self, args: &[crate::ty::TyArg]) -> Option<Ty> {
        for (param, arg) in self.params.iter().zip(args) {
            let found = match arg {
                TyArg::Bool(_) => Some(Kind::Bool),
                TyArg::Order(_) => Some(Kind::Order),
                TyArg::Nexp(_) => Some(Kind::Int),
                TyArg::Type(t)
                    if matches!(
                        t.kind(),
                        TyKind::Scalar(_) | TyKind::Tuple(_) | TyKind::FnPtr(_)
                    ) =>
                {
                    Some(Kind::Type)
                }
                _ => None,
            };
            if self.kind_bounds.get(param).zip(found).is_some_and(|(want, found)| *want != found) {
                return None;
            }
        }
        super::substitution::apply_arguments(&self.body, &self.params, args)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecordInfo {
    pub(crate) params: Vec<String>,
    pub(crate) fields: HashMap<String, Ty>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BitfieldInfo {
    pub(crate) underlying: Ty,
    pub(crate) fields: HashMap<String, Ty>,
}

pub(super) fn empty_names() -> Arc<HashSet<String>> {
    static EMPTY: std::sync::OnceLock<Arc<HashSet<String>>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default).clone()
}
pub(super) fn empty_schemes() -> Arc<HashMap<String, Vec<Arc<TypeScheme>>>> {
    static EMPTY: std::sync::OnceLock<Arc<HashMap<String, Vec<Arc<TypeScheme>>>>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default).clone()
}
pub(super) fn empty_mappings() -> Arc<HashMap<String, Vec<Arc<MappingScheme>>>> {
    static EMPTY: std::sync::OnceLock<Arc<HashMap<String, Vec<Arc<MappingScheme>>>>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default).clone()
}
pub(super) fn empty_arities() -> Arc<HashMap<String, Vec<(usize, usize)>>> {
    static EMPTY: std::sync::OnceLock<Arc<HashMap<String, Vec<(usize, usize)>>>> =
        std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default).clone()
}
pub(super) fn empty_index() -> Arc<WorkspaceSymbolIndex> {
    static EMPTY: std::sync::OnceLock<Arc<WorkspaceSymbolIndex>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Default::default).clone()
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
            constraint_synonyms: HashMap::new(),
            alias_schemes: HashMap::new(),
            cross_file_function_names: empty_names(),
            cross_file_constructor_names: empty_names(),
            cross_file_constructor_schemes: empty_schemes(),
            cross_file_mapping_schemes: empty_mappings(),
            cross_file_value_names: empty_names(),
            cross_file_register_names: empty_names(),
            cross_file_function_arity: empty_arities(),
            cross_file_schemes: None,
            known_field_names: HashSet::new(),
            has_workspace_context: false,
            report_unknown_calls: false,
            global_constraints: Vec::new(),
            vector_order: VectorOrder::Dec,
            symbol_index: empty_index(),
        }
    }
}

impl TopLevelEnv {
    pub fn function_names(&self) -> impl Iterator<Item = &String> {
        self.functions.keys()
    }

    /// Mapping names. A mapping is callable in both directions, so for
    /// "is this name bound to something callable" it counts alongside
    /// functions.
    pub fn mapping_names(&self) -> impl Iterator<Item = &String> {
        self.mappings.keys()
    }

    /// Enum members. A scattered `enum clause E = M` registers `M` here and
    /// in `enums`, but not in `constructors`, so callers that want every
    /// constant pattern name need both.
    pub fn enum_member_names(&self) -> impl Iterator<Item = &String> {
        self.enums.values().flatten()
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
    ///
    /// Uses only the default preprocessor symbols; callers that know the
    /// workspace's `default Order` should prefer
    /// [`TopLevelEnv::from_cst_with_symbols`].
    pub fn from_cst(root: &syntax::SyntaxNode) -> (Self, HashSet<String>) {
        let mut symbols = syntax::preprocess::default_symbols();
        Self::from_cst_with_symbols(root, &mut symbols)
    }

    /// Build a `TopLevelEnv`, honouring conditional compilation.
    ///
    /// Definitions inside a non-taken `$ifdef` / `$ifndef` / `$iftarget`
    /// branch are skipped, exactly as in ItemTree lowering. Without this the
    /// environment holds *both* branches — in the bundled prelude that means
    /// two conflicting `subrange_bits`, `bitvector_access` and
    /// `bitvector_update` signatures, one dec-ordered and one inc-ordered.
    pub fn from_cst_with_symbols(
        root: &syntax::SyntaxNode,
        symbols: &mut HashSet<String>,
    ) -> (Self, HashSet<String>) {
        use parser::SyntaxKind as SK;

        let mut env = Self::default();
        let mut pattern_constants = HashSet::new();
        let mut cond = syntax::preprocess::CondFilter::new();
        let mut declaration_symbols = symbols.clone();
        let mut declaration_filter = syntax::preprocess::CondFilter::new();
        let explicit_vals = hir_def::item_tree::generated::explicit_val_names(
            root.children()
                .filter(|node| declaration_filter.accept(node, &mut declaration_symbols, None)),
        );

        for def_node in root.children() {
            if !cond.accept(&def_node, symbols, None) {
                continue;
            }
            let kind = def_node.kind();
            // `default Order dec | inc`. Handled before the name extraction
            // below: `default`, `Order`, `dec` and `inc` are all keywords, so
            // the definition has no IDENT at all and the extraction's `None`
            // arm would `continue` past it.
            if kind == SK::DEFAULT_DEF {
                let mut toks = def_node
                    .descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .filter(|t| !t.kind().is_trivia());
                if toks.any(|t| t.kind() == SK::KW_ORDER) {
                    match toks.next().map(|t| t.kind()) {
                        Some(SK::KW_DEC) => env.vector_order = VectorOrder::Dec,
                        Some(SK::KW_INC) => env.vector_order = VectorOrder::Inc,
                        _ => {}
                    }
                }
                continue;
            }
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
                            .or_else(|| literal_type_after_eq(&def_node))
                            .unwrap_or(Ty::error());
                        for tok in def_node.descendants_with_tokens() {
                            if let Some(t) = tok.as_token() {
                                if t.kind() == SK::EQ {
                                    break;
                                }
                                if t.kind() == SK::IDENT {
                                    env.values.insert(t.text().to_string(), ty.clone());
                                }
                                // `let 'cap_size = 32` binds the value
                                // `cap_size` — `atomic_pat: | kid` is
                                // `P_var (P_id (id_of_kid kid), ..)`
                                // (parser.mly:541-543) and `id_of_kid` drops
                                // the `'` (ast_util.ml:983). The item tree
                                // records the definition; without this the
                                // environment had no type for it.
                                if t.kind() == SK::TY_VAR {
                                    env.values.insert(
                                        t.text().trim_start_matches('\'').to_string(),
                                        ty.clone(),
                                    );
                                }
                            }
                        }
                        preserve_global_numeric_binders(&def_node, &mut env);
                    }
                    continue;
                }
            };

            match kind {
                SK::CALLABLE_SPEC => {
                    // `val name : typschm`. A bidirectional signature declares
                    // a mapping whichever keyword introduced it — `val f : A
                    // <-> B` is how a mapping is declared separately from its
                    // clauses. Filing it under functions instead gave it a
                    // function's arity, so calling it with the components of a
                    // tuple side drew a spurious argument-count error.
                    if let Some(type_node) = find_type_child(&def_node) {
                        if let Some(mapping) = mapping_scheme_from_cst(&type_node) {
                            env.mappings.entry(name).or_default().push(Arc::new(mapping));
                        } else {
                            let scheme = scheme_from_cst_node(&type_node);
                            env.functions.entry(name).or_default().push(Arc::new(scheme));
                        }
                    }
                }
                SK::OUTCOME_DEF => {
                    // `outcome name : typschm [with ...] [= { defs }]`
                    // declares a callable, and `instantiation` supplies its
                    // implementation. Register it like a val spec so calls to
                    // it resolve (parse_ast.ml `OV_outcome`).
                    if let Some(type_node) = find_type_child(&def_node) {
                        let scheme = scheme_from_cst_node(&type_node);
                        env.functions.entry(name).or_default().push(Arc::new(scheme));
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
                        //
                        // The `typquant` comes first, and its constraint is
                        // a type node too — searching from the start of the
                        // definition would read `forall 'n, 0 < 'n <= 8 .`
                        // as the signature.
                        let (quantifiers, quant_constraints, quant_end) =
                            super::callable_typquant(&def_node);
                        let direct_type = match quant_end {
                            Some(end) => def_node
                                .children()
                                .find(|c| c.kind().is_type_node() && c.text_range().start() >= end),
                            None => find_type_child(&def_node),
                        };
                        let param_types = extract_param_types_from_param_list(&def_node);

                        // Inline quantifiers have the same kinds as a `val`
                        // scheme. In particular `bool('b)` must retain a
                        // Boolean predicate through freshening, not become
                        // an unprintable type inference variable `bool(_)`.
                        let inline_scheme = |mut scheme: TypeScheme| {
                            scheme.kind_bounds.extend(super::kinds::declared(&def_node));
                            for ty in scheme.params.iter().chain(std::iter::once(&scheme.ret)) {
                                super::kinds::infer(ty, &mut scheme.kind_bounds);
                            }
                            scheme.params = scheme
                                .params
                                .iter()
                                .map(|ty| super::kinds::normalize(ty, &scheme.kind_bounds))
                                .collect();
                            scheme.ret = super::kinds::normalize(&scheme.ret, &scheme.kind_bounds);
                            for constraint in &mut scheme.constraints {
                                constraint.expr = super::kinds::normalize_constraint(
                                    &constraint.expr,
                                    &scheme.kind_bounds,
                                );
                            }
                            Arc::new(scheme)
                        };

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
                                env.functions.entry(name).or_default().push(inline_scheme(
                                    TypeScheme {
                                        quantifiers,
                                        kind_bounds: HashMap::new(),
                                        constraints: quant_constraints,
                                        params,
                                        implicit_params: implicit,
                                        ret,
                                        declared_effects: Vec::new(),
                                        is_declared_pure: false,
                                    },
                                ));
                            }
                            Some(ref tn) => {
                                // Full inline sig (`: (T) -> R`)
                                let mut scheme = scheme_from_cst_node(tn);
                                // A `typquant` on the definition head binds
                                // for the signature written after it.
                                if !quantifiers.is_empty() || !quant_constraints.is_empty() {
                                    scheme.quantifiers.extend(quantifiers);
                                    scheme.constraints.extend(quant_constraints);
                                }
                                env.functions.entry(name).or_default().push(inline_scheme(scheme));
                            }
                            None if !param_types.is_empty() => {
                                // No ret type, only param types from `: Type`
                                let implicit = vec![false; param_types.len()];
                                env.functions.entry(name).or_default().push(inline_scheme(
                                    TypeScheme {
                                        quantifiers,
                                        kind_bounds: HashMap::new(),
                                        constraints: quant_constraints,
                                        params: param_types,
                                        implicit_params: implicit,
                                        ret: Ty::error(),
                                        declared_effects: Vec::new(),
                                        is_declared_pure: false,
                                    },
                                ));
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
                            // Enum members: all IDENTs inside `{ }`, or the
                            // bar-separated `enum E = A | B | C` spelling,
                            // which the grammar leaves as a chain of
                            // `BIN_EXPR`s rather than a braced list.
                            let members = extract_enum_members(&def_node);
                            let entry = env.enums.entry(name.clone()).or_default();
                            for member in &members {
                                pattern_constants.insert(member.clone());
                                env.values.insert(member.clone(), Ty::named(name.clone()));
                                if !entry.contains(member) {
                                    entry.push(member.clone());
                                }
                            }
                            register_enum_bindings(
                                &def_node,
                                &name,
                                members.len(),
                                &explicit_vals,
                                &mut env,
                            );
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
                                env.unions.entry(name).or_default().push(ctor_name.clone());
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
                                .or_else(|| literal_type_after_eq(&def_node))
                                .unwrap_or(Ty::error());
                            env.values.insert(name, ty);
                            preserve_global_numeric_binders(&def_node, &mut env);
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
                    // Visibility may precede the clause keyword.
                    let has = |kind| def_node.children_with_tokens().any(|t| t.kind() == kind);
                    if has(SK::KW_ENUM) {
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
                    } else if has(SK::KW_UNION) {
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
                        definition_eq_index(&text).and_then(|eq| {
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
                                } else if body_text.contains('{') {
                                    // The lightweight text reader treats braced types as
                                    // opaque names, losing existential binders/predicates.
                                    // Reuse the full CST reader for these alias bodies,
                                    // including existentials nested in tuples/applications.
                                    Some(super::type_from_type_text(body_text))
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
                        if let Some(eq_idx) = definition_eq_index(&text) {
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
                        let mut kind_bounds = super::kinds::declared(&def_node);
                        super::kinds::infer(&underlying, &mut kind_bounds);
                        let underlying = super::kinds::normalize(&underlying, &kind_bounds);
                        let synonym_params = params.clone();
                        let config_dependent =
                            super::workspace::contains_config_dependent(&underlying);
                        env.alias_schemes.insert(
                            name.clone(),
                            Arc::new(AliasScheme {
                                params,
                                body: underlying.clone(),
                                config_dependent,
                                kind_bounds,
                            }),
                        );
                        // The body may be a constraint rather than a type.
                        let full = def_node.text().to_string();
                        if let Some(eq) = definition_eq_index(&full) {
                            if let Some(c) =
                                Some(super::parse_constraint_text(full[eq + 1..].trim()))
                            {
                                if !matches!(c, ConstraintExpr::Unsupported) {
                                    env.constraint_synonyms
                                        .insert(name.clone(), (synonym_params, c));
                                }
                            }
                        }
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
    }

    pub(crate) fn value_binding_exists(&self, name: &str) -> bool {
        self.lookup_value(&LocalEnv::new(), name).is_some()
            || self.functions.contains_key(name)
            || self.constructors.contains_key(name)
            || self.overloads.contains_key(name)
            || self.cross_file_function_names.contains(name)
            || self.cross_file_constructor_names.contains(name)
            || self.cross_file_value_names.contains(name)
            || self.cross_file_register_names.contains(name)
    }

    pub(crate) fn lookup_value(&self, locals: &LocalEnv, name: &str) -> Option<Ty> {
        locals
            .lookup(name)
            .cloned()
            .or_else(|| self.values.get(name).cloned())
            // Registers are accessed via bare identifiers (e.g., `x25 = v`).
            .or_else(|| self.registers.get(name).cloned())
            // A nullary enum member is a *value*: `Read_plain`, the whole
            // `Barrier_RISCV_*` family. Nothing here looked at `enums`, so
            // they fell through to the constructor branch below — where the
            // prelude's `union barrier_kind = { Barrier_RISCV_i : unit, .. }`
            // declares the same names, a constructor is a function, and two
            // declarations of one name make that branch give up entirely.
            .or_else(|| {
                let mut hits =
                    self.enums.iter().filter(|(_, members)| members.iter().any(|m| m == name));
                let (ename, _) = hits.next()?;
                if hits.next().is_some() {
                    return None;
                }
                Some(Ty::named(ename))
            })
            // A *bitfield* field name is an index: `v[SSI]` reads bit SSI,
            // so the bare name has to type as an integer.
            //
            // `known_field_names` holds struct fields too, and answering
            // `int` for those turns every binding that shares a name with
            // some struct field anywhere in the workspace into an integer.
            // A library struct with a field called `value` is enough to
            // break 34 unrelated expressions in sail-riscv — which is what
            // happened the moment one became visible.
            .or_else(|| {
                let is_bitfield_field =
                    self.bitfields.values().any(|bf| bf.fields.contains_key(name));
                if is_bitfield_field
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
                // An overload can contain bidirectional mappings as well as
                // functions. Their two directions participate in the same
                // argument/return-type selection as the other candidates.
                out.extend(self.derived_mapping_functions(&format!("{member}_forwards")));
                out.extend(self.derived_mapping_functions(&format!("{member}_backwards")));
            }
            if !out.is_empty() {
                return out;
            }
        }
        if let Some(schemes) = self.functions.get(name) {
            out.extend(schemes.iter().cloned());
        }
        // Local constructors come before anything cross-file: a file that
        // declares `union U('a) = { Err : 'a, Ok : bits(16) }` means its own
        // `Ok`, not the prelude's `result` constructor of the same name.
        if let Some(schemes) = self.constructors.get(name) {
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
        // A `union clause ast = ZICBOZ : regidx` in another file left
        // `ZICBOZ` with a name but no payload type, so `mapping clause
        // encdec = ZICBOZ(rs1)` bound `rs1` to `error()` — the single
        // largest source of untyped expressions on sail-riscv.
        if out.is_empty() {
            if let Some(schemes) = self.cross_file_constructor_schemes.get(name) {
                out.extend(schemes.iter().cloned());
            }
        }
        if out.is_empty() {
            out.extend(self.derived_mapping_functions(name));
        }
        out
    }

    /// Identify an original overload candidate by allocation identity. Equal
    /// signatures need not denote the same declaration. Synthesized mapping
    /// signatures have no stable allocation here and deliberately return None.
    pub(crate) fn selected_overload_name(
        &self,
        group: &str,
        selected: &Arc<TypeScheme>,
    ) -> Option<String> {
        let members = self.overloads.get(group)?;
        let mut found = None;
        for member in members {
            let matches = self
                .functions
                .get(member)
                .into_iter()
                .flatten()
                .chain(
                    self.cross_file_schemes
                        .as_ref()
                        .and_then(|cf| cf.get(member))
                        .into_iter()
                        .flatten(),
                )
                .any(|scheme| Arc::ptr_eq(scheme, selected));
            if matches {
                if found.as_ref().is_some_and(|name| name != member) {
                    return None;
                }
                found = Some(member.clone());
            }
        }
        found
    }

    /// The four functions upstream derives from a mapping.
    ///
    /// `mapping X : A <-> B` — or a bidirectional `val X : A <-> B` — yields
    /// `X_forwards : A -> B`, `X_backwards : B -> A`,
    /// `X_forwards_matches : A -> bool` and `X_backwards_matches : B -> bool`,
    /// carrying the mapping's own quantifiers and constraints
    /// (`rewrites.ml:3430-3459`). Source calls them: sail-riscv writes
    /// `page_based_mem_type_forwards_matches(...)` and
    /// `pmpAddrMatchType_encdec(...)` directly, and this environment recorded
    /// the mapping and none of its four, so those calls had no signature at
    /// all.
    ///
    /// Derived on a miss rather than synthesized when the mapping is read,
    /// because the mapping is usually in another file from the call and
    /// `lookup_mappings` is what already knows about the cross-file table.
    /// The eager version was written first and reached only the defining
    /// file, which is why it is not what is here.
    fn derived_mapping_functions(&self, name: &str) -> SmallVec<[Arc<TypeScheme>; 4]> {
        let mut out: SmallVec<[Arc<TypeScheme>; 4]> = SmallVec::new();
        let suffixes = ["_forwards_matches", "_backwards_matches", "_forwards", "_backwards"];
        let Some(suffix) = suffixes.into_iter().find(|s| name.ends_with(s)) else { return out };
        let base = &name[..name.len() - suffix.len()];
        for mapping in self.lookup_mappings(base) {
            let (param, ret) = match suffix {
                "_forwards" => (mapping.lhs.clone(), mapping.rhs.clone()),
                "_backwards" => (mapping.rhs.clone(), mapping.lhs.clone()),
                "_forwards_matches" => (mapping.lhs.clone(), Ty::named("bool".to_string())),
                _ => (mapping.rhs.clone(), Ty::named("bool".to_string())),
            };
            let mut scheme = plain_scheme(vec![param], ret);
            scheme.quantifiers = mapping.quantifiers.clone();
            scheme.constraints = mapping.constraints.clone();
            out.push(Arc::new(scheme));
        }
        out
    }

    /// Overload members, including implementations defined in another file.
    ///
    /// Separate from [`TopLevelEnv::lookup_functions`] on purpose. The main
    /// call path reports a type error when every candidate fails to unify,
    /// and cross-file quantified schemes are not reliable enough for that.
    /// Operator resolution only asks "is there a candidate that fits" and
    /// reports nothing on failure, so it can afford them — and it needs
    /// them, because an operator's implementation almost always lives in the
    /// prelude.
    pub(crate) fn lookup_overload_members_cross_file(
        &self,
        name: &str,
    ) -> SmallVec<[Arc<TypeScheme>; 4]> {
        let mut out = self.lookup_functions(name);
        if !out.is_empty() {
            return out;
        }
        let Some(members) = self.overloads.get(name) else { return out };
        let Some(cf) = self.cross_file_schemes.as_ref() else { return out };
        for member in members {
            if let Some(schemes) = cf.get(member) {
                out.extend(schemes.iter().cloned());
            }
        }
        out
    }

    pub(crate) fn lookup_mappings(&self, name: &str) -> SmallVec<[Arc<MappingScheme>; 4]> {
        let mut out: SmallVec<[Arc<MappingScheme>; 4]> = SmallVec::new();
        if let Some(schemes) = self.mappings.get(name) {
            out.extend(schemes.iter().cloned());
        }
        if out.is_empty() {
            if let Some(schemes) = self.cross_file_mapping_schemes.get(name) {
                out.extend(schemes.iter().cloned());
            }
        }
        out
    }
}

pub(crate) fn synthesize_bitfield_accessors(
    env: &mut TopLevelEnv,
    bitfield_name: &str,
    field_name: &str,
    field_ty: Ty,
) {
    let bitfield_ty = Ty::named(bitfield_name.to_string());
    let (_, getter_name, updater_name, setter_name) =
        hir_def::bitfield::accessor_names(bitfield_name, field_name);
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
        .entry(updater_name.clone())
        .or_default()
        .push(Arc::new(plain_scheme(vec![bitfield_ty.clone(), field_ty.clone()], bitfield_ty)));
    env.overloads.entry(overload_name).or_default().extend([getter_name, setter_name]);
    // `overload update_<field> = {_update_<bitfield>_<field>}` is emitted
    // alongside the updater (bitfield.ml:164-167).
    env.overloads.entry(format!("update_{field_name}")).or_default().push(updater_name);
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
            // The clause is not authoritative about arity. `val f :
            // ((string, string)) -> string` takes one tuple, and
            // `function f(s1, s2) = ..` destructures it; flattening the
            // parameter to match the clause made every call site expect two
            // arguments. `(A, B) -> C` already arrives as two parameters, so
            // a lone tuple parameter here is always the `((A, B))` spelling.
            if scheme.params.len() == implicit_params.len() {
                scheme.implicit_params = implicit_params.clone();
            }
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
        Literal::Bool(value) => {
            Ty::app("bool", vec![TyArg::Bool(ConstraintExpr::Bool(*value))], "")
        }
        Literal::Unit => Ty::named("unit".to_string()),
        Literal::Number(text) => {
            if text.contains('.') {
                Ty::named("real".to_string())
            } else {
                // Upstream gives a literal the singleton `atom(3)`
                // (`infer_lit`, type_check.ml). Doing that here draws false
                // positives on sail-riscv, all of one shape: two singletons
                // never join. `if c then 1 else 2` makes `atom(1)` and
                // `atom(2)` disagree, where upstream generalises to `{'n, 'n
                // in {1, 2}. atom('n)}`. The literal's value is used against
                // a range annotation in `infer_expr_hir_with` instead, which
                // needs no join.
                //
                // **Measured again, 2026-08-29, and the answer is still no.**
                // The count is 257 now rather than 381, every one still
                // `atom(A) is not a subtype of atom(B)`. Two supporting
                // fixes take sail-riscv back to 0 — an array literal reading
                // its expectation instead of letting the first element set
                // the element type, and a literal *pattern* not deciding
                // what the scrutinee is — and every other probe is then
                // worse: conformance 1 false positive → 4 (a list literal, a
                // vector literal and a register assignment, each with no
                // expectation to read and elements that must *join*),
                // mutation 98.1% → 97.4%, inference coverage 7 untyped → 11,
                // all four of the new ones a `match` whose arms are
                // different literals.
                //
                // The imprecision is load-bearing. `int` for a literal is
                // what lets joins, overload resolution and unification work
                // without a subsumption layer; the two lost mutants are
                // `add_int`/`sub_int` mutated to return `bool`, no longer
                // caught because with singleton operands the prelude's
                // `add_atom : forall 'n 'm. (atom('n), atom('m)) -> atom('n
                // + 'm)` is the more specific overload and wins — which is
                // upstream's own resolution, and upstream rejects those
                // files only because it does not see our bundled prelude.
                //
                // What it would take is the rest of upstream's numeric
                // system, not a change here: generalising a join to `{'n, 'n
                // in {..}. atom('n)}`, reading `int` as `{'n. atom('n)}` in
                // subsumption, and checking every merge point — match arms,
                // `if` branches, array and list elements, an assignment's
                // value — against its expectation rather than against the
                // first thing that arrived. Until that exists, this line
                // stays.
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

/// Consume the same generated-name inventory as navigation and completion.
fn register_enum_bindings(
    node: &syntax::SyntaxNode,
    name: &str,
    members: usize,
    explicit_vals: &HashSet<String>,
    env: &mut TopLevelEnv,
) {
    use hir_def::item_tree::generated::{enum_bindings, Kind};
    for binding in enum_bindings(node, name, members) {
        if binding.kind == Kind::EnumNumber && explicit_vals.contains(&binding.name) {
            continue;
        }
        if binding.kind == Kind::EnumVector {
            env.values.insert(binding.name, vector_ty(members, Ty::named(name.to_owned())));
            continue;
        }
        let (root, _) =
            syntax::parse_text(&format!("val {} : {}", binding.name, binding.type_text));
        let Some(ty) = root.children().next().and_then(|node| find_type_child(&node)) else {
            continue;
        };
        env.functions.entry(binding.name).or_default().push(Arc::new(scheme_from_cst_node(&ty)));
    }
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
        TyKind::Named(name, _) | TyKind::App { name, .. }
            if matches!(name.as_str(), "bool" | "atom_bool") =>
        {
            MatchTy::Bool
        }
        TyKind::Named(name, _) if name == "unit" => MatchTy::Named("unit".to_string(), Vec::new()),
        TyKind::Named(name, _) => MatchTy::Named(name.clone(), Vec::new()),
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
                    TyArg::Nexp(_) | TyArg::Bool(_) | TyArg::Order(_) | TyArg::Value(_) => None,
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
        TyKind::Named(name, _) | TyKind::App { name, .. }
            if matches!(name.as_str(), "bool" | "atom_bool") =>
        {
            MatchTy::Bool
        }
        TyKind::Named(name, _) => {
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
                    TyArg::Nexp(_) | TyArg::Bool(_) | TyArg::Order(_) | TyArg::Value(_) => None,
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

/// The `=` that separates a definition from its body.
///
/// `text.find('=')` is not it: `type regno ('n : Int), 0 <= 'n < 32 =
/// atom('n)` has its first `=` inside the `<=` of the constraint, so the
/// alias's body came out as `'n < 32 = atom('n)` and every use of `regno`
/// classified as garbage. `==`, `=>`, `<=`, `>=` and `!=` are all operators,
/// not separators.
fn definition_eq_index(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' {
            let before = i.checked_sub(1).map(|j| bytes[j]);
            let after = bytes.get(i + 1).copied();
            let is_operator = matches!(before, Some(b'<' | b'>' | b'!' | b'='))
                || matches!(after, Some(b'=' | b'>'));
            if !is_operator {
                return Some(i);
            }
            // Step past a two-character operator so `==` is not read twice.
            if after == Some(b'=') {
                i += 1;
            }
        }
        i += 1;
    }
    None
}

/// The type of a top-level `let`'s initialiser, when the initialiser states
/// one on its own.
///
/// Upstream checks the whole `letbind` and binds each name at what it infers
/// (`check_letdef`, type_check.ml). This environment is built *before*
/// anything can be inferred against it, so only the initialisers that need no
/// environment are read — a literal is one. Everything else stays untyped,
/// which is what it was.
///
/// `test/c/pointer_assign.sail` is why: `let STATUS_1 = 0x0000_0002` had no
/// type, so `c | STATUS_1` had no operand it could resolve the operator by.
fn literal_type_after_eq(node: &syntax::SyntaxNode) -> Option<Ty> {
    use parser::SyntaxKind as SK;
    let lit = find_type_after_eq(node)?;
    // An immutable sizeof binding has an exact symbolic value even when the
    // configuration is unavailable. Preserve it for dependent branch facts.
    if lit.kind() == SK::SIZEOF_EXPR && first_keyword_text(node).as_deref() == Some("let") {
        let expr = find_type_child(&lit)?;
        let value = NumericExpr::parse(&expr.text().to_string())?;
        return Some(Ty::app("atom", vec![TyArg::Nexp(value)], ""));
    }
    if lit.kind() != SK::LITERAL_EXPR {
        return None;
    }
    let tok = lit
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())?;
    let text = tok.text().to_string();
    let literal = match tok.kind() {
        SK::NUM_LIT => Literal::Number(text),
        SK::HEX_LIT => Literal::Hex(text),
        SK::BIN_LIT => Literal::Binary(text),
        SK::STRING_LIT => Literal::String(text),
        SK::KW_BITZERO => Literal::BitZero,
        SK::KW_BITONE => Literal::BitOne,
        SK::KW_TRUE => Literal::Bool(true),
        SK::KW_FALSE => Literal::Bool(false),
        SK::UNIT => Literal::Unit,
        _ => return None,
    };
    let ty = infer_literal_type(&literal);
    (!ty.is_error()).then_some(ty)
}

/// `let (x as 'n) : range(lo, hi) = ...` gives x the singleton atom('n).
/// Keep this identity instead of widening x to the annotation's entire range.
fn preserve_global_numeric_binders(node: &syntax::SyntaxNode, env: &mut TopLevelEnv) {
    use parser::SyntaxKind as SK;
    let tokens: Vec<_> = node
        .descendants_with_tokens()
        .filter_map(|t| t.into_token())
        .filter(|t| !t.kind().is_trivia())
        .take_while(|t| t.kind() != SK::EQ)
        .collect();
    for triple in tokens.windows(3) {
        if triple[0].kind() != SK::IDENT
            || triple[1].kind() != SK::KW_AS
            || triple[2].kind() != SK::TY_VAR
        {
            continue;
        }
        let name = triple[0].text().to_string();
        let Some(ty) = env.values.get(&name) else {
            continue;
        };
        if !super::constraint::is_numeric_scalar_ty(ty) {
            continue;
        }
        let variable = NumericExpr::Var(triple[2].text().to_string());
        if let Some((lo, hi)) = super::expr::concrete_range_bounds(ty) {
            if let (Ok(lo), Ok(hi)) = (i64::try_from(lo), i64::try_from(hi)) {
                env.global_constraints.push(ConstraintExpr::And(vec![
                    ConstraintExpr::Compare {
                        lhs: NumericExpr::Const(lo),
                        op: CompareOp::Lte,
                        rhs: variable.clone(),
                    },
                    ConstraintExpr::Compare {
                        lhs: variable.clone(),
                        op: CompareOp::Lte,
                        rhs: NumericExpr::Const(hi),
                    },
                ]));
            }
        }
        env.values.insert(
            name,
            Ty::app(
                "atom",
                vec![TyArg::Nexp(variable.clone())],
                format!("atom({})", variable.to_string_repr()),
            ),
        );
    }
}
