//! Semantic name projections between workspace aggregation and body inference.
//! Missing names are dependencies too. Signature references are followed to a
//! fixed point; reverse field/member edges preserve structural lookup.
use super::env::{AliasScheme, BitfieldInfo, RecordInfo};
use super::*;
use crate::query::{ArcTopLevelEnv, TopLevelEnvData};
use base_db::FileText;
use hir_def::name::Name;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
type Names = BTreeSet<Name>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Symbol<T> {
    pub data: T,
    pub references: Names,
    pub pattern_constant: bool,
}
impl<T: Default> Default for Symbol<T> {
    fn default() -> Self {
        Self { data: T::default(), references: Names::new(), pattern_constant: false }
    }
}
/// Include operators, ignore trivia and literals.
pub(crate) fn names_in(text: &str) -> Names {
    let lexed = parser::LexedStr::new(text);
    (0..lexed.len())
        .filter(|&i| {
            use parser::SyntaxKind::*;
            // Fixed punctuation/keywords cannot resolve to user declarations.
            matches!(
                lexed.kind(i),
                IDENT
                    | OP_IDENT
                    | TY_VAR
                    | L_ANGLE
                    | R_ANGLE
                    | LE
                    | GE
                    | PERCENT
                    | STAR
                    | SLASH
                    | EQ
                    | EQ_EQ
                    | NEQ
                    | AMP
                    | PIPE
                    | SCOPE
                    | PLUS
                    | MINUS
                    | CARET
                    | AT
                    | KW_BOOL
                    | KW_INT
                    | KW_NAT
            )
        })
        .map(|i| Name::new(lexed.text(i)))
        .collect()
}
pub(super) fn names_in_type(ty: &Ty) -> BTreeSet<Name> {
    let mut names = BTreeSet::new();
    ty.references(&mut names);
    names
}

trait References {
    fn references(&self, out: &mut Names);
}
impl References for Ty {
    fn references(&self, out: &mut Names) {
        match self.kind() {
            TyKind::Error | TyKind::Infer(_) => {}
            TyKind::Scalar(s) => {
                out.insert(Name::new(s.name()));
            }
            TyKind::Param(name) | TyKind::Abstract { name, .. } => {
                out.extend(names_in(name));
            }
            TyKind::Named(name, args) | TyKind::App { name, args, .. } => {
                out.extend(names_in(name));
                for arg in args {
                    match arg {
                        TyArg::Type(ty) => ty.references(out),
                        TyArg::Bool(c) => out.extend(names_in(&c.to_text())),
                        TyArg::Order(text) | TyArg::Value(text) => out.extend(names_in(text)),
                        TyArg::Nexp(n) => out.extend(names_in(&n.to_string_repr())),
                    }
                }
            }
            TyKind::Tuple(items) => items.references(out),
            TyKind::FnPtr(sig) => {
                sig.params.references(out);
                sig.ret.references(out);
            }
            TyKind::Bidir { lhs, rhs } => {
                lhs.references(out);
                rhs.references(out);
            }
            TyKind::Exist { constraint, inner, .. } => {
                out.extend(names_in(&constraint.to_text()));
                inner.references(out);
            }
        }
    }
}
impl References for String {
    fn references(&self, out: &mut Names) {
        out.insert(Name::new(self));
    }
}
impl<T: References> References for Vec<T> {
    fn references(&self, out: &mut Names) {
        for item in self {
            item.references(out);
        }
    }
}
impl<T: References> References for Arc<T> {
    fn references(&self, out: &mut Names) {
        (**self).references(out);
    }
}
impl References for TypeScheme {
    fn references(&self, out: &mut Names) {
        self.params.references(out);
        self.ret.references(out);
        for c in &self.constraints {
            out.extend(names_in(&c.text));
        }
    }
}
impl References for MappingScheme {
    fn references(&self, out: &mut Names) {
        self.lhs.references(out);
        self.rhs.references(out);
        for c in &self.constraints {
            out.extend(names_in(&c.text));
        }
    }
}
impl References for AliasScheme {
    fn references(&self, out: &mut Names) {
        self.body.references(out);
    }
}
impl References for RecordInfo {
    fn references(&self, out: &mut Names) {
        for ty in self.fields.values() {
            ty.references(out);
        }
    }
}
impl References for BitfieldInfo {
    fn references(&self, out: &mut Names) {
        self.underlying.references(out);
        for ty in self.fields.values() {
            ty.references(out);
        }
    }
}
impl References for (Vec<String>, ConstraintExpr) {
    fn references(&self, out: &mut Names) {
        out.extend(names_in(&self.1.to_text()));
    }
}
impl References for Vec<(usize, usize)> {
    fn references(&self, _: &mut Names) {}
}

