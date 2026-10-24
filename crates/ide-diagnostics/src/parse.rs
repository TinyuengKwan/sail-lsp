use ide_db::FileDb;

use crate::reporting::WarningEmitter;
use crate::types::DiagnosticCode;
use hir::diagnostics::{Diagnostic as HirDiagnostic, Severity};

/// Lexical errors used to arrive here, through a `LexError` type and a
/// `collect_lex_errors` pass whose one caller passed the empty slice.
/// They arrive through `Parse::errors()` now — `LexedStr` carries them and
/// `build_tree` merges them, which is rust-analyzer's route — so the type,
/// the parameter and the pass are gone rather than left as a second way in.
pub fn compute_parse_diagnostics(file: &dyn FileDb) -> Vec<HirDiagnostic> {
    let mut collector = ParseDiagnosticCollector::new(file);
    // Check for config bitvector truncation in directives
    if let Some(text) = Some(file.text()) {
        // Match `$option NAME:WIDTH VALUE` patterns in the file text
        for cap in config_bitvector_truncation_matches(text) {
            collector.diagnostics.push(cap);
        }
    }
    // Check for deprecated effect annotations + missing extern purity via ItemTree
    if let Some(item_tree) = file.item_tree() {
        let text = file.text();
        for &id in item_tree.top_level_items() {
            let span = id.span(item_tree);
            let def_text = text.get(span.start..span.end).unwrap_or("");
            let sig = id.signature(item_tree);

            // Deprecated effect annotations: `effect {wmv}` etc.
            if sig.contains("effect {") || sig.contains("effect{") {
                if let Some(pos) = def_text.find("effect") {
                    let abs_pos = span.start + pos;
                    let end_pos = abs_pos + 6;
                    collector.diagnostics.push(HirDiagnostic::new(
                        DiagnosticCode::SailLint("deprecated-effect-annotation", Severity::Warning),
                        "Explicit effect annotations are deprecated. They are no longer used and can be removed.".to_string(),
                        base_db::text_range(abs_pos, end_pos),
                        Severity::Warning,
                    ));
                }
            }

            // Missing extern purity: `val name = {backend: "..."} : type`
            // should have `pure` or `impure` annotation.
            // Only applies to ValSpec entries (not functions/mappings).
            if id.def_kind(item_tree) == hir::DefKind::ValSpec {
                // Extern bindings have `= { lem: "...", c: "..." }` in their text.
                // Look for `= {` followed by a string literal (distinguishes from
                // function body `= { ... }` which has expressions, not strings).
                let is_extern = def_text.contains("= {") && def_text.contains('"');
                if is_extern {
                    let has_purity = def_text.contains("pure") || def_text.contains("impure");
                    if !has_purity {
                        let name_len = id.name(item_tree).as_str().len();
                        let name_span = base_db::text_range(
                            span.start,
                            span.start + name_len.min(span.end - span.start),
                        );
                        collector.diagnostics.push(HirDiagnostic::new(
                            DiagnosticCode::SailLint("missing-extern-purity", Severity::Warning),
                            "All external bindings should be marked as either pure or impure"
                                .to_string(),
                            name_span,
                            Severity::Warning,
                        ));
                    }
                }
            }
        }
    }
    collector.finish()
}

/// Check for `$option NAME:WIDTH VALUE` directives where the bitvector literal
/// exceeds the declared type width.
fn config_bitvector_truncation_matches(text: &str) -> Vec<HirDiagnostic> {
    let mut results = Vec::new();
    // Regex-free approach: scan for lines starting with $option
    for (line_start, line) in line_offsets(text) {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("$option") {
            continue;
        }
        // Parse: $option NAME:WIDTH VALUE
        // e.g. $option ISA:64 0xFF_FF_FF_FF_FF_FF_FF_FF_FF
        let after_kw = &trimmed["$option".len()..];
        if !after_kw.starts_with(|c: char| c.is_whitespace()) {
            continue;
        }
        let after_kw = after_kw.trim_start();
        // Find NAME:WIDTH
        let colon_pos = match after_kw.find(':') {
            Some(p) => p,
            None => continue,
        };
        let after_colon = &after_kw[colon_pos + 1..];
        // WIDTH is digits until whitespace
        let width_end =
            after_colon.find(|c: char| !c.is_ascii_digit()).unwrap_or(after_colon.len());
        if width_end == 0 {
            continue;
        }
        let width: u64 = match after_colon[..width_end].parse() {
            Ok(w) => w,
            Err(_) => continue,
        };
        // VALUE is after whitespace
        let value_part = after_colon[width_end..].trim_start();
        if value_part.is_empty() {
            continue;
        }
        // First token of value (until whitespace or end)
        let value_token = value_part.split_whitespace().next().unwrap_or("");
        // Remove underscores for counting
        let clean = value_token.replace('_', "");
        let value_bits = if clean.starts_with("0x") || clean.starts_with("0X") {
            ((clean.len() - 2) as u64) * 4
        } else if clean.starts_with("0b") || clean.starts_with("0B") {
            (clean.len() - 2) as u64
        } else {
            continue; // Only check hex/binary literals
        };
        if value_bits > width {
            // Compute the absolute offset of the value in the file
            let trimmed_offset = line.len() - line.trim_start().len();
            let value_offset_in_line = trimmed.len() - value_part.len();
            let abs_start = line_start + trimmed_offset + value_offset_in_line;
            let abs_end = abs_start + value_token.len();
            results.push(HirDiagnostic::new(
                DiagnosticCode::SailLint("config-bitvector-truncation", Severity::Warning),
                format!(
                    "bitvector literal has {} bits but declared width is {} — value will be truncated",
                    value_bits, width
                ),
                base_db::text_range(abs_start, abs_end),
                Severity::Warning,
            ));
        }
    }
    results
}

/// Iterate lines in text, yielding (byte_offset_of_line_start, line_str).
fn line_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.split('\n').scan(0usize, |offset, line| {
        let start = *offset;
        *offset += line.len() + 1; // +1 for the '\n'
        Some((start, line))
    })
}

struct ParseDiagnosticCollector {
    diagnostics: Vec<HirDiagnostic>,
    #[expect(dead_code)]
    warnings: WarningEmitter,
}

impl ParseDiagnosticCollector {
    fn new(_file: &dyn FileDb) -> Self {
        Self { diagnostics: Vec::new(), warnings: WarningEmitter::new() }
    }

    fn finish(self) -> Vec<HirDiagnostic> {
        self.diagnostics
    }
}
