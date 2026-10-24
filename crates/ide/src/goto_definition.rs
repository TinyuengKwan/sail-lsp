//! Go to definition — resolves the definition site of a symbol.
//!
//! ```text
//! pub(crate) fn goto_definition(
//!     db: &RootDatabase,
//!     position: FilePosition,
//! ) -> Option<RangeInfo<Vec<NavigationTarget>>>
//! ```
//!
//! # Architecture
//!
//! Two resolution paths:
//!
//! 1. **Semantic**: Offset → token → name → Resolver (scope-aware) → definition
//!    - Handles local bindings, shadowing, same-file definitions
//!    - Uses body source maps for local variable binding locations
//!
//! 2. **Index fallback**: Token text → SymbolIndex → FileLocation
//!    - Fast O(1) for cross-file navigation
//!    - Does not handle shadowing or scope-awareness

use crate::navigation_target::NavigationTarget;
use ide_db::ide_types::{FileLocation, SymbolKind};
use ide_db::line_index::LineCol;
use ide_db::symbol_index::SymbolIndex;
use ide_db::FileDb;
use url::Url;

use hir::DefKind;

// Re-export the legacy generic version for callers that pass file iterators.
pub use crate::navigation::symbol_definition_locations;

//
// Sail supports `@property` pragmas that annotate functions with metadata:
//
//   @property("termination_measure", f, "n")
//   function f(n : int) -> int = ...
//
// When the cursor is on a property reference (e.g., the function name `f`
// inside an @property pragma), goto-definition should navigate to the
// annotated function's definition site.
//
// Implementation plan:
// 1. In `item_tree/lower.rs`, recognize `@property` directives and store
//    them as metadata on the annotated function's ItemTree entry.
// 2. Here, detect when the cursor is inside a `@property` pragma token
//    (check if the line starts with `@property` and cursor is on a name).
// 3. Extract the function name from the pragma and delegate to the
//    existing definition resolution (semantic or index-based).
//
// This requires pragma parsing infrastructure in hir-def and is deferred
// to a follow-up pass.

/// Semantic goto-definition: resolve the symbol at `position` in `file`
/// using scope-aware name resolution.
///
/// position via semantic analysis rather than text-based index lookup.
///
/// Returns `None` if semantic resolution fails (caller should fall
/// back to the index-based `goto_definition`).
///
/// ## Resolution strategy
///
/// 1. Find the token at the cursor position
/// 2. Extract the symbol name from the token
/// 3. Check if the offset is inside a callable body:
///    a. If yes: check for local binding (let/var/match pattern)
///    b. If no binding: check for same-file definition
/// 4. If same-file definition found: return its span as NavigationTarget
pub fn goto_definition_semantic<F: FileDb>(
    file: &F,
    position: LineCol,
    file_url: &Url,
) -> Option<Vec<NavigationTarget>> {
    // 1. Find token at cursor position
    let (token, _span) = file.token_at(position)?;
    let name = ide_db::token_symbol_key(token)?;

    // 2. Check local bindings first (scope-aware: handles shadowing)
    if let Some(bodies) = file.bodies() {
        let offset = file.offset_at(&position);
        for entry in bodies.entries() {
            // Check if offset is inside this callable's body
            if let Some(expr_id) = entry.source_map.expr_at_offset(offset) {
                // The cursor is inside this body — check if the name
                // is a reference to a pattern-bound local variable.
                if let hir::Expr::Ident(ref ident_name) = entry.body.store[expr_id] {
                    if ident_name == &name {
                        // Search this body's patterns for the binding site
                        if let Some(nav) = find_local_binding_nav(entry, &name, file_url) {
                            return Some(vec![nav]);
                        }
                    }
                }
            }
        }
    }

    // 3. Check same-file top-level definitions
    if let Some(tree) = file.item_tree() {
        let in_type_position = names_a_type(file.text(), file.offset_at(&position));
        let targets =
            find_same_file_definition(tree, file.file_text(), &name, file_url, in_type_position);
        if !targets.is_empty() {
            return Some(targets);
        }
    }

    None
}

/// What the identifier under the cursor *is*, decided from its parent node.
///
/// rust-analyzer routes every navigation feature through one classifier
/// (`ide-db/src/defs.rs`, `IdentClass::classify_node`) that dispatches on the
/// parent syntax node: a `Name` defines something, a `NameRef` refers to
/// something, and a field expression resolves through the receiver's type.
/// sail-lsp had the `NameClass`/`NameRefClass` types but nothing that built
/// them, so each feature matched on names by hand and every one of them
/// missed a different set of constructs.
pub enum IdentClass {
    /// The identifier is itself a definition — a pattern binding, or the
    /// name in a declaration. Navigating from it lands on itself, which is
    /// what rust-analyzer does and what an editor expects.
    Definition { name: String, span: base_db::TextRange },
    /// A field of some record or bitfield: `x.field`, or `f` in
    /// `struct { f = v }`.
    Field { field: String },
    /// Anything else — resolved by name, as before.
    Reference { name: String },
}

