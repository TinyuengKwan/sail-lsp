//! Semantic diagnostic analysis using HIR Body arenas + ItemTree.
//!
//! multiple layers (parse, name-resolution, type-inference) and adds
//! IDE-specific semantic checks.
//!
//! Each check function walks Body arenas via `iter_exprs()` / `iter_pats()`.

mod declarations;
mod ordered_scope;
use hir::source_order::{bindings::Namespace, inline::parse_project_pragma};
use ordered_scope::OrderedScope;
pub(crate) mod ordered_scattered;

use std::collections::HashMap;

use hir::diagnostics::{Diagnostic, DiagnosticCode, Severity};
use hir::{is_builtin_type_level_fn, is_builtin_type_name};
use hir::{DefKind, ItemTree};
use ide_db::FileDb;

/// Compute semantic diagnostics for a single file with workspace context.
pub fn compute_semantic_diagnostics_with_workspace<'a, F, I>(
    file: &dyn FileDb,
    _all_files: I,
) -> Vec<Diagnostic>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = &'a F> + Clone,
{
    // Workspace-aware checks (cross-file unused functions etc.) are
    // deferred — for now delegate to single-file checks.
    compute_semantic_diagnostics(file)
}

/// Compute semantic diagnostics for a single file.
///
/// Only includes checks that walk the AST directly — NOT type inference
/// (which goes through `DefWithBody::diagnostics` → salsa queries).
///
/// Removed checks:
/// - `check_unused_variables`: duplicated by `InferenceDiagnostic::UnusedVariable`
///   in the per-callable inference pipeline.
/// - `check_scattered_completeness_single_file`: single-file scattered check
///   is almost always a false positive.
///   Workspace-level `check_workspace_scattered_completeness()` remains.
pub fn compute_semantic_diagnostics(file: &dyn FileDb) -> Vec<Diagnostic> {
    compute_semantic_diagnostics_with_names(file, None)
}

/// As [`compute_semantic_diagnostics`], but with workspace-wide name sets.
///
/// Registers, top-level bindings and scattered definition heads routinely
/// live in a different file from the code that uses them, so the checks that
/// depend on them need the whole workspace. Without it they would report
/// every cross-file reference as undeclared.
pub fn compute_semantic_diagnostics_with_names(
    file: &dyn FileDb,
    workspace: Option<&crate::WorkspaceNames>,
) -> Vec<Diagnostic> {
    let text = file.text();
    if text.is_empty() {
        return Vec::new();
    }
    let cst_root = file
        .file_text()
        .and_then(|(db, ft)| syntax::parse_query::semantic_parse_file(db, ft).syntax_node())
        .or_else(|| file.syntax())
        .unwrap_or_else(|| syntax::parse_text(text).0);
    let fallback;
    let item_tree = match file.item_tree() {
        Some(tree) => tree,
        None => {
            let mut symbols = syntax::preprocess::default_symbols();
            fallback = ItemTree::build_from_cst_with_preprocess(&cst_root, &mut symbols);
            &fallback
        }
    };
    // Definitions surviving conditional compilation. Checks that walk the CST
    // directly must use these rather than every top-level node, or they see
    // both branches of every `$ifdef` and fire on code the compiler never
    // sees. `symbols` is threaded through so `$define` inside a taken branch
    // affects later conditionals, as upstream does.
    let live_defs = live_top_level_defs(&cst_root);

    let scope = OrderedScope::new(file);
    let mut diagnostics = Vec::new();
    check_duplicate_enum_members(&cst_root, &mut diagnostics);
    check_value_namespace_collisions(&live_defs, &mut diagnostics);
    check_test_attribute_signature(&live_defs, scope.as_ref(), &mut diagnostics);
    check_outcome_instantiations(&live_defs, scope.as_ref(), None, &mut diagnostics);
    check_enum_number_conversions_attribute(&live_defs, &mut diagnostics);
    let ordered = scope.is_some();
    if !ordered {
        check_scattered_end_markers(&live_defs, workspace, &mut diagnostics);
    }
    check_termination_measure_targets(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    check_duplicate_struct_update_fields(&live_defs, &mut diagnostics);
    check_builtin_type_redefinition(&item_tree, &mut diagnostics);
    check_builtin_type_arity(&live_defs, &mut diagnostics);
    check_type_argument_kinds(&live_defs, scope.as_ref(), &mut diagnostics);
    check_assignments(file, &cst_root, &live_defs, workspace, scope.as_ref(), &mut diagnostics);
    if ordered {
        check_local_binding_visibility(file, &mut diagnostics);
    }
    check_module_visibility(file, &cst_root, &live_defs, &mut diagnostics);
    check_struct_literals(&live_defs, &item_tree, scope.as_ref(), &mut diagnostics);
    check_enum_mappings(&live_defs, &mut diagnostics);
    check_instantiation_constraints(&live_defs, &item_tree, scope.as_ref(), &mut diagnostics);
    check_misplaced_private(&live_defs, &mut diagnostics);
    check_extension_in_private(&live_defs, &mut diagnostics);
    check_type_level_names(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    check_type_synonym_tyvars(&live_defs, &mut diagnostics);
    check_abstract_type_kind(&live_defs, &mut diagnostics);
    diagnostics.extend(hir::infer::declaration_constraints::check(file.file_text(), &live_defs));
    check_kinded_synonym_bodies(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    ordered_scope::check_selected_signatures(
        &live_defs,
        scope.as_ref(),
        check_duplicate_quantifiers,
        &mut diagnostics,
    );
    check_overload_members(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    check_instantiation_targets(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    check_instantiation_references(&live_defs, scope.as_ref(), &mut diagnostics);
    ordered_scope::check_selected_signatures(
        &live_defs,
        scope.as_ref(),
        check_mapping_declared_type,
        &mut diagnostics,
    );
    ordered_scope::check_selected_signatures(
        &live_defs,
        scope.as_ref(),
        check_function_with_mapping_val,
        &mut diagnostics,
    );
    check_mapping_clause_symmetry(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    check_enum_vector_attribute(&live_defs, &mut diagnostics);
    check_subrange_contiguity(&live_defs, &mut diagnostics);
    check_enum_clause_arms(&live_defs, &mut diagnostics);
    check_uninferable_literals(&live_defs, &mut diagnostics);
    check_global_constraints(&live_defs, &mut diagnostics);
    ordered_scope::check_selected_signatures(
        &live_defs,
        scope.as_ref(),
        check_signature_agreement,
        &mut diagnostics,
    );
    check_phantom_type_params(&live_defs, &mut diagnostics);
    check_config_key_prefixes(&live_defs, &mut diagnostics);
    check_overload_call_arity(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    check_mapping_call_arity(&live_defs, scope.as_ref(), &mut diagnostics);
    check_outcome_variable_kinds(&live_defs, &mut diagnostics);
    check_duplicate_mapping_bindings(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    ordered_scope::check_selected_signatures(
        &live_defs,
        scope.as_ref(),
        check_subrange_pattern_width,
        &mut diagnostics,
    );
    ordered_scope::check_selected_signatures(
        &live_defs,
        scope.as_ref(),
        check_concat_pattern_widths,
        &mut diagnostics,
    );
    check_include_error(&live_defs, workspace, &mut diagnostics);
    // Configured inference checks field values against the selected record
    // and signature, including aliases and dependent widths. The legacy
    // spelling-only heuristic can select unrelated or future declarations.
    if !ordered {
        check_struct_field_widths(&live_defs, &mut diagnostics);
    }
    check_concat_pattern_inference(&live_defs, &mut diagnostics);
    check_config_value_types(&live_defs, workspace, &mut diagnostics);
    check_configured_constraints(&live_defs, workspace, &mut diagnostics);
    check_unprovable_local_equality(&live_defs, &mut diagnostics);
    // Singleton sizeof values already participate in ordered call checking.
    if !ordered {
        check_sizeof_constant_args(&live_defs, &mut diagnostics);
    }
    if !ordered {
        check_scattered_union_forward_types(&live_defs, &mut diagnostics);
    }
    check_mapping_annotation_agreement(&live_defs, &mut diagnostics);
    declarations::check(&live_defs, workspace, scope.as_ref(), &mut diagnostics);
    diagnostics
}

/// What a name in the *value* namespace is bound to.
///
/// Upstream keeps enum members, union constructors, registers, top-level
/// lets and functions in one namespace and rejects a second binding with
/// "Cannot create X, name is already bound to a Y"
/// (type_check.ml `Env.add_*` guards).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ValueBinding {
    EnumMember,
    UnionCtor,
    Register,
    TopLevelLet,
    Function,
}

impl ValueBinding {
    /// How upstream names this kind of binding in its diagnostics.
    fn describe(self) -> &'static str {
        match self {
            ValueBinding::EnumMember => "an enumeration member",
            ValueBinding::UnionCtor => "a union constructor",
            ValueBinding::Register => "a register",
            ValueBinding::TopLevelLet => "a top-level let binding",
            ValueBinding::Function => "a function",
        }
    }

    /// The verb phrase for the binding being *created*.
    fn creating(self) -> &'static str {
        match self {
            ValueBinding::EnumMember => "enumeration member",
            ValueBinding::UnionCtor => "union constructor",
            ValueBinding::Register => "register",
            ValueBinding::TopLevelLet => "top-level let binding",
            ValueBinding::Function => "function",
        }
    }

    /// Whether creating `self` when `prior` already exists is an error.
    ///
    /// Function-vs-function is excluded: `val f` / `function f` pairs and
    /// scattered `function clause f` legitimately repeat a name.
    fn collides_with(self, prior: ValueBinding) -> bool {
        use ValueBinding::*;
        match (self, prior) {
            (Function, Function) => false,
            (Function, _) | (_, Function) => matches!(self, Register) || matches!(prior, Register),
            _ => true,
        }
    }
}

/// Detect value-namespace collisions between enum members, union
/// constructors, registers, top-level lets and functions.
///
/// Mirrors upstream's `Env.add_*` guards, which reject a second binding of a
/// name already present in the value namespace.
fn check_value_namespace_collisions(
    defs: &[syntax::SyntaxNode],
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    let mut bound: HashMap<String, ValueBinding> = HashMap::new();
    let report = |diagnostics: &mut Vec<Diagnostic>,
                  bound: &mut HashMap<String, ValueBinding>,
                  name: String,
                  kind: ValueBinding,
                  range: syntax::TextRange,
                  // Upstream's caret is not always on the name. A duplicate
                  // toplevel let is reported at the whole `let` definition
                  // (`check_duplicate_letbinding` takes the letdef's own
                  // location), where every other member of this family is
                  // reported at `id_loc id`.
                  def_range: syntax::TextRange| {
        let mut range = range;
        if let Some(&prior) = bound.get(&name) {
            if kind.collides_with(prior) {
                // Three of these pairs have their own sentence upstream,
                // raised by a check of their own rather than by
                // `already_bound_global`. The generic wording is right for
                // the rest.
                use ValueBinding::*;
                let message = match (kind, prior) {
                    // `check_duplicate_letbinding` (`type_check.ml:4621`).
                    (TopLevelLet, TopLevelLet) => {
                        range = def_range;
                        format!("Duplicate toplevel let binding {name}")
                    }
                    // `Env.add_enum` checks the let bindings and the
                    // registers itself, before falling through to
                    // `already_bound_global` (`type_env.ml:1395-1409`).
                    (EnumMember, TopLevelLet) => format!(
                        "Enumeration member {name} is already bound as a global let-binding"
                    ),
                    (EnumMember, Register) => {
                        format!("Enumeration member {name} is already bound as a register")
                    }
                    _ => format!(
                        "Cannot create {} {name}, name is already bound to {}",
                        kind.creating(),
                        prior.describe()
                    ),
                };
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::SailError("duplicate-definition"),
                    message,
                    base_db::text_range(
                        u32::from(range.start()) as usize,
                        u32::from(range.end()) as usize,
                    ),
                    Severity::Error,
                ));
                return;
            }
        }
        bound.insert(name, kind);
    };

    for node in defs {
        match node.kind() {
            SK::NAMED_DEF => {
                let first_kw =
                    node.children_with_tokens().filter_map(|el| el.into_token()).find_map(|t| {
                        (!t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE).then_some(t.kind())
                    });
                match first_kw {
                    Some(SK::KW_ENUM) => {
                        for (name, range) in enum_member_names(node) {
                            report(
                                diagnostics,
                                &mut bound,
                                name,
                                ValueBinding::EnumMember,
                                range,
                                node.text_range(),
                            );
                        }
                    }
                    Some(SK::KW_UNION) => {
                        for (name, range) in union_ctor_names(node) {
                            report(
                                diagnostics,
                                &mut bound,
                                name,
                                ValueBinding::UnionCtor,
                                range,
                                node.text_range(),
                            );
                        }
                    }
                    Some(SK::KW_REGISTER) => {
                        if let Some((name, range)) = def_name(node) {
                            report(
                                diagnostics,
                                &mut bound,
                                name,
                                ValueBinding::Register,
                                range,
                                node.text_range(),
                            );
                        }
                    }
                    Some(SK::KW_LET) => {
                        if let Some((name, range)) = def_name(node) {
                            if name != "_" {
                                report(
                                    diagnostics,
                                    &mut bound,
                                    name,
                                    ValueBinding::TopLevelLet,
                                    range,
                                    node.text_range(),
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
            SK::CALLABLE_DEF => {
                // Only `function` definitions bind; `val` signatures and
                // scattered clauses do not create a fresh binding.
                let is_function = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::KW_FUNCTION);
                let is_clause = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::KW_CLAUSE);
                if is_function && !is_clause {
                    if let Some((name, range)) = def_name(node) {
                        report(
                            diagnostics,
                            &mut bound,
                            name,
                            ValueBinding::Function,
                            range,
                            node.text_range(),
                        );
                    }
                }
            }
            _ => {}
        }
    }
}

/// The `NAME` of a definition, with its source range.
fn def_name(node: &syntax::SyntaxNode) -> Option<(String, syntax::TextRange)> {
    use parser::SyntaxKind as SK;
    for child in node.children() {
        if child.kind() != SK::NAME {
            continue;
        }
        for tok in child.children_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() == SK::IDENT {
                return Some((tok.text().to_string(), tok.text_range()));
            }
        }
    }
    // `function foo(...)` has no NAME node — take the first IDENT before `(`.
    for tok in node.children_with_tokens().filter_map(|el| el.into_token()) {
        match tok.kind() {
            SK::IDENT => return Some((tok.text().to_string(), tok.text_range())),
            SK::L_PAREN | SK::UNIT | SK::EQ => break,
            _ => {}
        }
    }
    None
}

/// Member names of an `enum` definition body.
///
/// Handles both spellings upstream accepts: `enum E = { A, B }` and the
/// bar-separated `enum E = A | B` (parser.mly `enum_bar`), which sail-lsp
/// parses as a chain of `BIN_EXPR`s rather than a braced member list.
fn enum_member_names(node: &syntax::SyntaxNode) -> Vec<(String, syntax::TextRange)> {
    use parser::SyntaxKind as SK;
    let has_braces = node
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .any(|t| t.kind() == SK::L_CURLY);
    if !has_braces {
        let mut out = Vec::new();
        let mut seen_eq = false;
        for el in node.descendants_with_tokens() {
            let Some(tok) = el.into_token() else { continue };
            match tok.kind() {
                SK::EQ if !seen_eq => seen_eq = true,
                SK::IDENT if seen_eq => out.push((tok.text().to_string(), tok.text_range())),
                _ => {}
            }
        }
        return out;
    }
    let mut out = Vec::new();
    let mut depth = 0u32;
    let mut expect_name = true;
    for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
        match tok.kind() {
            SK::L_CURLY => {
                depth += 1;
                expect_name = true;
            }
            SK::R_CURLY => depth = depth.saturating_sub(1),
            SK::COMMA if depth == 1 => expect_name = true,
            // `A = 3` — skip the discriminant expression.
            SK::EQ if depth == 1 => expect_name = false,
            SK::IDENT if depth == 1 && expect_name => {
                out.push((tok.text().to_string(), tok.text_range()));
                expect_name = false;
            }
            _ => {}
        }
    }
    out
}

/// Constructor names of a `union` definition body (`Name : Type`).
fn union_ctor_names(node: &syntax::SyntaxNode) -> Vec<(String, syntax::TextRange)> {
    use parser::SyntaxKind as SK;
    let mut out = Vec::new();
    for field in node.descendants() {
        if field.kind() != SK::FIELD_INIT {
            continue;
        }
        for tok in field.descendants_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() == SK::COLON {
                break;
            }
            if tok.kind() == SK::IDENT {
                out.push((tok.text().to_string(), tok.text_range()));
                break;
            }
        }
    }
    out
}

/// Top-level definitions that survive conditional compilation.
fn live_top_level_defs(cst_root: &syntax::SyntaxNode) -> Vec<syntax::SyntaxNode> {
    let mut symbols = syntax::preprocess::default_symbols();
    let mut cond = syntax::preprocess::CondFilter::new();
    cst_root
        .children()
        .filter(|node| {
            let accepted = cond.accept(node, &mut symbols, None);
            // `CondFilter` reports every directive as "not a definition".
            // Keep the ones in a taken branch anyway: `$option`, `$project#`
            // and the module delimiters are inputs to later checks, and
            // checks that only want definitions already match on node kind.
            accepted || (node.kind() == parser::SyntaxKind::DIRECTIVE_DEF && cond.taking())
        })
        .collect()
}

/// Attributes attached to the definition node that follows them.
///
/// In the CST an attribute is a sibling `ATTRIBUTE` node preceding its
/// definition, so this collects the run of attributes immediately before
/// `node`.
fn preceding_attributes(node: &syntax::SyntaxNode) -> Vec<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;
    let mut attrs = Vec::new();
    let mut cur = node.prev_sibling();
    while let Some(n) = cur {
        if n.kind() != SK::ATTRIBUTE {
            break;
        }
        cur = n.prev_sibling();
        attrs.push(n);
    }
    attrs.reverse();
    attrs
}

/// The attribute's name, i.e. `test` in `$[test]`.
fn attribute_name(attr: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;
    attr.children_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT)
        .map(|t| t.text().to_string())
}

/// `$[test]` requires the annotated function to have type `unit -> unit`.
///
/// Upstream checks this when processing the attribute; the message is
/// "Functions with the $[test] attribute must have type 'unit -> unit'".
/// An `instantiation` must supply functions matching the `outcome`'s `val`s.
///
/// `outcome o : .. with 'a: Type = { val to_bits : 'a -> bits(64) }` followed
/// by `instantiation o with 'a = bits(32), to_bits = f` says `f` has type
/// `bits(32) -> bits(64)`. Nothing checked it.
///
/// Textual: the comparison is between two written signatures with the
/// outcome's type variables substituted. It only fires when both are present
/// and the substitution leaves no variable behind — an `instantiation`
/// naming a function this file does not declare says nothing here.
fn check_outcome_instantiations(
    defs: &[syntax::SyntaxNode],
    scope: Option<&OrderedScope<'_>>,
    comparison: Option<(&OrderedScope<'_>, &syntax::SyntaxNode)>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(scope) = scope {
        for node in defs.iter().filter(|n| n.kind() == syntax::SyntaxKind::INSTANTIATION_DEF) {
            let mut inputs: Vec<_> =
                scope.referenced(node, Namespace::Callable).into_values().collect();
            if let Some(outcome) = syntax::definition_name(node)
                .and_then(|name| scope.node(node, &name, Namespace::Outcome))
            {
                inputs.push(outcome);
            }
            inputs.push(node.clone());
            check_outcome_instantiations(&inputs, None, Some((scope, node)), diagnostics);
        }
        return;
    }
    use parser::SyntaxKind as SK;
    use std::collections::HashMap;

    let mut outcomes: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut vals: HashMap<String, String> = HashMap::new();

    for node in defs {
        let text = node.text().to_string();
        match node.kind() {
            SK::OUTCOME_DEF => {
                let Some(name) = syntax::definition_name(node) else { continue };
                let mut inner = HashMap::new();
                if let Some((_, block)) = text.split_once('{') {
                    for part in block.trim_end_matches('}').split("val ").skip(1) {
                        let Some((vname, sig)) = part.trim().split_once(':') else { continue };
                        inner.insert(
                            vname.trim().to_string(),
                            sig.trim().trim_end_matches('}').trim().to_string(),
                        );
                    }
                }
                if !inner.is_empty() {
                    outcomes.insert(name, inner);
                }
            }
            SK::CALLABLE_SPEC => {
                if let (Some(name), Some((_, sig))) =
                    (syntax::definition_name(node), text.split_once(':'))
                {
                    vals.insert(name, sig.trim().to_string());
                }
            }
            _ => {}
        }
    }

    for node in defs {
        if node.kind() != SK::INSTANTIATION_DEF {
            continue;
        }
        let Some(name) = syntax::definition_name(node) else { continue };
        let Some(inner_vals) = outcomes.get(&name) else { continue };
        let (subst, supplied): (Vec<_>, Vec<_>) = syntax::instantiation_substitutions(node)
            .into_iter()
            .partition(|(key, _)| key.starts_with('\''));

        for (val_name, declared) in &supplied {
            let (Some(expected_sig), Some(actual_sig)) =
                (inner_vals.get(val_name), vals.get(declared))
            else {
                continue;
            };
            // This text is for presentation (and the legacy fallback). Replace
            // whole variable tokens simultaneously: `'a` must not rewrite
            // `'aa`, or a variable introduced by another replacement.
            let lexed = parser::LexedStr::new(expected_sig);
            let expected: String = (0..lexed.len())
                .map(|i| {
                    let text = lexed.text(i);
                    if lexed.kind(i) == SK::TY_VAR {
                        subst.iter().find(|(key, _)| key == text).map_or(text, |(_, value)| value)
                    } else {
                        text
                    }
                })
                .collect();
            if expected.contains('\'') {
                continue;
            }
            let squash = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
            // Identical spelling can denote different aliases at the outcome
            // and implementation declarations. Ordered comparison must retain
            // both environments, including when the text appears equal.
            if comparison.is_none() && squash(&expected) == squash(actual_sig) {
                continue;
            }
            if let Some((scope, use_node)) = comparison {
                let mismatch = scope
                    .at(use_node)
                    .zip(scope.resolve(use_node, &name, Namespace::Outcome))
                    .zip(scope.resolve(use_node, declared, Namespace::Callable))
                    .is_some_and(|((use_at, outcome_at), actual_at)| {
                        hir::infer::declaration_constraints::instantiated_signature_mismatch(
                            scope.db,
                            outcome_at,
                            use_at,
                            actual_at,
                            expected_sig,
                            actual_sig,
                            &subst,
                        )
                    });
                if !mismatch {
                    continue;
                }
            }
            let r = node.text_range();
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailError("instantiation-mismatch"),
                format!(
                    "`{declared}` has type `{actual_sig}`, but `{name}` declares `{val_name} : {expected}`"
                ),
                base_db::text_range(u32::from(r.start()) as usize, u32::from(r.end()) as usize),
                Severity::Error,
            ));
        }
    }
}

fn check_test_attribute_signature(
    defs: &[syntax::SyntaxNode],
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    // The `val`s in this file, so an attribute on the *function* half of
    // a `val` + `function` pair can be told from one on a function that
    // declares itself.
    let mut vals: HashMap<String, syntax::TextRange> = HashMap::new();
    if scope.is_none() {
        for node in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
            if let Some((name, range)) = def_name(node) {
                vals.insert(name, range);
            }
        }
    }

    for node in defs {
        if !matches!(node.kind(), SK::CALLABLE_DEF | SK::CALLABLE_SPEC) {
            continue;
        }
        let attrs = preceding_attributes(node);
        let Some(attr) = attrs.iter().find(|a| attribute_name(a).as_deref() == Some("test")) else {
            continue;
        };

        // A `$[test]` on a function that has a separate `val` is an error
        // whatever its type: upstream checks *where the attribute is*
        // before it checks the signature (`type_check.ml:4864`), because
        // the prototype is where an attribute about the function belongs.
        // This checked only the signature, so the case upstream is
        // pointing at — the attribute in the wrong place, on a function
        // that is `unit -> unit` — went unreported until the signature
        // happened to be wrong too.
        if node.kind() == SK::CALLABLE_DEF {
            if let Some((name, _)) = def_name(node) {
                let selected = scope
                    .and_then(|scope| {
                        let decl = scope.resolve(node, &name, Namespace::Callable)?;
                        let spec = hir::source_order::node(scope.db, decl)?;
                        if spec.kind() != SK::CALLABLE_SPEC {
                            return None;
                        }
                        Some((def_name(&spec)?.1, Some(decl.file.file_id(scope.db))))
                    })
                    .or_else(|| {
                        scope
                            .is_none()
                            .then(|| vals.get(&name).copied().map(|range| (range, None)))
                            .flatten()
                    });
                if let Some((val_at, origin)) = selected {
                    let mut diagnostic = module_diagnostic(
                        "invalid-test-signature",
                        "Function with separate val prototype has a $[test] attribute, it \
                         should be attached there instead."
                            .to_string(),
                        attr.text_range(),
                    );
                    let message = "val declaration here".to_string();
                    if let Some(origin) = origin {
                        diagnostic.related_in_files.push((origin, val_at, message));
                    } else {
                        diagnostic.related.push((val_at, message));
                    }
                    diagnostics.push(diagnostic);
                    continue;
                }
            }
        }
        if test_signature_is_unit_to_unit(node) {
            continue;
        }
        let range = node.text_range();
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::SailError("invalid-test-signature"),
            "Functions with the $[test] attribute must have type 'unit -> unit'".to_string(),
            base_db::text_range(u32::from(range.start()) as usize, u32::from(range.end()) as usize),
            Severity::Error,
        ));
    }
}

