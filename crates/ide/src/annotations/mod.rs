//! Annotations / Code Lenses — reference counts, implementation counts.
//! Architecture: `annotations()` collects `Annotation` values with
//! `AnnotationKind` variants. Data fields are `None` initially
//! (lazy). `resolve_annotation()` fills in the data on demand.

use ide_db::ide_types::SymbolKind;
use ide_db::line_index::TextRange;
use ide_db::{extract_symbol_decls, FileDb};

use std::collections::HashMap;
use url::Url;

/// A single annotation (code lens) on a source range.
#[derive(Debug)]
pub struct Annotation {
    /// Range this annotation targets.
    pub range: TextRange,
    /// Kind of annotation.
    pub kind: AnnotationKind,
}

/// Kind of annotation.
#[derive(Debug)]
pub enum AnnotationKind {
    /// Definition has implementations/clauses.
    HasImpls {
        /// Position to query for implementations.
        name: String,
        /// Lazily resolved implementation count.
        data: Option<usize>,
    },
    /// Definition has references.
    HasReferences {
        /// Symbol name to query references for.
        name: String,
        /// Lazily resolved reference count.
        data: Option<usize>,
    },
    /// Runnable function.
    Runnable {
        /// Function name.
        name: String,
    },
}

/// Configuration for which annotations to compute.
pub struct AnnotationConfig {
    /// Whether to show implementation count lenses.
    pub annotate_impls: bool,
    /// Whether to show reference count lenses.
    pub annotate_references: bool,
    /// Whether to show runnable lenses.
    pub annotate_runnables: bool,
    /// Where to place annotations.
    pub location: AnnotationLocation,
}

impl Default for AnnotationConfig {
    fn default() -> Self {
        Self {
            annotate_impls: true,
            annotate_references: true,
            annotate_runnables: true,
            location: AnnotationLocation::AboveName,
        }
    }
}

/// Where annotations are placed relative to the item.
pub enum AnnotationLocation {
    /// Above the item name.
    AboveName,
    /// Above the entire item (including attributes, docs).
    AboveWholeItem,
}

/// Collect annotations for a file (first phase — data is `None`).
/// Callers should subsequently call `resolve_annotation()` to fill
/// in the data lazily (e.g., when the client requests code lens
/// resolution).
pub fn annotations(file: &dyn FileDb, config: &AnnotationConfig) -> Vec<Annotation> {
    let mut result = Vec::new();

    for decl in extract_symbol_decls(file) {
        // Skip local bindings and enum members
        if decl.detail == "binding" || decl.kind == SymbolKind::EnumMember {
            continue;
        }

        let range = base_db::text_range(decl.offset, decl.offset + decl.name.len());

        // HasReferences annotation
        if config.annotate_references {
            result.push(Annotation {
                range,
                kind: AnnotationKind::HasReferences {
                    name: decl.name.clone(),
                    data: None, // lazy
                },
            });
        }

        // HasImpls annotation for functions, types
        if config.annotate_impls
            && matches!(decl.kind, SymbolKind::Function | SymbolKind::Struct | SymbolKind::Enum)
        {
            result.push(Annotation {
                range,
                kind: AnnotationKind::HasImpls {
                    name: decl.name.clone(),
                    data: None, // lazy
                },
            });
        }

        // Runnable annotation for functions
        if config.annotate_runnables && decl.kind == SymbolKind::Function {
            result.push(Annotation {
                range,
                kind: AnnotationKind::Runnable { name: decl.name.clone() },
            });
        }
    }

    result
}

/// Resolve an annotation by filling in its data.
pub fn resolve_annotation(
    annotation: &mut Annotation,
    ref_counts: &HashMap<String, usize>,
    impl_counts: &HashMap<String, usize>,
) {
    match &mut annotation.kind {
        AnnotationKind::HasReferences { name, data } => {
            *data = Some(ref_counts.get(name.as_str()).copied().unwrap_or(0));
        }
        AnnotationKind::HasImpls { name, data } => {
            *data = Some(impl_counts.get(name.as_str()).copied().unwrap_or(0));
        }
        AnnotationKind::Runnable { .. } => {
            // Nothing to resolve for runnables.
        }
    }
}

//
// These functions preserve the old API used by callers.
// New code should use annotations() + resolve_annotation().