macro_rules! maps {
    ($src:expr, $out:ident, $($field:ident),* $(,)?) => {$(
        for (name, value) in &$src.$field {
            let symbol = $out.entry(name.clone()).or_default();
            symbol.data.$field.insert(name.clone(), value.clone());
            value.references(&mut symbol.references);
        }
    )*};
}
macro_rules! arc_maps {
    ($src:expr, $out:ident, $($field:ident),* $(,)?) => {$(
        for (name, value) in $src.$field.iter() {
            let symbol = $out.entry(name.clone()).or_default();
            Arc::make_mut(&mut symbol.data.$field).insert(name.clone(), value.clone());
            value.references(&mut symbol.references);
        }
    )*};
}
macro_rules! sets {
    ($src:expr, $out:ident, $($field:ident),* $(,)?) => {$(
        for name in &$src.$field { $out.entry(name.clone()).or_default().data.$field.insert(name.clone()); }
    )*};
}
macro_rules! arc_sets {
    ($src:expr, $out:ident, $($field:ident),* $(,)?) => {$(
        for name in $src.$field.iter() {
            Arc::make_mut(&mut $out.entry(name.clone()).or_default().data.$field).insert(name.clone());
        }
    )*};
}
macro_rules! merge_maps {
    ($dst:expr, $src:expr, $($field:ident),* $(,)?) => {$( $dst.$field.extend($src.$field.clone()); )*};
}
macro_rules! merge_arcs {
    ($dst:expr, $src:expr, $($field:ident),* $(,)?) => {$(
        if !$src.$field.is_empty() { Arc::make_mut(&mut $dst.$field).extend($src.$field.iter().map(|(k,v)| (k.clone(),v.clone()))); }
    )*};
}
macro_rules! merge_arc_sets {
    ($dst:expr, $src:expr, $($field:ident),* $(,)?) => {$(
        if !$src.$field.is_empty() { Arc::make_mut(&mut $dst.$field).extend($src.$field.iter().cloned()); }
    )*};
}
fn field_key(name: &str) -> String {
    format!("\0field:{name}")
}

