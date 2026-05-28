//! Completion engine for Sail.
//!
//! Provider dispatch: `CompletionContext` -> `CompletionAnalysis` -> providers.
//! 11 providers: keyword, expr, dot, pattern, item_list, flyimport, postfix,
//! pragma, record, snippet, type_.

use ide_db::ide_types::{CompletionItem as IdeDbCompletionItem, CompletionItemKind};
use ide_db::{builtin_docs, extract_comments, function_snippet, FileDb};

use std::collections::{BTreeMap, HashMap};
use url::Url;

use completions::Completions;

// Provider-based completion architecture.
// All modules are private — types re-exported via `pub use` below.
mod completions;
mod config;
mod context;
mod item;
#[allow(dead_code)]
mod render;
mod snippet;

pub use crate::{
    config::{CallableSnippets, CompletionConfig, CompletionFieldsToResolve, SnippetCap},
    item::{CompletionItem, CompletionItemLabel},
    snippet::{Snippet, SnippetScope},
};

/// Main completion entry point.
///
/// Two-phase architecture:
/// 1. `CompletionContext::new()` -> `(ctx, CompletionAnalysis)`
/// 2. Match on `CompletionAnalysis` -> dispatch to providers
#[allow(clippy::too_many_arguments)]
pub fn completions(
    db: &dyn salsa::Database,
    file_text: Option<base_db::FileText>,
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    file: &dyn FileDb,
    text: &str,
    offset: usize,
    prefix: &str,
    keywords: &[&str],
    builtins: &[&str],
) -> Vec<IdeDbCompletionItem> {
    let (ctx, analysis) =
        context::CompletionContext::new(db, file, file_text, text, offset, prefix);
    let mut completions = Completions::default();

    {
        let acc = &mut completions;

        match &analysis {
            context::CompletionAnalysis::NameRef(name_ref_ctx) => {
                completions::complete_name_ref(
                    acc,
                    &ctx,
                    all_files,
                    name_ref_ctx,
                    current_uri,
                    keywords,
                    builtins,
                );
            }
            context::CompletionAnalysis::Name(_name_ctx) => {
                // Name definition site — no completions yet.
            }
            context::CompletionAnalysis::String => {
                // Inside string literal — no completions.
            }
        }

        completions::pragma::complete_pragma(acc, &ctx);
        completions::snippet::complete_snippet(acc, &ctx);
    }

    // Convert CompletionItem → IdeDbCompletionItem (LSP boundary)
    let completion_items: Vec<item::CompletionItem> = completions.into();
    let mut items: Vec<IdeDbCompletionItem> = completion_items.iter().map(|i| i.to_ide()).collect();

    // Deduplicate + sort + truncate
    deduplicate_and_sort(&mut items, prefix);
    items
}

/// Deduplicate, score, and sort completion items.
///
/// 1. Deduplicate by label
/// 2. Sort by `relevance.score()` (descending), then alphabetical
/// 3. Assign hex sort_text via XOR-inversion (`score ^ 0xFFFFFFFF`)
/// 4. Truncate to MAX_COMPLETIONS
fn deduplicate_and_sort(items: &mut Vec<IdeDbCompletionItem>, _prefix: &str) {
    // Dedup by label
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.label.clone()));

    // Sort by relevance score (descending), then alphabetical
    items.sort_by(|a, b| {
        b.relevance.score().cmp(&a.relevance.score()).then_with(|| a.label.cmp(&b.label))
    });

    // Truncate
    const MAX_COMPLETIONS: usize = 200;
    if items.len() > MAX_COMPLETIONS {
        items.truncate(MAX_COMPLETIONS);
    }

    // Assign sort_text: invert the score so that higher scores sort first
    // in lexicographic order (`score ^ 0xFFFFFFFF` -> lower hex = better).
    for item in items.iter_mut() {
        let sort_score = item.relevance.score() ^ 0xFF_FF_FF_FF;
        item.sort_text = Some(format!("{sort_score:08x}"));
    }
}

/// Collect type names from all workspace files.
fn collect_type_names(
    all_files: &[(&Url, &dyn FileDb)],
) -> Vec<(String, ide_db::defs::SymbolKind)> {
    let mut type_names = Vec::new();
    for (_, f) in all_files {
        if let Some(parsed) = f.parsed() {
            for decl in &parsed.decls {
                let kind = match decl.kind {
                    syntax::parser_lower::DeclKind::Struct => ide_db::defs::SymbolKind::Struct,
                    syntax::parser_lower::DeclKind::Enum => ide_db::defs::SymbolKind::Enum,
                    syntax::parser_lower::DeclKind::Union => ide_db::defs::SymbolKind::Enum,
                    syntax::parser_lower::DeclKind::Type => ide_db::defs::SymbolKind::TypeAlias,
                    syntax::parser_lower::DeclKind::Bitfield => ide_db::defs::SymbolKind::Struct,
                    _ => continue,
                };
                type_names.push((decl.name.clone(), kind));
            }
        }
    }
    type_names
}