/// Collect workspace-wide reference counts.
pub fn collect_reference_counts<F: FileDb + ?Sized>(
    files: &[(&Url, &F)],
) -> HashMap<String, usize> {
    let mut counts = HashMap::<String, usize>::new();
    for (_, file) in files {
        let cached = file.ref_counts();
        if !cached.is_empty() {
            for (name, count) in cached.iter() {
                *counts.entry(name.clone()).or_insert(0) += count;
            }
            continue;
        }
        if let Some(parsed) = file.parsed() {
            for occ in &parsed.symbol_occurrences {
                if occ.kind == syntax::parser_lower::SymbolOccurrenceKind::Value
                    && occ.role.is_none()
                {
                    *counts.entry(occ.name.clone()).or_insert(0) += 1;
                }
            }
        }
    }
    counts
}

/// Collect workspace-wide implementation counts.
pub fn collect_implementation_counts<F: FileDb + ?Sized>(
    files: &[(&Url, &F)],
) -> HashMap<String, usize> {
    let mut counts = HashMap::<String, usize>::new();
    for (_, file) in files {
        let cached = file.impl_counts();
        if !cached.is_empty() {
            for (name, count) in cached.iter() {
                *counts.entry(name.clone()).or_insert(0) += count;
            }
            continue;
        }
        if let Some(parsed) = file.parsed() {
            for decl in &parsed.decls {
                if decl.role == syntax::parser_lower::DeclRole::Definition
                    && matches!(
                        decl.kind,
                        syntax::parser_lower::DeclKind::Function
                            | syntax::parser_lower::DeclKind::Mapping
                    )
                {
                    *counts.entry(decl.name.clone()).or_insert(0) += 1;
                }
            }
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

/// Render annotation title from JSON data (backward compat).
pub fn code_lens_title(data: &serde_json::Value) -> Option<String> {
    let kind = data.get("kind")?.as_str()?;
    let count = data.get("count").and_then(|v| v.as_u64()).map(|n| n as usize).unwrap_or(0);
    match kind {
        "refs" => Some(pluralize(count, "reference", "references")),
        "impls" => Some(pluralize(count, "implementation", "implementations")),
        "runnable" => {
            let runnable_kind = data.get("runnableKind").and_then(|v| v.as_str()).unwrap_or("run");
            match runnable_kind {
                "test" => Some("▶ Run Test".to_string()),
                _ => Some("▶ Run".to_string()),
            }
        }
        _ => None,
    }
}

/// Code lenses via backward-compat Annotation (old API).
pub fn code_lenses_ide(
    file: &dyn FileDb,
    ref_counts: &HashMap<String, usize>,
    impl_counts: &HashMap<String, usize>,
) -> Vec<ide_db::ide_types::Annotation> {
    let mut out = Vec::new();

    for decl in extract_symbol_decls(file) {
        if decl.detail == "binding" || decl.kind == SymbolKind::EnumMember {
            continue;
        }
        let range = base_db::text_range(decl.offset, decl.offset + decl.name.len());
        let refs = ref_counts.get(&decl.name).copied().unwrap_or(0);

        out.push(ide_db::ide_types::Annotation {
            range,
            title: String::new(),
            command: None,
            data: Some(serde_json::json!({
                "kind": "refs",
                "name": decl.name,
                "count": refs
            })),
        });

        if matches!(decl.kind, SymbolKind::Function | SymbolKind::Struct | SymbolKind::Enum) {
            let impls = impl_counts.get(&decl.name).copied().unwrap_or(0);
            out.push(ide_db::ide_types::Annotation {
                range,
                title: String::new(),
                command: None,
                data: Some(serde_json::json!({
                    "kind": "impls",
                    "name": decl.name,
                    "count": impls
                })),
            });
        }
    }

    // Runnable lenses: detect `main` and `$[test]`-annotated functions.
    for runnable in crate::runnables::runnables(file) {
        let range = base_db::span_to_text_range(&runnable.span);
        let runnable_kind = match runnable.kind {
            crate::runnables::RunnableKind::Main => "run",
            crate::runnables::RunnableKind::Test => "test",
        };
        out.push(ide_db::ide_types::Annotation {
            range,
            title: String::new(),
            command: None,
            data: Some(serde_json::json!({
                "kind": "runnable",
                "name": runnable.name,
                "runnableKind": runnable_kind
            })),
        });
    }

    out
}
mod fn_references;