/// Classify the identifier at `offset` in a parsed file.
pub fn classify_ident(root: &syntax::SyntaxNode, offset: usize, name: &str) -> IdentClass {
    use parser::SyntaxKind as SK;

    let parent = root
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT && u32::from(t.text_range().start()) as usize == offset)
        .and_then(|t| t.parent());
    let span = parent
        .as_ref()
        .map(|_| base_db::text_range(offset, offset + name.len()))
        .unwrap_or_else(|| base_db::text_range(offset, offset + name.len()));
    match parent.map(|p| p.kind()) {
        Some(SK::IDENT_PAT) => IdentClass::Definition { name: name.to_string(), span },
        Some(SK::FIELD_ACCESS_EXPR) | Some(SK::FIELD_INIT) => {
            IdentClass::Field { field: name.to_string() }
        }
        _ => IdentClass::Reference { name: name.to_string() },
    }
}

/// Search a callable body's patterns for a binding with the given name.
/// Returns the binding's location as a NavigationTarget.
fn find_local_binding_nav(
    entry: &hir::CallableBody,
    name: &str,
    file_url: &Url,
) -> Option<NavigationTarget> {
    for (pat_id, pat) in entry.body.iter_pats() {
        if let hir::Pat::Bind(ref pat_name) = pat {
            if pat_name == name {
                if let Some(pat_span) = entry.source_map.pat_syntax(pat_id) {
                    return Some(NavigationTarget {
                        file_id: None,
                        url: file_url.clone(),
                        name: name.to_string(),
                        kind: SymbolKind::Variable,
                        full_range: base_db::text_range(pat_span.start, pat_span.end),
                        focus_range: base_db::text_range(pat_span.start, pat_span.end),
                        detail: None,
                        docs: None,
                        children: Vec::new(),
                    });
                }
            }
        }
    }
    None
}

/// Where `name` is defined in this file.
///
/// From the item tree. A definition wins over a declaration — goto on a
/// call should land on the `function f`, not its `val` — and the cursor
/// lands on the name, which is what `name_span` records. Every producer
/// here used to pass the whole definition span as the focus range, so
/// goto put the cursor on the `function` keyword.
/// Whether the token at `offset` sits where a *type* is expected.
///
/// Sail has two namespaces — `PerNs` is Types and Values — and nothing at a
/// use site said which one it was in. `find_same_file_definition` matched
/// on the spelling alone, so `x : int` in a file that also declares
/// `val int : ..` navigated to the function, and every builtin type name
/// reused as a value name did the same.
pub fn names_a_type(text: &str, offset: usize) -> bool {
    use parser::SyntaxKind as SK;
    let (root, _) = syntax::parse_text(text);
    let Some(token) = root
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .find(|t| t.kind() == SK::IDENT && usize::from(t.text_range().start()) == offset)
    else {
        return false;
    };
    let Some(parent) = token.parent() else { return false };
    if !parent.kind().is_type_node() {
        return false;
    }
    // `instantiation sail_mem_read with pa_bits = physaddrbits_zero_extend`
    // binds an outcome's parameter to a *function*, and the grammar wraps
    // the right-hand side in a type node. It is a value there.
    !parent.ancestors().any(|a| a.kind() == SK::INSTANTIATION_DEF)
}