/// Adding another record with matching fields must invalidate structural lookup.
fn reverse_edges<T: Default>(
    out: &mut HashMap<String, Symbol<T>>,
    records: &HashMap<String, RecordInfo>,
    bitfields: &HashMap<String, BitfieldInfo>,
    enums: &HashMap<String, Vec<String>>,
) {
    for (name, info) in records {
        for field in info.fields.keys() {
            out.entry(field_key(field)).or_default().references.insert(Name::new(name));
        }
    }
    for (name, info) in bitfields {
        for field in info.fields.keys() {
            for lookup in [
                field_key(field),
                format!("update_{field}"),
                format!("_mod_{field}"),
                format!("_get_{name}_{field}"),
                format!("_set_{name}_{field}"),
                format!("_update_{name}_{field}"),
            ] {
                out.entry(lookup).or_default().references.insert(Name::new(name));
            }
        }
    }
    for (name, members) in enums {
        for member in members {
            out.entry(member.clone()).or_default().references.insert(Name::new(name));
        }
    }
}
impl TopLevelEnv {
    fn symbols(&self) -> HashMap<String, Symbol<Self>> {
        // Keep this exhaustive: new semantic fields need an explicit projection.
        let Self {
            functions: _,
            mappings: _,
            overloads: _,
            values: _,
            registers: _,
            records: _,
            bitfields: _,
            constructors: _,
            enums: _,
            unions: _,
            scattered_open_types: _,
            type_aliases: _,
            constraint_synonyms: _,
            alias_schemes: _,
            cross_file_function_names: _,
            cross_file_constructor_names: _,
            cross_file_constructor_schemes: _,
            cross_file_mapping_schemes: _,
            cross_file_value_names: _,
            cross_file_register_names: _,
            cross_file_function_arity: _,
            cross_file_schemes: _,
            known_field_names: _,
            has_workspace_context: _,
            report_unknown_calls: _,
            global_constraints: _,
            vector_order: _,
            symbol_index: _,
        } = self;
        let mut out: HashMap<String, Symbol<Self>> = HashMap::new();
        maps!(
            self,
            out,
            functions,
            mappings,
            overloads,
            values,
            registers,
            records,
            bitfields,
            constructors,
            enums,
            unions,
            type_aliases,
            constraint_synonyms,
            alias_schemes
        );
        sets!(self, out, scattered_open_types, known_field_names);
        reverse_edges(&mut out, &self.records, &self.bitfields, &self.enums);
        let globals = out.entry(String::new()).or_default();
        globals.data.global_constraints = self.global_constraints.clone();
        globals.data.vector_order = self.vector_order;
        for c in &self.global_constraints {
            globals.references.extend(names_in(&c.to_text()));
        }
        out
    }
    fn extend_symbols(&mut self, other: &Self) {
        merge_maps!(
            self,
            other,
            functions,
            mappings,
            overloads,
            values,
            registers,
            records,
            bitfields,
            constructors,
            enums,
            unions,
            type_aliases,
            constraint_synonyms,
            alias_schemes,
            scattered_open_types,
            known_field_names
        );
    }
}
impl WorkspaceContext {
    fn symbols(&self) -> HashMap<String, Symbol<Self>> {
        // Keep this exhaustive: new semantic fields need an explicit projection.
        let Self {
            global_constraints: _,
            type_aliases: _,
            alias_schemes: _,
            constraint_synonyms: _,
            overloads: _,
            known_field_names: _,
            cross_file_records: _,
            cross_file_bitfields: _,
            cross_file_function_names: _,
            cross_file_constructor_names: _,
            cross_file_value_names: _,
            cross_file_register_names: _,
            cross_file_pattern_constants: _,
            cross_file_function_schemes: _,
            cross_file_constructor_schemes: _,
            cross_file_mapping_schemes: _,
            enums: _,
            unions: _,
            scattered_open_types: _,
            cross_file_function_arity: _,
            signatures_fingerprint: _,
            symbol_sig_hashes: _,
            file_referenced_symbols: _,
            symbol_index: _,
        } = self;
        let mut out: HashMap<String, Symbol<Self>> = HashMap::new();
        maps!(
            self,
            out,
            type_aliases,
            alias_schemes,
            constraint_synonyms,
            overloads,
            cross_file_records,
            cross_file_bitfields,
            enums,
            unions
        );
        arc_maps!(
            self,
            out,
            cross_file_function_schemes,
            cross_file_constructor_schemes,
            cross_file_mapping_schemes,
            cross_file_function_arity
        );
        sets!(self, out, known_field_names, cross_file_pattern_constants, scattered_open_types);
        arc_sets!(
            self,
            out,
            cross_file_function_names,
            cross_file_constructor_names,
            cross_file_value_names,
            cross_file_register_names
        );
        for (name, &(file, kind)) in &self.symbol_index.entries {
            Arc::make_mut(&mut out.entry(name.clone()).or_default().data.symbol_index).insert(
                name.clone(),
                file,
                kind,
            );
        }
        for (name, &file) in &self.symbol_index.types {
            Arc::make_mut(&mut out.entry(name.clone()).or_default().data.symbol_index)
                .insert_type(name.clone(), file);
        }
        reverse_edges(&mut out, &self.cross_file_records, &self.cross_file_bitfields, &self.enums);
        // Import a global fact with the names it constrains, so changing an
        // unrelated configuration does not invalidate every callable.
        for fact in &self.global_constraints {
            let mut references: Names = names_in(&fact.to_text())
                .into_iter()
                .filter(|n| {
                    n.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '\'')
                })
                .collect();
            if references.is_empty() {
                references.insert(Name::new(""));
            }
            for name in &references {
                let symbol = out.entry(name.to_string()).or_default();
                symbol.data.global_constraints.push(fact.clone());
                symbol.references.extend(references.iter().cloned());
            }
        }
        out
    }
    fn extend_symbols(&mut self, other: &Self) {
        for fact in &other.global_constraints {
            if !self.global_constraints.contains(fact) {
                self.global_constraints.push(fact.clone());
            }
        }
        merge_maps!(
            self,
            other,
            type_aliases,
            alias_schemes,
            constraint_synonyms,
            overloads,
            cross_file_records,
            cross_file_bitfields,
            enums,
            unions,
            known_field_names,
            cross_file_pattern_constants,
            scattered_open_types
        );
        merge_arcs!(
            self,
            other,
            cross_file_function_schemes,
            cross_file_constructor_schemes,
            cross_file_mapping_schemes,
            cross_file_function_arity
        );
        merge_arc_sets!(
            self,
            other,
            cross_file_function_names,
            cross_file_constructor_names,
            cross_file_value_names,
            cross_file_register_names
        );
        if !other.symbol_index.entries.is_empty() || !other.symbol_index.types.is_empty() {
            let index = Arc::make_mut(&mut self.symbol_index);
            index.entries.extend(other.symbol_index.entries.clone());
            index.types.extend(other.symbol_index.types.clone());
        }
    }
}
#[salsa::tracked(returns(ref))]
fn file_symbols(
    db: &dyn salsa::Database,
    file: FileText,
) -> HashMap<String, Arc<Symbol<TopLevelEnv>>> {
    let _span = tracing::info_span!("file_symbols").entered();
    let data = crate::query::top_level_env(db, file);
    let mut symbols = data.0.env.symbols();
    for name in &data.0.pattern_constants {
        symbols.entry(name.clone()).or_default().pattern_constant = true;
    }
    symbols.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()
}
#[salsa::tracked(returns(ref))]
pub(crate) fn file_symbol(
    db: &dyn salsa::Database,
    file: FileText,
    name: String,
) -> Arc<Symbol<TopLevelEnv>> {
    static EMPTY: std::sync::OnceLock<Arc<Symbol<TopLevelEnv>>> = std::sync::OnceLock::new();
    file_symbols(db, file)
        .get(&name)
        .cloned()
        .unwrap_or_else(|| EMPTY.get_or_init(Default::default).clone())
}
mod index;
pub(crate) mod ordered;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Selection<T> {
    symbols: Vec<(Name, Arc<Symbol<T>>)>,
    names: Names,
}

