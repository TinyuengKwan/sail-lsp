//! Versioned compiler output. Byte spans are decoded against exactly the bytes
//! hashed by the compiler; terminal text is never interpreted as structured data.
use super::FlycheckResult;
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub(crate) fn fingerprint(text: &str) -> String {
    let hash = text
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, b| (hash ^ u64::from(*b)).wrapping_mul(0x100000001b3));
    format!("{hash:016x}")
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SourceHash {
    algorithm: String,
    value: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Span {
    file: PathBuf,
    byte_start: usize,
    byte_end: usize,
    source_hash: SourceHash,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Related {
    span: Span,
    message: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Edit {
    span: Span,
    old_text: String,
    new_text: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Fix {
    title: String,
    edits: Vec<Edit>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Data {
    span: Span,
    fixes: Vec<Fix>,
}
#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "lowercase", deny_unknown_fields)]
enum Record {
    Diagnostic {
        protocol: String,
        version: u32,
        severity: String,
        code: String,
        message: String,
        span: serde_json::Value,
        related: Vec<Related>,
        fixes: Vec<Fix>,
    },
    Summary {
        protocol: String,
        version: u32,
        success: bool,
        diagnostics: usize,
    },
}
pub(crate) struct Report {
    pub found: FlycheckResult,
    pub unlocated: Vec<String>,
    pub success: bool,
    pub errors: usize,
}
#[derive(Debug)]
pub(crate) enum Error {
    Protocol(String),
    Stale(String),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Protocol(message) | Self::Stale(message) => f.write_str(message),
        }
    }
}
fn protocol(message: impl Into<String>) -> Error {
    Error::Protocol(message.into())
}
struct Source {
    text: String,
    hash: String,
    index: ide_db::line_index::LineIndex,
}
fn path(span: &Span) -> Result<PathBuf, Error> {
    if !span.file.is_absolute() {
        return Err(protocol("compiler span must use an absolute filesystem path"));
    }
    if span.source_hash.algorithm != "fnv1a64"
        || span.source_hash.value.len() != 16
        || !span.source_hash.value.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(protocol("unsupported or invalid compiler source hash"));
    }
    Ok(syntax::preprocess::normalize_path(&span.file))
}
fn validate_range(text: &str, hash: &str, span: &Span) -> Result<(), Error> {
    if hash != span.source_hash.value {
        return Err(Error::Stale(format!("Compiler source changed: {}", span.file.display())));
    }
    if span.byte_start > span.byte_end
        || span.byte_end > text.len()
        || !text.is_char_boundary(span.byte_start)
        || !text.is_char_boundary(span.byte_end)
    {
        return Err(protocol("compiler span is outside the source or splits a UTF-8 character"));
    }
    Ok(())
}
fn source<'a>(
    cache: &'a mut BTreeMap<PathBuf, Source>,
    span: &Span,
) -> Result<(&'a Source, PathBuf), Error> {
    let path = path(span)?;
    if !cache.contains_key(&path) {
        let text = std::fs::read_to_string(&path).map_err(|error| {
            Error::Stale(format!("Cannot read compiler source {}: {error}", path.display()))
        })?;
        let hash = fingerprint(&text);
        let index = ide_db::line_index::LineIndex::new(&text);
        cache.insert(path.clone(), Source { text, hash, index });
    }
    let source = &cache[&path];
    validate_range(&source.text, &source.hash, span)?;
    Ok((source, path))
}
fn range(index: &ide_db::line_index::LineIndex, span: &Span) -> lsp_types::Range {
    lsp_types::Range::new(
        crate::to_proto::position_in(index, span.byte_start),
        crate::to_proto::position_in(index, span.byte_end),
    )
}
fn nonoverlapping(edits: &[Edit]) -> Result<(), Error> {
    let mut ranges: BTreeMap<&Path, Vec<(usize, usize)>> = BTreeMap::new();
    for edit in edits {
        ranges.entry(&edit.span.file).or_default().push((edit.span.byte_start, edit.span.byte_end));
    }
    for ranges in ranges.values_mut() {
        ranges.sort_unstable();
        if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0 || pair[0].0 == pair[1].0) {
            return Err(protocol("compiler fix contains overlapping edits"));
        }
    }
    Ok(())
}
pub(crate) fn parse(stdout: &str, exit_success: bool) -> Result<Report, Error> {
    let mut cache = BTreeMap::new();
    let mut found = Vec::new();
    let mut unlocated = Vec::new();
    let mut count = 0;
    let mut errors = 0;
    let mut summary = None;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        if summary.is_some() {
            return Err(protocol("compiler sent data after the final summary"));
        }
        let record: Record = serde_json::from_str(line)
            .map_err(|error| protocol(format!("Invalid compiler diagnostic record: {error}")))?;
        let (protocol_name, version) = match &record {
            Record::Diagnostic { protocol, version, .. }
            | Record::Summary { protocol, version, .. } => (protocol, *version),
        };
        if protocol_name != "sail-editor-diagnostics" || version != 1 {
            return Err(protocol("unsupported compiler diagnostic protocol or version"));
        }
        match record {
            Record::Summary { success, diagnostics, .. } => {
                if diagnostics != count || success != exit_success || (success && errors > 0) {
                    return Err(protocol(
                        "compiler summary disagrees with its records or exit status",
                    ));
                }
                summary = Some(success);
            }
            Record::Diagnostic { severity, code, message, span, related, mut fixes, .. } => {
                if code.is_empty() {
                    return Err(protocol("compiler diagnostic code must not be empty"));
                }
                count += 1;
                let severity = match severity.as_str() {
                    "error" => {
                        errors += 1;
                        DiagnosticSeverity::ERROR
                    }
                    "warning" => DiagnosticSeverity::WARNING,
                    "information" => DiagnosticSeverity::INFORMATION,
                    "hint" => DiagnosticSeverity::HINT,
                    _ => return Err(protocol("invalid compiler diagnostic severity")),
                };
                if fixes.len() > 32 || related.len() > 128 {
                    return Err(protocol(
                        "compiler diagnostic exceeds the fix/related-location limit",
                    ));
                }
                let span: Option<Span> = serde_json::from_value(span)
                    .map_err(|error| protocol(format!("Invalid compiler span: {error}")))?;
                let Some(span) = span else {
                    if !fixes.is_empty() {
                        return Err(protocol("unlocated compiler diagnostics cannot carry fixes"));
                    }
                    unlocated.push(format!("{code}: {message}"));
                    continue;
                };
                let (src, file) = source(&mut cache, &span)?;
                let primary = range(&src.index, &span);
                let mut locations = Vec::new();
                for related in related {
                    let (src, path) = source(&mut cache, &related.span)?;
                    let uri = Url::from_file_path(path)
                        .map_err(|_| protocol("invalid related source path"))?;
                    locations.push(lsp_types::DiagnosticRelatedInformation {
                        location: lsp_types::Location::new(uri, range(&src.index, &related.span)),
                        message: related.message,
                    });
                }
                for fix in &mut fixes {
                    if fix.title.trim().is_empty() {
                        return Err(protocol("compiler fix title must not be empty"));
                    }
                    if fix.edits.is_empty() || fix.edits.len() > 128 {
                        return Err(protocol("compiler fix must contain 1..128 edits"));
                    }
                    for edit in &mut fix.edits {
                        let (src, path) = source(&mut cache, &edit.span)?;
                        if src.text[edit.span.byte_start..edit.span.byte_end] != edit.old_text {
                            return Err(protocol(
                                "compiler fix oldText disagrees with its source span",
                            ));
                        }
                        edit.span.file = path;
                    }
                    nonoverlapping(&fix.edits)?;
                }
                let mut span = span;
                span.file = file.clone();
                found.push((
                    file,
                    Diagnostic {
                        range: primary,
                        severity: Some(severity),
                        code: Some(NumberOrString::String(code)),
                        source: Some("sail".into()),
                        message,
                        related_information: (!locations.is_empty()).then_some(locations),
                        data: Some(serde_json::json!({"sailCompiler":Data{span,fixes}})),
                        ..Default::default()
                    },
                ));
            }
        }
    }
    let success =
        summary.ok_or_else(|| protocol("compiler diagnostic stream has no final summary"))?;
    Ok(Report { found, unlocated, success, errors })
}