fn find_same_file_definition(
    tree: &hir::ItemTree,
    file_text: Option<(&dyn salsa::Database, base_db::FileText)>,
    name: &str,
    file_url: &Url,
    in_type_position: bool,
) -> Vec<NavigationTarget> {
    let declares_only = |kind: hir::DefKind| {
        matches!(
            kind,
            hir::DefKind::ValSpec | hir::DefKind::MappingSpec | hir::DefKind::ScatteredHead
        )
    };
    let make = |id: &hir::DefItem| {
        let span = id.span(tree);
        let name_span = id.name_span(tree);
        NavigationTarget::new(
            file_url.clone(),
            name,
            item_kind_to_symbol_kind(id.def_kind(tree)),
            base_db::text_range(span.start, span.end),
            base_db::text_range(name_span.start, name_span.end),
        )
    };
    let named = |id: &&hir::DefItem| id.name(tree).as_str() == name;
    // In type position, a declaration that can only be a *value* is not what
    // the name means. Only these kinds are dropped, and only when something
    // is left — a union constructor named in a value position still resolves
    // to the union that declares it, and a name this rule cannot place goes
    // through unfiltered rather than answering nothing.
    let value_only = |kind: hir::DefKind| {
        matches!(
            kind,
            hir::DefKind::Function
                | hir::DefKind::ValSpec
                | hir::DefKind::Mapping
                | hir::DefKind::MappingSpec
                | hir::DefKind::Register
                | hir::DefKind::Let
                | hir::DefKind::Var
                | hir::DefKind::Overload
        )
    };
    let right_namespace = |id: &&hir::DefItem| !in_type_position || !value_only(id.def_kind(tree));

    let mut targets: Vec<NavigationTarget> = tree
        .top_level_items()
        .iter()
        .filter(named)
        .filter(right_namespace)
        .filter(|id| !declares_only(id.def_kind(tree)))
        .map(make)
        .collect();
    if targets.is_empty() {
        targets =
            tree.top_level_items().iter().filter(named).filter(right_namespace).map(make).collect();
    }
    if !targets.is_empty() {
        return targets;
    }
    // A local binding, from the bodies. The flat `decls` list this replaced
    // held local declarations too and matched them without regard to scope,
    // so a name bound anywhere in the file resolved from anywhere in it.
    if let Some((db, ft)) = file_text {
        for span in hir::Semantics::new(db).binding_sites(db, ft, name) {
            let r = base_db::text_range(span.start, span.end);
            targets.push(NavigationTarget::new(
                file_url.clone(),
                name.to_string(),
                SymbolKind::Variable,
                r,
                r,
            ));
        }
        if !targets.is_empty() {
            return targets;
        }
    }

    // A field or a union/enum member is not a top-level item — the item tree
    // records it on the definition that declares it — but it is a name an
    // editor jumps to, and the flat `decls` list this replaced had them.
    for &id in tree.top_level_items() {
        for field in id.fields(tree) {
            if field.name.as_str() != name {
                continue;
            }
            let r = base_db::text_range(field.name_span.start, field.name_span.end);
            targets.push(NavigationTarget {
                file_id: None,
                url: file_url.clone(),
                name: name.to_string(),
                kind: match id.def_kind(tree) {
                    hir::DefKind::Enum | hir::DefKind::Union => SymbolKind::EnumMember,
                    _ => SymbolKind::Field,
                },
                full_range: r,
                focus_range: r,
                detail: None,
                docs: None,
                children: Vec::new(),
            });
        }
    }
    targets
}

/// 投産-4: Semantic goto-definition using SourceAnalyzer for field access
/// and method call resolution. Requires salsa database + FileText.
///
/// This supplements `goto_definition_semantic` by resolving:
/// - Field access expressions (e.g., `x.field_name`) via `resolve_field`
/// - Function call expressions (e.g., `f(args)`) via `resolve_method_call`
///
/// Returns `None` if no field/method resolution is found at the offset.
///
/// two separate resolution calls.
pub fn goto_definition_field_or_method(
    db: &dyn salsa::Database,
    file_text: base_db::FileText,
    offset: usize,
    file_url: &Url,
    index: &SymbolIndex,
    url_of: &dyn Fn(base_db::FileId) -> Option<Url>,
) -> Option<Vec<NavigationTarget>> {
    let source = file_text.text(db);

    // Extract the identifier token at the offset from source text
    let token_name = extract_identifier_at(source, offset)?;

    let sema = hir::Semantics::new(db);

    // Use combined field/method resolution (Either<field, method>).
    // Both branches look up the token name in the SymbolIndex.
    let _resolution = sema.resolve_field_or_method(file_text, offset)?;

    let targets = symbol_index_to_nav_targets(index, &token_name, file_url, url_of);
    if !targets.is_empty() {
        return Some(targets);
    }

    None
}