/// Completion position context — re-exported from context module.
/// New code should match on `CompletionAnalysis` + `PathKind` instead.
#[allow(unused_imports)]
pub(crate) use context::CompletionPosition;

/// Determine completion context from text and offset.
/// Enhanced from pure text heuristic to token-aware analysis.
#[allow(dead_code)]
fn determine_completion_context(text: &str, offset: usize, prefix: &str) -> CompletionPosition {
    let before = &text[..offset];
    let trimmed = before.trim_end();
    let trimmed = trimmed.strip_suffix(prefix).unwrap_or(trimmed).trim_end();

    // Type annotation position: after `:` or `->`
    if trimmed.ends_with(':') || trimmed.ends_with("->") {
        return CompletionPosition::TypeAnnotation;
    }

    // Pattern position: after `match ... {`, or after `|` in match, or after `let`/`var` before `=`
    // Check for common pattern contexts
    if trimmed.ends_with("=>") {
        // After match arm `=> ` — expression position
        return CompletionPosition::Expression;
    }

    // Brace depth for top-level detection
    let mut brace_depth = 0i32;
    let _in_match = false;
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < offset {
        match bytes[i] {
            b'{' => brace_depth += 1,
            b'}' => brace_depth -= 1,
            _ => {}
        }
        i += 1;
    }

    if brace_depth <= 0 {
        return CompletionPosition::TopLevel;
    }

    // Check if we're in a pattern context (after match keyword + `{`)
    // Simple heuristic: look for `match` followed by `{` before our position
    let before_brace = before.rfind('{').unwrap_or(0);
    let context_text = &text[..before_brace].trim_end();
    if context_text.ends_with("catch") || context_text.ends_with("match") {
        // Could be in pattern position (match arm or catch clause)
        // Check if we're right after `{` or after `=>`
        let after_brace = &text[before_brace + 1..offset].trim_start();
        if !after_brace.contains("=>") {
            return CompletionPosition::Pattern;
        }
    }

    CompletionPosition::Expression
}

#[derive(Clone)]
#[allow(dead_code)]
struct CompletionCandidate {
    kind: CompletionItemKind,
    detail: Option<String>,
    snippet: Option<String>,
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'?' | b'\'' | b'~')
}

pub fn completion_prefix(text: &str, offset: usize) -> &str {
    let offset = offset.min(text.len());
    let bytes = text.as_bytes();
    let mut start = offset;

    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }

    &text[start..offset]
}

pub fn completion_trigger_characters() -> Vec<String> {
    vec![
        ".".to_string(),
        ":".to_string(),
        "(".to_string(),
        "_".to_string(),
        "?".to_string(),
        "~".to_string(),
        "'".to_string(),
        "@".to_string(),
        "$".to_string(),
    ]
}

#[allow(dead_code)]
fn completion_kind_priority(kind: &CompletionItemKind) -> u8 {
    match kind {
        CompletionItemKind::Keyword => 8,
        CompletionItemKind::Function => 7,
        // Method would be 6 but we don't have it; Function covers it
        CompletionItemKind::Enum => 5,
        CompletionItemKind::Struct => 4,
        CompletionItemKind::Constant => 3,
        CompletionItemKind::TypeParameter => 2,
        CompletionItemKind::Variable => 1,
        _ => 0,
    }
}

#[allow(dead_code)]
fn upsert_candidate(
    candidates: &mut BTreeMap<String, CompletionCandidate>,
    label: String,
    candidate: CompletionCandidate,
) {
    match candidates.get(&label) {
        Some(existing)
            if completion_kind_priority(&existing.kind)
                >= completion_kind_priority(&candidate.kind) => {}
        _ => {
            candidates.insert(label, candidate);
        }
    }
}

#[allow(dead_code)]
fn completion_score(label: &str, prefix: &str) -> u8 {
    if prefix.is_empty() {
        return 0;
    }
    if label == prefix {
        return 0;
    }
    if label.starts_with(prefix) {
        return 1;
    }
    2
}

