//! Workspace-level scattered definition aggregation.
//!
//! Groups scattered heads, clauses, and `end` markers by name across
//! all files so consumers can check completeness in O(1).

use std::collections::HashMap;

use crate::item_tree::{ItemKind, ItemTree};
use crate::Span;

/// Kind of a scattered definition group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatteredKind {
    Function,
    Mapping,
    Union,
    Enum,
}

/// Aggregated scattered-definition status for a given name.
#[derive(Debug, Clone, Default)]
pub struct ScatteredStatus {
    pub kind: Option<ScatteredKind>,
    pub heads: Vec<ScatteredHeadLocation>,
    pub clauses: Vec<Span>,
    pub ends: Vec<Span>,
    /// Member names from union/enum clauses (for completeness checks).
    pub clause_member_names: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ScatteredHeadLocation {
    pub name_span: Span,
    pub def_span: Span,
}

impl ScatteredStatus {
    /// True iff any clause exists for this name.
    pub fn has_clauses(&self) -> bool {
        !self.clauses.is_empty()
    }

    /// True iff an `end` marker exists for this name.
    pub fn has_end(&self) -> bool {
        !self.ends.is_empty()
    }
}

/// Infer `ScatteredKind` from signature text.
fn scattered_kind_from_sig(sig: &str) -> Option<ScatteredKind> {
    let rest = sig.strip_prefix("scattered")?.trim_start();
    if rest.starts_with("function") {
        Some(ScatteredKind::Function)
    } else if rest.starts_with("mapping") {
        Some(ScatteredKind::Mapping)
    } else if rest.starts_with("union") {
        Some(ScatteredKind::Union)
    } else if rest.starts_with("enum") {
        Some(ScatteredKind::Enum)
    } else {
        None
    }
}

/// Aggregate scattered groups by name from `item_trees`.
pub fn workspace_scattered_status<'a, I>(item_trees: I) -> HashMap<String, ScatteredStatus>
where
    I: IntoIterator<Item = &'a ItemTree>,
{
    let mut out: HashMap<String, ScatteredStatus> = HashMap::new();
    for tree in item_trees {
        for &id in tree.top_level_items() {
            let name_str = id.name(tree).as_str();
            let kind = id.item_kind(tree);
            let is_clause = id.is_clause(tree);
            let span = id.span(tree);
            match kind {
                ItemKind::ScatteredHead => {
                    let e = out.entry(name_str.to_string()).or_default();
                    e.kind = scattered_kind_from_sig(id.signature(tree));
                    e.heads.push(ScatteredHeadLocation { name_span: span, def_span: span });
                }
                ItemKind::EndMarker => {
                    let e = out.entry(name_str.to_string()).or_default();
                    e.ends.push(span);
                }
                // `function clause` or `mapping clause`
                ItemKind::Function if is_clause => {
                    let e = out.entry(name_str.to_string()).or_default();
                    if e.kind.is_none() {
                        e.kind = Some(ScatteredKind::Function);
                    }
                    e.clauses.push(span);
                }
                ItemKind::Mapping if is_clause => {
                    let e = out.entry(name_str.to_string()).or_default();
                    if e.kind.is_none() {
                        e.kind = Some(ScatteredKind::Mapping);
                    }
                    e.clauses.push(span);
                }
                // `union clause X = Y` or `enum clause X = Y`
                ItemKind::Union if is_clause => {
                    let e = out.entry(name_str.to_string()).or_default();
                    if e.kind.is_none() {
                        e.kind = Some(ScatteredKind::Union);
                    }
                    e.clauses.push(span);
                    if let Some(member) = id.member_name(tree) {
                        e.clause_member_names.push(member.to_owned());
                    }
                }
                ItemKind::Enum if is_clause => {
                    let e = out.entry(name_str.to_string()).or_default();
                    if e.kind.is_none() {
                        e.kind = Some(ScatteredKind::Enum);
                    }
                    e.clauses.push(span);
                    if let Some(member) = id.member_name(tree) {
                        e.clause_member_names.push(member.to_owned());
                    }
                }
                // Also handle SCATTERED_CLAUSE_DEF entries (if CST emits them)
                ItemKind::ScatteredClause => {
                    let e = out.entry(name_str.to_string()).or_default();
                    // Infer kind from signature text
                    let sig = id.signature(tree);
                    if e.kind.is_none() {
                        if sig.starts_with("union") {
                            e.kind = Some(ScatteredKind::Union);
                        } else if sig.starts_with("enum") {
                            e.kind = Some(ScatteredKind::Enum);
                        }
                    }
                    e.clauses.push(span);
                    if let Some(member) = id.member_name(tree) {
                        e.clause_member_names.push(member.to_owned());
                    }
                }
                _ => {}
            }
        }
    }
    out
}

/// Kind of scattered completeness problem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScatteredDiagnosticKind {
    /// `scattered function foo` without a corresponding `end foo`.
    MissingEnd,
    /// `scattered function foo` + `end foo` but zero `function clause foo(...)`.
    NoClauses,
    /// `end foo` without a preceding `scattered function foo` head.
    OrphanEnd,
}

