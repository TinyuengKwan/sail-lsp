//! The `// Diagnostic:` comments on `ide-diagnostics`' handlers, which
//! `docs/diagnostics.md` is generated from — rust-analyzer's
//! `diagnostics_docs` codegen, with its doc-tests replaced by running every
//! example through `Analysis::file_diagnostics`, the call the editor makes.
//!
//! A comment is the paragraph a user reads when they follow a finding's
//! `codeDescription`, so each one is held to what the code does: the code it
//! names is one the handler emits, its example really reports that code, and
//! a handler nothing feeds says so with a claim the sources bear out.
//!
//! Here rather than in `ide-diagnostics` because an example is a project —
//! `inactive-code` needs the preprocessor's regions, an include needs a
//! second file — and `fixture::project_analysis` is the one place a test
//! gets one.

use std::path::{Path, PathBuf};

/// One handler's documentation block.
struct HandlerDoc {
    handler: String,
    code: String,
    /// Paragraphs, with fenced examples kept in place.
    parts: Vec<Part>,
    /// Whether the comment says no analysis pass reports this yet.
    unproduced: bool,
    /// The construction the comment says nothing performs, when it names one.
    claim: Option<String>,
    /// The severities the handler gives this code, as written beside it.
    severities: Vec<String>,
}

enum Part {
    Prose(String),
    Example(String),
}

fn crates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn handlers_dir() -> PathBuf {
    crates_dir().join("ide-diagnostics/src/handlers")
}

/// The handler modules, in the order `ide-diagnostics`' `lib.rs` registers them.
fn handler_names() -> Vec<String> {
    let lib = std::fs::read_to_string(crates_dir().join("ide-diagnostics/src/lib.rs")).unwrap();
    let start = lib.find("mod handlers {").expect("the handler registry");
    let end = start + lib[start..].find("\n}\n").expect("the registry's end");
    lib[start..end]
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub(crate) mod ")?.strip_suffix(';'))
        .map(str::to_owned)
        .collect()
}

fn parse(handler: &str, text: &str) -> HandlerDoc {
    let mut lines = text.lines().peekable();
    let mut header = None;
    for line in lines.by_ref() {
        if let Some(code) = line.strip_prefix("// Diagnostic: ") {
            header = Some(code.trim().to_owned());
            break;
        }
    }
    let code = header.unwrap_or_else(|| panic!("{handler}.rs has no `// Diagnostic:` comment"));
    let mut body = Vec::new();
    while let Some(line) = lines.peek() {
        let Some(rest) = line.strip_prefix("//") else { break };
        if rest.starts_with('/') || rest.starts_with('!') {
            break;
        }
        body.push(rest.strip_prefix(' ').unwrap_or(rest).to_owned());
        lines.next();
    }
    let mut parts = Vec::new();
    let mut prose: Vec<String> = Vec::new();
    let mut example: Option<Vec<String>> = None;
    let mut unproduced = false;
    let mut claim = None;
    for line in body {
        match (&mut example, line.as_str()) {
            (None, "```sail") => example = Some(Vec::new()),
            (Some(lines), "```") => {
                parts.push(Part::Example(lines.join("\n") + "\n"));
                example = None;
            }
            (Some(lines), _) => lines.push(line),
            (None, "") => {
                if !prose.is_empty() {
                    parts.push(Part::Prose(prose.join(" ")));
                    prose.clear();
                }
            }
            (None, _) => {
                if let Some(reason) = line.strip_prefix("Not produced: ") {
                    unproduced = true;
                    if let Some(rest) = reason.strip_prefix("nothing constructs `") {
                        claim = Some(rest.split('`').next().unwrap().to_owned());
                    }
                }
                prose.push(line);
            }
        }
    }
    assert!(example.is_none(), "{handler}.rs: an example is not closed");
    if !prose.is_empty() {
        parts.push(Part::Prose(prose.join(" ")));
    }
    let mut severities: Vec<String> = text
        .match_indices(&format!("\"{code}\""))
        .filter_map(|(at, _)| {
            // Only a `DiagnosticCode::` constructor counts; the code is also
            // quoted in messages and tests.
            let before = text[..at].trim_end();
            if before.ends_with("SailError(") {
                return Some("error".to_owned());
            }
            if !before.ends_with("SailLint(") {
                return None;
            }
            let after = &text[at + code.len() + 2..];
            let severity = after.split("Severity::").nth(1)?;
            Some(severity.split(|c: char| !c.is_alphanumeric()).next()?.to_lowercase())
        })
        .collect();
    severities.sort();
    severities.dedup();
    assert!(
        !severities.is_empty(),
        "{handler}.rs documents `{code}` and never emits it: the code beside a \
         `DiagnosticCode::` constructor is the one to document"
    );
    HandlerDoc { handler: handler.to_owned(), code, parts, unproduced, claim, severities }
}

