use crate::state::File;
use crate::symbols::{builtin_docs, extract_comments, function_snippet, Parameter};
use std::collections::{BTreeMap, HashMap};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Url};

#[derive(Clone)]
struct CompletionCandidate {
    kind: CompletionItemKind,
    detail: Option<String>,
    snippet: Option<String>,
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'?' | b'\'' | b'~')
}

pub(crate) fn completion_prefix(text: &str, offset: usize) -> &str {
    let offset = offset.min(text.len());
    let bytes = text.as_bytes();
    let mut start = offset;

    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }

    &text[start..offset]
}

pub(crate) fn completion_trigger_characters() -> Vec<String> {
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

fn completion_kind_priority(kind: &CompletionItemKind) -> u8 {
    match kind {
        &CompletionItemKind::KEYWORD => 8,
        &CompletionItemKind::FUNCTION => 7,
        &CompletionItemKind::METHOD => 6,
        &CompletionItemKind::ENUM => 5,
        &CompletionItemKind::CLASS => 4,
        &CompletionItemKind::CONSTANT => 3,
        &CompletionItemKind::TYPE_PARAMETER => 2,
        &CompletionItemKind::VARIABLE => 1,
        _ => 0,
    }
}

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

pub(crate) fn build_completion_items<'a, I>(
    files: I,
    current_uri: &Url,
    text: &str,
    offset: usize,
    prefix: &str,
    keywords: &[&str],
    builtins: &[&str],
) -> Vec<CompletionItem>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let all_files = files.into_iter().collect::<Vec<_>>();
    let prefix_lower = prefix.to_ascii_lowercase();

    // RA-style: Context detection
    let is_top_level = {
        let mut depth = 0i32;
        let mut i = 0usize;
        let bytes = text.as_bytes();
        while i < offset {
            match bytes[i] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        depth <= 0
    };

    let mut candidates: BTreeMap<String, CompletionCandidate> = BTreeMap::new();
    let mut call_signatures: HashMap<String, Vec<Parameter>> = HashMap::new();
    for (_, candidate_file) in &all_files {
        for sig in candidate_file.signature_index.values() {
            call_signatures
                .entry(sig.name.clone())
                .or_insert_with(|| sig.params.clone());
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
                "foreach" if !is_top_level => Some("foreach (${1:i} from ${2:0} to ${3:n}) {\n\t$0\n}".to_string()),
                "if" if !is_top_level => Some("if ${1:condition} then {\n\t$0\n}".to_string()),
                "match" if !is_top_level => Some("match ${1:x} {\n\t${2:case} => $0\n}".to_string()),
                "while" if !is_top_level => Some("while ${1:condition} do {\n\t$0\n}".to_string()),
                "let" if !is_top_level => Some("let ${1:x} = $0".to_string()),
                "var" if !is_top_level => Some("var ${1:x} = $0".to_string()),
                _ => None,
            };
            upsert_candidate(
                &mut candidates,
                (*keyword).to_string(),
                CompletionCandidate {
                    kind: CompletionItemKind::KEYWORD,
                    detail: Some("keyword".to_string()),
                    snippet,
                },
            );
        }
    }

    for builtin in builtins {
        let kind = if builtin
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_uppercase())
        {
            CompletionItemKind::CLASS
        } else {
            CompletionItemKind::CONSTANT
        };
        upsert_candidate(
            &mut candidates,
            (*builtin).to_string(),
            CompletionCandidate {
                kind,
                detail: Some("builtin".to_string()),
                snippet: None,
            },
        );
    }

    for (candidate_uri, candidate_file) in &all_files {
        if let Some(parsed) = candidate_file.parsed() {
            for decl in &parsed.decls {
                if decl.scope != sail_parser::Scope::TopLevel {
                    continue;
                }
                let (name, kind, detail) = match decl.kind {
                    sail_parser::DeclKind::Function => (
                        decl.name.clone(),
                        CompletionItemKind::FUNCTION,
                        Some("function".to_string()),
                    ),
                    sail_parser::DeclKind::Value => (
                        decl.name.clone(),
                        CompletionItemKind::FUNCTION,
                        Some("value specification".to_string()),
                    ),
                    sail_parser::DeclKind::Mapping => (
                        decl.name.clone(),
                        CompletionItemKind::FUNCTION,
                        Some("mapping".to_string()),
                    ),
                    sail_parser::DeclKind::Overload => (
                        decl.name.clone(),
                        CompletionItemKind::FUNCTION,
                        Some("overload".to_string()),
                    ),
                    sail_parser::DeclKind::Type
                    | sail_parser::DeclKind::Struct
                    | sail_parser::DeclKind::Union
                    | sail_parser::DeclKind::Bitfield
                    | sail_parser::DeclKind::Newtype => (
                        decl.name.clone(),
                        CompletionItemKind::CLASS,
                        Some("type".to_string()),
                    ),
                    sail_parser::DeclKind::Enum => (
                        decl.name.clone(),
                        CompletionItemKind::ENUM,
                        Some("enum".to_string()),
                    ),
                    sail_parser::DeclKind::Register => (
                        decl.name.clone(),
                        CompletionItemKind::VARIABLE,
                        Some("register".to_string()),
                    ),
                    sail_parser::DeclKind::Parameter => continue,
                    _ => continue,
                };
                let snippet = if matches!(
                    kind,
                    CompletionItemKind::FUNCTION | CompletionItemKind::METHOD
                ) {
                    call_signatures
                        .get(&name)
                        .map(|params| function_snippet(&name, params))
                } else {
                    None
                };
                upsert_candidate(
                    &mut candidates,
                    name,
                    CompletionCandidate {
                        kind,
                        detail,
                        snippet,
                    },
                );
            }
            if *candidate_uri == current_uri {
                for occurrence in &parsed.symbol_occurrences {
                    if occurrence.role.is_none()
                        || occurrence.scope != Some(sail_parser::Scope::Local)
                    {
                        continue;
                    }
                    match occurrence.kind {
                        sail_parser::SymbolOccurrenceKind::Value => {
                            upsert_candidate(
                                &mut candidates,
                                occurrence.name.clone(),
                                CompletionCandidate {
                                    kind: CompletionItemKind::VARIABLE,
                                    detail: Some("binding".to_string()),
                                    snippet: None,
                                },
                            );
                        }
                        sail_parser::SymbolOccurrenceKind::TypeVar => {
                            upsert_candidate(
                                &mut candidates,
                                occurrence.name.clone(),
                                CompletionCandidate {
                                    kind: CompletionItemKind::TYPE_PARAMETER,
                                    detail: Some("type parameter".to_string()),
                                    snippet: None,
                                },
                            );
                        }
                        sail_parser::SymbolOccurrenceKind::Type => {}
                    }
                }
            }
        }
    }

    let mut items = candidates
        .into_iter()
        .filter_map(|(label, candidate)| {
            let label_lower = label.to_ascii_lowercase();
            let score = completion_score(&label_lower, &prefix_lower);
            if score >= 2 {
                return None;
            }

            Some((score, completion_kind_priority(&candidate.kind), {
                let insert_text_format = if candidate.snippet.is_some() {
                    InsertTextFormat::SNIPPET
                } else {
                    InsertTextFormat::PLAIN_TEXT
                };
                let detail = candidate
                    .detail
                    .clone()
                    .unwrap_or_else(|| "symbol".to_string());
                let kind_name = format!("{:?}", candidate.kind);
                CompletionItem {
                    label: label.clone(),
                    kind: Some(candidate.kind),
                    detail: candidate.detail,
                    filter_text: Some(label.clone()),
                    insert_text: Some(candidate.snippet.unwrap_or(label)),
                    insert_text_format: Some(insert_text_format),
                    data: Some(serde_json::json!({
                        "source": "sail-lsp",
                        "kind": kind_name,
                        "detail": detail,
                    })),
                    ..CompletionItem::default()
                }
            }))
        })
        .collect::<Vec<_>>();

    items.sort_by(
        |(score_a, priority_a, item_a), (score_b, priority_b, item_b)| {
            score_a
                .cmp(score_b)
                .then_with(|| priority_b.cmp(priority_a))
                .then_with(|| item_a.label.cmp(&item_b.label))
        },
    );

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