/// Batch name lookups behind one semantic query edge. Keep shared symbol
/// values instead of copying environments while verifying signature edits.
/// Even absent names remain in `names`, so insertions are observed.
fn select<T>(
    db: &dyn salsa::Database,
    symbols: &HashMap<String, Arc<Symbol<T>>>,
    roots: Names,
) -> Selection<T> {
    let mut selected = Selection { symbols: Vec::new(), names: Names::new() };
    let mut pending = roots;
    while let Some(name) = pending.pop_first() {
        if !selected.names.insert(name.clone()) {
            continue;
        }
        db.unwind_if_revision_cancelled();
        mapping_base(&name, &mut pending);
        if let Some(symbol) = symbols.get(name.as_str()) {
            pending.extend(symbol.references.iter().cloned());
            selected.symbols.push((name, symbol.clone()));
        }
    }
    selected
}

fn local_environment(selected: &Selection<TopLevelEnv>) -> TopLevelEnvData {
    let _span = tracing::info_span!("local_environment").entered();
    // BTreeSet traversal places the empty global key first.
    let mut env = selected.symbols[0].1.data.clone();
    let mut pattern_constants = HashSet::new();
    for (name, symbol) in &selected.symbols {
        env.extend_symbols(&symbol.data);
        if symbol.pattern_constant {
            pattern_constants.insert(name.to_string());
        }
    }
    TopLevelEnvData { env, pattern_constants }
}

fn mapping_base(name: &str, pending: &mut Names) {
    for suffix in ["_forwards", "_backwards", "_forwards_matches", "_backwards_matches"] {
        if let Some(base) = name.strip_suffix(suffix) {
            pending.insert(Name::new(base));
        }
    }
}