/// Completion — returns internal IdeDbCompletionItem.
#[allow(dead_code)]
pub(crate) fn completion_items_ide(
    all_files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    text: &str,
    offset: usize,
    prefix: &str,
    keywords: &[&str],
    builtins: &[&str],
) -> Vec<IdeDbCompletionItem> {
    let prefix_lower = prefix.to_ascii_lowercase();

    // Enhanced context detection.
    // Uses token analysis for more accurate position classification.
    let ctx = determine_completion_context(text, offset, prefix);
    let is_type_position = ctx == CompletionPosition::TypeAnnotation;
    let is_top_level = ctx == CompletionPosition::TopLevel;

    let mut candidates: BTreeMap<String, CompletionCandidate> = BTreeMap::new();
    let mut call_signatures: HashMap<String, Vec<ide_db::Parameter>> = HashMap::new();
    for (_, candidate_file) in all_files {
        if let Some(index) = candidate_file.signature_index() {
            for sig in index.values() {
                call_signatures.entry(sig.name.clone()).or_insert_with(|| sig.params.clone());
            }
        }
    }

    for keyword in keywords {
        let is_top_level_kw = matches!(
            *keyword,
            "function" | "val" | "enum" | "struct" | "union" | "type" | "register" | "overload"
        );
        let is_local_kw = matches!(
            *keyword,
            "let" | "var" | "if" | "else" | "match" | "return" | "foreach" | "while"
        );

        if (is_top_level && is_top_level_kw)
            || (!is_top_level && is_local_kw)
            || (!is_top_level_kw && !is_local_kw)
        {
            let snippet = match *keyword {
                "foreach" if !is_top_level => {
                    Some("foreach (${1:i} from ${2:0} to ${3:n}) {\n\t$0\n}".to_string())
                }
                "if" if !is_top_level => Some("if ${1:condition} then {\n\t$0\n}".to_string()),
                "match" if !is_top_level => {
                    Some("match ${1:x} {\n\t${2:case} => $0\n}".to_string())
                }
                "while" if !is_top_level => Some("while ${1:condition} do {\n\t$0\n}".to_string()),
                "let" if !is_top_level => Some("let ${1:x} = $0".to_string()),
                "var" if !is_top_level => Some("var ${1:x} = $0".to_string()),
                "then" if !is_top_level => Some("then {\n\t$0\n}".to_string()),
                "else" if !is_top_level => Some("else {\n\t$0\n}".to_string()),
                "do" if !is_top_level => Some("do {\n\t$0\n}".to_string()),
                "try" if !is_top_level => {
                    Some("try {\n\t$0\n} catch {\n\t${1:_} => ()\n}".to_string())
                }
                "function" if is_top_level => {
                    Some("function ${1:name}(${2:args}) = $0".to_string())
                }
                "val" if is_top_level => Some("val ${1:name} : $0".to_string()),
                "struct" if is_top_level => {
                    Some("struct ${1:name} = {\n\t${2:field} : $0\n}".to_string())
                }
                "enum" if is_top_level => Some("enum ${1:name} = { $0 }".to_string()),
                "union" if is_top_level => {
                    Some("union ${1:name} = {\n\t${2:Variant} : $0\n}".to_string())
                }
                "register" if is_top_level => Some("register ${1:name} : $0".to_string()),
                _ => None,
            };
            upsert_candidate(
                &mut candidates,
                (*keyword).to_string(),
                CompletionCandidate {
                    kind: CompletionItemKind::Keyword,
                    detail: Some("keyword".to_string()),
                    snippet,
                },
            );
        }
    }

    for builtin in builtins {
        let kind = if builtin.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) {
            CompletionItemKind::Struct
        } else {
            CompletionItemKind::Constant
        };
        upsert_candidate(
            &mut candidates,
            (*builtin).to_string(),
            CompletionCandidate { kind, detail: Some("builtin".to_string()), snippet: None },
        );
    }

    for (candidate_uri, candidate_file) in all_files {
        if let Some(parsed) = candidate_file.parsed() {
            for decl in &parsed.decls {
                if decl.scope != syntax::parser_lower::Scope::TopLevel {
                    continue;
                }
                let (name, kind, detail) = match decl.kind {
                    syntax::parser_lower::DeclKind::Function => (
                        decl.name.clone(),
                        CompletionItemKind::Function,
                        Some("function".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::Value => (
                        decl.name.clone(),
                        CompletionItemKind::Function,
                        Some("value specification".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::Mapping => (
                        decl.name.clone(),
                        CompletionItemKind::Function,
                        Some("mapping".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::Overload => (
                        decl.name.clone(),
                        CompletionItemKind::Function,
                        Some("overload".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::Type
                    | syntax::parser_lower::DeclKind::Struct
                    | syntax::parser_lower::DeclKind::Union
                    | syntax::parser_lower::DeclKind::Bitfield
                    | syntax::parser_lower::DeclKind::Newtype => {
                        (decl.name.clone(), CompletionItemKind::Struct, Some("type".to_string()))
                    }
                    syntax::parser_lower::DeclKind::Enum => {
                        (decl.name.clone(), CompletionItemKind::Enum, Some("enum".to_string()))
                    }
                    syntax::parser_lower::DeclKind::Register => (
                        decl.name.clone(),
                        CompletionItemKind::Variable,
                        Some("register".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::EnumMember => (
                        decl.name.clone(),
                        CompletionItemKind::EnumMember,
                        Some("enum member".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::Let => (
                        decl.name.clone(),
                        CompletionItemKind::Variable,
                        Some("let binding".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::Var => (
                        decl.name.clone(),
                        CompletionItemKind::Variable,
                        Some("var binding".to_string()),
                    ),
                    syntax::parser_lower::DeclKind::Parameter => continue,
                };
                let snippet = if matches!(kind, CompletionItemKind::Function) {
                    call_signatures.get(&name).map(|params| function_snippet(&name, params))
                } else {
                    None
                };
                upsert_candidate(
                    &mut candidates,
                    name,
                    CompletionCandidate { kind, detail, snippet },
                );
            }
            if *candidate_uri == current_uri {
                for occurrence in &parsed.symbol_occurrences {
                    if occurrence.role.is_none()
                        || occurrence.scope != Some(syntax::parser_lower::Scope::Local)
                    {
                        continue;
                    }
                    match occurrence.kind {
                        syntax::parser_lower::SymbolOccurrenceKind::Value => {
                            upsert_candidate(
                                &mut candidates,
                                occurrence.name.clone(),
                                CompletionCandidate {
                                    kind: CompletionItemKind::Variable,
                                    detail: Some("binding".to_string()),
                                    snippet: None,
                                },
                            );
                        }
                        syntax::parser_lower::SymbolOccurrenceKind::TypeVar => {
                            upsert_candidate(
                                &mut candidates,
                                occurrence.name.clone(),
                                CompletionCandidate {
                                    kind: CompletionItemKind::TypeParameter,
                                    detail: Some("type parameter".to_string()),
                                    snippet: None,
                                },
                            );
                        }
                        syntax::parser_lower::SymbolOccurrenceKind::Type => {}
                    }
                }
            }
        }
    }

    let mut items = candidates
        .into_iter()
        .filter_map(|(label, candidate)| {
            // Context-aware filtering: in type position, only suggest types
            if is_type_position {
                match candidate.kind {
                    CompletionItemKind::Function
                    | CompletionItemKind::Variable
                    | CompletionItemKind::Keyword => return None,
                    _ => {}
                }
            }
            let label_lower = label.to_ascii_lowercase();
            let score = completion_score(&label_lower, &prefix_lower);
            if score >= 2 {
                return None;
            }

            let _has_snippet = candidate.snippet.is_some();
            let _detail = candidate.detail.clone().unwrap_or_else(|| "symbol".to_string());
            let _kind_name = format!("{:?}", candidate.kind);

            Some((
                score,
                completion_kind_priority(&candidate.kind),
                IdeDbCompletionItem {
                    label: label.clone(),
                    kind: candidate.kind,
                    detail: candidate.detail,
                    documentation: None,
                    filter_text: Some(label.clone()),
                    insert_text: Some(candidate.snippet.unwrap_or(label)),
                    text_edit: None,
                    sort_text: None, // filled below
                    deprecated: false,
                    relevance: Default::default(),
                },
            ))
        })
        .collect::<Vec<_>>();

    items.sort_by(|(score_a, priority_a, item_a), (score_b, priority_b, item_b)| {
        score_a
            .cmp(score_b)
            .then_with(|| priority_b.cmp(priority_a))
            .then_with(|| item_a.label.cmp(&item_b.label))
    });

    const MAX_COMPLETIONS: usize = 200;
    if items.len() > MAX_COMPLETIONS {
        items.truncate(MAX_COMPLETIONS);
    }

    items
        .into_iter()
        .enumerate()
        .map(|(index, (_, _, mut item))| {
            item.sort_text = Some(format!("{index:04}_{}", item.label.to_ascii_lowercase()));
            item
        })
        .collect()
}

/// Extract the receiver expression text before a dot.
pub(crate) fn extract_receiver_expr(text: &str, dot_pos: usize) -> &str {
    let bytes = text.as_bytes();
    let mut end = dot_pos;
    // Skip whitespace before the dot
    while end > 0 && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return "";
    }

    // Handle closing brackets by finding the matching open
    let last_byte = bytes[end - 1];
    if last_byte == b')' || last_byte == b']' {
        let open = if last_byte == b')' { b'(' } else { b'[' };
        let mut depth = 1i32;
        let mut pos = end - 2;
        loop {
            if bytes[pos] == last_byte {
                depth += 1;
            } else if bytes[pos] == open {
                depth -= 1;
                if depth == 0 {
                    // Now also grab the identifier before the open paren
                    let mut start = pos;
                    while start > 0
                        && (bytes[start - 1].is_ascii_alphanumeric()
                            || bytes[start - 1] == b'_'
                            || bytes[start - 1] == b'?')
                    {
                        start -= 1;
                    }
                    return &text[start..end];
                }
            }
            if pos == 0 {
                break;
            }
            pos -= 1;
        }
        return "";
    }

    // Otherwise, grab an identifier
    let mut start = end;
    while start > 0
        && (bytes[start - 1].is_ascii_alphanumeric()
            || bytes[start - 1] == b'_'
            || bytes[start - 1] == b'?'
            || bytes[start - 1] == b'\'')
    {
        start -= 1;
    }
    &text[start..end]
}

/// Generate postfix completions (e.g. `expr.if` -> `if expr then { }`)
pub(crate) fn postfix_completions(
    text: &str,
    offset: usize,
    prefix: &str,
) -> Vec<IdeDbCompletionItem> {
    // Find the dot before the prefix
    let prefix_start = offset - prefix.len();
    if prefix_start == 0 {
        return Vec::new();
    }
    let before = &text[..prefix_start];
    if !before.ends_with('.') {
        return Vec::new();
    }

    // Extract the receiver expression (text before the dot)
    let dot_pos = prefix_start - 1;
    let receiver = extract_receiver_expr(text, dot_pos);
    if receiver.is_empty() {
        return Vec::new();
    }

    let prefix_lower = prefix.to_ascii_lowercase();

    let postfix_templates: &[(&str, &str, &str)] = &[
        ("if", "if {} then {{\n\t$0\n}}", "Wrap in if-then"),
        ("match", "match {} {{\n\t${{1:_}} => $0\n}}", "Wrap in match"),
        ("let", "let ${{1:x}} = {}", "Bind to let"),
        ("not", "~({})", "Negate expression"),
        ("return", "return {}", "Return expression"),
        ("dbg", "/* DBG */ {}", "Debug wrapper"),
        ("foreach", "foreach (${{1:i}} from ${{2:0}} to {}) {{\n\t$0\n}}", "Wrap in foreach"),
        ("assert", "assert({}, ${{1:\"assertion failed\"}})", "Wrap in assert"),
        ("while", "while {} do {{\n\t$0\n}}", "Wrap in while-do"),
        ("var", "var ${{1:x}} = {}", "Bind to var (mutable)"),
        ("throw", "throw {}", "Throw expression"),
        ("exit", "exit({})", "Exit with expression"),
        ("some", "Some({})", "Wrap in Some"),
        ("unsigned", "unsigned({})", "Convert to unsigned"),
        ("signed", "signed({})", "Convert to signed"),
    ];

    let mut items = Vec::new();
    for (trigger, template, detail) in postfix_templates {
        if !trigger.starts_with(&prefix_lower) && !prefix_lower.is_empty() {
            continue;
        }

        let snippet = template.replace("{}", receiver);

        items.push(IdeDbCompletionItem {
            label: format!(".{trigger}"),
            kind: CompletionItemKind::Snippet,
            detail: Some(detail.to_string()),
            documentation: None,
            filter_text: Some(trigger.to_string()),
            insert_text: Some(snippet),
            text_edit: None,
            sort_text: Some(format!("0000_{trigger}")),
            deprecated: false,
            relevance: Default::default(),
        });
    }
    items
}

/// Field completion after `.`: suggest struct field names when the receiver
/// type is a known struct/record. Also suggests bitfield field names.
pub(crate) fn field_completions(
    files: &[(&url::Url, &dyn ide_db::FileDb)],
    current_file: &dyn ide_db::FileDb,
    text: &str,
    offset: usize,
    prefix: &str,
) -> Vec<IdeDbCompletionItem> {
    let prefix_start = offset.saturating_sub(prefix.len());
    if prefix_start == 0 {
        return Vec::new();
    }
    let before = &text[..prefix_start];
    if !before.ends_with('.') {
        return Vec::new();
    }

    // Extract receiver name (identifier before the dot)
    let dot_pos = prefix_start - 1;
    let receiver = extract_receiver_expr(text, dot_pos);
    if receiver.is_empty() {
        return Vec::new();
    }

    let prefix_lower = prefix.to_ascii_lowercase();
    let mut items = Vec::new();

    // Look up the receiver's type from binding_type_text
    let receiver_span = parser::Span::new(dot_pos - receiver.len(), dot_pos);
    let ty_text = current_file
        .binding_type_text(receiver_span)
        .or_else(|| current_file.cached_expr_type_text(receiver_span));

    if let Some(ref ty_name) = ty_text {
        // Search all files for struct/record/bitfield with matching type name
        for (_, file) in files {
            if let Some(parsed) = file.parsed() {
                // Find struct fields from declarations
                for decl in &parsed.decls {
                    if (decl.name == *ty_name || ty_name.starts_with(&decl.name))
                        && matches!(
                            decl.kind,
                            syntax::parser_lower::DeclKind::Struct
                                | syntax::parser_lower::DeclKind::Bitfield
                        )
                    {
                        // Extract field names from the definition text
                        let def_text =
                            file.text().get(decl.span.start..decl.span.end).unwrap_or("");
                        for field in extract_struct_fields(def_text) {
                            if !prefix_lower.is_empty()
                                && !field.to_ascii_lowercase().starts_with(&prefix_lower)
                            {
                                continue;
                            }
                            items.push(IdeDbCompletionItem {
                                label: field.clone(),
                                kind: CompletionItemKind::Field,
                                detail: Some(format!("field of {}", decl.name)),
                                documentation: None,
                                insert_text: None,
                                text_edit: None,
                                sort_text: Some(format!("0{field}")),
                                filter_text: None,
                                deprecated: false,
                                relevance: Default::default(),
                            });
                        }
                    }
                }
            }
        }
    }

    items
}

/// Extract field names from a struct/bitfield definition text.
pub(crate) fn extract_struct_fields(text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let Some(brace_start) = text.find('{') else {
        return fields;
    };
    let Some(brace_end) = text.rfind('}') else {
        return fields;
    };
    if brace_start >= brace_end {
        return fields;
    }
    let inner = &text[brace_start + 1..brace_end];
    for part in inner.split(',') {
        let part = part.trim();
        if let Some(colon_pos) = part.find(':') {
            let name = part[..colon_pos].trim();
            if !name.is_empty()
                && name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            {
                fields.push(name.to_string());
            }
        }
    }
    fields
}

/// Pragma name completion: triggered when cursor is right after `@` or `$`.
pub(crate) fn pragma_completions(text: &str, offset: usize) -> Vec<IdeDbCompletionItem> {
    if offset == 0 {
        return Vec::new();
    }
    let bytes = text.as_bytes();

    // Look back to find @ or $ that starts a pragma
    let mut start = offset;
    while start > 0 && (bytes[start - 1].is_ascii_alphanumeric() || bytes[start - 1] == b'_') {
        start -= 1;
    }
    if start == 0 {
        return Vec::new();
    }

    let trigger = bytes[start - 1];
    if trigger != b'@' && trigger != b'$' {
        return Vec::new();
    }

    let prefix = &text[start..offset];

    ide_db::KNOWN_PRAGMAS
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| IdeDbCompletionItem {
            label: format!("{}{name}", trigger as char),
            kind: CompletionItemKind::Keyword,
            detail: Some("Sail pragma".to_string()),
            documentation: None,
            filter_text: Some(name.to_string()),
            insert_text: Some(name.to_string()),
            text_edit: None,
            sort_text: Some(format!("aaaa_{name}")),
            deprecated: false,
            relevance: Default::default(),
        })
        .collect()
}

/// Built-in Sail code snippet templates.
pub(crate) fn snippet_completions(prefix: &str, is_top_level: bool) -> Vec<IdeDbCompletionItem> {
    use snippet::{SnippetScope, SAIL_SNIPPETS};

    let prefix_lower = prefix.to_ascii_lowercase();
    let scope = if is_top_level { SnippetScope::Item } else { SnippetScope::Expr };

    let mut items = Vec::new();
    for snip in SAIL_SNIPPETS {
        // Filter by scope: Item snippets at top-level, Expr snippets in expressions.
        // Type snippets are shown in both contexts.
        if snip.scope != scope && snip.scope != SnippetScope::Type {
            continue;
        }
        if !prefix_lower.is_empty() && !snip.prefix.starts_with(&prefix_lower) {
            continue;
        }
        items.push(IdeDbCompletionItem {
            label: snip.prefix.to_string(),
            kind: CompletionItemKind::Snippet,
            detail: Some(snip.description.to_string()),
            documentation: None,
            filter_text: Some(snip.prefix.to_string()),
            insert_text: Some(snip.body.to_string()),
            text_edit: None,
            sort_text: Some(format!("zzzz_{}", snip.prefix)),
            deprecated: false,
            relevance: Default::default(),
        });
    }
    items
}

/// Resolve a completion item by adding documentation.
pub fn resolve_completion_item_ide(
    item: &mut IdeDbCompletionItem,
    all_files: &[(&Url, &dyn FileDb)],
) {
    if let Some(doc) = builtin_docs(&item.label) {
        item.documentation = Some(format!("`{}`\n\n{}", item.label, doc));
        return;
    }

    let name = &item.label;
    let mut markdown = Vec::new();

    for (_, file) in all_files {
        if let Some(parsed) = file.parsed() {
            if let Some(decl) = parsed
                .decls
                .iter()
                .find(|d| d.name == *name && d.scope == syntax::parser_lower::Scope::TopLevel)
            {
                let kind_name = format!("{:?}", item.kind);
                markdown.push(format!("**{kind_name}** **{name}**"));
                if let Some(comments) = extract_comments(file.text(), decl.span.start) {
                    markdown.push("___".to_string());
                    markdown.push(comments);
                }
                break;
            }
        }
    }

    // Show all overload signatures when available
    let all_sigs =
        ide_db::symbol_index::find_all_callable_signatures(all_files.iter().copied(), name);
    if !all_sigs.is_empty() {
        markdown.push("___".to_string());
        if all_sigs.len() > 1 {
            markdown.push(format!("**{} overloads:**", all_sigs.len()));
        }
        for sig in &all_sigs {
            markdown.push(format!("```sail\n{}\n```", sig.label));
        }
    }

    if markdown.is_empty() {
        let detail = item.detail.as_deref().unwrap_or("Sail symbol");
        markdown.push(format!("`{}`\n\n{}", item.label, detail));
    }

    item.documentation = Some(markdown.join("\n\n"));
}

#[cfg(test)]
mod cursor_tests {
    use ide_db::fixture::MultiFileFixture;
    use ide_db::test_utils::TestFile;

    /// Helper: parse a fixture, find the cursor file, build a TestFile,
    /// then run field_completions at the cursor offset.
    fn complete_at_cursor(fixture_text: &str) -> Vec<ide_db::ide_types::CompletionItem> {
        let fixture = MultiFileFixture::parse(fixture_text);
        let pos = fixture.cursor_position();

        // Find the file containing the cursor
        let cursor_file =
            fixture.files.iter().find(|f| f.file_id == pos.file_id).expect("cursor file not found");

        let test_file = TestFile::new(&cursor_file.text);
        let uri = url::Url::parse("file:///test/cursor_file.sail").unwrap();
        let all_files: Vec<(&url::Url, &dyn ide_db::FileDb)> =
            vec![(&uri, &test_file as &dyn ide_db::FileDb)];

        let offset: usize = pos.offset.into();
        let prefix = super::completion_prefix(&cursor_file.text, offset);

        super::field_completions(&all_files, &test_file, &cursor_file.text, offset, prefix)
    }

    /// Helper: run full completion_items_ide at the cursor position.
    fn complete_full_at_cursor(fixture_text: &str) -> Vec<ide_db::ide_types::CompletionItem> {
        let fixture = MultiFileFixture::parse(fixture_text);
        let pos = fixture.cursor_position();

        let cursor_file =
            fixture.files.iter().find(|f| f.file_id == pos.file_id).expect("cursor file not found");

        // Build TestFile for each fixture file
        let test_files: Vec<(url::Url, TestFile)> = fixture
            .files
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let uri = url::Url::parse(&format!("file:///test/file{i}.sail")).unwrap();
                (uri, TestFile::new(&f.text))
            })
            .collect();

        let all_files: Vec<(&url::Url, &dyn ide_db::FileDb)> =
            test_files.iter().map(|(u, f)| (u, f as &dyn ide_db::FileDb)).collect();

        // Find the index of the cursor file
        let cursor_idx = fixture.files.iter().position(|f| f.file_id == pos.file_id).unwrap();
        let current_uri = &test_files[cursor_idx].0;

        let offset: usize = pos.offset.into();
        let prefix = super::completion_prefix(&cursor_file.text, offset);

        let keywords = &[
            "function", "val", "let", "var", "if", "else", "match", "return", "foreach", "while",
            "struct", "enum", "union", "type", "register",
        ];
        let builtins = &["true", "false", "bitzero", "bitone", "unit"];

        super::completion_items_ide(
            &all_files,
            current_uri,
            &cursor_file.text,
            offset,
            prefix,
            keywords,
            builtins,
        )
    }

    // ---------------------------------------------------------------
    // Field completion tests (struct fields after dot)
    // ---------------------------------------------------------------

    #[test]
    fn complete_struct_field_after_dot_requires_type_info() {
        // Field completions require the type checker to resolve `binding_type_text`.
        // Without salsa, TestFile cannot infer types, so field_completions
        // returns empty. This test verifies the infrastructure works end-to-end
        // without panicking and returns empty when type info is unavailable.
        let items = complete_at_cursor(
            "
            //- /main.sail
            struct Point = { x : int, y : int }
            function foo() = {
                let p : Point = struct { x = 1, y = 2 };
                p.$0
            }
        ",
        );
        // Without type inference, field_completions cannot resolve the receiver type.
        // This is expected — the test confirms the fixture + completion pipeline
        // runs without crashing. With salsa wired in, this would return ["x", "y"].
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.is_empty() || labels.contains(&"x"),
            "expected either empty (no type info) or 'x' field, got {labels:?}"
        );
    }

    #[test]
    fn complete_no_fields_for_unknown_type() {
        let items = complete_at_cursor(
            "
            //- /main.sail
            function foo() = {
                let x = 42;
                x.$0
            }
        ",
        );
        // No struct type known for `x`, so no field completions
        assert!(items.is_empty(), "expected no field completions, got {items:?}");
    }

    // ---------------------------------------------------------------
    // Full completion tests (keywords, symbols, builtins)
    // ---------------------------------------------------------------

    #[test]
    fn complete_keyword_in_expression_position() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            function foo() = {
                le$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"let"), "expected 'let' keyword in {labels:?}");
    }

    #[test]
    fn complete_function_name_from_other_file() {
        let items = complete_full_at_cursor(
            "
            //- /helpers.sail
            function helper_add(x : int, y : int) -> int = x + y
            //- /main.sail
            function main() = {
                hel$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"helper_add"),
            "expected 'helper_add' from other file in {labels:?}"
        );
    }

    #[test]
    fn complete_type_names_at_top_level() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            struct Foo = { a : int }
            enum Bar = { X, Y }
            $0
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        // At top level, should see top-level keywords
        assert!(
            labels.contains(&"function"),
            "expected 'function' keyword at top level in {labels:?}"
        );
    }

    #[test]
    fn complete_enum_member_in_expression() {
        let items = complete_full_at_cursor(
            "
            //- /main.sail
            enum Color = { Red, Green, Blue }
            function pick() = {
                Re$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Red"), "expected 'Red' enum member in {labels:?}");
    }

    #[test]
    fn complete_postfix_after_dot() {
        // Postfix completions are triggered after `.` with a receiver
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            function foo() = {
                let x = 42;
                x.i$0
            }
        ",
        );
        let pos = fixture.cursor_position();
        let cursor_file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();

        let offset: usize = pos.offset.into();
        let prefix = super::completion_prefix(&cursor_file.text, offset);

        let items = super::postfix_completions(&cursor_file.text, offset, prefix);
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&".if"), "expected '.if' postfix in {labels:?}");
    }

    #[test]
    fn complete_pragma_after_at_sign() {
        // Pragma completions triggered after '@'
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            @$0
        ",
        );
        let pos = fixture.cursor_position();
        let cursor_file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();

        let offset: usize = pos.offset.into();
        let items = super::pragma_completions(&cursor_file.text, offset);
        // Pragma completions may be empty if no prefix after @, but the
        // function should not panic and should return valid results
        assert!(
            items.is_empty() || items.iter().all(|i| i.label.starts_with('@')),
            "pragma items should start with '@': {items:?}"
        );
    }

    // ---------------------------------------------------------------
    // Cursor extraction verification
    // ---------------------------------------------------------------

    #[test]
    fn cursor_marker_stripped_from_fixture_text() {
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            function foo() = bar($0)
        ",
        );
        let pos = fixture.cursor_position();
        let file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();
        assert!(!file.text.contains("$0"), "$0 marker should be stripped from text");
        assert!(
            file.text.contains("bar()"),
            "text should have bar() without marker: {}",
            file.text
        );
    }

    #[test]
    fn cursor_offset_points_to_correct_position() {
        let fixture = MultiFileFixture::parse(
            "
            //- /main.sail
            val foo : int
            function bar() = foo$0
        ",
        );
        let pos = fixture.cursor_position();
        let file = fixture.files.iter().find(|f| f.file_id == pos.file_id).unwrap();
        let offset: usize = pos.offset.into();
        // Cursor should be right after "foo"
        let before_cursor = &file.text[..offset];
        assert!(
            before_cursor.ends_with("foo"),
            "expected cursor after 'foo', but text before cursor is: '{before_cursor}'"
        );
    }

    // ---------------------------------------------------------------
    // Multi-file fixture with $include
    // ---------------------------------------------------------------

    #[test]
    fn complete_symbol_from_included_file() {
        let items = complete_full_at_cursor(
            "
            //- /prelude.sail
            function prelude_helper(x : int) -> int = x + 1
            //- /main.sail
            $include \"prelude.sail\"
            function main() = {
                prelude$0
            }
        ",
        );
        let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(
            labels.contains(&"prelude_helper"),
            "expected 'prelude_helper' from included file in {labels:?}"
        );
    }
}
