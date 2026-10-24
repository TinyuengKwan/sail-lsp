//! Building an [`Analysis`] from a test fixture.
//!
//! Mirrors rust-analyzer's `ide/src/fixture.rs`: a fixture string becomes a
//! real salsa database, and tests then call the same entry points the LSP
//! server calls. Nothing here reimplements analysis, so a test cannot pass
//! against a pipeline the server does not use.
//!
//! ```ignore
//! check_diagnostics(r#"
//! val f : int -> int
//! function f(x) = g(x)
//! //              ^ error: Function g not found
//! "#);
//! ```
//!
//! A fixture is either a single file or several, split by `//- /name.sail`
//! lines as in [`ide_db::fixture`]. A leading `//- prelude` line loads the
//! bundled `data/sail-lib`, which most real Sail needs; it is off by default
//! because reading 60 files costs more than most tests need.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use base_db::FileId;
use ide_db::annotations::extract_annotations;
use ide_db::FileDb;
use url::Url;

use crate::analysis::{Analysis, AnalysisHost, UrlMap};

/// Marker line that pulls the bundled Sail library into the fixture.
const PRELUDE_MARKER: &str = "//- prelude";

/// The bundled `data/sail-lib`, read once per test process.
fn prelude_files() -> &'static [(String, String)] {
    static FILES: OnceLock<Vec<(String, String)>> = OnceLock::new();
    FILES.get_or_init(|| {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../sail-lsp/data/sail-lib")
            .canonicalize()
            .expect("bundled sail-lib not found next to the ide crate");
        let mut out = Vec::new();
        collect_sail_files(&root, &root, &mut out);
        out.sort();
        out
    })
}

fn collect_sail_files(root: &PathBuf, dir: &PathBuf, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sail_files(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "sail") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let rel = path.strip_prefix(root).unwrap_or(&path).display().to_string();
                out.push((format!("/sail-lib/{rel}"), text));
            }
        }
    }
}