/// Whether a `$[test]`-annotated definition really is `unit -> unit`.
fn test_signature_is_unit_to_unit(node: &syntax::SyntaxNode) -> bool {
    use parser::SyntaxKind as SK;

    let is_unit_type = |n: &syntax::SyntaxNode| n.text().to_string().trim() == "unit";

    if node.kind() == SK::CALLABLE_SPEC {
        // `val f : unit -> unit`
        let Some(arrow) = node.children().find(|c| c.kind() == SK::TYPE_ARROW) else {
            return false;
        };
        let parts: Vec<_> = arrow.children().collect();
        return parts.len() == 2 && is_unit_type(&parts[0]) && is_unit_type(&parts[1]);
    }

    // `function f() -> unit = ...`: the parameter list must be `()` and the
    // declared return type must be `unit`. A function with no inline return
    // type inherits it from a `val`, which upstream requires to carry the
    // attribute itself, so treat that as non-conforming.
    let params_are_unit = node
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .take_while(|t| t.kind() != SK::EQ)
        .any(|t| t.kind() == SK::UNIT)
        && !node.children().any(|c| c.kind() == SK::PARAM_LIST && c.text() != "()");

    let ret_is_unit = node
        .children_with_tokens()
        .skip_while(|el| el.as_token().is_none_or(|t| t.kind() != SK::R_ARROW))
        .filter_map(|el| el.into_node())
        .next()
        .as_ref()
        .is_some_and(is_unit_type);

    params_are_unit && ret_is_unit
}

/// `$[enum_number_conversions]` needs both `to_enum` and `from_enum`.
fn check_enum_number_conversions_attribute(
    defs: &[syntax::SyntaxNode],
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    for attr in defs.iter().filter(|n| n.kind() == SK::ATTRIBUTE) {
        if attribute_name(attr).as_deref() != Some("enum_number_conversions") {
            continue;
        }
        let text = attr.text().to_string();
        if text.contains("to_enum") && text.contains("from_enum") {
            continue;
        }
        let range = attr.text_range();
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::SailError("invalid-attribute"),
            "Expected to_enum and from_enum fields in attribute".to_string(),
            base_db::text_range(u32::from(range.start()) as usize, u32::from(range.end()) as usize),
            Severity::Error,
        ));
    }
}

/// `end X` must name a scattered definition, and a scattered definition must
/// have at least one clause before its `end`.
///
/// Upstream reports "X is not a scattered definition, so it cannot be ended"
/// and "No clauses found for scattered union type" (scattered.ml:167,184).
fn check_scattered_end_markers(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;
    use std::collections::HashMap;

    // name -> (is_union_or_enum, clause_count)
    // Name -> (needs clauses, clauses seen, where the head is). The head's
    // own span is kept because upstream reports an empty scattered union
    // *there* — `SD_variant`'s location (`scattered.ml:167`) — not at the
    // `end` that made it visible.
    let mut heads: HashMap<String, (bool, usize, syntax::TextRange)> = HashMap::new();
    let mut ended = HashMap::new();

    for node in defs {
        let tokens: Vec<_> = node.children_with_tokens().filter_map(|el| el.into_token()).collect();
        let is_clause = node.kind() == SK::SCATTERED_CLAUSE_DEF
            || (node.kind() == SK::CALLABLE_DEF
                && tokens.iter().any(|t| t.kind() == SK::KW_CLAUSE));
        if is_clause {
            if let Some((name, range)) = def_name(node) {
                if let Some(&end_range) = ended.get(&name) {
                    let kind = tokens
                        .iter()
                        .find_map(|t| match t.kind() {
                            SK::KW_UNION => Some("union"),
                            SK::KW_ENUM => Some("enum"),
                            SK::KW_FUNCTION => Some("function"),
                            SK::KW_MAPPING => Some("mapping"),
                            _ => None,
                        })
                        .unwrap_or("definition");
                    diagnostics.push(module_diagnostic(
                        "scattered-clause-after-end",
                        format!("Cannot add clause to scattered {kind} {name} as it has already been ended"),
                        range,
                    ).with_related(vec![(end_range, "previously ended here".to_string())]));
                }
            }
        }
        match node.kind() {
            SK::SCATTERED_DEF => {
                let needs_clauses = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| matches!(t.kind(), SK::KW_UNION | SK::KW_ENUM));
                if let Some((name, _)) = def_name(node) {
                    heads.entry(name).or_insert((needs_clauses, 0, node.text_range()));
                }
            }
            SK::SCATTERED_CLAUSE_DEF => {
                if let Some((name, _)) = def_name(node) {
                    heads.entry(name).or_insert((false, 0, node.text_range())).1 += 1;
                }
            }
            SK::CALLABLE_DEF => {
                // `function clause f ...` / `mapping clause m ...`
                let is_clause = node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::KW_CLAUSE);
                if is_clause {
                    if let Some((name, _)) = def_name(node) {
                        heads.entry(name).or_insert((false, 0, node.text_range())).1 += 1;
                    }
                }
            }
            SK::END_DEF => {
                let Some((name, range)) = def_name(node) else { continue };
                ended.insert(
                    name.clone(),
                    base_db::text_range(
                        u32::from(range.start()) as usize,
                        u32::from(range.end()) as usize,
                    ),
                );
                match heads.get(&name) {
                    // A scattered head can be declared in another file, so
                    // "not a scattered definition" is only an error once the
                    // whole workspace agrees. The empty-definition case below
                    // stays file-local: it fires on a head this file declares.
                    None if workspace.is_some_and(|ws| ws.scattered_heads.contains(&name)) => {}
                    None => {
                        diagnostics.push(Diagnostic::new(
                            DiagnosticCode::SailError("invalid-scattered-end"),
                            format!("{name} is not a scattered definition, so it cannot be ended"),
                            base_db::text_range(
                                u32::from(range.start()) as usize,
                                u32::from(range.end()) as usize,
                            ),
                            Severity::Error,
                        ));
                    }
                    Some(&(needs_clauses, 0, head)) if needs_clauses => {
                        let r = head;
                        diagnostics.push(Diagnostic::new(
                            DiagnosticCode::SailError("empty-scattered-definition"),
                            "No clauses found for scattered union type".to_string(),
                            base_db::text_range(
                                u32::from(r.start()) as usize,
                                u32::from(r.end()) as usize,
                            ),
                            Severity::Error,
                        ));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

/// A `termination_measure` must name a function that exists.
///
/// `check_termination_measure_decl` opens with `Env.get_val_spec id env`
/// (`type_check.ml:4760`), so a measure for a name nothing declares fails
/// there: "Function test_rec not found" (`type_error.ml:322`), at the name.
///
/// Only when the whole workspace agrees the name is unknown. A measure and
/// the function it measures are routinely in different files —
/// `sail/test/rocq/complex/move_termination` is exactly that — so a
/// file-local view would report every one of them.
fn check_termination_measure_targets(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;
    use std::collections::HashSet;

    if workspace.is_none() && scope.is_none() {
        return;
    }

    // What this file itself declares under a callable name, so a measure
    // beside its function needs no workspace entry to be accepted.
    let mut local: HashSet<String> = HashSet::new();
    for node in defs {
        if matches!(
            node.kind(),
            SK::CALLABLE_DEF | SK::CALLABLE_SPEC | SK::SCATTERED_DEF | SK::OUTCOME_DEF
        ) {
            if let Some((name, _)) = def_name(node) {
                local.insert(name);
            }
        }
    }

    for node in defs.iter().filter(|n| n.kind() == SK::TERMINATION_MEASURE_DEF) {
        let Some((name, range)) = def_name(node) else { continue };
        let known = scope.map_or_else(
            || {
                local.contains(&name)
                    || workspace.is_some_and(|ws| {
                        ws.function_names.contains(&name)
                            || ws.scattered_heads.contains(&name)
                            || ws.value_names.contains(&name)
                    })
            },
            |scope| scope.resolve(node, &name, Namespace::Callable).is_some(),
        );
        if known {
            continue;
        }
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::SailError("unresolved-ident"),
            format!("Function {name} not found"),
            base_db::text_range(u32::from(range.start()) as usize, u32::from(range.end()) as usize),
            Severity::Error,
        ));
    }
}

/// A struct update expression cannot set the same field twice.
fn check_duplicate_struct_update_fields(
    defs: &[syntax::SyntaxNode],
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;
    use std::collections::HashSet;

    for node in defs.iter().flat_map(|d| d.descendants()) {
        if !matches!(node.kind(), SK::UPDATE_EXPR | SK::STRUCT_EXPR) {
            continue;
        }
        let mut seen: HashSet<String> = HashSet::new();
        for field in node.children().filter(|c| c.kind() == SK::FIELD_INIT) {
            let Some(tok) = field
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
            else {
                continue;
            };
            let name = tok.text().to_string();
            if !seen.insert(name.clone()) {
                let r = tok.text_range();
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::SailError("duplicate-field"),
                    // `check_duplicate_fields` again, with the wording for
                    // an update rather than a pattern
                    // (`initial_check.ml:1484`).
                    format!("Duplicate field '{name}' in struct update"),
                    base_db::text_range(u32::from(r.start()) as usize, u32::from(r.end()) as usize),
                    Severity::Error,
                ));
            }
        }
    }
}

/// A type definition cannot reuse the name of a Sail built-in type.
fn check_builtin_type_redefinition(item_tree: &ItemTree, diagnostics: &mut Vec<Diagnostic>) {
    for &id in item_tree.top_level_items() {
        let kind = id.def_kind(item_tree);
        let creating = match kind {
            DefKind::Enum => "enum type",
            DefKind::Union => "union type",
            DefKind::Struct => "struct type",
            DefKind::TypeAlias => "type synonym",
            // A `newtype` is a union upstream — `to_ast_typedef` routes it
            // through the same `already_bound "union"` (`type_env.ml:1597`),
            // and `sail` says "union type" on `newtype foo` twice over.
            DefKind::Newtype => "union type",
            DefKind::Bitfield => "bitfield type",
            _ => continue,
        };
        let name = id.name(item_tree).as_str();
        if !is_builtin_type_name(name) {
            continue;
        }
        // `typ_error (id_loc id)` (`type_env.ml:434`) — the caret is on the
        // name, not on the definition that reuses it.
        let span = id.name_span(item_tree);
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::SailError("duplicate-definition"),
            format!("Cannot create {creating} {name}, name is already bound as a built-in type"),
            base_db::text_range(span.start, span.end),
            Severity::Error,
        ));
    }
}

/// How many arguments each built-in type constructor takes, as written.
///
/// Two arities where Sail still accepts the legacy order argument:
/// `vector('n, 'ord, T)` and `bitvector('n, 'ord)` desugar to the two- and
/// one-argument forms. `int` takes none, or one as sugar for `atom`.
/// Checked against `sail --just-check`, not read off one of upstream's two
/// tables: `initial_check.ml` lists the arities before desugaring and
/// `type_env.ml` the ones after, and they disagree for exactly these.
fn builtin_type_arity(name: &str) -> Option<&'static [usize]> {
    Some(match name {
        "vector" => &[2, 3],
        "bitvector" => &[1, 2],
        "bits" | "list" | "atom" | "atom_bool" | "implicit" | "register" | "itself" => &[1],
        "range" => &[2],
        "int" | "bool" => &[0, 1],
        "nat" | "unit" | "string" | "string_literal" | "real" => &[0],
        _ => return None,
    })
}

/// The type variables a definition quantifies at kind `Order`.
///
/// Read off `TYPE_PARAM_LIST`, which the parser now builds. This scanned
/// `def.text()` for a `:` followed by `Order` and then walked backwards over
/// whitespace-separated words — it had to, because the quantifier list was
/// a run of loose tokens with no structure to ask.
fn order_quantified_vars(def: &syntax::SyntaxNode) -> std::collections::HashSet<String> {
    use parser::SyntaxKind as SK;
    let mut out = std::collections::HashSet::new();
    for param in def
        .descendants()
        .filter(|n| n.kind() == SK::TYPE_PARAM)
        .filter(|p| type_param_kind(p).as_deref() == Some("Order"))
    {
        out.extend(
            param
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| t.kind() == SK::TY_VAR)
                .filter_map(|t| t.text().strip_prefix('\'').map(str::to_string)),
        );
    }
    out
}

/// The kind written on one `kopt`, if one was.
fn type_param_kind(param: &syntax::SyntaxNode) -> Option<String> {
    use parser::SyntaxKind as SK;
    let mut toks = param.children_with_tokens().filter_map(|el| el.into_token());
    toks.find(|t| t.kind() == SK::COLON)?;
    toks.find(|t| !t.kind().is_trivia()).map(|t| t.text().to_string())
}

/// A type argument at the wrong kind.
///
/// Upstream's `Type_env.builtin_typs` (`type_env.ml:364`) carries
/// `(parameter_kinds, kind)` per constructor and its checker reports
/// `Expected this type to have kind Int but found kind Type`. This crate
/// copied the *names* only, so `range(bool, 3)`, `vector(8, 4)`,
/// `bits('b)` with `'b : Bool` and `list('n)` with `'n : Int` all went
/// unreported. `check_builtin_type_arity` hand-coded the `vector` /
/// `bitvector` order argument as a special case; this is that special case
/// as the general rule.
///
/// Only a *written* kind counts. A bare `forall 'n.` leaves the kind to be
/// inferred from use, and guessing `Int` there would report the most common
/// signature in the corpus.
fn check_type_argument_kinds(
    defs: &[syntax::SyntaxNode],
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    for def in defs {
        // Kinds this definition declares, from its own quantifiers and, for
        // a synonym, its parameter list.
        let mut declared: std::collections::HashMap<String, hir::Kind> =
            std::collections::HashMap::new();
        for param in def.descendants().filter(|n| n.kind() == SK::TYPE_PARAM) {
            let Some(kind) = type_param_kind(&param).as_deref().and_then(hir::Kind::from_name)
            else {
                continue;
            };
            for tok in param.children_with_tokens().filter_map(|el| el.into_token()) {
                if tok.kind() == SK::TY_VAR {
                    declared.insert(tok.text().to_string(), kind);
                }
            }
        }
        // What each type synonym in this file declares its parameters at.
        let (synonyms, type_names) = if let Some(scope) = scope {
            let selected = scope.referenced(def, Namespace::Type);
            (
                selected
                    .iter()
                    .filter_map(|(name, node)| {
                        alias_parameter_kinds(node).map(|kinds| (name.clone(), kinds))
                    })
                    .collect(),
                selected
                    .iter()
                    .filter_map(|(name, node)| {
                        declared_type_kind(node).map(|kind| (name.clone(), kind))
                    })
                    .collect(),
            )
        } else {
            (synonym_parameter_kinds(def), declared_type_kinds(def))
        };

        for node in def.descendants().filter(|n| n.kind() == SK::TYPE_APP) {
            let mut children = node.children();
            let Some(head) = children.next() else { continue };
            if head.kind() != SK::TYPE_NAMED
                || !node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::L_PAREN)
            {
                continue;
            }
            let name = head.text().to_string();
            let name = name.trim();
            let args: Vec<_> = children.collect();
            let Some(expected) = expected_argument_kinds(name, args.len(), &synonyms) else {
                continue;
            };
            for (index, (arg, want)) in args.iter().zip(expected.iter()).enumerate() {
                let Some(found) = argument_kind(arg, &declared, &type_names) else { continue };
                // `vector('n, 'ord)` is missing its element type, and
                // `check_builtin_type_arity` says so — in more useful words
                // than "expected kind Type". One mistake, one diagnostic;
                // `vector(8, 4)` is not that mistake and is still judged.
                if index == 1 && matches!(name, "vector" | "bitvector") && found == hir::Kind::Order
                {
                    continue;
                }
                if found == *want {
                    continue;
                }
                diagnostics.push(module_diagnostic(
                    "kind-mismatch",
                    format!(
                        "Expected this type to have kind {} but found kind {}",
                        want.as_str(),
                        found.as_str()
                    ),
                    arg.text_range(),
                ));
            }
        }
    }
}

/// The kinds `name` takes, for the number of arguments it was given.
fn expected_argument_kinds(
    name: &str,
    argc: usize,
    synonyms: &std::collections::HashMap<String, Vec<Option<hir::Kind>>>,
) -> Option<Vec<hir::Kind>> {
    // The legacy order forms carry an extra argument; `builtin_type_kinds`
    // holds both spellings.
    let builtin = match (name, argc) {
        ("vector", 3) => hir::builtin_type_kinds("vector_legacy"),
        ("bitvector", 2) => hir::builtin_type_kinds("bitvector_legacy"),
        ("int", 1) => hir::builtin_type_kinds("int_applied"),
        ("bool", 1) => hir::builtin_type_kinds("bool_applied"),
        // `type bits('n) = bitvector('n)` is the prelude's, not the type
        // checker's — `is_builtin_type_name` excludes it on purpose, because
        // treating it as built in would make a file that never included the
        // prelude typecheck. Its parameter's kind is not in doubt, though,
        // and a parameterised synonym's body is still a token run, so there
        // is nothing to infer it from.
        ("bits", 1) => hir::builtin_type_kinds("bitvector"),
        _ => hir::builtin_type_kinds(name),
    };
    if let Some(kinds) = builtin {
        return (kinds.len() == argc).then(|| kinds.to_vec());
    }
    // A synonym constrains only the parameters whose kind it wrote down.
    // `type bits('n) = bitvector('n)` writes none, and Sail infers `'n : Int`
    // from the body — so guessing `Type` reported `bits(xlen)` for an
    // `Int`-kinded `xlen`, which is the idiom the whole corpus is built on.
    let declared = synonyms.get(name)?;
    if declared.len() != argc || declared.iter().all(Option::is_none) {
        return None;
    }
    Some(declared.iter().map(|k| k.unwrap_or(hir::Kind::Type)).collect())
}

/// The kind of a type argument, when it can be told.
///
/// `None` is the common answer and the safe one: an expression whose kind
/// this cannot name is not evidence of anything.
fn argument_kind(
    arg: &syntax::SyntaxNode,
    declared: &std::collections::HashMap<String, hir::Kind>,
    type_names: &std::collections::HashMap<String, hir::Kind>,
) -> Option<hir::Kind> {
    use parser::SyntaxKind as SK;
    let text = arg.text().to_string();
    let text = text.trim();

    // A type variable, at the kind its binder wrote.
    if text.starts_with('\'') && !text.contains(char::is_whitespace) {
        return declared.get(text).copied();
    }
    match text {
        "inc" | "dec" => return Some(hir::Kind::Order),
        "true" | "false" => return Some(hir::Kind::Bool),
        _ => {}
    }
    // A number, or a numeric expression over them.
    if text.parse::<i128>().is_ok() {
        return Some(hir::Kind::Int);
    }
    // A named type — and only a name this file knows to *be* a type.
    // `range(1, max_mem_access)` and `range(0, num_tlb_entries - 1)` name
    // integer constants in type position; calling every bare name a `Type`
    // reported twelve correct signatures in sail-riscv.
    if arg.kind() == SK::TYPE_NAMED {
        if hir::is_builtin_type_name(text) {
            return Some(hir::Kind::Type);
        }
        return type_names.get(text).copied();
    }
    None
}

/// `type ds('n: Int, 'm: Int) = ..` — what each synonym in this file
/// declares its parameters at, `None` where none was written.
fn synonym_parameter_kinds(
    def: &syntax::SyntaxNode,
) -> std::collections::HashMap<String, Vec<Option<hir::Kind>>> {
    use parser::SyntaxKind as SK;
    let mut out = std::collections::HashMap::new();
    let root = def.ancestors().last().unwrap_or_else(|| def.clone());
    for alias in root.descendants().filter(|n| n.kind() == SK::TYPE_ALIAS_DEF) {
        let Some(name) = alias.children().find(|n| n.kind() == SK::NAME) else { continue };
        let Some(kinds) = alias_parameter_kinds(&alias) else { continue };
        if !kinds.is_empty() {
            out.insert(name.text().to_string().trim().to_string(), kinds);
        }
    }
    out
}

fn alias_parameter_kinds(alias: &syntax::SyntaxNode) -> Option<Vec<Option<hir::Kind>>> {
    use parser::SyntaxKind as SK;
    let list = alias.children().find(|n| n.kind() == SK::TYPE_PARAM_LIST)?;
    Some(
        list.children()
            .filter(|n| n.kind() == SK::TYPE_PARAM)
            .flat_map(|p| {
                let kind = type_param_kind(&p).as_deref().and_then(hir::Kind::from_name);
                let count = p.children_with_tokens().filter(|t| t.kind() == SK::TY_VAR).count();
                std::iter::repeat_n(kind, count)
            })
            .collect(),
    )
}

fn declared_type_kind(d: &syntax::SyntaxNode) -> Option<hir::Kind> {
    use parser::SyntaxKind as SK;
    if d.kind() == SK::TYPE_ALIAS_DEF {
        let mut head = d
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .take_while(|t| t.kind() != SK::EQ)
            .filter(|t| !t.kind().is_trivia());
        head.find(|t| matches!(t.kind(), SK::COLON | SK::R_ARROW))?;
        hir::Kind::from_name(head.next()?.text())
    } else if d.children_with_tokens().any(|t| {
        matches!(
            t.kind(),
            SK::KW_STRUCT | SK::KW_ENUM | SK::KW_UNION | SK::KW_NEWTYPE | SK::KW_BITFIELD
        )
    }) {
        Some(hir::Kind::Type)
    } else {
        None
    }
}

/// The names this file declares as types, each at the kind it declares.
///
/// A bare name in type position is only a `Type` if something makes it one,
/// and Sail's synonyms carry their own kind: `type max_mem_access : Int =
/// 4096` and `type num_tlb_entries : Int = 2 ^ n` are *Int*-kinded names,
/// used as bounds in `range(1, max_mem_access)`. Calling every declared type
/// name a `Type` reported those two correct signatures; calling every bare
/// name a `Type` reported twelve.
fn declared_type_kinds(def: &syntax::SyntaxNode) -> std::collections::HashMap<String, hir::Kind> {
    use parser::SyntaxKind as SK;
    let root = def.ancestors().last().unwrap_or_else(|| def.clone());
    let mut out = std::collections::HashMap::new();
    for d in root.descendants().filter(|n| matches!(n.kind(), SK::TYPE_ALIAS_DEF | SK::NAMED_DEF)) {
        let Some((name, _)) = def_name(&d) else { continue };
        let Some(kind) = declared_type_kind(&d) else { continue };
        out.insert(name, kind);
    }
    out
}

/// A built-in type constructor applied to the wrong arguments.
///
/// Two things, because they are the same mistake seen from two sides:
/// the wrong *number* of arguments, and — for `vector` and `bitvector`,
/// which still accept the legacy order argument — the right number in the
/// wrong *kinds*. `vector('n, 'ord, bit)` with `bit` dropped is
/// `vector('n, 'ord)`, which counts two and is still wrong, because the
/// two-argument form wants a type where `'ord` is an order.
///
/// `mutation_check` scores this class at 22%, the worst of the six it
/// measures, and this shape is 34 of the 62 it was missing.
fn check_builtin_type_arity(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        let orders = order_quantified_vars(def);
        let is_order = |arg: &syntax::SyntaxNode| {
            let t = arg.text().to_string();
            let t = t.trim();
            t == "inc" || t == "dec" || t.strip_prefix('\'').is_some_and(|v| orders.contains(v))
        };
        for node in def.descendants() {
            if node.kind() != SK::TYPE_APP {
                continue;
            }
            // `TYPE_APP` also covers infix type operators (`'n + 'm`), where
            // the first child is the left operand rather than a constructor,
            // and an infix application has no parenthesis after its head.
            let mut children = node.children();
            let Some(head) = children.next() else { continue };
            if head.kind() != SK::TYPE_NAMED
                || !node
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::L_PAREN)
            {
                continue;
            }
            let name = head.text().to_string();
            let name = name.trim();
            let Some(allowed) = builtin_type_arity(name) else { continue };
            let args: Vec<_> = children.collect();

            let complaint = if !allowed.contains(&args.len()) {
                let want = match allowed {
                    [n] => format!("{n}"),
                    _ => allowed.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(" or "),
                };
                Some(format!("Type {name} takes {want} arguments, not {}", args.len()))
            } else {
                match (name, args.len()) {
                    // `vector('n, T)` — the second argument is the element
                    // type. An order there means the element type is missing.
                    ("vector", 2) if is_order(&args[1]) => Some(format!(
                        "Type vector is missing its element type: `{}` is an order, \
                         so this needs a third argument",
                        args[1].text()
                    )),
                    // `vector('n, 'ord, T)` — the second must be the order.
                    ("vector", 3) if !is_order(&args[1]) => Some(format!(
                        "Type vector's second argument must be an order, not `{}`",
                        args[1].text()
                    )),
                    ("bitvector", 2) if !is_order(&args[1]) => Some(format!(
                        "Type bitvector's second argument must be an order, not `{}`",
                        args[1].text()
                    )),
                    _ => None,
                }
            };
            let Some(message) = complaint else { continue };
            let r = node.text_range();
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailError("type-error"),
                message,
                base_db::text_range(u32::from(r.start()) as usize, u32::from(r.end()) as usize),
                Severity::Error,
            ));
        }
    }
}

/// Mutability of a name visible inside a function body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mutability {
    /// `var`, or a register — assignable.
    Mutable,
    /// `let`, a parameter, a pattern binding, a top-level let, or an
    /// enumeration member — not assignable.
    Immutable,
}

