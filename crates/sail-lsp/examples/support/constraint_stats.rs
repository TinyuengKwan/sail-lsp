//! JSON inventory of final deferred obligations. Formatting is opt-in and stays
//! outside the inference query/cache; proved goals cost only a counter there.
use hir::infer::{InferenceResult, ObligationStats};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct ConstraintReport {
    pub totals: ObligationStats,
    reasons: BTreeMap<String, usize>,
    sites: Vec<Value>,
}
impl ConstraintReport {
    pub fn record(
        &mut self,
        file: &str,
        callable: &str,
        clause: u32,
        text: &str,
        body: &hir::Body,
        source_map: &hir::BodySourceMap,
        original_base: usize,
        result: &InferenceResult,
    ) {
        self.totals.merge(result.obligation_stats);
        // Byte offsets are zero-based, line/column are one-based (UTF-8 bytes).
        let location = |expr| {
            source_map.expr_syntax(expr).map(|span| {
                let span = source_map.rebase(span, original_base);
                let at = span.start.min(text.len());
                let prefix = &text[..at];
                let line_start = prefix.rfind('\n').map_or(0, |p| p + 1);
                let line_end = text[at..].find('\n').map_or(text.len(), |p| at + p);
                json!({"start": at, "end": span.end.min(text.len()),
                    "line": prefix.bytes().filter(|b| *b == b'\n').count() + 1,
                    "column": at - line_start + 1, "source": text[line_start..line_end].trim()})
            })
        };
        for (status, obligations) in
            [("unknown", &result.unsolved_constraints), ("refuted", &result.refuted_constraints)]
        {
            for ob in obligations {
                let reason = ob.unknown_reason.map(|r| r.as_str());
                if let Some(reason) = reason {
                    *self.reasons.entry(reason.into()).or_default() += 1;
                }
                self.sites.push(json!({
                "file": file, "callable": callable, "clause": clause,
                "status": status, "reason": reason, "cause": format!("{:?}", ob.cause),
                "location": location(ob.origin.unwrap_or(body.root())),
                "origin_is_fallback": ob.origin.is_none(),
                "goal": ob.goal.constraint().to_text(),
                "assumptions": ob.assumptions.iter().map(|a| a.to_text()).collect::<Vec<_>>(),
                "rigid_variables": ob.rigid_variables,
                "compatibility_types": ob.compatibility_types.as_ref().map(|(actual, expected)|
                    json!({"actual": actual.display_text(), "expected": expected.display_text()})),
                "recovery_types": ob.recovery_types.as_ref().map(|(actual, expected)|
                    json!({"actual": actual.display_text(), "expected": expected.display_text()})),
            }));
            }
        }
    }
    pub fn write(&mut self, path: &std::path::Path, root: &std::path::Path) -> std::io::Result<()> {
        self.sites.sort_by_cached_key(|site| {
            (
                site["file"].as_str().unwrap_or("").to_owned(),
                site["location"]["start"].as_u64().unwrap_or(0),
                site["goal"].as_str().unwrap_or("").to_owned(),
            )
        });
        let report = json!({"schema_version": 1, "workspace": root,
            "scope": "committed_deferred_obligations",
            "smt_enabled": cfg!(feature = "z3-solver"),
            "totals": {"total": self.totals.total(), "proved": self.totals.proved,
                "refuted": self.totals.refuted, "unknown": self.totals.unknown},
            "unknown_by_reason": self.reasons, "sites": self.sites});
        std::fs::write(path, serde_json::to_string_pretty(&report)? + "\n")
    }
}
