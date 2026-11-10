//! Read-only preprocessing transcript, from the production walker.
use base_db::{preprocess::InstanceKey, FileId};
use hir::preprocess::{TraceKind, TraceStep};
use syntax::preprocess::Event;

#[derive(Debug, Clone)]
pub struct ExpansionMapping {
    pub generated: base_db::TextRange,
    pub original: hir::FileRange,
    pub kind: &'static str,
}

#[derive(Debug, Clone)]
pub struct ExpandIncludeResult {
    pub root: String,
    pub expansion: String,
    pub context: Option<InstanceKey>,
    pub mappings: Vec<ExpansionMapping>,
}

/// Render only the selected visit and its descendants. The incoming environment
/// already includes effects of earlier project roots and includes. Inactive
/// source never appears as active code; its directive is retained as evidence.
pub fn render(
    selected: &InstanceKey,
    trace: &[TraceStep],
    file_text: impl Fn(FileId) -> Option<String>,
    file_name: impl Fn(FileId) -> String,
) -> ExpandIncludeResult {
    let mut result = ExpandIncludeResult {
        root: file_name(selected.root),
        expansion: String::new(),
        context: Some(selected.clone()),
        mappings: Vec::new(),
    };
    let mut sources = std::collections::HashMap::new();
    for step in trace.iter().filter(|step| {
        step.key.root == selected.root
            && step.key.occurrence == selected.occurrence
            && step.key.module == selected.module
            && step.key.includes.starts_with(&selected.includes)
    }) {
        let Some((text, events)) = sources.entry(step.source).or_insert_with(|| {
            file_text(step.source).map(|text| {
                let root = syntax::parse_text(&text).0;
                let events = root
                    .children()
                    .filter_map(|node| {
                        Event::from_node(&node).map(|event| (node.text_range(), event))
                    })
                    .collect::<Vec<_>>();
                (text, events)
            })
        }) else {
            continue;
        };
        match &step.kind {
            TraceKind::Enter { symbols, target } => {
                result.expansion.push_str(&format!(
                    "// Enter {} (module {:?}, occurrence {}, includes {:?})\n// Target: {}; symbols: {}\n",
                    file_name(step.source), step.key.module, step.key.occurrence, step.key.includes,
                    target.as_deref().unwrap_or("default"), symbols.join(", "),
                ));
            }
            TraceKind::Region { ordinal, active } => {
                let start = ordinal
                    .checked_sub(1)
                    .and_then(|i| events.get(i))
                    .map_or(0, |(range, _)| base_db::range_end(*range));
                let end = events
                    .get(*ordinal)
                    .map_or(text.len(), |(range, _)| base_db::range_start(*range));
                if *active && start < end {
                    append(&mut result, step.source, start, end, &text[start..end], "source");
                } else if !*active && !text[start..end].trim().is_empty() {
                    result.expansion.push_str("// Inactive source omitted\n");
                }
            }
            TraceKind::Event {
                ordinal,
                active,
                following_active,
                symbols_added,
                include_target,
            } => {
                let Some((range, event)) = events.get(*ordinal) else { continue };
                let start = base_db::range_start(*range);
                let end = base_db::range_end(*range);
                let raw = &text[start..end];
                result.expansion.push_str(if *active { "// Active: " } else { "// Inactive: " });
                // Keep every line commented; source text is never interpreted as
                // transcript metadata or an executable command.
                let label = raw.replace('\n', "\n// ");
                append(&mut result, step.source, start, end, &label, "directive");
                result.expansion.push('\n');
                if matches!(
                    event,
                    Event::Ifdef(..) | Event::IfTarget(_) | Event::Else | Event::Endif
                ) {
                    result.expansion.push_str(if *following_active {
                        "// Following region: active\n"
                    } else {
                        "// Following region: inactive\n"
                    });
                }
                if let Some(target) = include_target {
                    result.expansion.push_str(&match target {
                        Ok(id) => format!("// Include resolved: {}\n", file_name(*id)),
                        Err(path) => format!("// Include missing: {}\n", path.display()),
                    });
                }
                if !symbols_added.is_empty() {
                    result.expansion.push_str(&format!(
                        "// Symbols added after directive: {}\n",
                        symbols_added.join(", ")
                    ));
                }
                if *active && matches!(event, Event::Order(_) | Event::Other) {
                    append(&mut result, step.source, start, end, raw, "source");
                    result.expansion.push('\n');
                }
            }
            TraceKind::Cycle => result
                .expansion
                .push_str(&format!("// Include cycle/depth limit: {}\n", file_name(step.source))),
            TraceKind::Exit => {
                result.expansion.push_str(&format!("\n// Leave {}\n", file_name(step.source)))
            }
        }
    }
    result
}

fn append(
    result: &mut ExpandIncludeResult,
    file: FileId,
    start: usize,
    end: usize,
    text: &str,
    kind: &'static str,
) {
    let generated = result.expansion.len();
    result.expansion.push_str(text);
    result.mappings.push(ExpansionMapping {
        generated: base_db::text_range(generated, result.expansion.len()),
        original: hir::FileRange { file_id: file, range: base_db::text_range(start, end) },
        kind,
    });
}