/// Names visible to assignment checking, with their mutability.
///
/// This over-approximates lexical scope: every binding occurrence anywhere in
/// the file is collected, and `var` wins over `let` for a name bound both
/// ways. Over-approximating is deliberate — it can only suppress a
/// diagnostic, never invent one.
fn collect_global_assignable_names(defs: &[syntax::SyntaxNode]) -> HashMap<String, Mutability> {
    use parser::SyntaxKind as SK;
    let mut names: HashMap<String, Mutability> = HashMap::new();
    let note = |names: &mut HashMap<String, Mutability>, name: String, m: Mutability| match names
        .entry(name)
    {
        std::collections::hash_map::Entry::Occupied(mut e) => {
            if m == Mutability::Mutable {
                e.insert(Mutability::Mutable);
            }
        }
        std::collections::hash_map::Entry::Vacant(e) => {
            e.insert(m);
        }
    };

    for def in defs {
        // Top-level `register` / `var` are mutable; `let` is not.
        if def.kind() == SK::NAMED_DEF {
            let kw = def
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| !t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE)
                .map(|t| t.kind());
            match kw {
                Some(SK::KW_REGISTER) | Some(SK::KW_VAR) => {
                    if let Some((n, _)) = def_name(def) {
                        note(&mut names, n, Mutability::Mutable);
                    }
                }
                Some(SK::KW_LET) => {
                    if let Some((n, _)) = def_name(def) {
                        note(&mut names, n, Mutability::Immutable);
                    }
                }
                Some(SK::KW_ENUM) => {
                    for (n, _) in enum_member_names(def) {
                        note(&mut names, n, Mutability::Immutable);
                    }
                }
                _ => {}
            }
        }
    }
    names
}

/// Check assignment targets: mutability, implicit declaration scope, and
/// tuple l-expression shape.
///
/// Mirrors the `check_lexp` family in upstream type_check.ml.
fn check_assignments(
    file: &dyn FileDb,
    root: &syntax::SyntaxNode,
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    let fallback;
    let bodies = match file.bodies() {
        Some(bodies) => bodies,
        None => {
            fallback = hir::CallableBodies::from_cst(root);
            &fallback
        }
    };

    // Registers, top-level lets/vars and enum members are visible everywhere.
    let mut globals =
        if scope.is_none() { collect_global_assignable_names(defs) } else { HashMap::new() };
    // Registers and top-level bindings declared in other files. Assume
    // mutable: a cross-file `let` cannot be assigned to anyway, and treating
    // them as immutable would report every register write in a model whose
    // registers live in a separate file.
    if let Some(ws) = workspace.filter(|_| scope.is_none()) {
        for name in &ws.value_names {
            globals.entry(name.clone()).or_insert(Mutability::Mutable);
        }
    }
    // `$option --strict-var` requires every variable to be introduced by an
    // explicit `var` (upstream `Initial_check.opt_strict_var`).
    let strict_var = defs.iter().any(|d| {
        d.kind() == parser::SyntaxKind::DIRECTIVE_DEF && {
            let t = d.text().to_string();
            let t = t.trim();
            t.starts_with("$option") && (t.contains("-strict_var") || t.contains("--strict-var"))
        }
    });
    let emit = |diagnostics: &mut Vec<Diagnostic>,
                code: &'static str,
                message: String,
                range: syntax::TextRange| {
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::SailError(code),
            message,
            base_db::text_range(u32::from(range.start()) as usize, u32::from(range.end()) as usize),
            Severity::Error,
        ));
    };

    // Local bindings are scoped to their top-level definition. Two function
    // clauses may each introduce the same name independently, so collecting
    // them file-wide would report a spurious immutable assignment.
    for def in defs {
        if !def.descendants().any(|n| n.kind() == SK::ASSIGN_EXPR) {
            continue;
        }
        let names = if let Some(scope) = scope {
            scope
                .referenced(def, Namespace::Value)
                .into_iter()
                .filter_map(|(name, node)| {
                    let has = |kind| node.children_with_tokens().any(|t| t.kind() == kind);
                    let mutable = if has(SK::KW_REGISTER) || has(SK::KW_VAR) {
                        Mutability::Mutable
                    } else if [SK::KW_LET, SK::KW_ENUM, SK::KW_UNION, SK::KW_NEWTYPE]
                        .into_iter()
                        .any(has)
                    {
                        Mutability::Immutable
                    } else {
                        return None;
                    };
                    Some((name, mutable))
                })
                .collect()
        } else {
            globals.clone()
        };
        let locals = bodies
            .entry_at_offset(usize::from(def.text_range().start()))
            .map(|body| AssignmentLocals::new(file, body, &names));

        for assign in def.descendants() {
            if assign.kind() != SK::ASSIGN_EXPR {
                continue;
            }
            let Some(lexp) = assign_lexp(&assign) else { continue };
            let rhs = assign_rhs(&assign);
            let mut names = names.clone();
            if let Some(locals) = &locals {
                names.extend(locals.at(usize::from(lexp.text_range().start())));
            }

            match lexp.kind() {
                SK::IDENT_EXPR | SK::CAST_EXPR => {
                    // `x = e` or `x : T = e`.
                    let Some(tok) = lexp
                        .descendants_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                    else {
                        continue;
                    };
                    let name = tok.text().to_string();
                    match names.get(&name) {
                    Some(Mutability::Immutable) => emit(
                        diagnostics,
                        "assign-to-immutable",
                        format!(
                            "Cannot modify immutable let-bound constant or enumeration constructor {name}"
                        ),
                        tok.text_range(),
                    ),
                    Some(Mutability::Mutable) => {}
                    None => {
                        if scope.is_some_and(|scope| scope.at(&assign).is_some_and(|at| {
                            hir::source_order::bindings::exists_at(
                                scope.db, at, hir::Name::new(&name), Namespace::Value,
                            )
                        })) {
                            // An inaccessible global is still an existing binding.
                            // Reference checking reports its visibility error.
                            continue;
                        }
                        // Implicit declaration. Legal only inside a block;
                        // upstream otherwise cannot tell what scope it has.
                        if strict_var {
                            emit(
                                diagnostics,
                                "implicit-var-declaration",
                                "Variables must be declared with an explicit var expression"
                                    .to_string(),
                                assign.text_range(),
                            );
                        } else if !locals.as_ref().map_or_else(|| inside_block(&assign), |locals| locals.declares_at(usize::from(tok.text_range().start()))) {
                            emit(
                                diagnostics,
                                "unclear-declaration-scope",
                                "Variable declaration with unclear (or no) scope. Use an explicit var statement instead, or place in a block".to_string(),
                                assign.text_range(),
                            );
                        }
                    }
                }
                }
                SK::FIELD_ACCESS_EXPR | SK::INDEX_EXPR | SK::SUBRANGE_EXPR => {
                    let kind_word =
                        if lexp.kind() == SK::FIELD_ACCESS_EXPR { "Field" } else { "Vector" };
                    let Some(base) = lexp.first_child() else { continue };
                    let Some(tok) = base
                        .descendants_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                    else {
                        continue;
                    };
                    if !names.contains_key(tok.text()) {
                        emit(
                        diagnostics,
                        "assign-to-undeclared",
                        format!(
                            "{kind_word} assignment can only be done to a variable that has already been declared"
                        ),
                        lexp.text_range(),
                    );
                    }
                }
                SK::TUPLE_EXPR => {
                    check_tuple_assignment(&lexp, rhs.as_ref(), &names, diagnostics);
                }
                _ => {}
            }
        }
    }
}

/// Assignment checking shares the HIR lexical scopes used by navigation.
/// Implicit block declarations additionally require a preceding assignment in
/// that scope; merely occurring somewhere in the callable is insufficient.
struct AssignmentLocals<'a> {
    callable: &'a hir::CallableBody,
    scopes: hir::ExprScopes,
}

impl<'a> AssignmentLocals<'a> {
    fn new(
        file: &dyn FileDb,
        callable: &'a hir::CallableBody,
        globals: &HashMap<String, Mutability>,
    ) -> Self {
        let scopes = if file.file_text().is_some() {
            file.body_scopes(callable)
        } else {
            hir::ExprScopes::new_with_globals(&callable.body, |name| {
                globals.contains_key(name.as_str())
            })
        };
        Self { callable, scopes }
    }

    fn declares_at(&self, offset: usize) -> bool {
        self.callable
            .source_map
            .expr_at_offset(offset)
            .and_then(|id| self.scopes.implicit_binding(id))
            .is_some()
    }

    fn at(&self, offset: usize) -> HashMap<String, Mutability> {
        let mut names = HashMap::new();
        let scope = self
            .callable
            .source_map
            .expr_at_offset(offset)
            .and_then(|id| self.scopes.scope_for(id));
        for scope in self.scopes.scope_chain(scope) {
            for entry in self.scopes.entries(scope) {
                if entry.is_implicit()
                    && self
                        .callable
                        .source_map
                        .pat_syntax(entry.pat())
                        .is_some_and(|span| span.start >= offset)
                {
                    continue;
                }
                names.entry(entry.name().to_string()).or_insert(if entry.is_mutable() {
                    Mutability::Mutable
                } else {
                    Mutability::Immutable
                });
            }
        }
        names
    }
}

/// `(x, y) = rhs` — every component must already exist or all must be new,
/// and the right-hand side must be a tuple of matching length.
fn check_tuple_assignment(
    lexp: &syntax::SyntaxNode,
    rhs: Option<&syntax::SyntaxNode>,
    names: &HashMap<String, Mutability>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    let components: Vec<_> = lexp.children().filter(|c| c.kind() == SK::IDENT_EXPR).collect();
    if components.is_empty() {
        return;
    }

    let existing: Vec<&syntax::SyntaxNode> =
        components.iter().filter(|c| names.contains_key(c.text().to_string().trim())).collect();

    // Mixing a fresh binding with an assignment to an existing variable is
    // rejected: upstream cannot tell whether the statement declares or updates.
    if !existing.is_empty() && existing.len() != components.len() {
        for c in &components {
            let text = c.text().to_string();
            let name = text.trim();
            if names.contains_key(name) {
                continue;
            }
            let r = c.text_range();
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailError("mixed-tuple-assignment"),
                format!(
                    "Assignment declaring new variable {name} is also assigning to an existing variable"
                ),
                base_db::text_range(
                    u32::from(r.start()) as usize,
                    u32::from(r.end()) as usize,
                ),
                Severity::Error,
            ));
        }
        return;
    }

    let Some(rhs) = rhs else { return };
    let r = rhs.text_range();
    let range = base_db::text_range(u32::from(r.start()) as usize, u32::from(r.end()) as usize);

    if rhs.kind() == SK::TUPLE_EXPR {
        let rhs_len = rhs.children().filter(|c| c.kind() != SK::ERROR).count();
        if rhs_len != components.len() {
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailError("tuple-arity-mismatch"),
                "Tuple l-expression and tuple type have different length".to_string(),
                range,
                Severity::Error,
            ));
        }
    } else if rhs.kind() == SK::LITERAL_EXPR {
        // A literal is definitely not a tuple.
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::SailError("tuple-lexp-non-tuple"),
            format!(
                "Cannot bind tuple l-expression against non tuple type {}",
                rhs.text().to_string().trim()
            ),
            range,
            Severity::Error,
        ));
    }
}

/// The l-expression of an assignment.
///
/// `let`/`var` statements absorb the rest of their block into their body, so
/// `let x = e; x = f` parses as `ASSIGN_EXPR(LET_EXPR(.., x), f)`. Descend
/// into such a wrapper to reach the real target.
fn assign_lexp(assign: &syntax::SyntaxNode) -> Option<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;
    let mut node = assign.first_child()?;
    while matches!(node.kind(), SK::LET_EXPR | SK::VAR_EXPR | SK::BLOCK_ITEM) {
        node = node.last_child()?;
    }
    Some(node)
}

/// The right-hand side of an assignment: the node after the top-level `=`.
fn assign_rhs(assign: &syntax::SyntaxNode) -> Option<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;
    let mut seen_eq = false;
    for el in assign.children_with_tokens() {
        match el {
            rowan::NodeOrToken::Token(t) if t.kind() == SK::EQ => seen_eq = true,
            rowan::NodeOrToken::Node(n) if seen_eq => return Some(n),
            _ => {}
        }
    }
    None
}

/// Whether an assignment sits inside a `{ ... }` block, which is what gives
/// an implicitly declared variable a well-defined scope.
fn inside_block(assign: &syntax::SyntaxNode) -> bool {
    use parser::SyntaxKind as SK;
    assign.ancestors().any(|a| a.kind() == SK::BLOCK_EXPR)
}

/// Sail checks existing global names even when introducing an explicit local.
/// Use the declaration point before accessibility filtering; a hidden global
/// cannot disappear merely because inference projects only visible values.
fn check_local_binding_visibility(file: &dyn FileDb, diagnostics: &mut Vec<Diagnostic>) {
    use hir::source_order::{self, bindings};
    let (Some((db, input)), Some(bodies)) = (file.file_text(), file.bodies()) else { return };
    for callable in bodies.entries() {
        let Some(at) = source_order::at_offset(db, input, callable.def_span.start) else {
            continue;
        };
        let scopes = file.body_scopes(callable);
        let mut checked = std::collections::HashSet::new();
        for (scope, _) in scopes.all_scopes() {
            for entry in scopes.entries(scope) {
                if !checked.insert(entry.pat()) {
                    continue;
                }
                let name = entry.name();
                let Some(declared) =
                    bindings::declared_before(db, at, name.clone(), Namespace::Value)
                else {
                    continue;
                };
                if bindings::resolve(db, at, name.clone(), Namespace::Value).is_some() {
                    continue;
                }
                let Some(span) = callable.source_map.pat_syntax(entry.pat()) else { continue };
                let Some(definition) = source_order::node(db, declared) else { continue };
                let private =
                    definition.children().any(|n| n.kind() == syntax::SyntaxKind::VISIBILITY);
                let mut diagnostic = module_diagnostic(
                    if private { "use-of-private-definition" } else { "not-in-scope" },
                    if private { "Cannot use private definition" } else { "Not in scope" }
                        .to_owned(),
                    syntax::TextRange::new((span.start as u32).into(), (span.end as u32).into()),
                );
                if let Some((_, range)) = def_name(&definition) {
                    diagnostic.related_in_files.push((
                        declared.file.file_id(db),
                        range,
                        format!("existing definition of {name}"),
                    ));
                }
                diagnostics.push(diagnostic);
            }
        }
    }
}

/// Where a definition lives and whether it is private to that module.
#[derive(Clone, Debug)]
struct ModuleBinding {
    module: Option<String>,
    private: bool,
    /// What the definition is. Upstream words "not in scope" per kind —
    /// `Err_not_in_scope` carries an `explanation`, and four call sites
    /// fill it in (`type_env.ml:1484`, `:1716`, `type_check.ml:2091`,
    /// `:4825`). Only when it is `None` does the renderer fall back to the
    /// bare "Not in scope" this reported for everything
    /// (`type_error.ml:551`).
    kind: BindingKind,
    /// For an enum member, the enum that declares it.
    enum_of: Option<String>,
    /// Range of the defining name, for the "definition here in M" hint.
    range: syntax::TextRange,
    arities: Option<Vec<(usize, usize)>>,
}

impl ModuleBinding {
    fn visible(&self, requires: &HashMap<String, Vec<String>>, user: Option<&str>) -> bool {
        match self.module.as_deref() {
            None => !self.private || user.is_none(),
            Some(owner) => user.is_some_and(|u| {
                u == owner || (!self.private && module_can_see(requires, u, owner))
            }),
        }
    }
}

/// Expand the currently selected overload group at the use site. Unknown
/// members keep arity checking inconclusive; hidden members contribute no
/// candidate. Visit each name once so shared nested groups cannot multiply
/// work exponentially or recurse indefinitely while the user edits a cycle.
fn inline_overload_candidates<'a>(
    name: &str,
    bindings: &'a HashMap<String, ModuleBinding>,
    groups: &HashMap<String, Vec<String>>,
    requires: &HashMap<String, Vec<String>>,
    user: Option<&str>,
) -> Vec<Option<&'a ModuleBinding>> {
    let mut pending = vec![name];
    let mut seen = std::collections::HashSet::new();
    let mut candidates = Vec::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name) {
            continue;
        }
        let binding = bindings.get(name);
        if binding.is_some_and(|b| !b.visible(requires, user)) {
            continue;
        }
        if let Some(members) = groups.get(name) {
            pending.extend(members.iter().rev().map(String::as_str));
        } else {
            candidates.push(binding);
        }
    }
    candidates
}

/// What kind of definition a module binding names.
#[derive(Clone, PartialEq, Eq, Debug)]
enum BindingKind {
    Overload,
    Enum,
    EnumMember,
    Callable,
    Other,
}

/// Enforce Sail's module system: `$project#` / `$start_module#` /
/// `$end_module#` plus the `private` modifier.
///
/// A definition inside module `M` is visible from module `U` only when
/// `U` directly `requires` `M`, and a `private` definition is visible
/// only inside `M` itself. Code outside every module sees nothing that is
/// inside one. Upstream reports "Not in scope" / "Cannot use private
/// definition" (type_env.ml `item_in_scope`).
///
/// The whole check is inert unless the file declares modules, so it cannot
/// affect ordinary single-module sources.
fn check_module_visibility(
    file: &dyn FileDb,
    root: &syntax::SyntaxNode,
    defs: &[syntax::SyntaxNode],
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    fn declares_type(node: &syntax::SyntaxNode) -> bool {
        node.kind() == SK::TYPE_ALIAS_DEF
            || [SK::KW_ENUM, SK::KW_STRUCT, SK::KW_UNION, SK::KW_BITFIELD, SK::KW_NEWTYPE]
                .into_iter()
                .any(|kind| node.children_with_tokens().any(|t| t.kind() == kind))
    }

    let Some(requires) = parse_project_pragma(defs) else { return };
    let fallback;
    let bodies = match file.bodies() {
        Some(bodies) => bodies,
        None => {
            fallback = hir::CallableBodies::from_cst(root);
            &fallback
        }
    };

    // Check and install definitions in execution order. Global declarations
    // participate too: a later module definition cannot rewrite earlier uses.
    let mut bindings: HashMap<String, ModuleBinding> = HashMap::new();
    let mut type_bindings: HashMap<String, ModuleBinding> = HashMap::new();
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    let mut current: Option<String> = None;
    for def in defs {
        if def.kind() == SK::DIRECTIVE_DEF {
            let text = def.text().to_string();
            let text = text.trim();
            if let Some(rest) = text.strip_prefix("$start_module#") {
                current = Some(rest.trim().to_string());
            } else if text.starts_with("$end_module#") {
                current = None;
            }
            continue;
        }
        if def.kind() == SK::INSTANTIATION_DEF
            && file.file_text().is_some_and(|(db, ft)| hir::source_order::program(db, ft).is_some())
        {
            continue; // Shared instantiation reference checks preserve namespaces.
        }
        let user_module = current.clone();
        // Only an actual local reference can shadow a module binding. A later
        // or sibling declaration cannot hide this token; a declaration's own
        // pattern must still pass the compiler's accessibility check.
        let mut local_uses = std::collections::HashSet::new();
        if let Some(callable) = bodies.entry_at_offset(usize::from(def.text_range().start())) {
            let scopes = file.body_scopes(callable);
            for (id, expr) in callable.body.iter_exprs() {
                let hir::Expr::Ident(name) = expr else { continue };
                if scopes.scope_for(id).is_some_and(|scope| {
                    scopes.resolve_name_in_scope(scope, &hir::Name::new(name)).is_some()
                }) {
                    if let Some(span) = callable.source_map.expr_syntax(id) {
                        local_uses.insert((span.start, span.end));
                    }
                }
            }
        }
        // A definition owns only the name it declares itself. A scattered
        // clause extends a name declared elsewhere, and a `function` body
        // whose `val` lives in a module is a use of that module's binding —
        // both must still be checked.
        let is_clause = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_CLAUSE);
        let own = def_name(def);
        let own_range = own
            .as_ref()
            .filter(|(name, _)| {
                !is_clause
                    && (def.kind() != SK::CALLABLE_DEF || !bindings.contains_key(name))
                    && !(is_overload_def(def) && groups.contains_key(name))
            })
            .map(|(_, range)| *range);
        let overload_members: std::collections::HashSet<_> = if is_overload_def(def) {
            overload_member_tokens(def).into_iter().map(|t| t.text_range()).collect()
        } else {
            Default::default()
        };

        for tok in def.descendants_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() != SK::IDENT {
                continue;
            }
            let name = tok.text();
            let in_type_position = tok.parent_ancestors().any(|a| {
                matches!(
                    a.kind(),
                    SK::TYPE_APP
                        | SK::TYPE_NAMED
                        | SK::TYPE_TUPLE
                        | SK::TYPE_ARROW
                        | SK::TYPE_EXISTENTIAL
                        | SK::TYPE_FORALL
                )
            });
            if local_uses.contains(&(
                usize::from(tok.text_range().start()),
                usize::from(tok.text_range().end()),
            )) || own_range == Some(tok.text_range())
                || overload_members.contains(&tok.text_range())
            {
                continue;
            }
            // An overload whose every member is out of reach is one
            // finding about the overload, not one per member: `Overload
            // {f} is defined, but nothing it overloads is in scope`
            // (`type_check.ml:2091`).
            if !in_type_position
                && own.as_ref().is_none_or(|(_, range)| *range != tok.text_range())
                && groups.contains_key(name)
                && bindings.get(name).is_some_and(|b| b.visible(&requires, user_module.as_deref()))
            {
                let candidates = inline_overload_candidates(
                    name,
                    &bindings,
                    &groups,
                    &requires,
                    user_module.as_deref(),
                );
                if candidates.is_empty() {
                    let mut diagnostic = module_diagnostic(
                        "not-in-scope",
                        format!("Overload {name} is defined, but nothing it overloads is in scope"),
                        tok.text_range(),
                    );
                    if let Some(binding) = bindings.get(name) {
                        diagnostic.related.push((binding.range, "overload declared here".into()));
                    }
                    diagnostics.push(diagnostic);
                    continue;
                }
            }
            // Reachable overload members are alternatives. Hidden alternatives
            // do not make a visible group inaccessible.
            let target = name;
            let bindings = if in_type_position { &type_bindings } else { &bindings };
            let Some(binding) = bindings.get(target) else { continue };
            if binding.visible(&requires, user_module.as_deref()) {
                continue;
            }
            // `Err_not_in_scope`'s `explanation`, which upstream fills
            // in for four kinds and leaves `None` for the rest. When it
            // is set it replaces *both* defaults — the private branch
            // reads the same field (`type_error.ml:544`) — so an enum
            // out of scope says the same thing whether or not it is
            // also private.
            // A type that is out of scope fails the *well-formedness*
            // check that was looking it up, and upstream reports that
            // outer failure rather than the lookup
            // (`unscope_type.sail`, `private_union.sail`). Only in a
            // type position: the same name used as a value is the
            // scope error itself.
            let explanation = match binding.kind {
                // `type_env.ml:1484`
                BindingKind::Enum => Some(format!("Enumeration {target} is not in scope")),
                // `type_env.ml:1716`
                BindingKind::EnumMember => binding
                    .enum_of
                    .as_ref()
                    .map(|e| format!("Enumeration {e} containing {target} is not in scope")),
                // `type_check.ml:4825`: a `function` whose `val` lives
                // in a module this one cannot see. Upstream reaches
                // this while *inferring the function's own type*, so it
                // is the definition's own name that triggers it.
                // The definition's own name, on a `function` whose
                // `val` lives in a module this one cannot see.
                BindingKind::Callable
                    if def.kind() == SK::CALLABLE_DEF
                        && def_name(def).is_some_and(|(n, _)| n == target) =>
                {
                    Some(format!(
                        "Cannot infer type of function as it has a defined type already. \
                             However, this type is {}",
                        if binding.private { "private." } else { "not in scope." }
                    ))
                }
                _ => None,
            };
            // A kind upstream has no sentence for, used in a type
            // position, fails the *well-formedness* check that was
            // looking it up, and upstream reports that outer failure
            // rather than the lookup. An enum is not one of these: it
            // has its own sentence, and it wins here as it does there.
            if explanation.is_none() && in_type_position {
                let node = tok.parent().unwrap_or_else(|| def.clone());
                let mut diagnostic =
                    well_formedness_diagnostic(&node, format!("{target} is not in scope here"));
                diagnostic.related.push((binding.range, "selected declaration here".into()));
                diagnostics.push(diagnostic);
                continue;
            }
            if binding.private {
                diagnostics.push(
                    module_diagnostic(
                        "use-of-private-definition",
                        explanation.unwrap_or_else(|| "Cannot use private definition".to_string()),
                        tok.text_range(),
                    )
                    .with_related(vec![(binding.range, "private declaration here".into())]),
                );
            } else {
                diagnostics.push(
                    module_diagnostic(
                        "not-in-scope",
                        explanation.unwrap_or_else(|| "Not in scope".to_string()),
                        tok.text_range(),
                    )
                    .with_related(vec![(binding.range, "selected declaration here".into())]),
                );
            }
        }
        for call in def.descendants().filter(|n| n.kind() == SK::CALL_EXPR) {
            let Some(callee) = call.first_child().and_then(|c| {
                c.children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| t.kind() == SK::IDENT)
            }) else {
                continue;
            };
            let name = callee.text();
            if !groups.contains_key(name)
                || local_uses.contains(&(
                    usize::from(callee.text_range().start()),
                    usize::from(callee.text_range().end()),
                ))
            {
                continue;
            }
            let candidates = inline_overload_candidates(
                name,
                &bindings,
                &groups,
                &requires,
                user_module.as_deref(),
            );
            // Unknown signatures cannot establish an arity error. An empty
            // visible set has already received the scope/private diagnostic.
            if candidates.is_empty() {
                continue;
            }
            let Some(arities) = candidates
                .iter()
                .map(|b| b.and_then(|b| b.arities.as_ref()))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            let Some(args) = call.children().find(|c| c.kind() == SK::ARG_LIST) else { continue };
            let given = arg_count(&args);
            if !arities
                .iter()
                .flat_map(|xs| xs.iter())
                .any(|&(min, max)| (min..=max).contains(&given))
            {
                diagnostics.push(
                    module_diagnostic(
                        "no-possible-overload",
                        format!("No possible overloading for {name}"),
                        call.text_range(),
                    )
                    .with_related(
                        candidates
                            .into_iter()
                            .flatten()
                            .map(|b| (b.range, "visible candidate declared here".into()))
                            .collect(),
                    ),
                );
            }
        }
        let module = current.clone();
        // A constructor's flat `private` token belongs to that constructor,
        // not the containing public union. Declaration visibility is a node.
        let is_private = def.children().any(|c| c.kind() == SK::VISIBILITY);

        // The definition's own name, plus any constructors or enum members
        // it introduces, all become module-scoped.
        let mut names: Vec<(String, syntax::TextRange, bool, BindingKind, Option<String>)> =
            Vec::new();
        let is_enum = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_ENUM);
        let own = def_name(def).map(|(n, _)| n);
        if let Some((n, r)) = def_name(def) {
            let kind = if is_overload_def(def) {
                BindingKind::Overload
            } else if is_enum {
                BindingKind::Enum
            } else if matches!(def.kind(), SK::CALLABLE_DEF | SK::CALLABLE_SPEC) {
                BindingKind::Callable
            } else {
                BindingKind::Other
            };
            names.push((n, r, is_private, kind, None));
        }
        let has_constructors = is_enum
            || def
                .children_with_tokens()
                .any(|t| matches!(t.kind(), SK::KW_UNION | SK::KW_NEWTYPE));
        if has_constructors {
            for field in hir::extract_field_decls(def) {
                let name = field.name.to_string();
                let range = syntax::TextRange::new(
                    (field.name_span.start as u32).into(),
                    (field.name_span.end as u32).into(),
                );
                names.push((
                    name.clone(),
                    range,
                    is_private || field.visibility == hir::visibility::RawVisibility::Private,
                    if is_enum { BindingKind::EnumMember } else { BindingKind::Other },
                    is_enum.then(|| own.clone()).flatten(),
                ));
            }
        }
        // Read a signature once per declaration, not once per constructor in
        // a large union. A nested function type in a payload is not the
        // constructor's own call signature.
        let arities = (def.kind() == SK::CALLABLE_SPEC)
            .then(|| ordered_scope::function_arities(def))
            .flatten();
        let enum_owner = (is_enum && def.kind() == SK::SCATTERED_CLAUSE_DEF)
            .then(|| own.as_ref().and_then(|name| type_bindings.get(name)).cloned())
            .flatten();
        for (name, range, private, kind, enum_of) in names {
            let bindings = if declares_type(def) && def_name(def).is_some_and(|(_, r)| r == range) {
                &mut type_bindings
            } else {
                &mut bindings
            };
            let binding = ModuleBinding {
                module: if kind == BindingKind::Overload {
                    None
                } else if kind == BindingKind::EnumMember {
                    enum_owner.as_ref().map_or_else(|| module.clone(), |owner| owner.module.clone())
                } else {
                    module.clone()
                },
                private: if kind == BindingKind::EnumMember {
                    enum_owner.as_ref().map_or(private, |owner| owner.private)
                } else {
                    private
                },
                kind,
                enum_of,
                range,
                arities: arities.clone(),
            };
            // A function body retains its preceding prototype's ownership.
            // A new val replaces the selected signature, including privacy.
            if def.kind() == SK::CALLABLE_DEF || is_clause {
                bindings.entry(name).or_insert(binding);
            } else {
                bindings.insert(name, binding);
            }
        }
        if is_overload_def(def) {
            if let Some((name, _)) = def_name(def) {
                groups
                    .entry(name)
                    .or_default()
                    .extend(overload_member_tokens(def).iter().map(|t| t.text().to_owned()));
            }
        } else if matches!(def.kind(), SK::CALLABLE_SPEC | SK::CALLABLE_DEF) {
            if let Some((name, _)) = def_name(def) {
                groups.remove(&name);
            }
        }
    }
    check_private_on_function_body(defs, diagnostics);
}