/// Extract the identifier (word) at a byte offset in source text.
/// Returns the identifier string if the offset points into an identifier.
fn extract_identifier_at(source: &str, offset: usize) -> Option<String> {
    if offset >= source.len() {
        return None;
    }
    let bytes = source.as_bytes();
    // Check that offset is within an identifier character
    if !is_ident_char(bytes[offset]) {
        return None;
    }
    // Scan backward to find start
    let mut start = offset;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    // Scan forward to find end
    let mut end = offset;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }
    Some(source[start..end].to_string())
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Look up a symbol name in the SymbolIndex and convert to NavigationTargets.
/// Prefers definitions over declarations; prefers the same file.
fn symbol_index_to_nav_targets(
    index: &SymbolIndex,
    name: &str,
    current_url: &Url,
    url_of: &dyn Fn(base_db::FileId) -> Option<Url>,
) -> Vec<NavigationTarget> {
    let entries = index.find(name);
    if entries.is_empty() {
        return Vec::new();
    }

    // Prefer definitions (Function, Struct, etc.) over declarations (ValSpec)
    let definitions: Vec<_> = entries
        .iter()
        .filter(|e| !matches!(e.kind, DefKind::ValSpec | DefKind::MappingSpec))
        .collect();

    let chosen =
        if definitions.is_empty() { entries.iter().collect::<Vec<_>>() } else { definitions };

    // Prefer entries from the same file
    let mut sorted = chosen;
    sorted.sort_by(|a, b| {
        let a_same = url_of(a.file_id).as_ref() == Some(current_url);
        let b_same = url_of(b.file_id).as_ref() == Some(current_url);
        b_same.cmp(&a_same)
    });

    sorted
        .into_iter()
        .filter_map(|entry| {
            Some(NavigationTarget {
                file_id: Some(entry.file_id),
                url: url_of(entry.file_id)?,
                name: entry.name.clone(),
                kind: item_kind_to_symbol_kind(entry.kind),
                full_range: base_db::text_range(entry.span.start, entry.span.end),
                focus_range: base_db::text_range(entry.name_span.start, entry.name_span.end),
                detail: if entry.signature_text.is_empty() {
                    None
                } else {
                    Some(entry.signature_text.clone())
                },
                docs: entry.doc.clone(),
                children: Vec::new(),
            })
        })
        .collect()
}

/// Go to definition using the workspace symbol index (O(1) lookup).
///
/// This is the cross-file fallback path when semantic resolution
/// doesn't find a result in the current file.
///
/// Mirrors the legacy API (kept for backward compatibility).
pub fn goto_definition(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
    url_of: &dyn Fn(base_db::FileId) -> Option<Url>,
) -> Vec<FileLocation> {
    crate::navigation::definition_locations_indexed(index, symbol_key, uri_hint, url_of)
}

/// Convert an `DefKind` to a `SymbolKind` for NavigationTarget.
pub fn item_kind_to_symbol_kind(kind: DefKind) -> SymbolKind {
    match kind {
        DefKind::Function => SymbolKind::Function,
        DefKind::ValSpec | DefKind::Outcome => SymbolKind::Function,
        DefKind::Struct => SymbolKind::Struct,
        DefKind::Union => SymbolKind::Struct, // Sail unions mapped to Struct kind
        DefKind::Enum => SymbolKind::Enum,
        DefKind::Bitfield => SymbolKind::Struct,
        DefKind::Newtype => SymbolKind::Struct,
        DefKind::TypeAlias => SymbolKind::TypeAlias,
        DefKind::Register => SymbolKind::Variable,
        DefKind::Mapping | DefKind::MappingSpec => SymbolKind::Function,
        DefKind::Let | DefKind::Var => SymbolKind::Variable,
        DefKind::Overload => SymbolKind::Function,
        DefKind::ScatteredHead | DefKind::ScatteredClause => SymbolKind::Function,
        DefKind::Constraint => SymbolKind::TypeAlias,
        DefKind::Field => SymbolKind::Field,
        DefKind::Variant => SymbolKind::EnumMember,
        DefKind::TerminationMeasure | DefKind::EndMarker | DefKind::Instantiation => {
            SymbolKind::Variable
        }
    }
}

#[cfg(test)]
mod namespace_tests {
    use crate::fixture;

    /// A name in type position does not resolve to a value.
    ///
    /// Sail has two namespaces and `find_same_file_definition` matched on
    /// the spelling alone, so `x : int` in a file that also declares
    /// `val int : ..` navigated to the function — a wrong answer, which is
    /// worse than none.
    #[test]
    fn a_type_position_does_not_resolve_to_a_value() {
        let source = "\
default Order dec
val int : unit -> unit
type myint = int
struct S = { f : int }
function f(x : S) -> unit = ()
";
        let (analysis, files) = fixture::analysis(source);
        let url = analysis.url_for_file_id(files[0].0).expect("url");
        let li = ide_db::line_index::LineIndex::new(source);

        // `x : S` — the struct, not anything else.
        let at = source.find("x : S").expect("fixture") + 4;
        let targets = analysis.goto_definition(url, li.line_col(at)).expect("no cancellation");
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].kind, ide_db::ide_types::SymbolKind::Struct);

        // `type myint = int` — `int` names no type here, and the `val` is
        // not one either, so nothing rather than the function.
        let at = source.find("= int").expect("fixture") + 2;
        let targets = analysis.goto_definition(url, li.line_col(at)).expect("no cancellation");
        assert!(
            targets.iter().all(|t| t.kind != ide_db::ide_types::SymbolKind::Function),
            "a type resolved to a function: {targets:?}"
        );
    }
}