pub(crate) fn matches_source(diagnostic: &Diagnostic, text: &str) -> bool {
    diagnostic.data.as_ref().and_then(|data| data.get("sailCompiler")).is_none_or(|data| {
        data.get("span")
            .and_then(|span| span.get("sourceHash"))
            .and_then(|hash| hash.get("value"))
            .and_then(|value| value.as_str())
            == Some(fingerprint(text).as_str())
    })
}

fn snapshot_text(snap: &crate::global_state::GlobalStateSnapshot, path: &Path) -> Option<String> {
    let uri = Url::from_file_path(path).ok()?;
    if snap.document_versions.contains_key(&uri) {
        if let Some(file) = snap.analysis.file(&uri) {
            use ide_db::SourceFileInfo;
            return Some(file.text().to_owned());
        }
    }
    if let Some(document) = snap.manifest_texts.get(&uri) {
        return Some(document.data.clone());
    }
    std::fs::read_to_string(path).ok()
}
fn snapshot_span(
    snap: &crate::global_state::GlobalStateSnapshot,
    cache: &mut BTreeMap<PathBuf, Source>,
    span: &Span,
) -> Option<(Url, lsp_types::Range)> {
    let path = path(span).ok()?;
    if !cache.contains_key(&path) {
        let text = snapshot_text(snap, &path)?;
        let hash = fingerprint(&text);
        let index = ide_db::line_index::LineIndex::new(&text);
        cache.insert(path.clone(), Source { text, hash, index });
    }
    let source = &cache[&path];
    validate_range(&source.text, &source.hash, span).ok()?;
    Some((Url::from_file_path(path).ok()?, range(&source.index, span)))
}
fn edit(
    snap: &crate::global_state::GlobalStateSnapshot,
    data: &Data,
    fix: &Fix,
) -> Option<crate::lsp_ext::SnippetWorkspaceEdit> {
    let mut cache = BTreeMap::new();
    snapshot_span(snap, &mut cache, &data.span)?;
    nonoverlapping(&fix.edits).ok()?;
    let mut changes: std::collections::HashMap<Url, Vec<lsp_types::TextEdit>> =
        std::collections::HashMap::new();
    for edit in &fix.edits {
        let (uri, range) = snapshot_span(snap, &mut cache, &edit.span)?;
        let source = &cache[&path(&edit.span).ok()?];
        if source.text[edit.span.byte_start..edit.span.byte_end] != edit.old_text {
            return None;
        }
        changes
            .entry(uri)
            .or_default()
            .push(lsp_types::TextEdit { range, new_text: edit.new_text.clone() });
    }
    let workspace = if snap.client_caps.versioned_edit_support() {
        let edits = changes
            .into_iter()
            .map(|(uri, edits)| lsp_types::TextDocumentEdit {
                text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                    version: snap.document_versions.get(&uri).copied(),
                    uri,
                },
                edits: edits.into_iter().map(lsp_types::OneOf::Left).collect(),
            })
            .collect();
        lsp_types::WorkspaceEdit {
            document_changes: Some(lsp_types::DocumentChanges::Edits(edits)),
            ..Default::default()
        }
    } else {
        lsp_types::WorkspaceEdit { changes: Some(changes), ..Default::default() }
    };
    Some(crate::to_proto::snippet_workspace_edit_from_plain(&workspace))
}
fn fixes_for<'a>(
    snap: &'a crate::global_state::GlobalStateSnapshot,
    uri: &Url,
) -> impl Iterator<Item = (Diagnostic, Data)> + 'a {
    snap.analysis
        .file_id(uri)
        .into_iter()
        .flat_map(move |file| snap.check_diagnostics.for_file(file))
        .filter_map(|diagnostic| {
            if diagnostic.source.as_deref() != Some("sail") {
                return None;
            }
            let data: Data =
                serde_json::from_value(diagnostic.data.as_ref()?.get("sailCompiler")?.clone())
                    .ok()?;
            Some((diagnostic, data))
        })
}
fn fix_id(fix: &Fix) -> String {
    fingerprint(&serde_json::to_string(fix).expect("serializable compiler fix"))
}
pub(crate) fn quick_fixes(
    snap: &crate::global_state::GlobalStateSnapshot,
    uri: &Url,
    requested: lsp_types::Range,
    diagnostics: &[Diagnostic],
) -> Vec<crate::lsp_ext::CodeAction> {
    let mut actions = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (current, data) in fixes_for(snap, uri) {
        let selected = if diagnostics.is_empty() {
            current.range.start <= requested.end && requested.start <= current.range.end
        } else {
            diagnostics.iter().any(|diagnostic| {
                diagnostic.source == current.source
                    && diagnostic.range == current.range
                    && diagnostic.code == current.code
                    && diagnostic.message == current.message
            })
        };
        if !selected {
            continue;
        }
        for fix in &data.fixes {
            let id = fix_id(fix);
            if !seen.insert(id.clone()) || edit(snap, &data, fix).is_none() {
                continue;
            }
            actions.push(crate::lsp_ext::CodeAction {
                command: None,
                title: fix.title.clone(),
                kind: Some(lsp_types::CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![current.clone()]),
                edit: None,
                is_preferred: None,
                data: Some(serde_json::json!({"compilerFix":{"uri":uri,"id":id}})),
            });
        }
    }
    actions
}
pub(crate) fn resolve_fix(
    snap: &crate::global_state::GlobalStateSnapshot,
    key: &serde_json::Value,
) -> Option<crate::lsp_ext::SnippetWorkspaceEdit> {
    let uri: Url = serde_json::from_value(key.get("uri")?.clone()).ok()?;
    let id = key.get("id")?.as_str()?;
    fixes_for(snap, &uri).find_map(|(_, data)| {
        data.fixes.iter().find(|fix| fix_id(fix) == id).and_then(|fix| edit(snap, &data, fix))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    struct Fixture {
        dir: PathBuf,
        file: PathBuf,
        text: String,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sail-editor-json-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("model.sail");
            let text = "let banner = \"🙂\"; let value = 0xAb\r\n".to_owned();
            std::fs::write(&file, &text).unwrap();
            Self { dir, file, text }
        }
        fn span(&self, start: usize, end: usize) -> serde_json::Value {
            json!({"file":self.file,"byteStart":start,"byteEnd":end,"sourceHash":{"algorithm":"fnv1a64","value":fingerprint(&self.text)}})
        }
        fn record(&self) -> serde_json::Value {
            let start = self.text.find("0xAb").unwrap();
            let span = self.span(start, start + 4);
            json!({"protocol":"sail-editor-diagnostics","version":1,"event":"diagnostic","severity":"warning","code":"sail.warning.hex-case","message":"Mixed case","span":span,"related":[{"span":self.span(0,3),"message":"declaration"}],"fixes":[{"title":"Lowercase hex","edits":[{"span":span,"oldText":"0xAb","newText":"0xab"}]}]})
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    fn stream(record: &serde_json::Value, success: bool) -> String {
        format!(
            "{}\n{}\n",
            record,
            json!({"protocol":"sail-editor-diagnostics","version":1,"event":"summary","success":success,"diagnostics":1})
        )
    }
    #[test]
    fn byte_spans_are_utf16_positions_and_keep_fix_guards_and_related_locations() {
        assert_eq!(fingerprint(""), "cbf29ce484222325");
        assert_eq!(fingerprint("hello"), "a430d84680aabd0b");
        let fixture = Fixture::new();
        let report = parse(&stream(&fixture.record(), true), true).unwrap();
        assert_eq!(report.found.len(), 1);
        let (_, diagnostic) = &report.found[0];
        let at = fixture.text.find("0xAb").unwrap();
        assert_eq!(
            diagnostic.range.start,
            lsp_types::Position::new(0, fixture.text[..at].encode_utf16().count() as u32)
        );
        assert_eq!(diagnostic.range.end.character, diagnostic.range.start.character + 4);
        assert_eq!(diagnostic.related_information.as_ref().unwrap().len(), 1);
        assert_eq!(
            diagnostic.data.as_ref().unwrap()["sailCompiler"]["fixes"][0]["edits"][0]["oldText"],
            "0xAb"
        );
        std::fs::write(&fixture.file, fixture.text.replace("0xAb", "0xab")).unwrap();
        assert!(matches!(parse(&stream(&fixture.record(), true), true), Err(Error::Stale(_))));
    }
    #[test]
    fn malformed_partial_or_contradictory_streams_never_pass() {
        let fixture = Fixture::new();
        let record = fixture.record();
        assert!(parse(&record.to_string(), true).is_err());
        assert!(parse(&(stream(&record, true) + &record.to_string()), true).is_err());
        assert!(parse(&stream(&record, true), false).is_err());
        for name in ["version", "span", "fixes", "severity", "unknown"] {
            let mut broken = record.clone();
            match name {
                "version" => broken["version"] = json!(2),
                "span" => {
                    broken.as_object_mut().unwrap().remove("span");
                }
                "fixes" => broken["fixes"][0]["edits"][0]["oldText"] = json!("wrong"),
                "severity" => broken["severity"] = json!("fatal"),
                _ => broken["unknown"] = json!(true),
            }
            assert!(parse(&stream(&broken, true), true).is_err(), "{broken}");
        }
        let mut split = record.clone();
        let emoji = fixture.text.find('🙂').unwrap();
        split["span"] = fixture.span(emoji + 1, emoji + 2);
        assert!(parse(&stream(&split, true), true).is_err());
        let mut overlap = record.clone();
        let edit = overlap["fixes"][0]["edits"][0].clone();
        overlap["fixes"][0]["edits"].as_array_mut().unwrap().push(edit);
        assert!(parse(&stream(&overlap, true), true).is_err());
        let mut error = record.clone();
        error["severity"] = json!("error");
        assert!(parse(&stream(&error, true), true).is_err());
    }
    #[test]
    fn errors_without_locations_are_retained_as_global_findings() {
        let fixture = Fixture::new();
        let mut record = fixture.record();
        record["span"] = serde_json::Value::Null;
        record["fixes"] = json!([]);
        record["severity"] = json!("error");
        let report = parse(&stream(&record, false), false).unwrap();
        assert_eq!(report.errors, 1);
        assert_eq!(report.unlocated.len(), 1);
        assert!(!report.success);
        assert!(report.found.is_empty());
    }

    #[test]
    fn multi_file_fix_is_indivisible_and_resolve_uses_current_authoritative_data() {
        let fixture = Fixture::new();
        let secondary = fixture.dir.join("secondary.sail");
        let secondary_text = "let other = 0xCd\r\n";
        std::fs::write(&secondary, secondary_text).unwrap();
        let mut record = fixture.record();
        let at = secondary_text.find("0xCd").unwrap();
        let secondary_edit = json!({"span":{"file":secondary,"byteStart":at,"byteEnd":at+4,
            "sourceHash":{"algorithm":"fnv1a64","value":fingerprint(secondary_text)}},
            "oldText":"0xCd","newText":"0xcd"});
        record["fixes"][0]["edits"].as_array_mut().unwrap().push(secondary_edit);
        let report = parse(&stream(&record, true), true).unwrap();
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = crate::global_state::GlobalState::new(sender, task_sender);
        let uri = Url::from_file_path(&fixture.file).unwrap();
        state.sync_disk_text(&uri, &fixture.text);
        // A closed file may still have an old VFS view while its watch event
        // is queued. Resolve must consult disk rather than that stale view.
        state.sync_disk_text(&Url::from_file_path(&secondary).unwrap(), secondary_text);
        let file = state.snapshot().analysis.file_id(&uri).unwrap();
        let diagnostic = report.found[0].1.clone();
        let target = super::super::CheckTarget::Project(fixture.dir.join("sail.proj"));
        state.diagnostics.set_check_diagnostics(&target, vec![(file, vec![diagnostic.clone()])]);
        let snapshot = state.snapshot();
        let actions = quick_fixes(&snapshot, &uri, diagnostic.range, &[]);
        assert_eq!(actions.len(), 1);
        let key = &actions[0].data.as_ref().unwrap()["compilerFix"];
        let edit = serde_json::to_value(resolve_fix(&snapshot, key).unwrap()).unwrap();
        assert_eq!(edit["changes"].as_object().unwrap().len(), 2, "{edit}");

        // The secondary source is unopened: changing it on disk must invalidate
        // the entire action, including its still-current primary edit.
        std::fs::write(&secondary, secondary_text.replace("0xCd", "0xcd")).unwrap();
        assert!(resolve_fix(&snapshot, key).is_none());
        assert!(quick_fixes(&snapshot, &uri, diagnostic.range, &[]).is_empty());
        std::fs::write(&secondary, secondary_text).unwrap();
        assert!(resolve_fix(&snapshot, key).is_some());

        // An old client action cannot resurrect findings removed by a new run.
        state.diagnostics.set_check_diagnostics(&target, Vec::new());
        let refreshed = state.snapshot();
        assert!(resolve_fix(&refreshed, key).is_none());
        assert!(
            resolve_fix(&snapshot, key).is_some(),
            "an existing snapshot retains its original findings"
        );
    }
}