/// What upstream says when a *type definition* does not check, verbatim.
const TYPE_DEF_NOT_WF: &str = "Types are not well-formed within this type definition. \
Note that recursive types are forbidden.";

/// A well-formedness failure, at the type upstream checks rather than at
/// the part of it that is wrong.
///
/// `check_type` walks a type and raises at the top: for `let x : (bits(-1),
/// int)` the caret covers the whole tuple, for `val foo : unit ->
/// bits(nope)` the whole function type, and the specific cause is hung
/// underneath as `Caused by`. Inside a type *definition* the wording
/// changes too — `Types are not well-formed within this type definition.
/// Note that recursive types are forbidden.` — because upstream checks the
/// definition as a unit.
///
/// This reported the inner node, which is the more useful place to put a
/// cursor and the wrong place to put the error: fifteen of `fail/`'s cases
/// had upstream's words at a span upstream does not use. The cause keeps
/// its own span as related information, which is what upstream's nested
/// rendering is.
fn well_formedness_diagnostic(node: &syntax::SyntaxNode, cause: String) -> Diagnostic {
    use parser::SyntaxKind as SK;

    let inner = node.text_range();
    // The definition this type is written in, and whether it *is* a type.
    let def = node.ancestors().find(|a| {
        matches!(
            a.kind(),
            SK::NAMED_DEF
                | SK::CALLABLE_DEF
                | SK::CALLABLE_SPEC
                | SK::SCATTERED_DEF
                | SK::TYPE_ALIAS_DEF
        )
    });
    let is_type_def = def.as_ref().is_some_and(|d| {
        d.children_with_tokens().any(|el| {
            matches!(
                el.kind(),
                SK::KW_TYPE | SK::KW_STRUCT | SK::KW_UNION | SK::KW_ENUM | SK::KW_BITFIELD
            )
        })
    });

    // A synonym that declares a numeric kind — `type t : Nat = v` — has a
    // *numeric type expression* for a body, and upstream says so, at the
    // body: "Well-formedness check failed for numeric type expression"
    // (`initial_check.ml`, the `nexp` path). Inside a `bits(..)` it is
    // still "for type", which is why this is a property of the enclosing
    // synonym and not of the position.
    let numeric_synonym_body = def.as_ref().is_some_and(|d| {
        let mut toks = d
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia());
        toks.next().map(|t| t.kind()) == Some(SK::KW_TYPE)
            && d.children_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| matches!(t.text(), "Int" | "Nat"))
    });
    if numeric_synonym_body {
        // At the body. Some callers hand this the definition rather than
        // the offending node — the body is the token after `=`, and there
        // is exactly one in a synonym.
        let at = def
            .as_ref()
            .filter(|d| d.text_range() == inner)
            .and_then(|d| {
                d.descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .skip_while(|t| t.kind() != SK::EQ)
                    .find(|t| matches!(t.kind(), SK::IDENT | SK::TY_VAR | SK::NUM_LIT))
                    .map(|t| t.text_range())
            })
            .unwrap_or(inner);
        return module_diagnostic(
            "ill-formed-type",
            "Well-formedness check failed for numeric type expression".to_string(),
            at,
        );
    }

    let (range, message) = if is_type_def {
        (def.expect("is_type_def implies a definition").text_range(), TYPE_DEF_NOT_WF.to_string())
    } else {
        // The outermost type this one sits in. `unit -> bits(nope)` is a
        // type; so is `(bits(-1), int)`; so is `bits(-1)` on its own.
        let outer = node
            .ancestors()
            .take_while(|a| {
                matches!(
                    a.kind(),
                    SK::TYPE_APP
                        | SK::TYPE_NAMED
                        | SK::TYPE_TUPLE
                        | SK::TYPE_ARROW
                        | SK::TYPE_EXISTENTIAL
                        | SK::TYPE_FORALL
                )
            })
            .last()
            .unwrap_or_else(|| node.clone());
        (outer.text_range(), "Well-formedness check failed for type".to_string())
    };

    let d = module_diagnostic("ill-formed-type", message.clone(), range);
    if inner == range && cause == message {
        return d;
    }
    d.with_related(vec![(
        base_db::text_range(u32::from(inner.start()) as usize, u32::from(inner.end()) as usize),
        cause,
    )])
}

fn module_diagnostic(code: &'static str, message: String, range: syntax::TextRange) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::SailError(code),
        message,
        base_db::text_range(u32::from(range.start()) as usize, u32::from(range.end()) as usize),
        Severity::Error,
    )
}

/// Whether module `user` can see definitions in module `target`, following
/// direct `requires` edges.
fn module_can_see(requires: &HashMap<String, Vec<String>>, user: &str, target: &str) -> bool {
    // `requires` is not transitive: `C { requires B }` where
    // `B { requires A }` does not put A in scope for C. Upstream's error says
    // "Try requiring module A ... for module C".
    user == target || requires.get(user).is_some_and(|deps| deps.iter().any(|d| d == target))
}

/// `private` belongs on the `val` declaration, not on the function body.
fn check_private_on_function_body(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    let mut signatures = HashMap::new();
    for def in defs {
        if def.kind() == SK::CALLABLE_SPEC {
            if let Some((name, range)) = def_name(def) {
                signatures.insert(name, range);
            }
            continue;
        }
        if def.kind() != SK::CALLABLE_DEF {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        let Some(&signature_range) = signatures.get(&name) else { continue };
        let Some(vis) = def.children().find(|c| c.kind() == SK::VISIBILITY) else { continue };
        // Upstream has the same sentence twice, once per definition form —
        // `type_check.ml:4844` for a function, `:4943` for a mapping — and
        // each says which it is.
        let what = if def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_MAPPING)
        {
            "Mapping"
        } else {
            "Function"
        };
        diagnostics.push(
            module_diagnostic(
                "misplaced-private",
                format!(
                    "{what} body has private modifier, which should be attached to 'val' \
                 declaration instead"
                ),
                vis.text_range(),
            )
            .with_related(vec![(signature_range, "function declared here".into())]),
        );
    }
}

/// Struct literals: the named type must exist, must match the annotation,
/// and every field of the struct must be initialised.
fn check_struct_literals(
    defs: &[syntax::SyntaxNode],
    item_tree: &ItemTree,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    // struct name -> declared field names.
    //
    // From the item tree, which already lowered them, rather than from a
    // second scrape of the CST: the scrape returned nothing for
    // `struct S('n : Int), 'n >= 0 = { .. }`, so no literal of a constrained
    // struct was ever checked, and — once unknown fields were reported too —
    // an empty list said every field in the literal was unknown.
    let mut structs: HashMap<String, Vec<String>> = HashMap::new();
    for &id in item_tree.top_level_items() {
        if id.def_kind(item_tree) != DefKind::Struct {
            continue;
        }
        let fields = id.fields(item_tree).iter().map(|f| f.name.to_string()).collect();
        structs.insert(id.name(item_tree).to_string(), fields);
    }
    if structs.is_empty() && scope.is_none() {
        return;
    }

    for lit in defs.iter().flat_map(|d| d.descendants()) {
        if lit.kind() != SK::STRUCT_EXPR {
            continue;
        }
        let literal_name = lit.children().find(|c| c.kind() == SK::NAME).and_then(|n| {
            n.children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .map(|t| (t.text().to_string(), t.text_range()))
        });

        // `let x : S2 = struct S { .. }` — the literal's name must match the
        // annotated type.
        if let Some((name, range)) = &literal_name {
            if let Some(expected) = expected_struct_type(&lit) {
                if expected != *name {
                    diagnostics.push(module_diagnostic(
                        "struct-type-mismatch",
                        format!("Struct type {name} found, {expected} expected"),
                        *range,
                    ));
                    continue;
                }
            }
        }

        // Missing fields. Only checked when the struct type is known — from
        // the literal's own name or from an enclosing type annotation.
        let type_name =
            literal_name.as_ref().map(|(n, _)| n.clone()).or_else(|| expected_struct_type(&lit));
        let Some(type_name) = type_name else { continue };
        let selected_fields;
        let declared = if let Some(scope) = scope {
            let Some(declaration) = scope.resolve(&lit, &type_name, Namespace::Type) else {
                continue;
            };
            let Some(node) = hir::source_order::node(scope.db, declaration) else { continue };
            let Some(tree) = hir::def_query::file_item_tree(scope.db, declaration.file) else {
                continue;
            };
            let Some(item) = tree.top_level_items().iter().find(|&&item| {
                item.def_kind(tree) == DefKind::Struct
                    && item.name(tree).as_str() == type_name
                    && node.text_range().contains((item.name_span(tree).start as u32).into())
            }) else {
                continue;
            };
            selected_fields =
                item.fields(tree).iter().map(|f| f.name.to_string()).collect::<Vec<_>>();
            &selected_fields
        } else {
            let Some(fields) = structs.get(&type_name) else { continue };
            fields
        };

        let assigned: std::collections::HashSet<String> = lit
            .children()
            .filter(|c| c.kind() == SK::FIELD_INIT)
            .filter_map(|f| {
                f.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
                    .map(|t| t.text().to_string())
            })
            .collect();
        // A field the struct does not declare, whether the literal builds
        // the whole value or updates one: `{ base with zzz = .. }` names a
        // field just as `struct { zzz = .. }` does.
        //
        // Only when the declaration yielded fields. An empty list means the
        // declaration was not understood, not that the struct has no fields,
        // and reporting from it calls every field in the literal unknown.
        for field in
            lit.children().filter(|c| c.kind() == SK::FIELD_INIT).filter(|_| !declared.is_empty())
        {
            let Some(tok) = field
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
            else {
                continue;
            };
            if declared.iter().any(|d| d == tok.text()) {
                continue;
            }
            diagnostics.push(module_diagnostic(
                "unknown-struct-field",
                format!("Struct {type_name} has no field {}", tok.text()),
                tok.text_range(),
            ));
        }

        // A `{ base with .. }` update need not mention every field.
        if lit
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_WITH)
        {
            continue;
        }
        let missing: Vec<&String> = declared.iter().filter(|f| !assigned.contains(*f)).collect();
        if !missing.is_empty() {
            let names = missing.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
            diagnostics.push(module_diagnostic(
                "missing-struct-fields",
                format!("struct literal missing fields: {names}"),
                lit.text_range(),
            ));
        }
    }
}

/// The struct type a literal is annotated with — `ide_db`'s, so the
/// hover that resolves a field name in the same literal asks the same
/// question of the same tree.
fn expected_struct_type(lit: &syntax::SyntaxNode) -> Option<String> {
    ide_db::helpers::struct_literal_type_name(lit)
}

/// Field names declared by a `struct` definition.
fn struct_field_names(def: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    let mut out = Vec::new();
    for field in def.descendants().filter(|n| n.kind() == SK::FIELD_INIT) {
        for tok in field.children_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() == SK::COLON {
                break;
            }
            if tok.kind() == SK::IDENT {
                out.push(tok.text().to_string());
                break;
            }
        }
    }
    out
}

/// An `$[extension]` attribute cannot sit inside a `private` scope.
///
/// `extension_def_attribute` raises "extension attribute within private
/// scope" with a second caret on the `private` that opened it
/// (`type_check.ml:5013-5018`). An extension names a module that may claim
/// the definition; a private definition is claimed by nobody, so the two
/// cannot both be true.
///
/// This had no check, and the file upstream rejects for it —
/// `fail/private_extension.sail` — was being rejected here *by accident*:
/// the attribute's four tokens fell through the union body's field loop
/// and something downstream complained about the wreckage. Teaching the
/// parser to read the attribute took the accident away, which is how the
/// gap became visible. An accident is not a check.
fn check_extension_in_private(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        let Some(private) = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::KW_PRIVATE)
            .or_else(|| {
                def.children()
                    .filter(|c| c.kind() == SK::VISIBILITY)
                    .flat_map(|c| c.children_with_tokens())
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::KW_PRIVATE)
            })
        else {
            continue;
        };
        for attr in def.descendants().filter(|n| n.kind() == SK::ATTRIBUTE) {
            let text = attr.text().to_string();
            if !text.trim_start().starts_with("$[extension") {
                continue;
            }
            diagnostics.push(
                module_diagnostic(
                    "extension-in-private",
                    "extension attribute within private scope".to_string(),
                    attr.text_range(),
                )
                .with_related(vec![(
                    private.text_range(),
                    "private scope started here".to_string(),
                )]),
            );
        }
    }
}

/// `private` belongs before the definition keyword, not inside it.
fn check_misplaced_private(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // `private` is a definition-start token, so `function private foo()`
    // parses as a `CALLABLE_DEF` holding just the keyword followed by a
    // separate definition that opens with `VISIBILITY`.
    for pair in defs.windows(2) {
        let (head, next) = (&pair[0], &pair[1]);
        if !matches!(head.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF) {
            continue;
        }
        let only_keyword = head
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .all(|t| matches!(t.kind(), SK::KW_FUNCTION | SK::KW_MAPPING | SK::KW_CLAUSE));
        if !only_keyword {
            continue;
        }
        if let Some(vis) = next.children().find(|c| c.kind() == SK::VISIBILITY) {
            diagnostics.push(module_diagnostic(
                "misplaced-private",
                "private visibility modifier on function clause".to_string(),
                vis.text_range(),
            ));
        }
    }

    for def in defs {
        if !matches!(def.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF) {
            continue;
        }
        let mut seen_keyword = false;
        for tok in def.children_with_tokens().filter_map(|el| el.into_token()) {
            match tok.kind() {
                SK::KW_FUNCTION | SK::KW_MAPPING => seen_keyword = true,
                SK::KW_PRIVATE if seen_keyword => {
                    diagnostics.push(module_diagnostic(
                        "misplaced-private",
                        "private visibility modifier on function clause".to_string(),
                        tok.text_range(),
                    ));
                    break;
                }
                SK::EQ => break,
                _ => {}
            }
        }
    }
}

/// Builtin type constructors whose leading arguments are numeric.
///
/// The value is how many leading arguments are numeric expressions; the rest
/// are ordinary type arguments (`vector('n, 'a)`, `vector('n, ord, 'a)`).
fn numeric_arg_count(name: &str) -> Option<usize> {
    Some(match name {
        "bits" | "bitvector" | "atom" | "implicit" | "vector" | "itself" => 1,
        "range" => 2,
        _ => return None,
    })
}

/// Identifiers used where a numeric type expression is required must name a
/// type-level constant, not a value.
///
/// Upstream reports "Well-formedness check failed for type" for
/// `bits(some_let_binding)` and friends: the type checker looks the name up
/// in the *type* environment, where a `let` binding does not appear
/// (type_env.ml `bound_typ_id`). Only type synonyms, abstract types and
/// user type definitions qualify; a type *variable* is written `'n` and
/// lexes as `TY_VAR`, so a bare `IDENT` here is always a name lookup.
fn check_type_level_names(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    // Without the workspace we cannot tell an unknown name from one defined
    // in another file, so the check would be pure noise.
    if scope.is_none() && workspace.is_none_or(|ws| ws.type_names.is_empty()) {
        return;
    }

    let known = |node: &syntax::SyntaxNode, name: &str| {
        scope.map_or_else(|| workspace.is_some_and(|ws| ws.type_names.contains(name)), |scope| scope.resolve(node, name, Namespace::Type).is_some())
            || is_builtin_type_name(name)
            || is_builtin_type_level_fn(name)
            // `dec` / `inc` reach here as identifiers in some positions.
            || matches!(name, "dec" | "inc")
    };

    for app in defs.iter().flat_map(|d| d.descendants()) {
        if app.kind() != SK::TYPE_APP {
            continue;
        }
        let Some(ctor) = app
            .children()
            .next()
            .and_then(|c| {
                c.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
            })
            .map(|t| t.text().to_string())
        else {
            continue;
        };
        let Some(n_numeric) = numeric_arg_count(&ctor) else { continue };

        for arg in app.children().skip(1).take(n_numeric) {
            for tok in arg.descendants_with_tokens().filter_map(|el| el.into_token()) {
                if tok.kind() != SK::IDENT || known(&app, tok.text()) {
                    continue;
                }
                diagnostics.push(well_formedness_diagnostic(
                    &app,
                    format!("{} is not a type-level name", tok.text()),
                ));
            }
        }
    }
}

/// A type synonym's body may only mention type variables the synonym binds.
///
/// `type tyvar : Nat = 'v` is ill-formed because `'v` is not a parameter of
/// `tyvar`; upstream reports it when checking the synonym's well-formedness.
fn check_type_synonym_tyvars(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        if def.kind() != SK::TYPE_ALIAS_DEF {
            continue;
        }
        // Type variables are bound either as synonym parameters (before `=`)
        // or by a quantifier in the body. An existential
        // `{'n, constraint. typ}` binds `'n`, and its variable list runs from
        // the `{` to the `.` that opens the body — the alias grammar leaves
        // those tokens flat rather than in a `TYPE_EXISTENTIAL` node, so the
        // brace/dot span is what identifies the binder.
        let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_eq = false;
        let mut binder_depth = 0u32;
        let mut unbound: Vec<syntax::SyntaxToken> = Vec::new();
        for el in def.descendants_with_tokens() {
            let Some(t) = el.into_token() else { continue };
            match t.kind() {
                SK::EQ if !seen_eq => seen_eq = true,
                SK::L_CURLY => binder_depth += 1,
                SK::DOT if binder_depth > 0 => binder_depth -= 1,
                SK::TY_VAR => {
                    if !seen_eq || binder_depth > 0 {
                        bound.insert(t.text().to_string());
                    } else if !bound.contains(t.text()) {
                        unbound.push(t);
                    }
                }
                _ => {}
            }
        }
        // A synonym that declares a *kind* says what failed in terms of
        // that kind, not in terms of the variable: `type t : Nat = 'v` is
        // "Well-formedness check failed for numeric type expression",
        // `: Bool` is "...for constraint", and only a synonym with no
        // declared kind reports the unbound variable itself. Upstream
        // checks the body against the kind and never gets as far as
        // asking what `'v` is.
        let declared_kind = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .take_while(|t| t.kind() != SK::EQ)
            .find_map(|t| match t.text() {
                "Int" | "Nat" => Some("numeric type expression"),
                "Bool" => Some("constraint"),
                _ => None,
            });
        for tok in unbound {
            let message = match declared_kind {
                Some(kind) => format!("Well-formedness check failed for {kind}"),
                None => format!("Unbound type variable {}", tok.text()),
            };
            diagnostics.push(module_diagnostic("unbound-type-variable", message, tok.text_range()));
        }
    }
}

/// `type a : Type` declares an abstract type of kind Type, which Sail
/// forbids — only `Int`, `Nat`, `Bool` and `Order` abstract types exist.
fn check_abstract_type_kind(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        if def.kind() != SK::TYPE_ALIAS_DEF {
            continue;
        }
        let toks: Vec<_> = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        // `type <name> : Type` with no `=` body.
        let has_body = toks.iter().any(|t| t.kind() == SK::EQ);
        let kind_is_type = toks
            .iter()
            .skip_while(|t| t.kind() != SK::COLON)
            .nth(1)
            .is_some_and(|t| t.kind() == SK::KW_TYPE_UPPER);
        let kind_is_order = toks
            .iter()
            .skip_while(|t| t.kind() != SK::COLON)
            .nth(1)
            .is_some_and(|t| t.kind() == SK::KW_ORDER);
        if !has_body && (kind_is_type || kind_is_order) {
            diagnostics.push(module_diagnostic(
                "invalid-abstract-type",
                // `type_check.ml:5085`
                "Abstract type must be either a boolean or integer type".to_string(),
                def.text_range(),
            ));
        }
    }
}