/// The files of a fixture, in order, as `(virtual path, text)`.
fn split_fixture(fixture: &str) -> (bool, Vec<(String, String)>) {
    let mut with_prelude = false;
    let mut files: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;

    for line in fixture.lines() {
        let trimmed = line.trim();
        if trimmed == PRELUDE_MARKER {
            with_prelude = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("//-") {
            if let Some((path, lines)) = current.take() {
                files.push((path, lines.join("\n") + "\n"));
            }
            current = Some((rest.trim().to_owned(), Vec::new()));
            continue;
        }
        match &mut current {
            Some((_, lines)) => lines.push(line),
            // Text before any `//-` is a single anonymous file.
            None => {
                current = Some(("/main.sail".to_owned(), vec![line]));
            }
        }
    }
    if let Some((path, lines)) = current {
        files.push((path, lines.join("\n") + "\n"));
    }
    (with_prelude, files)
}

/// Build an [`Analysis`] from a fixture.
///
/// Returns the analysis and the fixture's own files, in fixture order — the
/// prelude, when requested, is loaded but not returned, so a test never has
/// to filter it out.
pub fn analysis(fixture: &str) -> (Analysis, Vec<(FileId, String)>) {
    let (with_prelude, fixture_files) = split_fixture(fixture);

    let mut all: Vec<(String, String)> = Vec::new();
    if with_prelude {
        all.extend(prelude_files().iter().cloned());
    }
    let fixture_start = all.len();
    all.extend(fixture_files);

    let mut host = AnalysisHost::new(None);
    let mut change = ide_db::apply_change::Change::new();
    let mut vfs = vfs::Vfs::default();
    let urls: Vec<Url> = all
        .iter()
        .map(|(path, _)| {
            Url::parse(&format!("file://{path}")).expect("fixture path is a valid URL")
        })
        .collect();

    for (url, (_, text)) in urls.iter().zip(all.iter()) {
        let fid = vfs.file_id_for_url(url);
        change.change_file(fid, text.clone());
    }
    host.apply_change(std::mem::take(&mut change));
    let files = host.raw_database().files().clone();

    // Same workspace registration the server and `corpus_check` perform:
    // without it cross-file resolution and `infix` declarations are inert.
    {
        let db = host.raw_database_mut();
        let file_texts: Vec<base_db::FileText> = urls
            .iter()
            .filter_map(|url| files.file_text(vfs.lookup_file_id_by_url(url)?))
            .collect();
        hir::def_query::set_workspace_files(db, file_texts);
    }

    let mut url_to_fid = HashMap::new();
    let mut fid_to_url = HashMap::new();
    let mut ids = Vec::new();
    for (url, (path, _)) in urls.iter().zip(all.iter()) {
        let Some(fid) = vfs.lookup_file_id_by_url(url) else { continue };
        url_to_fid.insert(url.clone(), fid);
        fid_to_url.insert(fid, url.clone());
        ids.push((fid, path.clone()));
    }
    let url_map = UrlMap::new(url_to_fid, fid_to_url);
    let analysis = Analysis::with_url_map(host.raw_database().clone(), files, url_map);

    (analysis, ids.split_off(fixture_start))
}

/// Check that a fixture's diagnostics are exactly its `//^^^` annotations.
///
/// An annotation reads `<severity>: <message>`, e.g. `error: no matching
/// overload for \`g\``. A file with no annotations must produce no
/// diagnostics, which is how a "this is valid Sail" regression test is
/// written.
#[track_caller]
pub fn check_diagnostics(fixture: &str) {
    let (analysis, files) = analysis(fixture);
    let config = ide_diagnostics::DiagnosticsConfig::new();

    for (file_id, path) in files {
        let text = analysis
            .files_ref()
            .file_text(file_id)
            .map(|ft| ft.text(analysis.db()).to_string())
            .unwrap_or_default();
        let mut expected = extract_annotations(&text);
        let mut actual: Vec<_> = analysis
            .file_diagnostics(file_id, &config)
            .expect("the fixture database is never cancelled")
            .into_iter()
            .map(|d| {
                let severity = match d.severity {
                    ide_diagnostics::Severity::Error => "error",
                    ide_diagnostics::Severity::Warning => "warn",
                    ide_diagnostics::Severity::WeakWarning => "weak",
                    ide_diagnostics::Severity::Information => "info",
                    ide_diagnostics::Severity::Hint => "hint",
                };
                (d.range.range, format!("{severity}: {}", d.message))
            })
            .collect();
        expected.sort_by_key(|(range, s)| (range.start(), s.clone()));
        actual.sort_by_key(|(range, s)| (range.start(), s.clone()));

        if expected != actual {
            let missing: Vec<_> = expected.iter().filter(|e| !actual.contains(e)).collect();
            let extra: Vec<_> = actual.iter().filter(|a| !expected.contains(a)).collect();
            let show = |v: &Vec<&(base_db::TextRange, String)>| -> String {
                v.iter()
                    .map(|(r, s)| {
                        let (start, end) = (usize::from(r.start()), usize::from(r.end()));
                        let snippet = text.get(start..end).unwrap_or("<out of range>");
                        format!("\n    {start}..{end} {snippet:?} — {s}")
                    })
                    .collect()
            };
            panic!(
                "diagnostics in {path} do not match the annotations.\n\
                 expected but absent:{}\n  unexpected:{}",
                show(&missing),
                show(&extra)
            );
        }
    }
}

/// Assert that a fixture produces no diagnostics at all.
///
/// Equivalent to `check_diagnostics` on a fixture with no annotations; named
/// separately because "upstream accepts this program" is the common case.
#[track_caller]
pub fn check_no_diagnostics(fixture: &str) {
    check_diagnostics(fixture)
}

/// Build an [`Analysis`] and locate the `$0` cursor marker.
///
/// Mirrors rust-analyzer's `fixture::position`. The marker is removed from
/// the text before the database is built, so offsets are those of the real
/// source.
pub fn position(fixture: &str) -> (Analysis, FileId, ide_db::LineCol) {
    let (cleaned, marker) = strip_cursor(fixture);
    let (analysis, files) = analysis(&cleaned);
    let (file_index, offset) = marker.expect("fixture has no `$0` cursor marker");
    let (file_id, _) = files[file_index];
    let text = file_text(&analysis, file_id);
    (analysis, file_id, line_col_at(&text, offset))
}

/// Build an [`Analysis`] plus every `//^^^ text` annotation in the fixture.
///
/// Mirrors rust-analyzer's `fixture::annotations`: the annotations mark the
/// ranges a feature is expected to point at, rather than diagnostics.
pub fn annotations(fixture: &str) -> (Analysis, Vec<(FileId, base_db::TextRange, String)>) {
    let (analysis, files) = analysis(fixture);
    let mut out = Vec::new();
    for (file_id, _) in files {
        let text = file_text(&analysis, file_id);
        for (range, content) in extract_annotations(&text) {
            out.push((file_id, range, content));
        }
    }
    (analysis, out)
}

/// A workspace symbol index over the fixture, as the server has one.
///
/// Through the same salsa query the server reads, so a fixture cannot
/// answer a cross-file goto the server would miss, or miss one it answers.
pub fn symbol_index(
    analysis: &Analysis,
    files: &[(FileId, String)],
) -> ide_db::symbol_index::SymbolIndex {
    let pairs: Vec<_> = files
        .iter()
        .filter_map(|(file_id, _)| Some((*file_id, analysis.files_ref().file_text(*file_id)?)))
        .collect();
    ide_db::symbol_index::SymbolIndex::build(analysis.db(), pairs)
}

/// Check that goto-definition at `$0` reaches every annotated range.
///
/// Follows the same three steps `handle_goto_definition` follows — scope-aware
/// resolution, field/method resolution, then the workspace index — so a test
/// cannot pass through a path the server does not take. The annotation's text
/// is ignored beyond being present, so a target is written as `//^^^` under
/// the defining name.
#[track_caller]
pub fn check_goto_definition(fixture: &str) {
    let (cleaned, marker) = strip_cursor(fixture);
    let (analysis, files) = analysis(&cleaned);
    let (file_index, offset) = marker.expect("fixture has no `$0` cursor marker");
    let (file_id, _) = files[file_index];
    let text = file_text(&analysis, file_id);
    let url = analysis.url_for_file_id(file_id).expect("fixture file has a URL").clone();
    let file = analysis.file_by_id(file_id).expect("fixture file is in the database");
    let pos = line_col_at(&text, offset);

    let mut expected = Vec::new();
    for (target_file, _) in &files {
        for (range, _) in extract_annotations(&file_text(&analysis, *target_file)) {
            expected.push((*target_file, range));
        }
    }
    assert!(
        !expected.is_empty(),
        "goto fixture has no `//^^^` annotation marking the expected target"
    );

    let mut actual: Vec<(FileId, base_db::TextRange)> = Vec::new();
    let mut record_url = |target_url: &url::Url, range: base_db::TextRange| {
        if let Some(fid) = analysis.file_id(target_url) {
            actual.push((fid, range));
        }
    };
    for nav in
        crate::goto_definition::goto_definition_semantic(&file, pos, &url).unwrap_or_default()
    {
        record_url(&nav.url, nav.focus_range);
    }
    if let Some(ft) = analysis.files_ref().file_text(file_id) {
        let offset = file.offset_at(&pos);
        for nav in crate::goto_definition::goto_definition_field_or_method(
            analysis.db(),
            ft,
            offset,
            &url,
            &symbol_index(&analysis, &files),
            &|fid| analysis.url_for_file_id(fid).cloned(),
        )
        .unwrap_or_default()
        {
            record_url(&nav.url, nav.focus_range);
        }
    }
    if let Some((token, _)) = file.token_at(pos) {
        if let Some(key) = ide_db::token_symbol_key(token) {
            let index = symbol_index(&analysis, &files);
            for loc in crate::navigation::definition_locations_indexed(&index, &key, &url, &|fid| {
                analysis.url_for_file_id(fid).cloned()
            }) {
                record_url(&loc.url, loc.range);
            }
        }
    }

    // Every path narrows to the name, so a target equals the annotation.
    let reached = |want: &(FileId, base_db::TextRange)| actual.contains(want);
    assert!(
        expected.iter().all(reached),
        "goto-definition did not reach the annotated target.\n  expected: {expected:?}\n  found: {actual:?}"
    );
}

/// Check that the hover markup at `$0` contains `expected`.
///
/// Substring rather than exact match: hover markup carries documentation and
/// layout that a test about one fact should not have to restate.
#[track_caller]
pub fn check_hover_contains(fixture: &str, expected: &str) {
    let (analysis, file_id, pos) = position(fixture);
    let markup = analysis
        .hover(file_id, pos, &crate::hover::HoverConfig::default())
        .expect("hover was not cancelled")
        .map(|h| h.markup)
        .unwrap_or_default();
    assert!(markup.contains(expected), "hover does not mention {expected:?}.\n  markup: {markup}");
}

/// Rename the symbol under `$0` and check every edit the server would send.
///
/// Through `Analysis::rename` — the entry point the handler calls — so the
/// test cannot pass on a path no client takes.
#[track_caller]
pub fn check_rename(fixture: &str, new_name: &str, expected_edits: usize) {
    let (cleaned, marker) = strip_cursor(fixture);
    let (analysis, files) = analysis(&cleaned);
    let (file_index, offset) = marker.expect("fixture has no `$0` cursor marker");
    let (file_id, _) = files[file_index];
    let text = file_text(&analysis, file_id);
    let url = analysis.url_for_file_id(file_id).expect("fixture file has a URL").clone();
    let pos = line_col_at(&text, offset);

    let change = analysis
        .rename(&url, pos, new_name)
        .expect("the fixture database is never cancelled")
        .expect("rename returned an error")
        .expect("rename found nothing to rename");
    let mut where_: Vec<String> = Vec::new();
    for (fid, edits) in &change.source_file_edits {
        let t = file_text(&analysis, *fid);
        for e in edits {
            let s = base_db::range_start(e.range);
            let en = base_db::range_end(e.range);
            where_.push(
                t[t[..s].rfind('\n').map_or(0, |i| i + 1)
                    ..t[en..].find('\n').map_or(t.len(), |i| en + i)]
                    .to_owned(),
            );
        }
    }
    let edits: usize = change.source_file_edits.values().map(|e| e.len()).sum();
    assert_eq!(
        edits, expected_edits,
        "rename produced {edits} edits, expected {expected_edits}; it edited: {where_:?}"
    );
}

/// Every range find-all-references reports for the symbol under `$0`,
/// as `file index -> sorted ranges`, through `Analysis::find_all_refs`.
#[track_caller]
pub fn references_at(fixture: &str) -> Vec<(usize, Vec<base_db::TextRange>)> {
    let (cleaned, marker) = strip_cursor(fixture);
    let (analysis, files) = analysis(&cleaned);
    let (file_index, offset) = marker.expect("fixture has no `$0` cursor marker");
    let (file_id, _) = files[file_index];
    let text = file_text(&analysis, file_id);
    let url = analysis.url_for_file_id(file_id).expect("fixture file has a URL").clone();
    let pos = line_col_at(&text, offset);

    let index_of = |fid: FileId| files.iter().position(|(f, _)| *f == fid).unwrap_or(usize::MAX);
    let mut by_file: std::collections::BTreeMap<usize, Vec<base_db::TextRange>> =
        Default::default();
    for result in analysis
        .find_all_refs(&url, pos, None)
        .expect("the fixture database is never cancelled")
        .unwrap_or_default()
    {
        for decl in &result.declarations {
            by_file.entry(index_of(decl.file_id)).or_default().push(decl.range);
        }
        for (&fid, refs) in &result.references {
            by_file.entry(index_of(fid)).or_default().extend(refs.iter().map(|(r, _)| *r));
        }
    }
    by_file
        .into_iter()
        .map(|(f, mut rs)| {
            rs.sort_by_key(|r| base_db::range_start(*r));
            rs.dedup();
            (f, rs)
        })
        .collect()
}

/// The source text each reported reference covers, for the symbol under `$0`.
#[track_caller]
pub fn reference_texts(fixture: &str) -> Vec<String> {
    let (cleaned, _) = strip_cursor(fixture);
    let (analysis, files) = analysis(&cleaned);
    let mut out = Vec::new();
    for (file_index, ranges) in references_at(fixture) {
        let Some((fid, _)) = files.get(file_index) else { continue };
        let text = file_text(&analysis, *fid);
        for r in ranges {
            let (s, e) = (base_db::range_start(r), base_db::range_end(r));
            out.push(format!("{file_index}:{}", &text[s..e]));
        }
    }
    out
}

/// Remove the `$0` marker, returning the index of the fixture file that held
/// it and the byte offset within that file.
fn strip_cursor(fixture: &str) -> (String, Option<(usize, usize)>) {
    let Some(pos) = fixture.find("$0") else { return (fixture.to_owned(), None) };
    let cleaned = format!("{}{}", &fixture[..pos], &fixture[pos + 2..]);
    // Which fixture file, and where inside it — recomputed from the split so
    // header lines and the prelude marker do not shift the offset.
    let (_, files) = split_fixture(&cleaned);
    let (_, before) = split_fixture(&fixture[..pos]);
    let file_index = before.len().saturating_sub(1);
    let offset = before.last().map(|(_, text)| text.trim_end_matches('\n').len()).unwrap_or(0);
    debug_assert!(file_index < files.len());
    (cleaned, Some((file_index, offset)))
}

fn file_text(analysis: &Analysis, file_id: FileId) -> String {
    analysis
        .files_ref()
        .file_text(file_id)
        .map(|ft| ft.text(analysis.db()).to_string())
        .unwrap_or_default()
}

fn line_col_at(text: &str, offset: usize) -> ide_db::LineCol {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() as u32;
    let col = before.rsplit('\n').next().map(str::len).unwrap_or(0) as u32;
    ide_db::LineCol { line, col }
}

#[cfg(test)]
mod tests {
    use super::{check_diagnostics, split_fixture};

    #[test]
    fn a_single_file_fixture_needs_no_header() {
        let (prelude, files) = split_fixture("val f : int -> int\n");
        assert!(!prelude);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].0, "/main.sail");
    }

    #[test]
    fn files_split_on_the_header_line() {
        let (prelude, files) =
            split_fixture("//- prelude\n//- /a.sail\nval a : int\n//- /b.sail\nval b : int\n");
        assert!(prelude);
        assert_eq!(
            files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["/a.sail", "/b.sail"]
        );
    }

    #[test]
    fn valid_sail_draws_nothing() {
        check_diagnostics(
            r#"
default Order dec
val f : int -> int
function f(x) = x
"#,
        );
    }

    #[test]
    fn an_unresolved_call_is_reported_where_it_is_written() {
        check_diagnostics(
            r#"
default Order dec
val f : int -> int
function f(x) = nope(x)
//              ^^^^ error: Function nope not found
"#,
        );
    }
}
