//! Signature help — displays function signature at call site.

use ide_db::line_index::LineCol;
use ide_db::FileDb;
use url::Url;

/// Signature help — returns internal SignatureHelp (framework-independent).
pub(crate) fn signature_help(
    files: &[(&Url, &dyn FileDb)],
    _uri: &Url,
    file: &dyn FileDb,
    position: LineCol,
) -> Option<ide_db::ide_types::SignatureHelp> {
    let call = crate::calls::fallback_call_at_offset(file, file.offset_at(&position))?;
    let (callee, arg_index) = (call.callee, call.arg_index);
    let all_files: Vec<_> = files
        .iter()
        .copied()
        .filter(|(_, target)| ide_db::symbol_visible_from(file, *target, &callee))
        .collect();

    // Find ALL overloaded signatures (not just the first match)
    let all_sigs = file
        .file_text()
        .and_then(|(db, ft)| {
            ide_db::signatures::callable_signatures_at(db, ft, call.callee_span.start, &callee)
        })
        .unwrap_or_else(|| {
            ide_db::signatures::find_all_callable_signatures(all_files.iter().copied(), &callee)
        });
    if all_sigs.is_empty() {
        return None;
    }

    let ordered = file
        .file_text()
        .is_some_and(|(db, ft)| hir::source_order::declarations_for(db, ft).is_some());
    let mut documentation = if ordered {
        file.file_text().and_then(|(db, ft)| {
            if matches!(
                hir::Semantics::new(db).resolve_path(db, ft, call.callee_span.start, &callee),
                Some(hir::PathResolution::Local(_))
            ) {
                return None;
            }
            use hir::source_order::{
                self,
                bindings::{self, Namespace},
            };
            let at = source_order::at_offset(db, ft, file.offset_at(&position))?;
            let selected = bindings::resolve(db, at, hir::Name::new(&callee), Namespace::Callable)?;
            hir::def_at_declaration_in_namespace(db, selected, &callee, Namespace::Callable)?
                .location()
                .doc_text(db)
        })
    } else {
        None
    };
    for (_, candidate_file) in all_files.iter().filter(|_| !ordered) {
        if let Some(tree) = candidate_file.item_tree() {
            if let Some(id) =
                tree.top_level_items().iter().find(|id| id.name(tree).as_str() == callee)
            {
                if let Some(comments) = id.doc(tree).map(str::to_string) {
                    documentation = Some(comments);
                }
                break;
            }
        }
    }

    // Build signature info for each overload
    let mut signatures = Vec::new();
    let mut best_active = 0usize;
    let mut best_score = 0usize;

    for (sig_idx, sig) in all_sigs.iter().enumerate() {
        // Score this overload: prefer one where the arg count matches
        let non_implicit_count = sig.params.iter().filter(|p| !p.is_implicit).count();
        let score = if non_implicit_count > arg_index { non_implicit_count } else { 0 };
        if score > best_score {
            best_score = score;
            best_active = sig_idx;
        }

        signatures.push(ide_db::ide_types::SignatureInfo {
            label: sig.label.clone(),
            documentation: if sig_idx == 0 { documentation.clone() } else { None },
            parameters: sig
                .params
                .iter()
                .map(|param| ide_db::ide_types::ParameterInfo { label: param.name.clone() })
                .collect(),
        });
    }

    let active_parameter = {
        let sig = &all_sigs[best_active];
        let mut visible_idx = 0;
        let mut full_idx = 0;
        for param in &sig.params {
            if !param.is_implicit {
                if visible_idx == arg_index {
                    break;
                }
                visible_idx += 1;
            }
            full_idx += 1;
        }
        full_idx.min(sig.params.len().saturating_sub(1))
    };

    Some(ide_db::ide_types::SignatureHelp {
        signatures,
        active_signature: Some(best_active),
        active_parameter: Some(active_parameter),
    })
}

/// The call the cursor is inside, as `(callee, argument index)`.
///
/// `call_info_at_position` sat between this and `calls::fallback_call_at_offset`
/// with no caller of its own — a wrapper whose only job was to be wrapped.
pub fn find_call_at_position(file: &dyn FileDb, position: LineCol) -> Option<(String, usize)> {
    let offset = file.offset_at(&position);
    let call = crate::calls::fallback_call_at_offset(file, offset)?;
    Some((call.callee, call.arg_index))
}

pub use crate::calls::CallInfo;

#[cfg(test)]
mod tests {
    #[test]
    fn nested_calls_select_the_innermost_argument_and_return_to_outer_call() {
        for (call, name, parameter) in
            [("outer(1, inner(2, $03), 4)", "inner", 1), ("outer(1, inner(2, 3), $04)", "outer", 2)]
        {
            let fixture = format!(
                "//- /main.sail\nval inner : (int, int) -> int\nval outer : (int, int, int) -> int\nlet result = {call}\n"
            );
            let (analysis, file, position) = crate::fixture::position(&fixture);
            let help = analysis.signature_help(file, position).unwrap().expect("signature help");
            let selected = &help.signatures[help.active_signature.unwrap()];
            assert!(selected.label.contains(name), "{selected:?}");
            assert_eq!(help.active_parameter, Some(parameter));
        }
    }

    /// Signature help at a call in a top-level initializer.
    ///
    /// Moved here from the *server's* test file, which reached into
    /// `ide::signature_help` for a function the server itself calls through
    /// `Analysis`. A test that enters where no caller does measures a path
    /// no caller takes — the lesson this repo keeps relearning — and it is
    /// what kept `signature_help_ide` public under a name RA does not have.
    #[test]
    fn a_call_in_a_top_level_initializer_has_a_signature() {
        let (analysis, file_id, position) = crate::fixture::position(
            r#"
//- /main.sail
val f : bits('n) -> bits('n)
function f(x) = x
let _ = f(0xDE$0ADBEEF)
"#,
        );
        let help = analysis.signature_help(file_id, position).unwrap().expect("signature help");
        assert_eq!(help.active_parameter, Some(0));
        assert_eq!(help.signatures.len(), 1);
        assert!(help.signatures[0].label.contains("bits('n) -> bits('n)"));
    }
}