/// The type text of each field or constructor in a struct/union body.
fn member_type_texts(def: &syntax::SyntaxNode) -> Vec<String> {
    use parser::SyntaxKind as SK;
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut seen_body = false;
    let mut after_colon = false;
    let mut depth = 0u32;
    for tok in def.descendants_with_tokens().filter_map(|el| el.into_token()) {
        if tok.kind().is_trivia() {
            continue;
        }
        match tok.kind() {
            SK::L_CURLY if !seen_body => {
                seen_body = true;
                continue;
            }
            _ if !seen_body => continue,
            SK::COLON if depth == 0 => {
                after_colon = true;
                cur.clear();
                continue;
            }
            SK::COMMA | SK::R_CURLY if depth == 0 => {
                if after_colon && !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                after_colon = false;
                continue;
            }
            SK::L_PAREN | SK::L_CURLY => depth += 1,
            SK::R_PAREN => depth = depth.saturating_sub(1),
            _ => {}
        }
        if after_colon {
            cur.push_str(tok.text());
        }
    }
    if after_colon && !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The right-hand side of a type synonym, as text.
fn alias_body_text(def: &syntax::SyntaxNode) -> String {
    use parser::SyntaxKind as SK;
    let mut out = String::new();
    let mut seen_eq = false;
    for tok in def.descendants_with_tokens().filter_map(|el| el.into_token()) {
        if tok.kind().is_trivia() {
            continue;
        }
        if !seen_eq {
            if tok.kind() == SK::EQ {
                seen_eq = true;
            }
            continue;
        }
        out.push_str(tok.text());
    }
    out
}

/// Evaluate a constant integer expression over `+ - * / %` and parentheses.
fn eval_const_int(text: &str) -> Option<i64> {
    fn parse_sum(s: &[u8], i: &mut usize) -> Option<i64> {
        let mut acc = parse_product(s, i)?;
        loop {
            skip_ws(s, i);
            match s.get(*i) {
                Some(b'+') => {
                    *i += 1;
                    acc += parse_product(s, i)?;
                }
                Some(b'-') => {
                    *i += 1;
                    acc -= parse_product(s, i)?;
                }
                _ => return Some(acc),
            }
        }
    }
    fn parse_product(s: &[u8], i: &mut usize) -> Option<i64> {
        let mut acc = parse_atom(s, i)?;
        loop {
            skip_ws(s, i);
            match s.get(*i) {
                Some(b'*') => {
                    *i += 1;
                    acc *= parse_atom(s, i)?;
                }
                Some(b'/') => {
                    *i += 1;
                    let d = parse_atom(s, i)?;
                    if d == 0 {
                        return None;
                    }
                    acc /= d;
                }
                Some(b'%') => {
                    *i += 1;
                    let d = parse_atom(s, i)?;
                    if d == 0 {
                        return None;
                    }
                    acc %= d;
                }
                _ => return Some(acc),
            }
        }
    }
    fn parse_atom(s: &[u8], i: &mut usize) -> Option<i64> {
        skip_ws(s, i);
        match s.get(*i) {
            Some(b'-') => {
                *i += 1;
                Some(-parse_atom(s, i)?)
            }
            Some(b'(') => {
                *i += 1;
                let v = parse_sum(s, i)?;
                skip_ws(s, i);
                if s.get(*i) == Some(&b')') {
                    *i += 1;
                    Some(v)
                } else {
                    None
                }
            }
            Some(c) if c.is_ascii_digit() => {
                let start = *i;
                while s.get(*i).is_some_and(|c| c.is_ascii_digit()) {
                    *i += 1;
                }
                std::str::from_utf8(&s[start..*i]).ok()?.parse().ok()
            }
            _ => None,
        }
    }
    fn skip_ws(s: &[u8], i: &mut usize) {
        while s.get(*i).is_some_and(|c| c.is_ascii_whitespace()) {
            *i += 1;
        }
    }

    let bytes = text.as_bytes();
    let mut i = 0;
    let v = parse_sum(bytes, &mut i)?;
    skip_ws(bytes, &mut i);
    (i == bytes.len()).then_some(v)
}

/// A numeric- or boolean-kinded type synonym's body may not mention values.
///
/// `type t : Nat = v` where `v` is a `let` binding is ill-formed for the same
/// reason `bits(v)` is: `v` does not exist in the type environment.
fn check_kinded_synonym_bodies(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    if scope.is_none() && workspace.is_none_or(|ws| ws.type_names.is_empty()) {
        return;
    }

    for def in defs {
        if def.kind() != SK::TYPE_ALIAS_DEF {
            continue;
        }
        let toks: Vec<_> = def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        // Only the un-parameterised numeric/boolean form `type t : Nat = ..`.
        // A parameter list would put its own `'p : Bool` colon first, and a
        // `Type`-kinded synonym legitimately names other types.
        let head: Vec<_> = toks.iter().take_while(|t| t.kind() != SK::EQ).collect();
        if head.iter().any(|t| t.kind() == SK::L_PAREN) {
            continue;
        }
        let kinded = head
            .iter()
            .skip_while(|t| t.kind() != SK::COLON)
            .nth(1)
            .is_some_and(|t| matches!(t.kind(), SK::KW_INT | SK::KW_NAT | SK::KW_BOOL));
        if !kinded {
            continue;
        }
        // `config` paths name configuration keys, not types.
        if toks.iter().any(|t| t.kind() == SK::KW_CONFIG) {
            continue;
        }
        let mut seen_eq = false;
        for tok in &toks {
            if !seen_eq {
                seen_eq = tok.kind() == SK::EQ;
                continue;
            }
            if tok.kind() != SK::IDENT {
                continue;
            }
            if scope.map_or_else(
                || workspace.is_some_and(|ws| ws.type_names.contains(tok.text())),
                |scope| scope.resolve(def, tok.text(), Namespace::Type).is_some(),
            ) || is_builtin_type_name(tok.text())
                || is_builtin_type_level_fn(tok.text())
            {
                continue;
            }
            diagnostics.push(well_formedness_diagnostic(
                &tok.parent().unwrap_or_else(|| def.clone()),
                format!("{} is not bound here", tok.text()),
            ));
        }
    }
}

/// A function clause may not repeat a quantifier its `val` already declares.
fn check_duplicate_quantifiers(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    let has_forall = |def: &syntax::SyntaxNode| {
        def.descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_FORALL)
    };

    let mut quantified_specs: HashMap<String, syntax::TextRange> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
        if has_forall(def) {
            if let Some((name, range)) = def_name(def) {
                quantified_specs.insert(name, range);
            }
        }
    }

    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_DEF) {
        if !has_forall(def) {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        if let Some(spec_at) = quantified_specs.get(&name) {
            // Upstream carets the *inline annotation* — `forall 'n. (x:
            // int('n)) -> unit`, everything from the `forall` to the end
            // of the signature — and hints "declared here" at the `val`'s
            // name. This reported the whole definition, body included,
            // and said something upstream does not.
            let from = def
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::KW_FORALL)
                .map(|t| t.text_range().start())
                .unwrap_or_else(|| def.text_range().start());
            let to = def
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .take_while(|t| t.kind() != SK::EQ)
                .filter(|t| !t.kind().is_trivia())
                .last()
                .map(|t| t.text_range().end())
                .unwrap_or_else(|| def.text_range().end());
            diagnostics.push(
                module_diagnostic(
                    "duplicate-quantifier",
                    "Duplicate quantifier between inline annotation and 'val' declaration"
                        .to_string(),
                    syntax::TextRange::new(from, to.max(from)),
                )
                .with_related(vec![(*spec_at, "declared here".to_string())]),
            );
        }
    }
}

/// Overload declarations: every member must be bound, and the overload's own
/// name must not already name a function.
///
/// Upstream reports "Cannot create or extend overload f, X is not bound" and
/// "f cannot be defined as an overload, as it is already bound to a function"
/// (type_env.ml:503).
fn check_overload_members(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if scope.is_none() && workspace.is_none_or(|ws| ws.function_names.is_empty()) {
        return;
    }

    // Overload group names are themselves bindable targets for other groups.
    let mut overload_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    for def in defs.iter().filter(|d| is_overload_def(d)) {
        if let Some((name, _)) = def_name(def) {
            overload_names.insert(name);
        }
    }

    for def in defs.iter().filter(|d| is_overload_def(d)) {
        let Some((group, group_range)) = def_name(def) else { continue };

        if let Some(scope) = scope {
            let previous = scope
                .at(def)
                .and_then(|at| {
                    hir::source_order::bindings::declared_before(
                        scope.db,
                        at,
                        hir::Name::new(&group),
                        Namespace::Callable,
                    )
                })
                .and_then(|decl| hir::source_order::node(scope.db, decl));
            if previous.as_ref().is_some_and(is_overload_def)
                && scope.resolve(def, &group, Namespace::Callable).is_none()
            {
                diagnostics.push(module_diagnostic(
                    "invalid-overload",
                    format!("Cannot extend private overload {group} outside its scope"),
                    def.text_range(),
                ));
                continue;
            }
        }

        // `overload foo = {foo}` where `foo` is already a function.
        let already_function = scope.map_or_else(
            || workspace.is_some_and(|ws| ws.function_names.contains(&group)),
            |scope| {
                scope
                    .at(def)
                    .and_then(|at| {
                        hir::source_order::bindings::declared_before(
                            scope.db,
                            at,
                            hir::Name::new(&group),
                            Namespace::Callable,
                        )
                    })
                    .and_then(|decl| hir::source_order::node(scope.db, decl))
                    .is_some_and(|node| !is_overload_def(&node))
            },
        );
        if already_function {
            // `typ_error (Hint ("Previous binding", bound_l, l))`
            // (`type_env.ml:501`): the caret is `l`, the overload
            // definition, and the earlier binding is the hint. This put it
            // on the name.
            let _ = group_range;
            diagnostics.push(module_diagnostic(
                "invalid-overload",
                format!(
                    "{group} cannot be defined as an overload, as it is already bound to a function"
                ),
                def.text_range(),
            ));
            continue;
        }

        for tok in overload_member_tokens(def) {
            let bound = scope.map_or_else(
                || {
                    workspace.is_some_and(|ws| {
                        ws.function_names.contains(tok.text())
                            || ws.constructor_names.contains(tok.text())
                            || ws.value_names.contains(tok.text())
                    }) || overload_names.contains(tok.text())
                },
                |scope| {
                    scope
                        .at(def)
                        .and_then(|at| {
                            hir::source_order::bindings::declared_before(
                                scope.db,
                                at,
                                hir::Name::new(tok.text()),
                                Namespace::Value,
                            )
                        })
                        .is_some()
                },
            );
            if bound {
                continue;
            }
            // Same shape (`type_env.ml:508`): the definition is the
            // caret, the unbound member is `Hint ("unbound identifier",
            // ...)` — which is what this was reporting *as* the caret.
            diagnostics.push(
                module_diagnostic(
                    "invalid-overload",
                    format!(
                        "Cannot create or extend overload {group}, {} is not bound",
                        tok.text()
                    ),
                    def.text_range(),
                )
                .with_related(vec![(tok.text_range(), "unbound identifier".to_string())]),
            );
        }
    }
}

fn is_overload_def(def: &syntax::SyntaxNode) -> bool {
    use parser::SyntaxKind as SK;
    def.kind() == SK::NAMED_DEF
        && def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE)
            .is_some_and(|t| t.kind() == SK::KW_OVERLOAD)
}

/// The identifiers listed in an overload's `{ .. }` member list.
fn overload_member_tokens(def: &syntax::SyntaxNode) -> Vec<syntax::SyntaxToken> {
    use parser::SyntaxKind as SK;
    let mut out = Vec::new();
    let mut in_list = false;
    for tok in def.descendants_with_tokens().filter_map(|el| el.into_token()) {
        match tok.kind() {
            SK::L_CURLY => in_list = true,
            SK::R_CURLY => in_list = false,
            SK::IDENT if in_list => out.push(tok),
            _ => {}
        }
    }
    out
}

/// An outcome instantiation's type must satisfy the implementation's
/// constraints.
///
/// `instantiation O with 'a = bits(33), to_bits = f` applies `f` to
/// `bits(33)`, so `f`'s own `forall 'n, 'n in {32, 48}. bits('n) -> ..`
/// has to admit 33. Upstream proves it ("Could not resolve quantifiers for
/// to_bits: 33 in {32, 48}") and nothing here read the instantiation body
/// at all beyond the outcome's name.
///
/// Only the decidable shape: a concrete width bound to a type variable,
/// and an implementation whose constraint is membership of a set of
/// constants. Anything else says nothing.
fn check_instantiation_constraints(
    defs: &[syntax::SyntaxNode],
    item_tree: &ItemTree,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    // name -> declared signature text, for every `val` in the file.
    let mut specs: HashMap<String, String> = HashMap::new();
    for &id in item_tree.top_level_items().iter().filter(|_| scope.is_none()) {
        if matches!(id.def_kind(item_tree), DefKind::ValSpec) {
            specs.insert(id.name(item_tree).to_string(), id.signature(item_tree).to_string());
        }
    }
    if scope.is_none() && specs.is_empty() {
        return;
    }

    for def in defs.iter().filter(|d| d.kind() == SK::INSTANTIATION_DEF) {
        let selected;
        let specs = if let Some(scope) = scope {
            selected = scope
                .referenced(def, Namespace::Callable)
                .into_iter()
                .filter(|(_, node)| node.kind() == SK::CALLABLE_SPEC)
                .map(|(name, node)| (name, node.text().to_string()))
                .collect::<HashMap<_, _>>();
            &selected
        } else {
            &specs
        };
        let toks: Vec<_> = def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        // `'a = bits(33)` — the one concrete width this instantiation names.
        let mut bound_width: Option<i64> = None;
        for (i, t) in toks.iter().enumerate() {
            if t.kind() != SK::TY_VAR || toks.get(i + 1).map(|t| t.kind()) != Some(SK::EQ) {
                continue;
            }
            let rest: String = toks[i + 2..].iter().map(|t| t.text()).collect();
            if let Some(w) = rest
                .strip_prefix("bits(")
                .or_else(|| rest.strip_prefix("bitvector("))
                .and_then(|r| r.split(&[',', ')'][..]).next())
                .and_then(|w| w.trim().parse::<i64>().ok())
            {
                bound_width = Some(w);
            }
        }
        let Some(width) = bound_width else { continue };

        // `to_bits = f` — every implementation this instantiation supplies.
        for (i, t) in toks.iter().enumerate() {
            if t.kind() != SK::IDENT || toks.get(i + 1).map(|t| t.kind()) != Some(SK::EQ) {
                continue;
            }
            let Some(impl_name) = toks.get(i + 2).filter(|t| t.kind() == SK::IDENT) else {
                continue;
            };
            let Some(sig) = specs.get(impl_name.text()) else { continue };
            let Some(items) = constant_membership_set(sig) else { continue };
            if !items.contains(&width) {
                let names: Vec<String> = items.iter().map(|i| i.to_string()).collect();
                diagnostics.push(module_diagnostic(
                    "instantiation-constraint",
                    format!("{width} is not in {{{}}}", names.join(", ")),
                    impl_name.text_range(),
                ));
            }
        }
    }
}

/// `forall 'n, 'n in {32, 48}. ..` — the set, when every member is a
/// constant and the signature has exactly one such constraint.
fn constant_membership_set(sig: &str) -> Option<Vec<i64>> {
    let at = sig.find(" in ")?;
    let rest = &sig[at + 4..];
    let open = rest.find('{')?;
    let close = rest.find('}')?;
    if open > close {
        return None;
    }
    let items: Option<Vec<i64>> =
        rest[open + 1..close].split(',').map(|p| p.trim().parse::<i64>().ok()).collect();
    items.filter(|v| !v.is_empty())
}

/// `instantiation foo` requires a matching `outcome foo`.
fn check_instantiation_targets(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    let outcomes: std::collections::HashSet<String> = defs
        .iter()
        .filter(|d| d.kind() == SK::OUTCOME_DEF)
        .filter_map(|d| def_name(d).map(|(n, _)| n))
        .collect();

    for def in defs.iter().filter(|d| d.kind() == SK::INSTANTIATION_DEF) {
        let Some((name, range)) = def_name(def) else { continue };
        // An outcome may be declared in another file; it registers as a
        // callable there, so the workspace function set covers it.
        let known_elsewhere = workspace.is_some_and(|ws| ws.function_names.contains(&name));
        let known = scope.map_or(outcomes.contains(&name) || known_elsewhere, |scope| {
            scope.node(def, &name, Namespace::Outcome).is_some()
        });
        if !known {
            diagnostics.push(module_diagnostic(
                "unknown-outcome",
                format!("Unknown outcome {name}"),
                range,
            ));
        }
    }
}

/// Replacement names are checked where the instantiation is written. Keys
/// refer to the outcome's own members and never to same-named globals.
fn check_instantiation_references(
    defs: &[syntax::SyntaxNode],
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use hir::source_order::{self, bindings};
    let Some(scope) = scope else { return };
    for node in defs.iter().filter(|n| n.kind() == syntax::SyntaxKind::INSTANTIATION_DEF) {
        let Some(at) = scope.at(node) else { continue };
        for token in node
            .descendants_with_tokens()
            .filter_map(|t| t.into_token())
            .filter(|t| t.kind() == syntax::SyntaxKind::IDENT)
        {
            let namespace = match syntax::instantiation_reference(
                node,
                usize::from(token.text_range().start()),
            ) {
                Some(syntax::InstantiationReference::Type) => Namespace::Type,
                Some(syntax::InstantiationReference::Callable) => Namespace::Callable,
                _ => continue,
            };
            let name = token.text();
            if namespace == Namespace::Type
                && (hir::is_builtin_type_name(name) || hir::is_builtin_type_level_fn(name))
            {
                continue;
            }
            if scope.resolve(node, name, namespace).is_some() {
                continue;
            }
            let mut diagnostic = module_diagnostic(
                "unknown-instantiation-reference",
                format!(
                    "{} {name} is not available at this instantiation",
                    if namespace == Namespace::Type { "Type" } else { "Function" }
                ),
                token.text_range(),
            );
            let origin = bindings::declared_before(scope.db, at, hir::Name::new(name), namespace)
                .or_else(|| {
                    bindings::declarations_named_at(scope.db, at, hir::Name::new(name), namespace)
                        .first()
                        .copied()
                });
            if let Some(origin) = origin {
                if let Some((_, range)) =
                    source_order::node(scope.db, origin).as_ref().and_then(def_name)
                {
                    diagnostic.related_in_files.push((
                        origin.file.file_id(scope.db),
                        range,
                        "declaration is unavailable here".to_owned(),
                    ));
                }
            }
            diagnostics.push(diagnostic);
        }
    }
}

/// A `mapping` definition's type must match its `val` declaration.
fn check_mapping_declared_type(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    let mut declared: HashMap<String, String> = HashMap::new();
    // Where the `val` declares it, for upstream's "declared here" hint.
    let mut declared_at: HashMap<String, syntax::TextRange> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
        let Some((name, name_range)) = def_name(def) else { continue };
        if let Some(sig) = mapping_signature_text(def) {
            declared.insert(name.clone(), sig);
            declared_at.insert(name, name_range);
        }
    }
    if declared.is_empty() {
        return;
    }

    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_DEF) {
        let is_mapping = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_MAPPING);
        if !is_mapping {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        let (Some(spec), Some((here, ty_range))) = (declared.get(&name), mapping_signature(def))
        else {
            continue;
        };
        if *spec != here {
            let mut d = module_diagnostic(
                "mapping-type-mismatch",
                format!("{spec} and {here} do not match between mapping and 'val' declaration"),
                ty_range,
            );
            if let Some(decl) = declared_at.get(&name) {
                d = d.with_related(vec![(*decl, "declared here".to_string())]);
            }
            diagnostics.push(d);
        }
    }
}

/// A `function` clause whose `val` declares a mapping.
///
/// Upstream reports "function does not have a function type"
/// (`typecheck/fail/invalid_function_val.sail`): `val foo : unit <-> int`
/// binds `foo` to a mapping type, so `function foo() = 3` has nothing to
/// implement.
fn check_function_with_mapping_val(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    let bidir: std::collections::HashSet<String> = defs
        .iter()
        .filter(|d| d.kind() == SK::CALLABLE_SPEC)
        .filter(|d| mapping_signature_text(d).is_some())
        .filter_map(|d| def_name(d).map(|(n, _)| n))
        .collect();
    if bidir.is_empty() {
        return;
    }

    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_DEF) {
        let is_function = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_FUNCTION);
        if !is_function {
            continue;
        }
        let Some((name, range)) = def_name(def) else { continue };
        if bidir.contains(&name) {
            // `type_check.ml:4855` reports at the function definition,
            // with the `val` as a hint; this reported at the name.
            let _ = range;
            diagnostics.push(module_diagnostic(
                "not-a-function-type",
                "function does not have a function type".to_string(),
                def.text_range(),
            ));
        }
    }
}

/// The `a <-> b` signature of a definition, normalised to compare by shape.
fn mapping_signature_text(def: &syntax::SyntaxNode) -> Option<String> {
    mapping_signature(def).map(|(text, _)| text)
}

/// The same, with where the type is written.
///
/// Upstream reports a mapping that disagrees with its `val` at the
/// *type* — `mapping foo : unit <-> int` carets `unit <-> int` — with
/// the `val`'s name hinted "declared here". This reported at the whole
/// definition.
fn mapping_signature(def: &syntax::SyntaxNode) -> Option<(String, syntax::TextRange)> {
    use parser::SyntaxKind as SK;
    let ty = def.children().find(|c| c.kind() == SK::TYPE_ARROW)?;
    let is_bidir = ty
        .children_with_tokens()
        .filter_map(|el| el.into_token())
        .any(|t| t.kind() == SK::DOUBLE_ARROW);
    if !is_bidir {
        return None;
    }
    let normalised: String = ty
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| !t.kind().is_trivia())
        .map(|t| t.text().to_string())
        .collect::<Vec<_>>()
        .join(" ");
    Some((normalised, ty.text_range()))
}

/// Both sides of a bidirectional mapping clause must bind the same names.
///
/// Upstream reports "Identifier x found on left hand side of mapping, but not
/// on right". A name counts as bound when it appears as a bare pattern atom;
/// an identifier applied to arguments is a constructor, and one after `:` is
/// a type.
fn check_mapping_clause_symmetry(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    // Enum members and nullary constructors are constant patterns, not
    // bindings, so they need not appear on both sides. Without the workspace
    // there is no way to tell them from a variable.
    if workspace.is_none() && scope.is_none() {
        return;
    }

    for def in defs {
        if !matches!(def.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF) {
            continue;
        }
        if !def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_MAPPING)
        {
            continue;
        }
        for bidir in def.descendants().filter(|n| n.kind() == SK::BIN_EXPR) {
            let is_bidir = bidir
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::DOUBLE_ARROW);
            if !is_bidir {
                continue;
            }
            // A guarded clause `pat if guard <-> pat` parses with the guard,
            // not the pattern, as the left operand of `<->`, so the two sides
            // are not comparable. Upstream models this as `MCL_when`;
            // grouping it correctly is a parser change, and comparing the
            // guard against a pattern would be worse than saying nothing.
            if bidir.ancestors().take_while(|a| a != def).any(|a| a.kind() == SK::IF_EXPR) {
                continue;
            }
            let sides: Vec<_> = bidir.children().collect();
            if sides.len() != 2 {
                continue;
            }
            let mut lhs = mapping_pattern_bindings(&sides[0]);
            let mut rhs = mapping_pattern_bindings(&sides[1]);
            lhs.retain(|(n, _)| {
                !scope.map_or_else(
                    || workspace.is_some_and(|ws| ws.constructor_names.contains(n)),
                    |scope| scope.is_constructor(def, n),
                )
            });
            rhs.retain(|(n, _)| {
                !scope.map_or_else(
                    || workspace.is_some_and(|ws| ws.constructor_names.contains(n)),
                    |scope| scope.is_constructor(def, n),
                )
            });
            // A name bound twice on one side is a *duplicate*, and that is
            // the finding — `bind_pat` raises before either side is
            // compared with the other, so upstream never says the name is
            // missing from the far side. `(zeros_3, zeros_3) <-> 0b000010`
            // drew both here, at the same token.
            // `mapping_pattern_bindings` returns each name once, so the
            // duplicate has to be counted on the side's own tokens.
            let twice = |side: &syntax::SyntaxNode, name: &str| {
                side.descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .filter(|t| t.kind() == SK::IDENT && t.text() == name)
                    .count()
                    > 1
            };
            for (name, tok) in &lhs {
                if twice(&sides[0], name) {
                    continue;
                }
                if !rhs.iter().any(|(n, _)| n == name) {
                    diagnostics.push(module_diagnostic(
                        "mapping-binding-mismatch",
                        format!(
                            "Identifier {name} found on left hand side of mapping, but not on right"
                        ),
                        tok.text_range(),
                    ));
                }
            }
            for (name, tok) in &rhs {
                if twice(&sides[1], name) {
                    continue;
                }
                if !lhs.iter().any(|(n, _)| n == name) {
                    diagnostics.push(module_diagnostic(
                        "mapping-binding-mismatch",
                        format!(
                            "Identifier {name} found on right hand side of mapping, but not on left"
                        ),
                        tok.text_range(),
                    ));
                }
            }
        }
    }
}

/// Names bound by one side of a mapping clause.
///
/// Skips constructor heads (an identifier followed by `(`), type annotations
/// (anything after `:` within the side), the labels of struct fields written
/// `f = pat`, and the `if`-guard, none of which introduce bindings.
fn mapping_pattern_bindings(side: &syntax::SyntaxNode) -> Vec<(String, syntax::SyntaxToken)> {
    use parser::SyntaxKind as SK;
    let mut out: Vec<(String, syntax::SyntaxToken)> = Vec::new();
    let mut stack = vec![side.clone()];
    while let Some(node) = stack.pop() {
        // `x : T` — the type contributes no bindings.
        if matches!(node.kind(), SK::CAST_EXPR | SK::TYPED_PAT) {
            if let Some(first) = node.first_child() {
                stack.push(first);
            }
            continue;
        }
        // Type sub-trees never bind.
        if matches!(
            node.kind(),
            SK::TYPE_NAMED
                | SK::TYPE_APP
                | SK::TYPE_ARROW
                | SK::TYPE_TUPLE
                | SK::TYPE_VAR
                | SK::TYPE_FORALL
                | SK::TYPE_EXISTENTIAL
        ) {
            continue;
        }
        // `Ctor(a, b)` — the head is a constructor, its arguments bind.
        if matches!(node.kind(), SK::CALL_EXPR | SK::APP_PAT) {
            for child in node.children().skip(1) {
                stack.push(child);
            }
            continue;
        }
        // A struct field. `f = pat` binds whatever `pat` binds and the label
        // itself binds nothing; the punned `f` spelling is `f = f`
        // (`fpat: | id`, parser.mly:570), so there the label *is* the binding.
        if node.kind() == SK::FIELD_INIT {
            match node.first_child() {
                Some(pat) => stack.push(pat),
                None => {
                    if let Some(tok) = node
                        .children_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                    {
                        let name = tok.text().to_string();
                        if !out.iter().any(|(n, _)| *n == name) {
                            out.push((name, tok));
                        }
                    }
                }
            }
            continue;
        }
        if matches!(node.kind(), SK::IDENT_EXPR | SK::IDENT_PAT) {
            if let Some(tok) = node
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
            {
                let name = tok.text().to_string();
                if !out.iter().any(|(n, _)| *n == name) {
                    out.push((name, tok));
                }
            }
            continue;
        }
        for child in node.children() {
            stack.push(child);
        }
    }
    out
}

