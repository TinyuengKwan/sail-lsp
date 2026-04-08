use super::analysis::extract_symbol_decls;
use crate::state::File;
use std::collections::HashMap;
use tower_lsp::lsp_types::{CodeLens, Range, SymbolKind, Url};

pub(crate) fn collect_reference_counts(files: &[(&Url, &File)]) -> HashMap<String, usize> {
    let mut counts = HashMap::<String, usize>::new();
    for (_, file) in files {
        for (name, count) in &file.ref_counts {
            *counts.entry(name.clone()).or_insert(0) += count;
        }
    }
    counts
}

pub(crate) fn collect_implementation_counts(files: &[(&Url, &File)]) -> HashMap<String, usize> {
    let mut counts = HashMap::<String, usize>::new();
    for (_, file) in files {
        for (name, count) in &file.impl_counts {
            *counts.entry(name.clone()).or_insert(0) += count;
        }
    }
    counts
}

fn pluralize(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{count} {singular}")
    } else {
        format!("{count} {plural}")
    }
}

pub(crate) fn code_lens_title(data: &serde_json::Value) -> Option<String> {
    let kind = data.get("kind")?.as_str()?;
    let count = data
        .get("count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(0);
    match kind {
        "refs" => Some(pluralize(count, "reference", "references")),
        "impls" => Some(pluralize(count, "implementation", "implementations")),
        _ => None,
    }
}

pub(crate) fn code_lenses_for_file(
    file: &File,
    ref_counts: &HashMap<String, usize>,
    impl_counts: &HashMap<String, usize>,
) -> Vec<CodeLens> {
    let mut out = Vec::new();

    for decl in extract_symbol_decls(file) {
        if decl.detail == "binding" || decl.kind == SymbolKind::ENUM_MEMBER {
            continue;
        }
        let range = Range::new(
            file.source.position_at(decl.offset),
            file.source.position_at(decl.offset + decl.name.len()),
        );
        let refs = ref_counts.get(&decl.name).copied().unwrap_or(0);
        out.push(CodeLens {
            range,
            command: None,
            data: Some(serde_json::json!({
                "kind": "refs",
                "name": decl.name,
                "count": refs
            })),
        });

        if decl.kind == SymbolKind::FUNCTION {
            let impls = impl_counts.get(&decl.name).copied().unwrap_or(0);
            out.push(CodeLens {
                range,
                command: None,
                data: Some(serde_json::json!({
                    "kind": "impls",
                    "name": decl.name,
                    "count": impls
                })),
            });

            out.push(CodeLens {
                range,
                command: Some(tower_lsp::lsp_types::Command {
                    title: "▶\u{fe0e} Run in Sail".to_string(),
                    command: "sail.run".to_string(),
                    arguments: Some(vec![serde_json::json!(decl.name)]),
                }),
                data: None,
            });
        }
    }

    out
}