/// Extract roots only when inference executes. The body is already an infer
/// dependency; an extra body -> names -> environment validation chain is not
/// needed. Equal name sets reuse the same semantic environment query key.
pub(crate) fn callable_names(cb: &hir_def::def_query::ArcCallableBody) -> Names {
    let _span = tracing::info_span!("callable_names").entered();
    // These types can be introduced by literals/desugaring, without a type
    // name in the source (e.g. a hex literal introduces `bits`). Their aliases
    // still participate in normalization.
    let mut names: Names = [
        "bits",
        "bitvector",
        "int",
        "nat",
        "bool",
        "unit",
        "string",
        "real",
        "bit",
        "list",
        "vector",
        "atom",
        "range",
        "register",
    ]
    .into_iter()
    .map(Name::new)
    .collect();
    names.insert(Name::new(&cb.body.name));
    {
        names.extend(names_in(
            cb.text.get(cb.body.def_span.start..cb.body.def_span.end).unwrap_or(""),
        ));
        // Include desugared identifiers, which need not occur in the source.
        for (_, expr) in cb.body.body.exprs.iter() {
            match expr {
                hir_def::Expr::Ident(name) => {
                    names.insert(Name::new(name));
                    names.insert(Name::from(field_key(name)));
                }
                hir_def::Expr::Field { field, .. } => {
                    names.insert(Name::from(format!("_mod_{field}")));
                    names.insert(Name::from(field_key(field)));
                }
                hir_def::Expr::Struct { fields, .. } | hir_def::Expr::Update { fields, .. } => {
                    names.extend(fields.iter().map(|(field, _)| Name::from(field_key(field))));
                }
                hir_def::Expr::Index { .. } => {
                    names.insert(Name::new("vector_access"));
                }
                hir_def::Expr::BinaryOp { op, .. } => {
                    names.insert(Name::from(op.to_string()));
                }
                hir_def::Expr::UnaryOp { op, .. } => {
                    names.insert(Name::from(op.to_string()));
                }
                _ => {}
            }
        }
        for (_, pat) in cb.body.body.pats.iter() {
            if let hir_def::Pat::Struct { fields, .. } = pat {
                names.extend(fields.iter().map(|(field, _)| Name::from(field_key(field))));
            }
        }
    }
    names
}

/// Whole name sets are read only when computing a spelling suggestion.
#[salsa::tracked(returns(ref))]
pub(crate) fn function_names(db: &dyn salsa::Database, file: FileText) -> Names {
    let local = crate::query::top_level_env(db, file);
    let mut names: Names = local
        .0
        .env
        .functions
        .keys()
        .chain(local.0.env.overloads.keys())
        .map(|n| Name::new(n))
        .collect();
    if let Some(ws) = base_db::WorkspaceFiles::try_get(db) {
        names.extend(index::function_names(db, ws, file.file_id(db)).iter().cloned());
    }
    names
}
#[salsa::tracked(returns(ref))]
pub(crate) fn callable_environment(
    db: &dyn salsa::Database,
    file: FileText,
    mut names: Names,
) -> ArcTopLevelEnv {
    let _span = tracing::info_span!("callable_environment").entered();
    names.insert(Name::new(""));
    let symbols = file_symbols(db, file);
    let ws = base_db::WorkspaceFiles::try_get(db);
    loop {
        let local = select(db, symbols, names);
        let Some(ws) = ws else {
            return ArcTopLevelEnv(Arc::new(local_environment(&local)));
        };
        let selected = index::select_workspace(db, ws, local.names.clone(), file.file_id(db));
        if selected.names.is_subset(&local.names) {
            let mut data = local_environment(&local);
            let mut context = WorkspaceContext::default();
            for (_, symbol) in &selected.symbols {
                context.extend_symbols(&symbol.data);
            }
            context.apply_to(&mut data.env, &mut data.pattern_constants);
            data.env.has_workspace_context = true;
            data.env.report_unknown_calls = true;
            return ArcTopLevelEnv(Arc::new(data));
        }
        names = selected.names;
    }
}

/// Type navigation uses the incremental declaration index as well.
pub(crate) fn declaring_type_file(
    db: &dyn salsa::Database,
    ws: base_db::WorkspaceFiles,
    from: FileText,
    name: &str,
) -> Option<FileText> {
    index::scoped_workspace_symbol(db, ws, Some(from.file_id(db)), Name::new(name))
        .as_ref()?
        .data
        .type_file(name)
}
