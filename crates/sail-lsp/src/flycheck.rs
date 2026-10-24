//! Upstream Sail's own diagnostics, beside the server's.
//!
//! rust-analyzer runs `cargo check` and shows what the compiler says next to
//! what its own analysis says, because the two are not the same thing and
//! the IDE's is the weaker one. This server had no equivalent — the header
//! of `diagnostics.rs` recorded it as a "Sail simplification" — while every
//! probe in this repo uses `sail --just-check` as its oracle precisely
//! because it decides what the server cannot.
//!
//! What it closes is not hypothetical. `int` where `range(0, 3)` is expected
//! is a false negative this checker cannot fix without carrying every value's
//! bounds (see the reverted directional-coercion experiment), and the five
//! mutants `mutation_check` still misses are the same shape. Upstream proves
//! them.
//!
//! Off by default: it shells out to a binary that may not be installed, and
//! a language server that spawns processes nobody asked for is a surprise.

use std::path::{Path, PathBuf};
use std::process::Command;

use lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};

/// One run's findings, by the file they belong to.
pub(crate) type FlycheckResult = Vec<(PathBuf, Diagnostic)>;

/// Run `sail --just-check` over `files` and translate what it says.
///
/// `cwd` is where relative paths in the output are resolved from — upstream
/// prints the path it was given.
pub(crate) fn run(
    command: &str,
    extra_args: &[String],
    files: &[PathBuf],
    cwd: &Path,
) -> FlycheckResult {
    let mut cmd = Command::new(command);
    cmd.arg("--just-check").args(extra_args).args(files).current_dir(cwd);
    let Ok(output) = cmd.output() else {
        // No `sail` on PATH is not an error to report in the editor; the
        // feature is opt-in and the user chose the command.
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stderr);
    parse(&text, cwd)
}

/// Upstream's error format.
///
/// ```text
/// Type error:
/// a.sail:4.35-41:
/// 4 |function branch x = if x < 32 then x + 32 else x - 32
///   |                                   ^----^ (operator +)
///   | int(('ex179 + 32)) is not a subtype of range(0, 62)
/// ```
///
/// The location line is the anchor: a header before it says what kind of
/// error it is, and the `|`-prefixed lines after it say why. Source echoes
/// (`4 |...`) and the caret ruler are not the message.
pub(crate) fn parse(text: &str, cwd: &Path) -> FlycheckResult {
    let lines: Vec<String> = text.lines().map(strip_ansi).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((path, range)) = parse_location(&lines[i]) else {
            i += 1;
            continue;
        };
        // The header is the nearest preceding non-empty line; it names the
        // kind ("Type error:", "Syntax error:").
        let header = lines[..i]
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().trim_end_matches(':').to_string())
            .unwrap_or_else(|| "error".to_string());
        let mut message = Vec::new();
        let mut j = i + 1;
        while j < lines.len() {
            let line = lines[j].trim_start();
            // A following location line starts the next diagnostic.
            if parse_location(&lines[j]).is_some() {
                break;
            }
            match line.strip_prefix('|') {
                Some(rest) => {
                    let rest = rest.trim();
                    // What is *not* the message: the caret ruler, an echo of
                    // the source, a nested note's own location line, and a
                    // deeper nesting's prefix.
                    let is_ruler =
                        !rest.is_empty() && rest.chars().all(|c| matches!(c, '^' | '-' | ' '));
                    let is_echo = rest.split_once('|').is_some_and(|(n, _)| {
                        !n.is_empty() && n.trim().chars().all(|c| c.is_ascii_digit())
                    });
                    let is_nested_location = looks_like_location(rest);
                    if !rest.is_empty()
                        && !is_ruler
                        && !is_echo
                        && !is_nested_location
                        && !rest.starts_with('|')
                    {
                        message.push(rest.to_string());
                    }
                }
                // A source echo is `4 |...`; anything else ends the block.
                None if line.split_once('|').is_some_and(|(n, _)| {
                    !n.is_empty() && n.trim().chars().all(|c| c.is_ascii_digit())
                }) => {}
                None if line.is_empty() => {}
                None => break,
            }
            j += 1;
        }
        let body = if message.is_empty() {
            header.clone()
        } else {
            // Two lines is enough to say what happened; upstream's nested
            // overload listings run to dozens.
            message.iter().take(2).cloned().collect::<Vec<_>>().join(" ")
        };
        out.push((
            resolve(&path, cwd),
            Diagnostic {
                range,
                severity: Some(if header.to_ascii_lowercase().contains("warning") {
                    DiagnosticSeverity::WARNING
                } else {
                    DiagnosticSeverity::ERROR
                }),
                source: Some("sail".to_string()),
                message: body,
                ..Default::default()
            },
        ));
        i = j.max(i + 1);
    }
    out
}

fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        cwd.join(p)
    }
}

/// `path:line.col-col:` or `path:line.col-line.col:`, 1-based lines and
/// 0-based columns — which is what LSP wants for the column and one less
/// for the line.
fn parse_location(line: &str) -> Option<(String, Range)> {
    let line = line.trim();
    // Upstream nests notes under a diagnostic, each with its own location
    // line behind a `|`: "derived from here", "bound here". Those belong to
    // the diagnostic above them, and reading them as new ones turned one
    // error into three, two of them at a path called `| a.sail`.
    if line.starts_with('|') {
        return None;
    }
    let line = line.strip_suffix(':')?;
    let (path, span) = line.rsplit_once(':')?;
    if path.is_empty() {
        return None;
    }
    let (start, end) = span.split_once('-')?;
    let (l1, c1) = start.split_once('.')?;
    let l1: u32 = l1.trim().parse().ok()?;
    let c1: u32 = c1.trim().parse().ok()?;
    let (l2, c2) = match end.split_once('.') {
        Some((l, c)) => (l.trim().parse().ok()?, c.trim().parse().ok()?),
        None => (l1, end.trim().parse().ok()?),
    };
    Some((
        path.to_string(),
        Range {
            start: Position { line: l1.saturating_sub(1), character: c1 },
            end: Position { line: l2.saturating_sub(1), character: c2 },
        },
    ))
}

/// `a.sail:3.17-28:` — the shape of a location, wherever it appears.
fn looks_like_location(text: &str) -> bool {
    let Some(rest) = text.strip_suffix(':') else { return false };
    let Some((path, span)) = rest.rsplit_once(':') else { return false };
    !path.is_empty()
        && span.contains('.')
        && span.contains('-')
        && span.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '-')
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\u{1b}[93mType error\u{1b}[0m:\n\
        \u{1b}[96ma.sail\u{1b}[0m:4.35-41:\n\
        4\u{1b}[96m |\u{1b}[0mfunction branch x = if x < 32 then x + 32 else x - 32\n\
        \u{1b}[91m |\u{1b}[0m                                   \u{1b}[91m^----^\u{1b}[0m\n\
        \u{1b}[91m |\u{1b}[0m int(('ex179 + 32)) is not a subtype of range(0, 62)\n";

    #[test]
    fn a_location_line_anchors_a_diagnostic() {
        let found = parse(SAMPLE, Path::new("/w"));
        assert_eq!(found.len(), 1, "{found:?}");
        let (path, diag) = &found[0];
        assert_eq!(path, Path::new("/w/a.sail"));
        // Line 4 is LSP line 3; the columns pass through.
        assert_eq!(diag.range.start, Position { line: 3, character: 35 });
        assert_eq!(diag.range.end, Position { line: 3, character: 41 });
        assert_eq!(diag.source.as_deref(), Some("sail"));
        assert!(
            diag.message.contains("is not a subtype of range(0, 62)"),
            "the ruler and the source echo are not the message: {}",
            diag.message
        );
    }

    /// Two errors in one run, and a span that crosses lines.
    #[test]
    fn each_location_starts_a_new_diagnostic() {
        let text = "Type error:\n\
            b.sail:2.0-3.4:\n\
             | first\n\
            Type error:\n\
            b.sail:9.1-2:\n\
             | second\n";
        let found = parse(text, Path::new("/w"));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].1.range.end, Position { line: 2, character: 4 });
        assert_eq!(found[0].1.message, "first");
        assert_eq!(found[1].1.message, "second");
    }

    /// Output with no location line yields nothing rather than a diagnostic
    /// at 0:0 — a checker that cannot place a finding must not invent a
    /// place for it.
    #[test]
    fn output_without_a_location_is_not_a_diagnostic() {
        assert!(parse("sail: command not found\n", Path::new("/w")).is_empty());
        assert!(parse("", Path::new("/w")).is_empty());
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// Against the real binary. `#[ignore]` because it needs `sail` on PATH.
    #[test]
    #[ignore]
    fn upstream_says_what_we_read() {
        let dir = std::env::temp_dir().join("sail-lsp-flycheck-live");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.sail");
        std::fs::write(
            &file,
            "default Order dec\nval g : unit -> int\nval f : unit -> range(0, 3)\nfunction f() = g()\n",
        )
        .unwrap();
        let found = run("sail", &[], &[PathBuf::from("a.sail")], &dir);
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, file);
        assert!(found[0].1.message.contains("not a subtype"), "{}", found[0].1.message);
        assert_eq!(found[0].1.range.start.line, 3);
    }
}