/// Generate postfix completions (e.g. `expr.if` → `if expr then { }`)
pub(crate) fn postfix_completions(
    text: &str,
    offset: usize,
    prefix: &str,
) -> Vec<CompletionItem> {
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
    let receiver_range_start = dot_pos - receiver.len();

    let postfix_templates: &[(&str, &str, &str)] = &[
        ("if", "if {} then {{\n\t$0\n}}", "Wrap in if-then"),
        ("match", "match {} {{\n\t${{1:_}} => $0\n}}", "Wrap in match"),
        ("let", "let ${{1:x}} = {}", "Bind to let"),
        ("not", "~({})", "Negate expression"),
        ("return", "return {}", "Return expression"),
        ("dbg", "/* DBG */ {}", "Debug wrapper"),
        (
            "foreach",
            "foreach (${{1:i}} from ${{2:0}} to {}) {{\n\t$0\n}}",
            "Wrap in foreach",
        ),
        ("assert", "assert({}, ${{1:\"assertion failed\"}})", "Wrap in assert"),
    ];

    let mut items = Vec::new();
    for (trigger, template, detail) in postfix_templates {
        if !trigger.starts_with(&prefix_lower) && !prefix_lower.is_empty() {
            continue;
        }

        let snippet = template.replace("{}", &receiver);

        items.push(CompletionItem {
            label: format!(".{trigger}"),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(detail.to_string()),
            filter_text: Some(trigger.to_string()),
            insert_text: Some(snippet),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            // The edit should replace from the receiver start through the current position
            additional_text_edits: Some(vec![tower_lsp::lsp_types::TextEdit {
                range: tower_lsp::lsp_types::Range::new(
                    tower_lsp::lsp_types::Position::new(0, 0), // placeholder
                    tower_lsp::lsp_types::Position::new(0, 0),
                ),
                new_text: String::new(),
            }]),
            data: Some(serde_json::json!({
                "source": "sail-lsp",
                "kind": "postfix",
                "detail": detail,
                "receiver_start": receiver_range_start,
                "dot_pos": dot_pos,
            })),
            sort_text: Some(format!("0000_{trigger}")),
            ..CompletionItem::default()
        });
    }
    items
}