fn docs() -> Vec<HandlerDoc> {
    handler_names()
        .iter()
        .map(|name| {
            let text = std::fs::read_to_string(handlers_dir().join(format!("{name}.rs"))).unwrap();
            parse(name, &text)
        })
        .collect()
}

/// Every code an example's first file reports, with nothing disabled: the
/// defaults hide `deprecated-effect-annotation`, and the question here is
/// whether the pipeline *can* say it. An example without `//- /path` lines
/// is one file, `main.sail`.
fn codes_reported(source: &str) -> Vec<String> {
    let fixture = if source.starts_with("//- /") {
        source.to_owned()
    } else {
        format!("//- /main.sail\n{source}")
    };
    let (analysis, files) = crate::fixture::project_analysis(&fixture);
    let mut config = ide_diagnostics::DiagnosticsConfig::new();
    config.disabled.clear();
    let mut codes: Vec<String> = analysis
        .file_diagnostics(files[0].0, &config)
        .expect("the fixture database is never cancelled")
        .iter()
        .map(|d| d.code.as_str().to_owned())
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

/// Lines of the crates' sources that construct `claim`: the text outside a
/// comment, not in a `match` arm. `ide-diagnostics`' handlers, dispatch and
/// test helpers are left out because they name every variant.
fn constructions_of(claim: &str) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
    let crates = crates_dir();
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap() {
        let path = entry.unwrap().path();
        if path.join("src").is_dir() {
            walk(&path.join("src"), &mut files);
        }
    }
    let mut found = Vec::new();
    for path in files {
        let relative = path.strip_prefix(&crates).unwrap();
        if relative.starts_with("ide-diagnostics/src/handlers")
            || relative == Path::new("ide-diagnostics/src/lib.rs")
            || relative == Path::new("ide-diagnostics/src/tests.rs")
        {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (number, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains(claim) && !code.contains("=>") {
                found.push(format!("{}:{}: {}", relative.display(), number + 1, line.trim()));
            }
        }
    }
    found
}

/// Each handler documents a code it emits, with a description and an example.
#[test]
fn every_handler_documents_its_code_with_an_example() {
    let docs = docs();
    assert_eq!(docs.len(), handler_names().len());
    for doc in &docs {
        assert!(
            doc.parts.iter().any(|part| matches!(part, Part::Prose(_))),
            "{}.rs: the `// Diagnostic:` comment has no description",
            doc.handler
        );
        assert!(
            doc.parts.iter().any(|part| matches!(part, Part::Example(_))),
            "{}.rs: the `// Diagnostic:` comment has no ```sail example",
            doc.handler
        );
    }
}

/// An example reports the code it is an example of. A handler documented as
/// not produced is one no analysis pass feeds: its example does *not* report
/// the code — so wiring the producer is what makes the note stale — and the
/// construction it says is missing is missing. Every stale claim fails here
/// at once.
#[test]
fn every_documented_example_reports_its_code() {
    let mut failures = Vec::new();
    let docs = docs();
    let handlers_emitting = |code: &str| docs.iter().filter(|d| d.code == code).count();
    for doc in &docs {
        if let Some(claim) = &doc.claim {
            let constructions = constructions_of(claim);
            if !constructions.is_empty() {
                failures.push(format!(
                    "{}.rs says nothing constructs `{claim}`, but:\n    {}",
                    doc.handler,
                    constructions.join("\n    ")
                ));
            }
        }
        // `type-error` is several handlers' code, so an unproduced handler
        // sharing it cannot be told from its neighbours by the code alone.
        if doc.unproduced && handlers_emitting(&doc.code) > 1 {
            continue;
        }
        for part in &doc.parts {
            let Part::Example(source) = part else { continue };
            let codes = codes_reported(source);
            if codes.contains(&doc.code) == doc.unproduced {
                let (verb, note) = if doc.unproduced {
                    ("reports", "; the `Not produced` note is stale")
                } else {
                    ("does not report", "")
                };
                failures.push(format!(
                    "{}.rs: the example {verb} `{}`{note} (reported: {:?})\n{source}",
                    doc.handler, doc.code, codes
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn generate() -> String {
    use std::fmt::Write;
    let mut docs = docs();
    docs.sort_by(|a, b| a.code.cmp(&b.code).then(a.handler.cmp(&b.handler)));
    let mut out = String::new();
    out.push_str(
        "<!-- Generated from the `// Diagnostic:` comments in \
         `crates/ide-diagnostics/src/handlers/` by `cargo test -p ide diagnostics_docs`; \
         edit those and run it with `UPDATE_EXPECT=1` to regenerate. -->\n\n\
         # Diagnostics\n\n\
         Every finding sail-lsp reports on its own carries one of the codes below as its \
         `code`, with `source: \"sail-lsp\"` and this page as its `codeDescription`; a \
         finding relayed from the `sail` compiler by flycheck says `source: \"sail\"` \
         instead. These codes are what `diagnostics.disabled`, `diagnostics.warningsAsHint` \
         and `diagnostics.warningsAsInfo` take (see [configuration.md](configuration.md)).\n\n\
         A code marked *not produced* has a handler that no analysis pass feeds yet, so it \
         is never reported; its example shows what it would flag.\n\n",
    );
    let mut index = 0;
    while index < docs.len() {
        let code = docs[index].code.clone();
        let group: Vec<&HandlerDoc> = docs[index..].iter().take_while(|d| d.code == code).collect();
        index += group.len();
        let mut severities: Vec<&str> =
            group.iter().flat_map(|d| d.severities.iter().map(String::as_str)).collect();
        severities.sort();
        severities.dedup();
        writeln!(out, "## {code}\n").unwrap();
        writeln!(out, "Severity: {}.", severities.join(" or ")).unwrap();
        for doc in group {
            out.push('\n');
            for part in &doc.parts {
                match part {
                    Part::Prose(text) if text.starts_with("Not produced: ") => {
                        writeln!(out, "**{text}**\n").unwrap()
                    }
                    Part::Prose(text) => writeln!(out, "{text}\n").unwrap(),
                    Part::Example(source) => writeln!(out, "```sail\n{source}```\n").unwrap(),
                }
            }
            writeln!(out, "Handler: `crates/ide-diagnostics/src/handlers/{}.rs`", doc.handler)
                .unwrap();
        }
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out.push('\n');
    out
}

/// `docs/diagnostics.md` is what the comments say. Run with `UPDATE_EXPECT=1`
/// to regenerate it.
#[test]
#[allow(clippy::print_stderr)]
fn diagnostics_docs_are_current() {
    let path = crates_dir().join("../docs/diagnostics.md");
    let generated = generate();
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if generated != current {
        if std::env::var("UPDATE_EXPECT").is_ok() {
            std::fs::write(&path, &generated).expect("write docs/diagnostics.md");
            eprintln!("Updated docs/diagnostics.md");
        } else {
            let first = generated
                .lines()
                .zip(current.lines())
                .position(|(a, b)| a != b)
                .unwrap_or(generated.lines().count().min(current.lines().count()));
            panic!(
                "docs/diagnostics.md is out of date with the `// Diagnostic:` comments \
                 (first difference at line {}).\nRun with UPDATE_EXPECT=1 to regenerate.",
                first + 1
            );
        }
    }
}