/// `$[enum_vector X]` generates a top-level `let X`, so an explicit `let X`
/// collides with it.
fn check_enum_vector_attribute(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    let mut generated: Vec<(String, syntax::TextRange)> = Vec::new();
    for attr in defs.iter().filter(|n| n.kind() == SK::ATTRIBUTE) {
        if attribute_name(attr).as_deref() != Some("enum_vector") {
            continue;
        }
        // The generated binding's name is the attribute's argument.
        if let Some(tok) = attr
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| t.kind() == SK::IDENT)
            .nth(1)
        {
            generated.push((tok.text().to_string(), tok.text_range()));
        }
    }
    if generated.is_empty() {
        return;
    }

    for def in defs.iter().filter(|d| d.kind() == SK::NAMED_DEF) {
        let is_let = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE)
            .is_some_and(|t| t.kind() == SK::KW_LET);
        if !is_let {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        if let Some((_, prev)) = generated.iter().find(|(g, _)| *g == name) {
            // `check_duplicate_letbinding` (`type_check.ml:4621`) reports
            // at the whole `let`, with `Previous definition` hinted at
            // what bound the name first — here the `$[enum_vector ..]`
            // attribute that generates it. The other producer of this
            // sentence already reports at the definition; this one still
            // pointed at the name.
            diagnostics.push(
                module_diagnostic(
                    "duplicate-definition",
                    format!("Duplicate toplevel let binding {name}"),
                    def.text_range(),
                )
                .with_related(vec![(*prev, "Previous definition".to_string())]),
            );
        }
    }
}

/// The arms of `enum E with f -> T = { M => v, .. }` have to produce `T`.
///
/// Nothing else checks them: the arms are part of a type definition rather
/// than a function body, so inference never visits them, and a wrong width
/// went unreported. Only bitvector literals are judged here — those are what
/// the arms of an `as_bits` clause are made of, and their width is exact.
fn check_enum_clause_arms(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    /// Split on commas that are not inside brackets or a string.
    fn split_top_level_commas(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        let (mut depth, mut in_str, mut start) = (0i32, false, 0usize);
        for (i, c) in text.char_indices() {
            match c {
                '"' => in_str = !in_str,
                '(' | '[' | '{' if !in_str => depth += 1,
                ')' | ']' | '}' if !in_str => depth -= 1,
                ',' if !in_str && depth == 0 => {
                    out.push(text[start..i].to_string());
                    start = i + 1;
                }
                _ => {}
            }
        }
        out.push(text[start..].to_string());
        out
    }

    /// `0xF` is four bits, `0b1010` is four bits; anything else is unknown.
    fn literal_width(text: &str) -> Option<usize> {
        let t = text.trim();
        if let Some(hex) = t.strip_prefix("0x") {
            let digits = hex.chars().filter(|c| c.is_ascii_hexdigit()).count();
            (digits > 0 && hex.chars().all(|c| c.is_ascii_hexdigit() || c == '_'))
                .then_some(digits * 4)
        } else if let Some(bin) = t.strip_prefix("0b") {
            let digits = bin.chars().filter(|c| *c == '0' || *c == '1').count();
            (digits > 0 && bin.chars().all(|c| c == '0' || c == '1' || c == '_')).then_some(digits)
        } else {
            None
        }
    }

    /// The declared width of `bits(N)`, if it is written that plainly.
    fn declared_width(ty: &str) -> Option<usize> {
        ty.trim().strip_prefix("bits(")?.strip_suffix(')')?.trim().parse().ok()
    }

    for def in defs {
        if def.kind() != SK::NAMED_DEF {
            continue;
        }
        let raw = def.text().to_string();
        if !raw.trim_start().starts_with("enum") {
            continue;
        }
        // Comments are blanked rather than removed so byte offsets still line
        // up with the file; a `//` line inside the body would otherwise be
        // read as part of an arm.
        let text: String = {
            let mut out = String::with_capacity(raw.len());
            let mut in_line_comment = false;
            let mut chars = raw.char_indices().peekable();
            while let Some((_, c)) = chars.next() {
                if in_line_comment {
                    if c == '\n' {
                        in_line_comment = false;
                        out.push(c);
                    } else {
                        out.push(' ');
                    }
                    continue;
                }
                if c == '/' && chars.peek().map(|(_, n)| *n) == Some('/') {
                    in_line_comment = true;
                    out.push(' ');
                    continue;
                }
                out.push(c);
            }
            out
        };
        let Some((head, body)) = text.split_once('=') else { continue };
        let Some((_, clauses)) = head.split_once(" with ") else { continue };
        // `as_bits -> bits(16), as_string -> string` — the arm's value has
        // one component per clause, in this order.
        let widths: Vec<Option<usize>> = clauses
            .split(',')
            .filter_map(|c| c.split_once("->").map(|(_, ty)| ty))
            .map(declared_width)
            .collect();
        if widths.is_empty() || widths.iter().all(Option::is_none) {
            continue;
        }
        let Some(body) = body.trim().strip_prefix('{').and_then(|b| b.strip_suffix('}')) else {
            continue;
        };
        for arm in split_top_level_commas(body) {
            let Some((_, value)) = arm.split_once("=>") else { continue };
            let value = value.trim();
            // One clause: the arm is the value. Several: it is a tuple.
            let parts: Vec<String> = if widths.len() == 1 {
                vec![value.to_string()]
            } else {
                match value.strip_prefix('(').and_then(|v| v.strip_suffix(')')) {
                    Some(inner) => split_top_level_commas(inner),
                    None => continue,
                }
            };
            for (want, part) in widths.iter().zip(parts.iter()) {
                let (Some(want), Some(got)) = (want, literal_width(part)) else { continue };
                if *want == got {
                    continue;
                }
                // The arm text is searched for rather than tracked through the
                // splits: the def's own range is a safe anchor and the value
                // appears verbatim in it.
                let range = text
                    .find(part.trim())
                    .map(|i| {
                        let start = u32::from(def.text_range().start()) as usize + i;
                        syntax::TextRange::new(
                            (start as u32).into(),
                            ((start + part.trim().len()) as u32).into(),
                        )
                    })
                    .unwrap_or_else(|| def.text_range());
                diagnostics.push(module_diagnostic(
                    "type-error",
                    format!("bits({got}) is not a subtype of bits({want})"),
                    range,
                ));
            }
        }
    }
}

/// Subrange patterns that bind one name must cover a contiguous range.
///
/// `imm[20] @ imm[9..0] @ imm[11] @ imm[19..12]` leaves index 10 unbound;
/// upstream reports "Cannot bind imm as pattern subranges are non-contiguous.
/// imm[10] is not defined."
fn check_subrange_contiguity(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        if !matches!(def.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF) {
            continue;
        }
        // Group by the name being indexed, per side of a mapping clause so
        // that the two sides do not pool their indices.
        for region in subrange_regions(def) {
            let mut by_name: HashMap<String, (Vec<(i64, i64)>, syntax::TextRange)> = HashMap::new();
            for (name, lo, hi, range) in collect_subranges(&region) {
                let entry = by_name.entry(name).or_insert_with(|| (Vec::new(), range));
                entry.0.push((lo, hi));
            }
            for (name, (mut ranges, range)) in by_name {
                if ranges.len() < 2 {
                    continue;
                }
                ranges.sort_unstable();
                let mut covered: Vec<bool> = Vec::new();
                let max = ranges.iter().map(|(_, h)| *h).max().unwrap_or(0);
                if !(0..=4096).contains(&max) {
                    continue;
                }
                covered.resize(max as usize + 1, false);
                for (lo, hi) in &ranges {
                    for i in *lo..=*hi {
                        if i >= 0 {
                            covered[i as usize] = true;
                        }
                    }
                }
                // Coverage must start at index 0: the pattern binds the whole
                // vector, so a gap below the lowest subrange is still a gap.
                if let Some(missing) = (0..=max).find(|i| !covered[*i as usize]) {
                    // Two sentences upstream, and which one depends on the
                    // pieces *after merging* what is contiguous. `imm[20]
                    // @ imm[10..1] @ imm[11] @ imm[19..12]` is four
                    // pieces and one range, 1..20 — no hole in it, it
                    // simply never reaches bit 0 — so it is "do not start
                    // at bit 0 (lowest bit is 1)", not "non-contiguous"
                    // (`type_check.ml:1788-1800`, which matches on the
                    // merged list). Counting the pieces instead said the
                    // wrong one for every multi-piece pattern, and
                    // reported a gap at `imm[0]`: true, and not what
                    // upstream tells the reader to fix.
                    let mut merged: Vec<(i64, i64)> = Vec::new();
                    for &(lo, hi) in &ranges {
                        match merged.last_mut() {
                            Some(last) if lo <= last.1 + 1 => last.1 = last.1.max(hi),
                            _ => merged.push((lo, hi)),
                        }
                    }
                    let message = if merged.len() == 1 {
                        let lowest = merged[0].0;
                        format!(
                            "Cannot bind {name} as pattern subranges do not start at bit 0 \
                             (lowest bit is {lowest})."
                        )
                    } else {
                        format!(
                            "Cannot bind {name} as pattern subranges are non-contiguous. \
                             {name}[{missing}] is not defined."
                        )
                    };
                    diagnostics.push(module_diagnostic("non-contiguous-subrange", message, range));
                }
            }
        }
    }
}

/// The regions of a definition whose subrange patterns share one binding
/// scope: each side of a `<->` separately, or the whole definition.
fn subrange_regions(def: &syntax::SyntaxNode) -> Vec<syntax::SyntaxNode> {
    use parser::SyntaxKind as SK;
    let mut out = Vec::new();
    for bidir in def.descendants().filter(|n| n.kind() == SK::BIN_EXPR) {
        if bidir
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::DOUBLE_ARROW)
        {
            out.extend(bidir.children());
        }
    }
    if out.is_empty() {
        // Only the parameter pattern binds. Subranges elsewhere in a body are
        // ordinary reads — `sig[15..8] @ sig[3..0]` as an expression is fine.
        out.extend(def.children().filter(|c| c.kind() == SK::PARAM_LIST));
    }
    out
}

/// `name[hi .. lo]` and `name[i]` occurrences under `node`.
fn collect_subranges(node: &syntax::SyntaxNode) -> Vec<(String, i64, i64, syntax::TextRange)> {
    use parser::SyntaxKind as SK;
    let mut out = Vec::new();
    for idx in node.descendants() {
        if !matches!(
            idx.kind(),
            SK::INDEX_EXPR | SK::SUBRANGE_EXPR | SK::INDEX_PAT | SK::RANGE_INDEX_PAT
        ) {
            continue;
        }
        let Some(name) = idx
            .first_child()
            .and_then(|c| {
                c.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
            })
            .map(|t| t.text().to_string())
        else {
            continue;
        };
        let nums: Vec<i64> = idx
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| t.kind() == SK::NUM_LIT)
            .filter_map(|t| t.text().parse().ok())
            .collect();
        let (lo, hi) = match nums.as_slice() {
            [i] => (*i, *i),
            [a, b] => (*a.min(b), *a.max(b)),
            _ => continue,
        };
        out.push((name, lo, hi, idx.text_range()));
    }
    // Both scans always run, and their results are merged. Either one alone
    // can miss a piece — `(0b0 @ v[1..0] @ v [4..3] @ v[2])` does not produce
    // an index node for every part — and a missed piece is indistinguishable
    // from a real gap, so it was reported as one.
    // A parenthesised parameter pattern is lowered as flat tokens rather than
    // `INDEX_EXPR` nodes, so `f(v[2..0] @ v[3])` needs a token-level scan.
    let toks: Vec<_> = node
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    let mut i = 0;
    while i < toks.len() {
        if toks[i].kind() != SK::IDENT || toks.get(i + 1).map(|t| t.kind()) != Some(SK::L_BRACK) {
            i += 1;
            continue;
        }
        let name = toks[i].text().to_string();
        let mut j = i + 2;
        let mut nums = Vec::new();
        while j < toks.len() && toks[j].kind() != SK::R_BRACK {
            if toks[j].kind() == SK::NUM_LIT {
                if let Ok(v) = toks[j].text().parse::<i64>() {
                    nums.push(v);
                }
            }
            j += 1;
        }
        if let (Some(a), Some(b)) = (nums.first(), nums.last()) {
            let entry = (name, *a.min(b), *a.max(b), toks[i].text_range());
            if !out.iter().any(|(n, lo, hi, _)| (n, lo, hi) == (&entry.0, &entry.1, &entry.2)) {
                out.push(entry);
            }
        }
        i = j + 1;
    }
    out
}

/// A name bound on both sides of a mapping clause must have the same declared
/// type on each side.
fn check_mapping_annotation_agreement(
    defs: &[syntax::SyntaxNode],
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    for def in defs {
        if !matches!(def.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF) {
            continue;
        }
        for bidir in def.descendants().filter(|n| n.kind() == SK::BIN_EXPR) {
            if !bidir
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::DOUBLE_ARROW)
            {
                continue;
            }
            let sides: Vec<_> = bidir.children().collect();
            if sides.len() != 2 {
                continue;
            }
            let lhs = annotated_bindings(&sides[0]);
            let rhs = annotated_bindings(&sides[1]);
            for (name, lty, _) in &lhs {
                let Some((_, rty, range)) = rhs.iter().find(|(n, _, _)| n == name) else {
                    continue;
                };
                if lty != rty {
                    // `'{id}' must have the same type on both sides of the
                    // mapping` (`type_check.ml:1738`), with each side's
                    // type as a hint — `has type {typ}` at each binding.
                    // This said the two types in the sentence and named
                    // neither location.
                    // At the clause — `typ_raise l` where `l` is the
                    // mapping pattern being bound (`type_check.ml:1736`),
                    // with each side's type as a hint. This reported at
                    // the right-hand binding, which is one of the two
                    // hints.
                    diagnostics.push(
                        module_diagnostic(
                            "mapping-type-mismatch",
                            format!(
                                "'{name}' must have the same type on both sides of the mapping"
                            ),
                            bidir.text_range(),
                        )
                        .with_related(vec![(*range, format!("has type {rty}"))]),
                    );
                }
            }
        }
    }
}

/// `x : T` bindings directly under one side of a mapping clause.
fn annotated_bindings(side: &syntax::SyntaxNode) -> Vec<(String, String, syntax::TextRange)> {
    use parser::SyntaxKind as SK;
    let mut out = Vec::new();
    let mut stack = vec![side.clone()];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), SK::CAST_EXPR | SK::TYPED_PAT) {
            let name = node.first_child().and_then(|c| {
                c.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
                    .map(|t| t.text().to_string())
            });
            let ty = node
                .children()
                .find(|c| {
                    matches!(
                        c.kind(),
                        SK::TYPE_NAMED | SK::TYPE_APP | SK::TYPE_VAR | SK::TYPE_TUPLE
                    )
                })
                .map(|c| {
                    c.descendants_with_tokens()
                        .filter_map(|el| el.into_token())
                        .filter(|t| !t.kind().is_trivia())
                        .map(|t| t.text().to_string())
                        .collect::<String>()
                });
            if let (Some(name), Some(ty)) = (name, ty) {
                out.push((name, ty, node.text_range()));
            }
        }
        for child in node.children() {
            stack.push(child);
        }
    }
    out
}

/// `undefined` and `[]` carry no type of their own; a binding with no
/// annotation cannot infer one.
fn check_uninferable_literals(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        for binding in def.descendants().chain(std::iter::once(def.clone())) {
            let is_binding = match binding.kind() {
                SK::LET_EXPR | SK::VAR_EXPR => true,
                SK::NAMED_DEF => binding
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| !t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE)
                    .is_some_and(|t| matches!(t.kind(), SK::KW_LET | SK::KW_VAR)),
                _ => false,
            };
            if !is_binding {
                continue;
            }
            // An annotation anywhere before `=` supplies the type.
            let annotated = binding
                .children_with_tokens()
                .take_while(|el| el.as_token().is_none_or(|t| t.kind() != SK::EQ))
                .any(|el| {
                    el.as_token().is_some_and(|t| t.kind() == SK::COLON)
                        || el.as_node().is_some_and(|n| n.kind() == SK::TYPED_PAT)
                });
            if annotated {
                continue;
            }
            let Some(value) = binding
                .children_with_tokens()
                .skip_while(|el| el.as_token().is_none_or(|t| t.kind() != SK::EQ))
                .filter_map(|el| el.into_node())
                .next()
            else {
                continue;
            };
            let text = value.text().to_string();
            let text = text.trim();
            if text == "undefined" {
                diagnostics.push(module_diagnostic(
                    "uninferable-type",
                    "Cannot infer type of: undefined".to_string(),
                    value.text_range(),
                ));
            } else if text == "[]" {
                diagnostics.push(module_diagnostic(
                    "uninferable-type",
                    "Cannot infer type of empty vector".to_string(),
                    value.text_range(),
                ));
            }
        }
    }
}

/// Global `constraint` declarations must be mutually consistent.
///
/// Only the syntactically obvious cases: `constraint false`, and a constraint
/// declared alongside its own negation.
fn check_global_constraints(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    let normalise = |def: &syntax::SyntaxNode| -> String {
        def.descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia() && t.kind() != SK::KW_CONSTRAINT)
            .map(|t| t.text().to_string())
            .collect()
    };

    let mut seen: Vec<(String, syntax::TextRange)> = Vec::new();
    for def in defs.iter().filter(|d| d.kind() == SK::CONSTRAINT_DEF) {
        let body = normalise(def);
        let inconsistent = body == "false"
            || seen
                .iter()
                .any(|(prev, _)| *prev == format!("not({body})") || body == format!("not({prev})"));
        if inconsistent {
            diagnostics.push(module_diagnostic(
                "inconsistent-constraint",
                "Global constraint appears inconsistent with previous global constraints"
                    .to_string(),
                def.text_range(),
            ));
        }
        seen.push((body, def.text_range()));
    }
}

/// A `function` definition's parameter types must agree with its `val`.
fn check_signature_agreement(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // name -> the val's parameter type texts, when all are concrete.
    let mut declared: HashMap<String, Vec<String>> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
        // A quantified signature can be instantiated, so only compare
        // monomorphic ones.
        if def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| matches!(t.kind(), SK::KW_FORALL | SK::TY_VAR))
        {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        let Some(arrow) = def.children().find(|c| c.kind() == SK::TYPE_ARROW) else { continue };
        if arrow
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::DOUBLE_ARROW)
        {
            continue;
        }
        let Some(lhs) = arrow.children().next() else { continue };
        let params: Vec<String> = if lhs.kind() == SK::TYPE_TUPLE {
            lhs.children().map(|c| normalised_type_text(&c)).collect()
        } else {
            vec![normalised_type_text(&lhs)]
        };
        declared.insert(name, params);
    }
    if declared.is_empty() {
        return;
    }

    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_DEF) {
        if def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| matches!(t.kind(), SK::KW_CLAUSE | SK::KW_MAPPING))
        {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        let Some(expected) = declared.get(&name) else { continue };
        // A concatenation pattern annotates its *parts*: in
        // `function test 0b0 @ _ : bits(5) @ 0b1 = ..` the `bits(5)` types
        // the middle slice, not the parameter, so comparing it against the
        // `val` reports a mismatch that is not one.
        let head_has_concat = def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .take_while(|t| t.kind() != SK::EQ)
            .any(|t| t.kind() == SK::AT);
        if head_has_concat {
            continue;
        }
        let annotated: Vec<(String, syntax::TextRange)> = def
            .children()
            .filter(|c| c.kind() == SK::PARAM_LIST)
            .flat_map(|pl| pl.children())
            .filter(|c| {
                matches!(c.kind(), SK::TYPE_NAMED | SK::TYPE_APP | SK::TYPE_TUPLE | SK::TYPE_VAR)
            })
            .map(|c| (normalised_type_text(&c), param_range(&c)))
            .collect();
        if annotated.len() != expected.len() {
            continue;
        }
        for ((actual, range), want) in annotated.iter().zip(expected) {
            if actual != want && !actual.is_empty() && !want.is_empty() {
                // Two bitvectors of different width are a *constraint*
                // upstream, not a shape mismatch: the widths have to be
                // proved equal and cannot be. "Failed to prove constraint:
                // 32 == 64, as it simplifies directly to false"
                // (`type_error.ml:403`), expected first, at the whole
                // parameter — name included, because the parameter is what
                // disagrees with the declaration.
                let message = match (bitvector_width(actual), bitvector_width(want)) {
                    (Some(a), Some(w)) => format!(
                        "Failed to prove constraint: {w} == {a}, as it simplifies directly to \
                         false"
                    ),
                    _ => format!(
                        "Parameter type {actual} does not match {want} from the val declaration"
                    ),
                };
                diagnostics.push(module_diagnostic("signature-mismatch", message, *range));
            }
        }
    }
}

/// A type node's text with trivia removed, for structural comparison.
/// The width a `bits(N)` / `bitvector(N)` names, if it is a literal.
///
/// `bits` and `bitvector` are the same constructor — upstream's own
/// `issue853_2.sail` writes one on each side and still reports the two
/// widths against each other.
fn bitvector_width(text: &str) -> Option<i64> {
    let rest = text.strip_prefix("bits(").or_else(|| text.strip_prefix("bitvector("))?;
    rest.strip_suffix(')')?.split(',').next()?.trim().parse().ok()
}

/// A parameter's whole span: its name as well as its type.
///
/// `function foo(addr : bitvector(64))` carets `addr : bitvector(64)`
/// upstream — the parameter is what disagrees with the declaration, and
/// the type alone does not say which one.
fn param_range(ty: &syntax::SyntaxNode) -> syntax::TextRange {
    let mut start = ty.text_range().start();
    let mut prev = ty.prev_sibling_or_token();
    while let Some(el) = prev {
        if !el.kind().is_trivia() {
            if matches!(el.kind(), parser::SyntaxKind::IDENT | parser::SyntaxKind::COLON) {
                start = el.text_range().start();
            } else {
                break;
            }
        }
        prev = el.prev_sibling_or_token();
    }
    syntax::TextRange::new(start, ty.text_range().end())
}

fn normalised_type_text(node: &syntax::SyntaxNode) -> String {
    node.descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| !t.kind().is_trivia())
        .map(|t| t.text().to_string())
        .collect()
}

/// A `Type`-kinded struct parameter that no field mentions can never be
/// instantiated from a struct literal.
fn check_phantom_type_params(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs.iter().filter(|d| d.kind() == SK::NAMED_DEF) {
        let is_struct = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE)
            .is_some_and(|t| t.kind() == SK::KW_STRUCT);
        if !is_struct {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };

        // Parameters explicitly kinded `: Type`.
        let params: Vec<(String, syntax::TextRange)> = def
            .children()
            .filter(|c| c.kind() == SK::PARAM_LIST)
            .flat_map(|pl| {
                let toks: Vec<_> =
                    pl.descendants_with_tokens().filter_map(|el| el.into_token()).collect();
                let mut out = Vec::new();
                for (i, t) in toks.iter().enumerate() {
                    if t.kind() != SK::TY_VAR {
                        continue;
                    }
                    let kinded_type = toks[i + 1..]
                        .iter()
                        .take_while(|n| !matches!(n.kind(), SK::COMMA | SK::R_PAREN))
                        .any(|n| n.kind() == SK::KW_TYPE_UPPER);
                    if kinded_type {
                        out.push((t.text().to_string(), t.text_range()));
                    }
                }
                out
            })
            .collect();
        if params.is_empty() {
            continue;
        }

        // Upstream reports this "when inferring struct literal" — that is,
        // at a construction site that cannot pin the parameter down. A
        // struct nobody builds never gets there, so a phantom parameter on
        // a declaration alone is not an error.
        let constructed = defs.iter().any(|other| {
            if other.text_range() == def.text_range() {
                return false;
            }
            other
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .any(|t| t.kind() == SK::IDENT && t.text() == name)
        });
        if !constructed {
            continue;
        }

        let body_vars: std::collections::HashSet<String> = member_type_texts(def)
            .iter()
            .flat_map(|t| {
                t.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\''))
                    .filter(|w| w.starts_with('\''))
                    .map(|w| w.to_string())
                    .collect::<Vec<_>>()
            })
            .collect();

        for (param, range) in params {
            if !body_vars.contains(&param) {
                diagnostics.push(module_diagnostic(
                    "phantom-type-parameter",
                    format!(
                        "Failed to instantiate ({param} : Type) when inferring struct literal of type {name}"
                    ),
                    range,
                ));
            }
        }
    }
}

