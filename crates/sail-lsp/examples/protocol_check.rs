//! Protocol check: does a range that leaves the server point at the text it
//! claims to?
//!
//! The fifth probe, and it exists because the other four cannot see this
//! layer. `corpus_check`, `conformance_check`, `mutation_check` and
//! `analysis_stats` all enter at `Analysis::*` with **byte offsets** and
//! compare diagnostics. Everything between the LSP wire and `Analysis` —
//! `from_proto`, `to_proto`, the position encoding — is downstream of all of
//! them.
//!
//! What lived there: `Position::character` is a UTF-16 column (LSP's default
//! `positionEncoding`, which this server negotiates nothing else than) and it
//! was passed through as a *byte* column in both directions. On any line
//! holding a non-ASCII character every position was wrong and slicing at one
//! panicked — while all four probes stayed green, because a diagnostic can be
//! exactly right about *which bytes* and still be drawn on the wrong column.
//!
//! rust-analyzer covers this with `tests/slow-tests` (3.6k lines) driving a
//! real server over the protocol. This is the cheap half of that: the same
//! conversions the handlers call, over a real corpus, with the client's view
//! simulated.
//!
//! ## The two properties
//!
//! 1. **Round trip.** `from_proto(to_proto(range)) == range`. Catches a
//!    conversion that loses information.
//! 2. **Client view.** A client reads `Range` as UTF-16 and slices its own
//!    copy of the line. Re-encoding the line to UTF-16, slicing by the
//!    emitted columns and decoding must give back exactly the bytes the
//!    server meant. Catches a conversion that is *self-consistent and
//!    wrong* — which is what a missing encoding conversion is, and what the
//!    round trip alone would pass.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --release --example protocol_check -- /path/to/sail-riscv
//! cargo run --release --example protocol_check -- --list /path/to/sail-riscv
//! ```

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::path::PathBuf;

use ide_db::line_index::LineIndex;
use sail_lsp::lsp::{from_proto, to_proto};

/// What a client would select, given a `Range` it reads as UTF-16.
///
/// This is the whole point of the probe: the server's own conversions can
/// agree with each other and still disagree with every editor.
fn client_selection(text: &str, range: lsp_types::Range) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let start_line = lines.get(range.start.line as usize)?;
    let end_line = lines.get(range.end.line as usize)?;

    let cut = |line: &str, col: u32| -> Option<String> {
        let units: Vec<u16> = line.encode_utf16().collect();
        let col = col as usize;
        if col > units.len() {
            return None;
        }
        String::from_utf16(&units[..col]).ok()
    };

    if range.start.line == range.end.line {
        let head = cut(start_line, range.start.character)?;
        let upto = cut(start_line, range.end.character)?;
        return Some(upto[head.len()..].to_string());
    }
    // Multi-line: the head of the first line and the tail of the last are
    // what a selection's edges are; the middle is whole lines either way.
    let head = cut(start_line, range.start.character)?;
    let tail = cut(end_line, range.end.character)?;
    let mut out = start_line[head.len()..].to_string();
    for line in &lines[range.start.line as usize + 1..range.end.line as usize] {
        out.push('\n');
        out.push_str(line);
    }
    out.push('\n');
    out.push_str(&tail);
    Some(out)
}

struct Mismatch {
    file: String,
    kind: &'static str,
    meant: String,
    got: String,
}

fn check_range(
    file: &str,
    text: &str,
    index: &LineIndex,
    range: base_db::TextRange,
    out: &mut Vec<Mismatch>,
) {
    let (s, e) = (base_db::range_start(range), base_db::range_end(range));
    if s > e || e > text.len() || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
        out.push(Mismatch {
            file: file.to_string(),
            kind: "range is not a valid slice of the file",
            meant: format!("{s}..{e} of {} bytes", text.len()),
            got: String::new(),
        });
        return;
    }
    let meant = &text[s..e];
    let lsp = to_proto::range(index, range);

    // 1. Round trip. The two ends are converted separately rather than
    // through `from_proto::text_range`, which builds a `TextRange` and
    // asserts `start <= end` — a conversion bad enough to invert a range
    // would panic the probe instead of being reported. (It would panic the
    // *server* too: handlers call `text_range` on client ranges.)
    let back_s = from_proto::offset(index, lsp.start);
    let back_e = from_proto::offset(index, lsp.end);
    if (back_s, back_e) != (s, e) {
        out.push(Mismatch {
            file: file.to_string(),
            kind: "range does not survive the round trip",
            meant: format!("{s}..{e}"),
            got: format!("{back_s}..{back_e}"),
        });
    }

    // 2. What a client would select.
    match client_selection(text, lsp) {
        Some(got) if got == meant => {}
        Some(got) => out.push(Mismatch {
            file: file.to_string(),
            kind: "a client reading UTF-16 selects different text",
            meant: meant.to_string(),
            got,
        }),
        None => out.push(Mismatch {
            file: file.to_string(),
            kind: "the emitted range is out of the client's line",
            meant: meant.to_string(),
            got: format!("{lsp:?}"),
        }),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let list = args.iter().any(|a| a == "--list");
    let root = args.iter().skip(1).find(|a| !a.starts_with('-')).cloned().unwrap_or_else(|| {
        eprintln!("Usage: protocol_check [--list] /path/to/corpus");
        std::process::exit(2);
    });

    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![PathBuf::from(&root)];
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&p) {
                stack.extend(entries.flatten().map(|e| e.path()));
            }
        } else if p.extension().is_some_and(|e| e == "sail") {
            files.push(p);
        }
    }
    files.sort();

    let mut mismatches = Vec::new();
    let (mut n_files, mut n_ranges, mut n_wide) = (0usize, 0usize, 0usize);

    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let index = LineIndex::new(&text);
        n_files += 1;
        if !text.is_ascii() {
            n_wide += 1;
        }

        // Every token's span is a range the server may emit — for a
        // diagnostic, a hover, a navigation target. Checking all of them
        // covers far more of the surface than any one feature's output, and
        // needs no inference to run.
        let (root_node, _) = syntax::parse_text(&text);
        for el in root_node.descendants_with_tokens() {
            let r = el.text_range();
            let range = base_db::text_range(usize::from(r.start()), usize::from(r.end()));
            n_ranges += 1;
            check_range(&name, &text, &index, range, &mut mismatches);
        }
    }

    if list {
        for m in &mismatches {
            println!("{}\t{}\n  meant: {:?}\n  got:   {:?}", m.file, m.kind, m.meant, m.got);
        }
        println!();
    }

    println!("{}", "=".repeat(60));
    println!("PROTOCOL CHECK");
    println!("{}", "=".repeat(60));
    println!("  files:            {n_files} ({n_wide} with non-ASCII text)");
    println!("  ranges checked:   {n_ranges}");
    println!("  mismatches:       {}", mismatches.len());
    if !mismatches.is_empty() {
        let mut by_kind: std::collections::BTreeMap<&str, usize> = Default::default();
        for m in &mismatches {
            *by_kind.entry(m.kind).or_default() += 1;
        }
        println!();
        for (kind, n) in by_kind {
            println!("    {kind:<52} {n}");
        }
    }
    if n_wide == 0 {
        println!();
        println!("  NOTE: no file in this corpus holds a non-ASCII character, so");
        println!("        the encoding conversion is untested here. Point this at");
        println!("        a corpus that does, or the number means less than it looks.");
    }
    std::process::exit(if mismatches.is_empty() { 0 } else { 1 });
}