/// A completeness diagnostic for a scattered definition group.
#[derive(Debug, Clone)]
pub struct ScatteredDiagnostic {
    /// The scattered definition name (e.g., `"execute"`).
    pub name: String,
    /// What's wrong.
    pub kind: ScatteredDiagnosticKind,
    /// Span for the diagnostic (head span, or end span for orphans).
    pub span: Span,
}

/// Check all scattered groups for completeness (missing end/clauses/orphan end).
pub fn check_scattered_completeness(
    status: &HashMap<String, ScatteredStatus>,
) -> Vec<ScatteredDiagnostic> {
    let mut diagnostics = Vec::new();

    for (name, s) in status {
        // Case 1: Has head(s) but no end marker.
        if !s.heads.is_empty() && !s.has_end() {
            diagnostics.push(ScatteredDiagnostic {
                name: name.clone(),
                kind: ScatteredDiagnosticKind::MissingEnd,
                span: s.heads[0].name_span,
            });
        }

        // Case 2: Has head + end but no clauses.
        // Only check for function/mapping (unions/enums can be empty).
        if !s.heads.is_empty()
            && s.has_end()
            && !s.has_clauses()
            && matches!(s.kind, Some(ScatteredKind::Function) | Some(ScatteredKind::Mapping))
        {
            diagnostics.push(ScatteredDiagnostic {
                name: name.clone(),
                kind: ScatteredDiagnosticKind::NoClauses,
                span: s.heads[0].name_span,
            });
        }

        // Case 3: Has end(s) but no head — orphan end marker.
        if s.heads.is_empty() && s.has_end() {
            diagnostics.push(ScatteredDiagnostic {
                name: name.clone(),
                kind: ScatteredDiagnosticKind::OrphanEnd,
                span: s.ends[0],
            });
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item_tree(source: &str) -> ItemTree {
        let (root, _) = syntax::parse_text(source);
        ItemTree::build_from_cst(&root)
    }

    #[test]
    fn aggregates_function_clauses_from_single_file() {
        let source = "\
scattered function foo : int -> int

function clause foo(0) = 0
function clause foo(n) = n + 1

end foo
";
        let tree = item_tree(source);
        let status = workspace_scattered_status(std::iter::once(&tree));
        let foo = status.get("foo").expect("foo present");
        assert_eq!(foo.kind, Some(ScatteredKind::Function));
        assert_eq!(foo.heads.len(), 1);
        assert_eq!(foo.clauses.len(), 2);
        assert_eq!(foo.ends.len(), 1);
        assert!(foo.has_clauses());
        assert!(foo.has_end());
    }

    #[test]
    fn aggregates_clauses_split_across_files() {
        let head = item_tree("scattered function bar : int -> int\n");
        let mid_a = item_tree("function clause bar(0) = 0\n");
        let mid_b = item_tree("function clause bar(n) = n + 1\n");
        let tail = item_tree("end bar\n");

        let trees = [&head, &mid_a, &mid_b, &tail];
        let status = workspace_scattered_status(trees.iter().copied());
        let bar = status.get("bar").expect("bar present");
        assert_eq!(bar.kind, Some(ScatteredKind::Function));
        assert_eq!(bar.heads.len(), 1);
        assert_eq!(bar.clauses.len(), 2);
        assert_eq!(bar.ends.len(), 1);
        assert!(bar.has_clauses());
        assert!(bar.has_end());
    }

    #[test]
    fn detects_missing_end_across_workspace() {
        let head = item_tree("scattered function lonely : int -> int\n");
        let body = item_tree("function clause lonely(x) = x\n");
        let trees = [&head, &body];
        let status = workspace_scattered_status(trees.iter().copied());
        let lonely = status.get("lonely").expect("present");
        assert!(lonely.has_clauses());
        assert!(!lonely.has_end());
    }

    #[test]
    fn detects_head_without_clauses_or_end() {
        let head = item_tree("scattered function orphan : int -> int\n");
        let unrelated = item_tree("function clause other(x) = x\n");
        let trees = [&head, &unrelated];
        let status = workspace_scattered_status(trees.iter().copied());
        let orphan = status.get("orphan").expect("present");
        assert!(!orphan.has_clauses());
        assert!(!orphan.has_end());
    }

    #[test]
    fn aggregates_union_clauses() {
        let source = "\
scattered union my_op

union clause my_op = ADD : (int, int)
union clause my_op = SUB : (int, int)

end my_op
";
        let tree = item_tree(source);
        let status = workspace_scattered_status(std::iter::once(&tree));
        let op = status.get("my_op").expect("present");
        assert_eq!(op.kind, Some(ScatteredKind::Union));
        assert_eq!(op.clauses.len(), 2);
        assert_eq!(op.ends.len(), 1);
        assert!(op.clause_member_names.contains(&"ADD".to_string()));
        assert!(op.clause_member_names.contains(&"SUB".to_string()));
    }

    #[test]
    fn aggregates_enum_clauses() {
        let source = "\
scattered enum colour

enum clause colour = Red
enum clause colour = Green
enum clause colour = Blue

end colour
";
        let tree = item_tree(source);
        let status = workspace_scattered_status(std::iter::once(&tree));
        let colour = status.get("colour").expect("present");
        assert_eq!(colour.kind, Some(ScatteredKind::Enum));
        assert_eq!(colour.clauses.len(), 3);
    }

    #[test]
    fn aggregates_mapping_clauses() {
        let source = "\
scattered mapping enc : bits(32) <-> string

mapping clause enc = 0x00000000 <-> \"nop\"
mapping clause enc = 0x00000001 <-> \"add\"

end enc
";
        let tree = item_tree(source);
        let status = workspace_scattered_status(std::iter::once(&tree));
        let enc = status.get("enc").expect("present");
        assert_eq!(enc.kind, Some(ScatteredKind::Mapping));
        assert_eq!(enc.clauses.len(), 2);
    }

    #[test]
    fn duplicate_heads_are_recorded() {
        let head_a = item_tree("scattered function dup : int -> int\n");
        let head_b = item_tree("scattered function dup : int -> int\n");
        let trees = [&head_a, &head_b];
        let status = workspace_scattered_status(trees.iter().copied());
        let dup = status.get("dup").expect("present");
        assert_eq!(dup.heads.len(), 2);
    }

    #[test]
    fn sail_riscv_execute_pattern() {
        // Mimics sail-riscv: `execute` is scattered across many files.
        // Head in decode.sail, clauses in per-extension files, end in postlude.
        let head = item_tree("scattered function execute\n");
        let clause_rv32i = item_tree("function clause execute(ADDI(imm, rs1, rd)) = true\n");
        let clause_rv64i = item_tree("function clause execute(ADDW(rs2, rs1, rd)) = true\n");
        let clause_rv32m = item_tree("function clause execute(MUL(rs2, rs1, rd)) = true\n");
        let end_file = item_tree("end execute\n");

        let trees = [&head, &clause_rv32i, &clause_rv64i, &clause_rv32m, &end_file];
        let status = workspace_scattered_status(trees.iter().copied());
        let exec = status.get("execute").expect("execute present");
        assert_eq!(exec.kind, Some(ScatteredKind::Function));
        assert_eq!(exec.heads.len(), 1);
        assert_eq!(exec.clauses.len(), 3);
        assert_eq!(exec.ends.len(), 1);
        assert!(exec.has_clauses());
        assert!(exec.has_end());
    }

    #[test]
    fn completeness_missing_end() {
        let head = item_tree("scattered function foo\n");
        let clause = item_tree("function clause foo() = 0\n");
        let trees = [&head, &clause];
        let status = workspace_scattered_status(trees.iter().copied());
        let diags = check_scattered_completeness(&status);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].kind, ScatteredDiagnosticKind::MissingEnd);
        assert_eq!(diags[0].name, "foo");
    }

    #[test]
    fn completeness_no_clauses() {
        let head = item_tree("scattered function bar\n");
        let end = item_tree("end bar\n");
        let trees = [&head, &end];
        let status = workspace_scattered_status(trees.iter().copied());
        let diags = check_scattered_completeness(&status);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].kind, ScatteredDiagnosticKind::NoClauses);
        assert_eq!(diags[0].name, "bar");
    }

    #[test]
    fn completeness_orphan_end() {
        let end = item_tree("end orphan\n");
        let trees = [&end];
        let status = workspace_scattered_status(trees.iter().copied());
        let diags = check_scattered_completeness(&status);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].kind, ScatteredDiagnosticKind::OrphanEnd);
        assert_eq!(diags[0].name, "orphan");
    }

    #[test]
    fn completeness_valid_scattered_no_diags() {
        let head = item_tree("scattered function baz\n");
        let clause = item_tree("function clause baz() = 1\n");
        let end = item_tree("end baz\n");
        let trees = [&head, &clause, &end];
        let status = workspace_scattered_status(trees.iter().copied());
        let diags = check_scattered_completeness(&status);
        assert!(diags.is_empty(), "valid scattered should have no diagnostics, got: {:?}", diags);
    }

    #[test]
    fn completeness_union_without_clauses_is_ok() {
        // Unions/enums can be declared without clauses (empty scattered)
        let head = item_tree("scattered union myop\n");
        let end = item_tree("end myop\n");
        let trees = [&head, &end];
        let status = workspace_scattered_status(trees.iter().copied());
        let diags = check_scattered_completeness(&status);
        assert!(diags.is_empty(), "empty scattered union should not warn, got: {:?}", diags);
    }
}