/// Extract the receiver expression text before a dot.
fn extract_receiver_expr(text: &str, dot_pos: usize) -> &str {
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

/// Pragma name completion: triggered when cursor is right after `@` or `$`.
pub(crate) fn pragma_completions(text: &str, offset: usize) -> Vec<CompletionItem> {
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

    crate::diagnostics::semantic::KNOWN_PRAGMAS
        .iter()
        .filter(|name| name.starts_with(prefix))
        .map(|name| CompletionItem {
            label: format!("{}{name}", trigger as char),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("Sail pragma".to_string()),
            filter_text: Some(name.to_string()),
            insert_text: Some(name.to_string()),
            insert_text_format: Some(InsertTextFormat::PLAIN_TEXT),
            sort_text: Some(format!("aaaa_{name}")),
            data: Some(serde_json::json!({
                "source": "sail-lsp",
                "kind": "pragma",
            })),
            ..CompletionItem::default()
        })
        .collect()
}

/// Built-in Sail code snippet templates.
pub(crate) fn snippet_completions(prefix: &str, is_top_level: bool) -> Vec<CompletionItem> {
    let prefix_lower = prefix.to_ascii_lowercase();
    let snippets: &[(&str, &str, &str, bool)] = &[
        // (trigger, snippet, detail, top_level_only)
        (
            "funcdecl",
            "val ${1:name} : (${2:args}) -> ${3:result}\nfunction ${1:name}(${4:params}) = {\n\t$0\n}",
            "Function declaration + definition",
            true,
        ),
        (
            "scattered",
            "scattered function ${1:name}\n\nclause ${1:name}(${2:pat}) = $0\n\nend ${1:name}",
            "Scattered function definition",
            true,
        ),
        (
            "enumdef",
            "enum ${1:Name} = {\n\t${2:A},\n\t${3:B}\n}",
            "Enum definition",
            true,
        ),
        (
            "structdef",
            "struct ${1:Name} = {\n\t${2:field} : ${3:type}\n}",
            "Struct definition",
            true,
        ),
        (
            "uniondef",
            "union ${1:Name} = {\n\t${2:Variant} : ${3:type}\n}",
            "Union definition",
            true,
        ),
        (
            "bitfield",
            "bitfield ${1:Name} : bits(${2:32}) = {\n\t${3:field} : ${4:7 .. 0}\n}",
            "Bitfield definition",
            true,
        ),
        (
            "register",
            "register ${1:name} : ${2:type}",
            "Register definition",
            true,
        ),
        (
            "mapping",
            "mapping ${1:name} : ${2:type1} <-> ${3:type2} = {\n\t${4:pat1} <-> ${5:pat2}\n}",
            "Mapping definition",
            true,
        ),
        (
            "tryc",
            "try {\n\t$0\n} catch {\n\t${1:_} => ()\n}",
            "Try-catch block",
            false,
        ),
        (
            "matchopt",
            "match ${1:x} {\n\tSome(${2:v}) => $3,\n\tNone() => $0\n}",
            "Match on option",
            false,
        ),
    ];

    let mut items = Vec::new();
    for (trigger, snippet, detail, top_only) in snippets {
        if *top_only && !is_top_level {
            continue;
        }
        if !top_only && is_top_level {
            continue;
        }
        if !prefix_lower.is_empty() && !trigger.starts_with(&prefix_lower) {
            continue;
        }
        items.push(CompletionItem {
            label: trigger.to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(detail.to_string()),
            filter_text: Some(trigger.to_string()),
            insert_text: Some(snippet.to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            sort_text: Some(format!("zzzz_{trigger}")),
            data: Some(serde_json::json!({
                "source": "sail-lsp",
                "kind": "snippet",
                "detail": detail,
            })),
            ..CompletionItem::default()
        });
    }
    items
}

pub(crate) fn resolve_completion_item<'a, I>(mut item: CompletionItem, files: I) -> CompletionItem
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let all_files = files.into_iter().collect::<Vec<_>>();

    if let Some(doc) = builtin_docs(&item.label) {
        item.documentation = Some(tower_lsp::lsp_types::Documentation::MarkupContent(
            tower_lsp::lsp_types::MarkupContent {
                kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                value: format!("`{}`\n\n{}", item.label, doc),
            },
        ));
        return item;
    }

    if let Some(data) = &item.data {
        let kind = data
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("SYMBOL");
        let name = &item.label;

        let mut markdown = Vec::new();

        // Try to find the declaration to get comments
        for (_, file) in all_files {
            if let Some(parsed) = file.parsed() {
                if let Some(decl) = parsed
                    .decls
                    .iter()
                    .find(|d| d.name == *name && d.scope == sail_parser::Scope::TopLevel)
                {
                    markdown.push(format!("**{kind}** **{name}**"));
                    if let Some(comments) = extract_comments(file.source.text(), decl.span.start) {
                        markdown.push("___".to_string());
                        markdown.push(comments);
                    }
                    break;
                }
            }
        }

        if markdown.is_empty() {
            let detail = data
                .get("detail")
                .and_then(|v| v.as_str())
                .unwrap_or("Sail symbol");
            markdown.push(format!("`{}`\n\n{}", item.label, detail));
        }

        item.documentation = Some(tower_lsp::lsp_types::Documentation::MarkupContent(
            tower_lsp::lsp_types::MarkupContent {
                kind: tower_lsp::lsp_types::MarkupKind::Markdown,
                value: markdown.join("\n\n"),
            },
        ));
    }
    item
}