/// A `config` key may not also be a prefix of another key used in the same
/// file: `config a.b` and `config a.b.c` describe the same JSON node as both
/// a value and an object.
fn check_config_key_prefixes(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    let mut keys: Vec<(String, syntax::TextRange)> = Vec::new();
    for node in defs.iter().flat_map(|d| d.descendants()) {
        if node.kind() != SK::CONFIG_EXPR {
            continue;
        }
        // `config a.b.c` parses as field accesses wrapping a `CONFIG_EXPR`
        // that covers only `config a`, so the rest of the path is collected
        // by walking outwards.
        let Some(root) = node
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::IDENT)
        else {
            continue;
        };
        let mut path = root.text().to_string();
        let mut outermost = node.clone();
        let mut cursor = node.parent();
        while let Some(parent) = cursor {
            if parent.kind() != SK::FIELD_ACCESS_EXPR {
                break;
            }
            if let Some(field) = parent
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| t.kind() == SK::IDENT)
                .last()
            {
                path.push('.');
                path.push_str(field.text());
            }
            outermost = parent.clone();
            cursor = parent.parent();
        }
        keys.push((path, outermost.text_range()));
    }

    for (i, (key, range)) in keys.iter().enumerate() {
        for (j, (other, _)) in keys.iter().enumerate() {
            if i == j {
                continue;
            }
            if other.starts_with(&format!("{key}.")) {
                diagnostics.push(module_diagnostic(
                    "config-key-conflict",
                    // `config.ml:448`
                    format!(
                        "Attempting to access key {key}, but various subkeys have already \
                         been used"
                    ),
                    *range,
                ));
                break;
            }
        }
    }
}

/// A `scattered union` clause may not use a type declared after the
/// `scattered union` header.
///
/// Upstream: "Types are not well-formed within this type definition ... this
/// could also be caused by using a type defined after the 'scattered union'
/// declaration" (scattered.ml).
fn check_scattered_union_forward_types(
    defs: &[syntax::SyntaxNode],
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    // Scattered union heads, and the types declared after each one.
    let mut heads: Vec<(String, usize)> = Vec::new();
    let mut type_positions: HashMap<String, usize> = HashMap::new();
    for (i, def) in defs.iter().enumerate() {
        match def.kind() {
            SK::SCATTERED_DEF
                if def
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::KW_UNION) =>
            {
                if let Some((name, _)) = def_name(def) {
                    heads.push((name, i));
                }
            }
            SK::NAMED_DEF | SK::TYPE_ALIAS_DEF => {
                let kw = def
                    .children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| !t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE)
                    .map(|t| t.kind());
                if matches!(
                    kw,
                    Some(SK::KW_ENUM | SK::KW_UNION | SK::KW_STRUCT | SK::KW_TYPE | SK::KW_NEWTYPE)
                ) {
                    if let Some((name, _)) = def_name(def) {
                        type_positions.entry(name).or_insert(i);
                    }
                }
            }
            _ => {}
        }
    }
    if heads.is_empty() {
        return;
    }

    for (i, def) in defs.iter().enumerate() {
        if def.kind() != SK::SCATTERED_CLAUSE_DEF {
            continue;
        }
        if !def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_UNION)
        {
            continue;
        }
        let Some((union_name, _)) = def_name(def) else { continue };
        let Some((_, head_pos)) = heads.iter().find(|(n, _)| *n == union_name) else { continue };
        let _ = i;
        // Payload types mentioned by this clause.
        for tok in def.descendants_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() != SK::IDENT {
                continue;
            }
            let Some(&pos) = type_positions.get(tok.text()) else { continue };
            if pos > *head_pos {
                diagnostics.push(
                    module_diagnostic(
                        "scattered-forward-type",
                        TYPE_DEF_NOT_WF.to_string(),
                        def.text_range(),
                    )
                    .with_related(vec![(
                        tok.text_range(),
                        format!(
                            "{} is defined after the 'scattered union' declaration",
                            tok.text()
                        ),
                    )]),
                );
                break;
            }
        }
    }
}

/// A call to an overload group must match at least one member's arity.
fn check_overload_call_arity(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(scope) = scope {
        check_ordered_call_arity(defs, scope, true, diagnostics);
        return;
    }
    use parser::SyntaxKind as SK;

    // Inline modules are checked against the prefix environment below each
    // declaration in check_module_visibility, including visible alternatives.
    if parse_project_pragma(defs).is_some() {
        return;
    }
    let _ = workspace;
    // group -> member names
    let mut groups: HashMap<String, Vec<String>> = HashMap::new();
    for def in defs.iter().filter(|d| is_overload_def(d)) {
        let Some((name, _)) = def_name(def) else { continue };
        groups.insert(
            name,
            overload_member_tokens(def).iter().map(|t| t.text().to_string()).collect(),
        );
    }
    if groups.is_empty() {
        return;
    }

    // Arities of every monomorphic `val`, taken from its parameter list.
    let mut arities: HashMap<String, usize> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
        let Some((name, _)) = def_name(def) else { continue };
        let Some(arrow) = def.children().find(|c| c.kind() == SK::TYPE_ARROW) else { continue };
        if arrow
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::DOUBLE_ARROW)
        {
            continue;
        }
        let Some(lhs) = arrow.children().next() else { continue };
        // `unit -> T` is called `f()`, with an empty argument list. Counting
        // its parameter as one made `PC()` — against `overload PC = {rPC,
        // wPC}` where `rPC : unit -> bits(64)` — have no possible
        // overloading, in two of upstream's `lean` files.
        let n = if lhs.kind() == SK::TYPE_TUPLE {
            lhs.children().count()
        } else if lhs.text().to_string().trim() == "unit" {
            0
        } else {
            1
        };
        arities.insert(name, n);
    }

    for call in defs.iter().flat_map(|d| d.descendants()) {
        if call.kind() != SK::CALL_EXPR {
            continue;
        }
        let Some(callee) = call
            .first_child()
            .and_then(|c| {
                c.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
            })
            .map(|t| t.text().to_string())
        else {
            continue;
        };
        let Some(members) = groups.get(&callee) else { continue };
        // Every member's arity must be known, or we cannot conclude.
        let member_arities: Vec<usize> =
            members.iter().filter_map(|m| arities.get(m).copied()).collect();
        if member_arities.len() != members.len() || member_arities.is_empty() {
            continue;
        }
        let Some(args) = call.children().find(|c| c.kind() == SK::ARG_LIST) else { continue };
        let given = arg_count(&args);
        if !member_arities.contains(&given) {
            diagnostics.push(module_diagnostic(
                "no-possible-overload",
                format!("No possible overloading for {callee}"),
                call.text_range(),
            ));
        }
    }
}

/// A mapping called with the wrong number of arguments.
fn check_mapping_call_arity(
    defs: &[syntax::SyntaxNode],
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if let Some(scope) = scope {
        check_ordered_call_arity(defs, scope, false, diagnostics);
        return;
    }
    use parser::SyntaxKind as SK;

    let mut arities: HashMap<String, usize> = HashMap::new();
    for def in defs {
        if !matches!(def.kind(), SK::CALLABLE_SPEC | SK::CALLABLE_DEF) {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        let Some(arrow) = def.children().find(|c| c.kind() == SK::TYPE_ARROW) else { continue };
        if !arrow
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::DOUBLE_ARROW)
        {
            continue;
        }
        // A mapping is callable in both directions, so either side's arity
        // is legitimate at a call site.
        let sides: Vec<usize> = arrow
            .children()
            .map(|c| if c.kind() == SK::TYPE_TUPLE { c.children().count() } else { 1 })
            .collect();
        let n = sides.iter().copied().max().unwrap_or(1);
        arities.entry(name).or_insert(n);
    }
    if arities.is_empty() {
        return;
    }

    for call in defs.iter().flat_map(|d| d.descendants()) {
        if call.kind() != SK::CALL_EXPR {
            continue;
        }
        let Some(callee) = call
            .first_child()
            .and_then(|c| {
                c.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
            })
            .map(|t| t.text().to_string())
        else {
            continue;
        };
        let Some(&expected) = arities.get(&callee) else { continue };
        let Some(args) = call.children().find(|c| c.kind() == SK::ARG_LIST) else { continue };
        let given = arg_count(&args);
        // A mapping is also callable backwards with one argument, and a
        // tuple may be passed whole, so only an argument count *above* the
        // declared arity is unambiguously wrong.
        if given > expected {
            diagnostics.push(module_diagnostic(
                "mapping-arity-mismatch",
                "Tuple mapping-pattern and tuple type have different length".to_string(),
                call.text_range(),
            ));
        }
    }
}

fn check_ordered_call_arity(
    defs: &[syntax::SyntaxNode],
    scope: &OrderedScope<'_>,
    overload: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;
    for call in defs.iter().flat_map(|d| d.descendants()).filter(|n| n.kind() == SK::CALL_EXPR) {
        let Some(callee) = call.first_child().filter(|n| n.kind() == SK::IDENT_EXPR) else {
            continue;
        };
        let name = callee.text().to_string();
        let name = name.trim();
        let Some(selected) = scope.node(&call, name, Namespace::Callable) else { continue };
        if overload {
            if !is_overload_def(&selected) {
                continue;
            }
        } else if mapping_signature(&selected).is_none() {
            continue;
        }
        let Some(arities) = scope.call_arities(&call, name) else { continue };
        let Some(args) = call.children().find(|n| n.kind() == SK::ARG_LIST) else { continue };
        let given = arg_count(&args);
        if arities.iter().any(|&(min, max)| (min..=max).contains(&given)) {
            continue;
        }
        diagnostics.push(module_diagnostic(
            if overload { "no-possible-overload" } else { "mapping-arity-mismatch" },
            if overload {
                format!("No possible overloading for {name}")
            } else {
                "Tuple mapping-pattern and tuple type have different length".into()
            },
            call.text_range(),
        ));
    }
}

/// Number of arguments in an `ARG_LIST`, counting `()` as zero.
fn arg_count(args: &syntax::SyntaxNode) -> usize {
    use parser::SyntaxKind as SK;
    if args.children_with_tokens().filter_map(|el| el.into_token()).any(|t| t.kind() == SK::UNIT) {
        return 0;
    }
    args.children().count()
}

/// An `outcome`'s type variables must be used at one kind across all
/// outcome declarations.
fn check_outcome_variable_kinds(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // var -> the kind it was first used at ("Int" for a length position,
    // "Type" for an element position of `vector`).
    let mut kinds: HashMap<String, (&'static str, syntax::TextRange)> = HashMap::new();

    /// Where an outcome declares a variable — the `with 'n, 'm` list.
    ///
    /// Upstream reports a kind clash *there*, not at the use inside the
    /// type: `outcome bar : vector('n, 'm) -> unit with 'n, 'm` carets
    /// the `'n` after `with`, and hints "previous use here" at the same
    /// place in the earlier outcome. The declaration is what the two
    /// outcomes have in common; the use is incidental.
    fn declared_at(def: &syntax::SyntaxNode, name: &str) -> Option<syntax::TextRange> {
        let mut seen_with = false;
        for el in def.descendants_with_tokens() {
            let Some(tok) = el.into_token() else { continue };
            if tok.kind() == SK::KW_WITH {
                seen_with = true;
                continue;
            }
            if seen_with && tok.kind() == SK::TY_VAR && tok.text() == name {
                return Some(tok.text_range());
            }
        }
        None
    }

    for def in defs.iter().filter(|d| d.kind() == SK::OUTCOME_DEF) {
        for app in def.descendants().filter(|n| n.kind() == SK::TYPE_APP) {
            let Some(name) = app
                .children()
                .next()
                .and_then(|c| {
                    c.children_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                })
                .map(|t| t.text().to_string())
            else {
                continue;
            };
            if name != "vector" {
                continue;
            }
            for (i, arg) in app.children().skip(1).enumerate() {
                let Some(var) = arg
                    .descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::TY_VAR)
                else {
                    continue;
                };
                let kind = if i == 0 { "Int" } else { "Type" };
                let text = var.text().to_string();
                match kinds.get(&text) {
                    Some((prev, prev_range)) if *prev != kind => {
                        let at = declared_at(def, &text).unwrap_or_else(|| var.text_range());
                        diagnostics.push(
                            module_diagnostic(
                                "outcome-kind-mismatch",
                                format!(
                                    "Outcome variable {text} has kind {kind} here, but previously used with kind {prev}"
                                ),
                                at,
                            )
                            .with_related(vec![(*prev_range, "previous use here".to_string())]),
                        );
                    }
                    None => {
                        let at = declared_at(def, &text).unwrap_or_else(|| var.text_range());
                        kinds.insert(text, (kind, at));
                    }
                    _ => {}
                }
            }
        }
    }
}

/// A name may bind only once within one side of a mapping clause.
fn check_duplicate_mapping_bindings(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    scope: Option<&OrderedScope<'_>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    if workspace.is_none() && scope.is_none() {
        return;
    }
    for def in defs {
        if !matches!(def.kind(), SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF) {
            continue;
        }
        if !def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| t.kind() == SK::KW_MAPPING)
        {
            continue;
        }
        // A mapping clause reaches here as a `MAPPING_CLAUSE` whose sides
        // are patterns; the `BIN_EXPR` shape is what a clause looked like
        // before the grammar had a node for it, and still what a `<->`
        // written outside a mapping body looks like.
        let sides = def.descendants().flat_map(|n| match n.kind() {
            SK::MAPPING_CLAUSE => n
                .children()
                .filter(|c| {
                    matches!(
                        c.kind(),
                        SK::APP_PAT
                            | SK::AS_PAT
                            | SK::BIN_PAT
                            | SK::IDENT_PAT
                            | SK::INDEX_PAT
                            | SK::LIST_PAT
                            | SK::LITERAL_PAT
                            | SK::RANGE_INDEX_PAT
                            | SK::STRUCT_PAT
                            | SK::TUPLE_PAT
                            | SK::TYPED_PAT
                            | SK::TYVAR_PAT
                            | SK::VECTOR_PAT
                            | SK::WILD_PAT
                    )
                })
                .collect::<Vec<_>>(),
            SK::BIN_EXPR
                if n.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::DOUBLE_ARROW) =>
            {
                n.children().collect()
            }
            _ => Vec::new(),
        });
        {
            for side in sides {
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                for node in side.descendants() {
                    if !matches!(node.kind(), SK::IDENT_EXPR | SK::IDENT_PAT) {
                        continue;
                    }
                    // A constructor head is a reference, not a binding, and
                    // the base of a subrange is a *partial* binding —
                    // `imm[20] @ imm[9..0]` mentions `imm` once per slice and
                    // is checked for contiguity instead.
                    if node.parent().is_some_and(|p| {
                        ((p.kind() == SK::CALL_EXPR || p.kind() == SK::APP_PAT)
                            && p.first_child() == Some(node.clone()))
                            || matches!(
                                p.kind(),
                                SK::INDEX_EXPR
                                    | SK::SUBRANGE_EXPR
                                    | SK::FIELD_ACCESS_EXPR
                                    | SK::INDEX_PAT
                                    | SK::RANGE_INDEX_PAT
                            )
                    }) {
                        continue;
                    }
                    let Some(tok) = node
                        .children_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                    else {
                        continue;
                    };
                    if scope.map_or_else(
                        || workspace.is_some_and(|ws| ws.constructor_names.contains(tok.text())),
                        |scope| scope.is_constructor(def, tok.text()),
                    ) {
                        continue;
                    }
                    if !seen.insert(tok.text().to_string()) {
                        diagnostics.push(module_diagnostic(
                            "duplicate-binding",
                            // Upstream has one sentence for this, and it
                            // does not say "mapping": `bind_pat` reaches
                            // `check_duplicates` for every pattern
                            // (`type_check.ml:1650`).
                            format!("Duplicate binding for {} in pattern", tok.text()),
                            tok.text_range(),
                        ));
                    }
                }
            }
        }
    }
}

/// A parameter pattern built from subranges of one name must cover exactly
/// the width its `val` declares.
fn check_subrange_pattern_width(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // name -> declared width of the single `bits(N)` parameter.
    let mut widths: HashMap<String, i64> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
        let Some((name, _)) = def_name(def) else { continue };
        let Some(arrow) = def.children().find(|c| c.kind() == SK::TYPE_ARROW) else { continue };
        let Some(lhs) = arrow.children().next() else { continue };
        if lhs.kind() != SK::TYPE_APP {
            continue;
        }
        let text = normalised_type_text(&lhs);
        if let Some(rest) = text.strip_prefix("bits(").and_then(|r| r.strip_suffix(')')) {
            if let Ok(w) = rest.parse::<i64>() {
                widths.insert(name, w);
            }
        }
    }
    if widths.is_empty() {
        return;
    }

    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_DEF) {
        let Some((name, _)) = def_name(def) else { continue };
        let Some(&declared) = widths.get(&name) else { continue };
        for params in def.children().filter(|c| c.kind() == SK::PARAM_LIST) {
            let subranges = collect_subranges(&params);
            if subranges.len() < 2 {
                continue;
            }
            // All subranges must index the same name for the total to mean
            // anything.
            let first = &subranges[0].0;
            if subranges.iter().any(|(n, _, _, _)| n != first) {
                continue;
            }
            let covered: i64 = {
                let max = subranges.iter().map(|(_, _, h, _)| *h).max().unwrap_or(0);
                if !(0..=4096).contains(&max) {
                    continue;
                }
                let mut bits = vec![false; max as usize + 1];
                for (_, lo, hi, _) in &subranges {
                    for i in (*lo).max(0)..=*hi {
                        bits[i as usize] = true;
                    }
                }
                bits.iter().filter(|b| **b).count() as i64
            };
            if covered != declared {
                diagnostics.push(module_diagnostic(
                    "pattern-width-mismatch",
                    // Two lines upstream, and the first is the one that
                    // says what is wrong (`type_check.ml:3140`, typo
                    // included).
                    format!(
                        "Vector concatentation pattern does not have the correct width.\n\
                         Expected width {declared}, found {covered}."
                    ),
                    // The pattern, not the parentheses around it: upstream
                    // carets `v[2..0] @ v[3]` inside `function f(..)`.
                    inner_range(&params),
                ));
            }
        }
    }
}

/// A vector-concatenation pattern cannot claim more bits than the value it
/// matches has.
///
/// `match xs { _ : bits(33) @ _ => () }` where `xs : bits(32)` leaves the
/// trailing subpattern a negative width; upstream reports "Vector
/// concatenation subpattern cannot have a negative width".
/// A parameter list's contents, without its brackets.
fn inner_range(params: &syntax::SyntaxNode) -> syntax::TextRange {
    use parser::SyntaxKind as SK;
    let toks: Vec<_> = params.children_with_tokens().filter(|el| !el.kind().is_trivia()).collect();
    let start = match toks.first() {
        Some(el) if el.kind() == SK::L_PAREN => el.text_range().end(),
        _ => params.text_range().start(),
    };
    let end = match toks.last() {
        Some(el) if el.kind() == SK::R_PAREN => el.text_range().start(),
        _ => params.text_range().end(),
    };
    if start <= end {
        syntax::TextRange::new(start, end)
    } else {
        params.text_range()
    }
}

fn check_concat_pattern_widths(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // Declared `bits(N)` types of each function's parameters, by parameter
    // name, from the `val` or an inline annotation.
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_DEF) {
        let Some((fname, _)) = def_name(def) else { continue };
        let mut param_widths: HashMap<String, i64> = HashMap::new();

        // Inline `(xs : bits(64))`.
        for params in def.children().filter(|c| c.kind() == SK::PARAM_LIST) {
            let toks: Vec<_> = params
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| !t.kind().is_trivia())
                .collect();
            for (i, t) in toks.iter().enumerate() {
                if t.kind() != SK::IDENT || toks.get(i + 1).map(|n| n.kind()) != Some(SK::COLON) {
                    continue;
                }
                if toks.get(i + 2).map(|n| n.text()) != Some("bits") {
                    continue;
                }
                if let Some(w) = toks.get(i + 4).and_then(|n| n.text().parse::<i64>().ok()) {
                    param_widths.insert(t.text().to_string(), w);
                }
            }
        }
        // A single bare parameter takes its type from the `val`.
        if param_widths.is_empty() {
            let bare = def
                .children()
                .filter(|c| c.kind() == SK::PARAM_LIST)
                .flat_map(|pl| {
                    pl.children_with_tokens()
                        .filter_map(|el| el.into_token())
                        .filter(|t| t.kind() == SK::IDENT)
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            if bare.len() == 1 {
                if let Some(w) = val_single_bits_width(defs, &fname) {
                    param_widths.insert(bare[0].text().to_string(), w);
                }
            }
        }
        if param_widths.is_empty() {
            continue;
        }

        for m in def.descendants().filter(|n| n.kind() == SK::MATCH_EXPR) {
            let Some(scrutinee) = m
                .children()
                .next()
                .and_then(|c| {
                    c.children_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                })
                .map(|t| t.text().to_string())
            else {
                continue;
            };
            let Some(&width) = param_widths.get(&scrutinee) else { continue };

            for arm in m.children().filter(|c| c.kind() == SK::MATCH_ARM) {
                // Annotated widths in a `@` chain, plus whether any element
                // is unannotated (and so takes the remainder).
                let toks: Vec<_> = arm
                    .descendants_with_tokens()
                    .filter_map(|el| el.into_token())
                    .filter(|t| !t.kind().is_trivia())
                    .take_while(|t| t.kind() != SK::FAT_R_ARROW)
                    .collect();
                if !toks.iter().any(|t| t.kind() == SK::AT) {
                    continue;
                }
                let mut annotated = 0i64;
                let mut has_unannotated = false;
                let mut elements = 0usize;
                // Where each element sits, so the finding can be reported
                // at the one that comes out negative rather than at the
                // whole pattern — `_ : bits(33) @ _` carets the second
                // `_`, and `_ : bits(33) @ _ : bits(-1)` the whole second
                // element.
                let mut unannotated_at: Option<syntax::TextRange> = None;
                let mut last_at: Option<syntax::TextRange> = None;
                let mut i = 0;
                while i < toks.len() {
                    if toks[i].kind() == SK::AT {
                        i += 1;
                        continue;
                    }
                    elements += 1;
                    // `_ : bits(N)` or `x : bits(N)`
                    let annot = toks
                        .get(i + 1)
                        .filter(|t| t.kind() == SK::COLON)
                        .and(toks.get(i + 2).filter(|t| t.text() == "bits"))
                        .and_then(|_| toks.get(i + 4))
                        .and_then(|t| t.text().parse::<i64>().ok());
                    let span = |from: usize, to: usize| -> syntax::TextRange {
                        let start = toks[from].text_range().start();
                        let end = toks.get(to).unwrap_or(&toks[from]).text_range().end();
                        syntax::TextRange::new(start, end)
                    };
                    match annot {
                        Some(w) => {
                            annotated += w;
                            last_at = Some(span(i, (i + 5).min(toks.len() - 1)));
                            i += 6;
                        }
                        None => {
                            has_unannotated = true;
                            // An element with no *parsable* width: a bare
                            // `_`, or `bits(-1)` whose minus sign is a
                            // token of its own. Either way this is the one
                            // upstream points at.
                            let end = if toks.get(i + 1).map(|t| t.kind()) == Some(SK::COLON) {
                                toks.iter()
                                    .enumerate()
                                    .skip(i)
                                    .take_while(|(_, t)| t.kind() != SK::AT)
                                    .map(|(n, _)| n)
                                    .last()
                                    .unwrap_or(i)
                            } else {
                                i
                            };
                            let at = span(i, end);
                            unannotated_at.get_or_insert(at);
                            last_at = Some(at);
                            i = end + 1;
                        }
                    }
                }
                if elements < 2 {
                    continue;
                }
                if annotated > width || (!has_unannotated && annotated != width) {
                    let at = unannotated_at.or(last_at).unwrap_or_else(|| arm.text_range());
                    diagnostics.push(module_diagnostic(
                        "negative-concat-width",
                        format!(
                            "Vector concatenation subpattern cannot have a negative width ({})",
                            width - annotated
                        ),
                        at,
                    ));
                }
            }
        }
    }
}

/// The width of a function's single `bits(N)` parameter, from its `val`.
fn val_single_bits_width(defs: &[syntax::SyntaxNode], name: &str) -> Option<i64> {
    use parser::SyntaxKind as SK;
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
        if def_name(def).map(|(n, _)| n).as_deref() != Some(name) {
            continue;
        }
        let arrow = def.children().find(|c| c.kind() == SK::TYPE_ARROW)?;
        let lhs = arrow.children().next()?;
        let text = normalised_type_text(&lhs);
        return text.strip_prefix("bits(")?.strip_suffix(')')?.parse().ok();
    }
    None
}

/// Report an `$include_error` a file's includes reach.
///
/// `lib/vector.sail` uses this to require a `default Order` before it is
/// included, and the message belongs at the `$include` that pulls it in
/// (preprocess.ml:186-199). Only a file whose own `$include` names something
/// in the responsible set is reported — `$include <real.sail>` does not reach
/// `vector.sail` and must stay silent.
fn check_include_error(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    let Some((message, responsible)) = workspace.and_then(|ws| ws.include_error.as_ref()) else {
        return;
    };
    for def in defs.iter().filter(|d| d.kind() == SK::DIRECTIVE_DEF) {
        let text = def.text().to_string();
        let text = text.trim();
        let Some(arg) = text.strip_prefix("$include") else { continue };
        if text.starts_with("$include_error") {
            continue;
        }
        let name = arg
            .trim()
            .trim_matches(|c| c == '<' || c == '>' || c == '"')
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string();
        if responsible.contains(&name) {
            diagnostics.push(module_diagnostic("include-error", message.clone(), def.text_range()));
            return;
        }
    }
}

// NOTE: "could not infer constant length for vector concatenation
// subpattern" is not reported. Whether `_ : bits('n) @ _ : bits(64 - 'n)` is
// inferable depends on what the solver knows about `'n` — the same pattern is
// rejected in fail/vector_pattern_split.sail and accepted in the pass/ file of
// the same name, which differs only by an `assert('n == 32)`. A syntactic
// check cannot separate them.

/// `sizeof(T)` for a constant `type T : Int = N` is `N`, and must satisfy the
/// parameter's declared `int(M)`.
fn check_sizeof_constant_args(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // `type xlen : Int = 64`
    let mut int_synonyms: HashMap<String, i64> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::TYPE_ALIAS_DEF) {
        let Some((name, _)) = def_name(def) else { continue };
        let toks: Vec<_> = def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        if !toks.iter().any(|t| matches!(t.kind(), SK::KW_INT | SK::KW_NAT)) {
            continue;
        }
        let body: String = toks
            .iter()
            .skip_while(|t| t.kind() != SK::EQ)
            .skip(1)
            .map(|t| t.text().to_string())
            .collect();
        if let Some(v) = eval_const_int(&body) {
            int_synonyms.insert(name, v);
        }
    }
    if int_synonyms.is_empty() {
        return;
    }

    // `val foo : int(32) -> unit` — the expected value of each parameter.
    let mut expected: HashMap<String, Vec<Option<i64>>> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::CALLABLE_SPEC) {
        let Some((name, _)) = def_name(def) else { continue };
        let Some(arrow) = def.children().find(|c| c.kind() == SK::TYPE_ARROW) else { continue };
        let Some(lhs) = arrow.children().next() else { continue };
        let params: Vec<syntax::SyntaxNode> =
            if lhs.kind() == SK::TYPE_TUPLE { lhs.children().collect() } else { vec![lhs] };
        expected.insert(
            name,
            params
                .iter()
                .map(|p| {
                    let text = normalised_type_text(p);
                    text.strip_prefix("int(")
                        .or_else(|| text.strip_prefix("atom("))
                        .and_then(|r| r.strip_suffix(')'))
                        .and_then(eval_const_int)
                })
                .collect(),
        );
    }

    for call in defs.iter().flat_map(|d| d.descendants()) {
        if call.kind() != SK::CALL_EXPR {
            continue;
        }
        let Some(callee) = call
            .first_child()
            .and_then(|c| {
                c.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .find(|t| t.kind() == SK::IDENT)
            })
            .map(|t| t.text().to_string())
        else {
            continue;
        };
        let Some(want) = expected.get(&callee) else { continue };
        let Some(args) = call.children().find(|c| c.kind() == SK::ARG_LIST) else { continue };
        for (i, arg) in args.children().enumerate() {
            let Some(Some(want)) = want.get(i) else { continue };
            // `sizeof(T)` with `T` a constant Int synonym.
            if arg.kind() != SK::SIZEOF_EXPR {
                continue;
            }
            let Some(inner) = arg
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| t.kind() == SK::IDENT)
                .last()
            else {
                continue;
            };
            let Some(&actual) = int_synonyms.get(inner.text()) else { continue };
            if actual != *want {
                // `Err_subtype` renders as two lines — the subtype
                // sentence, then "as {nc} could not be proven"
                // (`type_error.ml:481-484`). The second was all this said,
                // and on its own it does not name the types that failed to
                // relate.
                diagnostics.push(module_diagnostic(
                    "unsolved-constraint",
                    format!(
                        "int({actual}) is not a subtype of int({want})\n\
                         as {actual} == {want} could not be proven"
                    ),
                    arg.text_range(),
                ));
            }
        }
    }
}

/// A struct literal field initialised from a call must match the field's
/// declared width, when both are constant-width bitvectors.
fn check_struct_field_widths(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    // `type xlenbits = bits(32)` — alias name to constant width.
    let mut widths: HashMap<String, i64> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::TYPE_ALIAS_DEF) {
        let Some((name, _)) = def_name(def) else { continue };
        let body = alias_body_text(def);
        if let Some(w) =
            body.strip_prefix("bits(").and_then(|r| r.strip_suffix(')')).and_then(eval_const_int)
        {
            widths.insert(name, w);
        }
    }
    let resolve = |text: &str| -> Option<i64> {
        text.strip_prefix("bits(")
            .and_then(|r| r.strip_suffix(')'))
            .and_then(eval_const_int)
            .or_else(|| widths.get(text).copied())
    };

    // Field name -> declared type, when the name belongs to exactly one
    // struct. A field shared by two structs is ambiguous without inference.
    let mut field_types: HashMap<String, Option<String>> = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::NAMED_DEF) {
        let is_struct = def
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| !t.kind().is_trivia() && t.kind() != SK::KW_PRIVATE)
            .is_some_and(|t| t.kind() == SK::KW_STRUCT);
        if !is_struct {
            continue;
        }
        for (name, ty) in struct_field_names(def).into_iter().zip(member_type_texts(def)) {
            match field_types.entry(name) {
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    if e.get().as_deref() != Some(ty.as_str()) {
                        e.insert(None);
                    }
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(Some(ty));
                }
            }
        }
    }
    if field_types.is_empty() {
        return;
    }

    // Function return types, from a `val` or an inline `-> T`.
    let mut returns: HashMap<String, String> = HashMap::new();
    for def in defs {
        if !matches!(def.kind(), SK::CALLABLE_SPEC | SK::CALLABLE_DEF) {
            continue;
        }
        if def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .any(|t| matches!(t.kind(), SK::KW_FORALL | SK::TY_VAR))
        {
            continue;
        }
        let Some((name, _)) = def_name(def) else { continue };
        let ret = def
            .children()
            .find(|c| c.kind() == SK::TYPE_ARROW)
            .and_then(|a| a.children().nth(1))
            .or_else(|| {
                // `function f(..) -> T = ..`
                def.children_with_tokens()
                    .skip_while(|el| el.as_token().is_none_or(|t| t.kind() != SK::R_ARROW))
                    .filter_map(|el| el.into_node())
                    .next()
            });
        if let Some(ret) = ret {
            returns.entry(name).or_insert_with(|| normalised_type_text(&ret));
        }
    }

    for lit in defs.iter().flat_map(|d| d.descendants()) {
        if lit.kind() != SK::STRUCT_EXPR {
            continue;
        }
        for field in lit.children().filter(|c| c.kind() == SK::FIELD_INIT) {
            let Some(fname) = field
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)
                .map(|t| t.text().to_string())
            else {
                continue;
            };
            let Some(Some(declared)) = field_types.get(&fname) else { continue };
            let Some(want) = resolve(declared) else { continue };
            let Some(value) = field.children().find(|c| c.kind() == SK::CALL_EXPR) else {
                continue;
            };
            let Some(callee) = value
                .first_child()
                .and_then(|c| {
                    c.children_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                })
                .map(|t| t.text().to_string())
            else {
                continue;
            };
            let Some(ret) = returns.get(&callee) else { continue };
            let Some(actual) = resolve(ret) else { continue };
            if actual != want {
                // Two widths that must be equal and are not: a
                // constraint, in upstream's words (`type_error.ml:403`) —
                // the same sentence a parameter that disagrees with its
                // `val` gets.
                //
                // The order is the constraint's, not a convention: here
                // the value's width comes first (`34 == 32`) and in the
                // parameter case the declaration's does (`32 == 64`),
                // because `subtyp` is called with the sides the other way
                // round. Taken from the two `.expect` files rather than
                // reasoned out.
                let _ = &fname;
                diagnostics.push(module_diagnostic(
                    "type-error",
                    format!(
                        "Failed to prove constraint: {actual} == {want}, as it simplifies \
                         directly to false"
                    ),
                    value.text_range(),
                ));
            }
        }
    }
}

/// At most one element of a vector-concatenation pattern may have a width the
/// checker cannot pin to a constant.
///
/// `_ : bits('n) @ _ : bits(64 - 'n)` is rejected on its own, but accepted
/// once an `assert('n == 32)` in the same body pins `'n` — which is the only
/// difference between the `pass/` and `fail/` copies of
/// `vector_pattern_split.sail`.
fn check_concat_pattern_inference(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        // Type variables pinned to a constant by an `assert('v == N)`, or
        // bound directly to a literal.
        let mut pinned: std::collections::HashSet<String> = std::collections::HashSet::new();
        for node in def.descendants() {
            if node.kind() != SK::ASSERT_EXPR {
                continue;
            }
            let toks: Vec<_> = node
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| !t.kind().is_trivia())
                .collect();
            for (i, t) in toks.iter().enumerate() {
                if t.kind() != SK::TY_VAR {
                    continue;
                }
                if toks.get(i + 1).map(|n| n.kind()) == Some(SK::EQ_EQ)
                    && toks.get(i + 2).map(|n| n.kind()) == Some(SK::NUM_LIT)
                {
                    pinned.insert(t.text().to_string());
                }
            }
        }

        for arm in def.descendants().filter(|n| n.kind() == SK::MATCH_ARM) {
            let toks: Vec<_> = arm
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| !t.kind().is_trivia())
                .take_while(|t| t.kind() != SK::FAT_R_ARROW)
                .collect();
            if !toks.iter().any(|t| t.kind() == SK::AT) {
                continue;
            }
            let mut non_constant = 0;
            for (i, t) in toks.iter().enumerate() {
                if t.text() != "bits" || toks.get(i + 1).map(|n| n.kind()) != Some(SK::L_PAREN) {
                    continue;
                }
                let mut j = i + 2;
                let mut constant = true;
                while j < toks.len() && toks[j].kind() != SK::R_PAREN {
                    match toks[j].kind() {
                        SK::NUM_LIT => {}
                        SK::TY_VAR if pinned.contains(toks[j].text()) => {}
                        _ => constant = false,
                    }
                    j += 1;
                }
                if !constant {
                    non_constant += 1;
                }
            }
            if non_constant >= 2 {
                diagnostics.push(module_diagnostic(
                    "uninferable-concat-width",
                    "Could not infer constant length for vector concatenation subpattern"
                        .to_string(),
                    arm.text_range(),
                ));
            }
        }
    }
}

/// Check global constraints after integer abstract types are configured.
fn check_configured_constraints(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;
    let Some(ws) = workspace else { return };
    if ws.config_values.is_empty() {
        return;
    }
    let mut values = HashMap::new();
    for def in defs.iter().filter(|d| d.kind() == SK::TYPE_ALIAS_DEF) {
        let Some((name, _)) = def_name(def) else { continue };
        let tokens: Vec<_> = def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        let Some(config) = tokens.iter().position(|t| t.kind() == SK::KW_CONFIG) else { continue };
        let key: String = tokens[config + 1..].iter().map(|t| t.text()).collect();
        if let Some(value) = ws.config_values.get(&key) {
            values.insert(name, value.clone());
        }
    }
    if values.is_empty() {
        return;
    }
    for def in defs.iter().filter(|d| d.kind() == SK::CONSTRAINT_DEF) {
        // Preserve whitespace: concatenating tokens changes `x in {32,64}`
        // into an identifier named `xin`.
        let text = def.text().to_string();
        let Some(body) = text.trim().strip_prefix("constraint") else { continue };
        if hir::infer::configured_constraint_status(body, &values)
            == hir::infer::ConstraintStatus::Failed
        {
            let range = def.children().next().map_or(def.text_range(), |n| n.text_range());
            diagnostics.push(module_diagnostic(
                "config-constraint-violated",
                format!("Constraint {} does not hold after instantiating abstract types from configuration", body.trim()),
                range,
            ));
        }
    }
}

/// A `config` key must hold a value of the annotated type.
/// A bitvector is written in the configuration as an array of booleans, so
/// `let _ : bits(5) = config k` fails when `k` holds a string.
fn check_config_value_types(
    defs: &[syntax::SyntaxNode],
    workspace: Option<&crate::WorkspaceNames>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    use parser::SyntaxKind as SK;

    let Some(ws) = workspace else { return };
    if ws.config_values.is_empty() {
        return;
    }

    for node in defs.iter().flat_map(|d| d.descendants()) {
        if node.kind() != SK::CONFIG_EXPR {
            continue;
        }
        // Full dotted key, walking outwards through field accesses.
        let Some(root) = node
            .children_with_tokens()
            .filter_map(|el| el.into_token())
            .find(|t| t.kind() == SK::IDENT)
        else {
            continue;
        };
        let mut key = root.text().to_string();
        let mut outermost = node.clone();
        let mut cursor = node.parent();
        while let Some(parent) = cursor {
            if parent.kind() != SK::FIELD_ACCESS_EXPR {
                break;
            }
            if let Some(field) = parent
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| t.kind() == SK::IDENT)
                .last()
            {
                key.push('.');
                key.push_str(field.text());
            }
            outermost = parent.clone();
            cursor = parent.parent();
        }
        let Some(value) = ws.config_values.get(&key) else { continue };

        // The annotated type: `config k : T`, or the enclosing binding's.
        // The annotation may be nested in a typed pattern, so search the
        // enclosing binding's subtree — but not the config expression itself.
        // Stop at the definition: beyond it every type in the file is in
        // scope of the search, and `function foo() = config foo` picked up
        // the `bits(xlen)` of an unrelated `register` declaration.
        let expected = outermost
            .ancestors()
            .take(4)
            .take_while(|a| {
                !matches!(
                    a.kind(),
                    SK::SOURCE_FILE | SK::DEFINITION | SK::CALLABLE_DEF | SK::SCATTERED_CLAUSE_DEF
                )
            })
            .find_map(|a| {
                a.descendants()
                    .filter(|c| !outermost.text_range().contains_range(c.text_range()))
                    .find(|c| matches!(c.kind(), SK::TYPE_NAMED | SK::TYPE_APP))
                    .map(|c| normalised_type_text(&c))
            });
        let Some(expected) = expected else { continue };

        if expected.starts_with("bits(") || expected.starts_with("bitvector(") {
            // Every element of the array must be a boolean bit.
            if let Some(inner) = value.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                if let Some(bad) =
                    inner.split(',').map(str::trim).find(|item| *item != "true" && *item != "false")
                {
                    diagnostics.push(module_diagnostic(
                        "config-type-error",
                        format!("Failed to interpret {bad} as a bit"),
                        outermost.text_range(),
                    ));
                }
            } else {
                diagnostics.push(module_diagnostic(
                    "config-type-error",
                    format!("Failed to interpret {value} as a bitvector"),
                    outermost.text_range(),
                ));
            }
        }
    }
}

/// `_prove(constraint('a == 'b))` over two locally bound type variables that
/// nothing relates cannot be discharged.
///
/// The `pass/` and `fail/` copies of the shadowed-binding test differ only in
/// that the passing one annotates the second binding `let 'y : int('x)`,
/// which is what ties the two variables together.
fn check_unprovable_local_equality(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    for def in defs {
        // Local `let 'v ...` bindings, and the type variables their
        // annotation mentions.
        let mut related: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for binding in def.descendants() {
            if !matches!(binding.kind(), SK::LET_EXPR | SK::VAR_EXPR) {
                continue;
            }
            // The bound variable may sit inside a pattern node, so scan
            // descendants up to the `=`.
            let toks: Vec<_> = binding
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| !t.kind().is_trivia())
                .take_while(|t| t.kind() != SK::EQ)
                .collect();
            let Some(name) = toks.iter().find(|t| t.kind() == SK::TY_VAR) else { continue };
            let others_in_annotation: std::collections::HashSet<String> = toks
                .iter()
                .filter(|t| t.kind() == SK::TY_VAR && t.text() != name.text())
                .map(|t| t.text().to_string())
                .collect();
            related.entry(name.text().to_string()).or_default().extend(others_in_annotation);
        }
        if related.len() < 2 {
            continue;
        }

        for call in def.descendants().filter(|n| n.kind() == SK::CALL_EXPR) {
            let is_prove = call
                .first_child()
                .and_then(|c| {
                    c.children_with_tokens()
                        .filter_map(|el| el.into_token())
                        .find(|t| t.kind() == SK::IDENT)
                })
                .is_some_and(|t| t.text() == "_prove");
            if !is_prove {
                continue;
            }
            let toks: Vec<_> = call
                .descendants_with_tokens()
                .filter_map(|el| el.into_token())
                .filter(|t| !t.kind().is_trivia())
                .collect();
            // Exactly `'a == 'b`, with no other operators in the constraint.
            let vars: Vec<&syntax::SyntaxToken> =
                toks.iter().filter(|t| t.kind() == SK::TY_VAR).collect();
            let eqs = toks.iter().filter(|t| t.kind() == SK::EQ_EQ).count();
            let ops = toks
                .iter()
                .filter(|t| {
                    matches!(t.kind(), SK::PLUS | SK::MINUS | SK::STAR | SK::AMP | SK::PIPE)
                })
                .count();
            if vars.len() != 2 || eqs != 1 || ops != 0 {
                continue;
            }
            let (a, b) = (vars[0].text().to_string(), vars[1].text().to_string());
            if a == b {
                continue;
            }
            let (Some(ra), Some(rb)) = (related.get(&a), related.get(&b)) else { continue };
            if ra.contains(&b) || rb.contains(&a) {
                continue;
            }
            diagnostics.push(module_diagnostic(
                "unprovable-constraint",
                format!("Cannot prove {a} == {b}"),
                call.text_range(),
            ));
        }
    }
}

/// Detect duplicate enum/union member names within a single definition.
///
/// `check_duplicate_definitions` catches duplicate top-level type names but
/// NOT duplicate variants within one enum/union. This fills that gap.
///
/// Walks CST NAMED_DEF nodes for enums/unions, collects member IDENTs inside
/// braces, and flags duplicates.
fn check_duplicate_enum_members(cst_root: &syntax::SyntaxNode, diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;
    use std::collections::HashSet;

    for node in cst_root.descendants() {
        if node.kind() != SK::NAMED_DEF {
            continue;
        }
        let first_kw = node.children_with_tokens().find_map(|c| c.into_token().map(|t| t.kind()));
        let is_enum = first_kw == Some(SK::KW_ENUM);
        let is_union = first_kw == Some(SK::KW_UNION);
        if !is_enum && !is_union {
            continue;
        }

        let mut seen = HashSet::new();

        if is_enum {
            // Enum: all IDENTs inside braces are member names.
            let mut in_braces = false;
            for tok in node.descendants_with_tokens().filter_map(|el| el.into_token()) {
                match tok.kind() {
                    SK::L_CURLY => in_braces = true,
                    SK::R_CURLY => in_braces = false,
                    SK::IDENT if in_braces => {
                        let name = tok.text().to_string();
                        if !seen.insert(name.clone()) {
                            emit_dup_member(diagnostics, &name, &tok);
                        }
                    }
                    _ => {}
                }
            }
        } else {
            // Union: syntax is `MemberName : Type`, so only the IDENT
            // in FIELD_INIT *before* `:` is the member name.
            for field in node.descendants() {
                if field.kind() != SK::FIELD_INIT {
                    continue;
                }
                // First IDENT token before COLON is the member name.
                for tok in field.descendants_with_tokens().filter_map(|el| el.into_token()) {
                    if tok.kind() == SK::COLON {
                        break;
                    }
                    if tok.kind() == SK::IDENT {
                        let name = tok.text().to_string();
                        if !seen.insert(name.clone()) {
                            emit_dup_member(diagnostics, &name, &tok);
                        }
                        break;
                    }
                }
            }
        }
    }
}

fn emit_dup_member(diagnostics: &mut Vec<Diagnostic>, name: &str, tok: &syntax::SyntaxToken) {
    let offset = tok.text_range().start();
    let end = tok.text_range().end();
    diagnostics.push(Diagnostic::new(
        DiagnosticCode::SailError("duplicate-enum-member"),
        // `initial_check.ml:1855`. Upstream says "Enumeration member"
        // whether the definition is an enum or a union — the check runs on
        // the enum path and the union's duplicate constructors reach the
        // same words.
        format!("Enumeration member '{name}' occurs twice in enum declaration"),
        base_db::text_range(u32::from(offset) as usize, u32::from(end) as usize),
        Severity::Error,
    ));
}

// check_unused_variables removed — duplicated by
// InferenceDiagnostic::UnusedVariable in per-callable inference pipeline.

// check_scattered_completeness_single_file removed — single-file
// scattered check is almost always false positive.
// Workspace-level check_workspace_scattered_completeness() remains.

/// `enum E with as_bits -> bits(16), as_string -> string = { A => (..), .. }`.
///
/// Upstream's `Enum id With enum_functions Eq Lcurly enum Rcurly`
/// (`parser.mly:1059`) declares one mapping per function, and each member's
/// value has to have the declared type. The header and the body are flat
/// tokens here and nothing read either, so a member could give a string
/// where a `bool` was declared, and `E_as_bits` was built from nothing.
///
/// Judged by *kind*, and only for a literal value: `sail_zeros(16)` is a
/// call and says nothing. A kind is all this needs — the mismatch upstream
/// reports is between a string and a boolean, not between two widths.
fn check_enum_mappings(defs: &[syntax::SyntaxNode], diagnostics: &mut Vec<Diagnostic>) {
    use parser::SyntaxKind as SK;

    /// bits / string / bool / numeric, for a declared type or a literal.
    fn kind_of_type(text: &str) -> Option<u8> {
        let t = text.trim();
        if t.starts_with("bits(") || t.starts_with("bitvector(") || t == "bit" {
            return Some(0);
        }
        match t {
            "string" => Some(1),
            "bool" => Some(2),
            "int" | "nat" => Some(3),
            _ if t.starts_with("range(") || t.starts_with("atom(") => Some(3),
            _ => None,
        }
    }

    for def in defs {
        if def.kind() != SK::NAMED_DEF {
            continue;
        }
        let toks: Vec<_> = def
            .descendants_with_tokens()
            .filter_map(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        if toks.first().map(|t| t.kind()) != Some(SK::KW_ENUM) {
            continue;
        }
        let Some(with_at) = toks.iter().position(|t| t.kind() == SK::KW_WITH) else { continue };
        let Some(eq_at) = toks.iter().position(|t| t.kind() == SK::EQ) else { continue };
        if eq_at <= with_at {
            continue;
        }
        // `name -> type` groups, comma separated, between `with` and `=`.
        let mut declared: Vec<String> = Vec::new();
        let mut group: Vec<&syntax::SyntaxToken> = Vec::new();
        let mut depth = 0i32;
        for t in &toks[with_at + 1..eq_at] {
            match t.kind() {
                SK::L_PAREN => depth += 1,
                SK::R_PAREN => depth -= 1,
                SK::COMMA if depth == 0 => {
                    declared.push(type_after_arrow(&group));
                    group.clear();
                    continue;
                }
                _ => {}
            }
            group.push(t);
        }
        if !group.is_empty() {
            declared.push(type_after_arrow(&group));
        }
        if declared.is_empty() || declared.iter().any(|d| d.is_empty()) {
            continue;
        }

        // The body: `NAME => value` per member, separated by top-level commas.
        let mut i = eq_at + 1;
        while i < toks.len() {
            let Some(arrow) = toks[i..].iter().position(|t| t.kind() == SK::FAT_R_ARROW) else {
                break;
            };
            let start = i + arrow + 1;
            // The member's value runs to the next top-level comma.
            let mut depth = 0i32;
            let mut end = start;
            while end < toks.len() {
                match toks[end].kind() {
                    SK::L_PAREN | SK::L_CURLY | SK::L_BRACK => depth += 1,
                    SK::R_PAREN | SK::R_BRACK => depth -= 1,
                    SK::R_CURLY => {
                        depth -= 1;
                        if depth < 0 {
                            break;
                        }
                    }
                    SK::COMMA if depth == 0 => break,
                    _ => {}
                }
                end += 1;
            }
            check_enum_member_value(&toks[start..end], &declared, diagnostics, &kind_of_type);
            i = end + 1;
        }
    }
}

/// The type text on the right of `->` in one `name -> type` group.
fn type_after_arrow(group: &[&syntax::SyntaxToken]) -> String {
    use parser::SyntaxKind as SK;
    let Some(at) = group.iter().position(|t| t.kind() == SK::R_ARROW) else {
        return String::new();
    };
    group[at + 1..].iter().map(|t| t.text()).collect::<String>()
}

/// One member's value against the declared mapping types.
fn check_enum_member_value(
    value: &[syntax::SyntaxToken],
    declared: &[String],
    diagnostics: &mut Vec<Diagnostic>,
    kind_of_type: &dyn Fn(&str) -> Option<u8>,
) {
    use parser::SyntaxKind as SK;
    let kind_of_literal = |t: &syntax::SyntaxToken| match t.kind() {
        SK::HEX_LIT | SK::BIN_LIT => Some(0),
        SK::STRING_LIT => Some(1),
        SK::KW_TRUE | SK::KW_FALSE => Some(2),
        SK::NUM_LIT => Some(3),
        _ => None,
    };
    // One function: the value is one expression. Several: a parenthesised
    // tuple, and each element lines up with its function.
    let parts: Vec<&[syntax::SyntaxToken]> = if declared.len() == 1 {
        vec![value]
    } else {
        if value.len() < 2
            || value[0].kind() != SK::L_PAREN
            || value[value.len() - 1].kind() != SK::R_PAREN
        {
            return;
        }
        let inner = &value[1..value.len() - 1];
        let mut out: Vec<&[syntax::SyntaxToken]> = Vec::new();
        let mut depth = 0i32;
        let mut start = 0usize;
        for (i, t) in inner.iter().enumerate() {
            match t.kind() {
                SK::L_PAREN | SK::L_BRACK | SK::L_CURLY => depth += 1,
                SK::R_PAREN | SK::R_BRACK | SK::R_CURLY => depth -= 1,
                SK::COMMA if depth == 0 => {
                    out.push(&inner[start..i]);
                    start = i + 1;
                }
                _ => {}
            }
        }
        out.push(&inner[start..]);
        out
    };
    if parts.len() != declared.len() {
        return;
    }
    for (part, ty) in parts.iter().zip(declared.iter()) {
        // A literal on its own, or nothing is said.
        let [only] = part else { continue };
        let (Some(want), Some(got)) = (kind_of_type(ty), kind_of_literal(only)) else { continue };
        if want != got {
            diagnostics.push(module_diagnostic(
                "enum-mapping-type",
                format!("this enum mapping is declared `{}`", ty.trim()),
                only.text_range(),
            ));
        }
    }
}
