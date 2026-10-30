//! Compare the normal parse query with and without its previous-tree seed.
//! Same inputs, fixities, query and validation; only the optional cache differs.
use base_db::{FileId, FileText};
use ide_db::root_database::RootDatabase;
use salsa::Setter;
use std::{sync::Arc, time::Instant};
fn percentile(values: &mut [u128], p: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() - 1) * p / 100]
}
fn main() {
    let source: String = (0..400)
        .map(|i| format!("function f{i}(x : int) -> int = {{ let a = x + 1000; a + 2 }}\n"))
        .collect();
    let mut incremental = RootDatabase::default();
    let mut full = RootDatabase::default();
    let a = FileText::new(&incremental, Arc::from(source.as_str()), FileId::from_raw(0));
    let b = FileText::new(&full, Arc::from(source.as_str()), FileId::from_raw(0));
    syntax::parse_query::parse_file(&incremental, a);
    syntax::parse_query::parse_file(&full, b);
    let mut inc_times = Vec::new();
    let mut full_times = Vec::new();
    for i in 0..120 {
        let text = source.replacen("1000", &format!("{}", 1001 + i), 1);
        a.set_text(&mut incremental).to(Arc::from(text.as_str()));
        b.set_text(&mut full).to(Arc::from(text.as_str()));
        b.parse_cache(&full).clear();
        // Alternate measurement order to avoid consistently favouring either path.
        let (cached, parsed, inc, all) = if i % 2 == 0 {
            let start = Instant::now();
            let cached = syntax::parse_query::parse_file(&incremental, a).clone();
            let inc = start.elapsed().as_nanos();
            let start = Instant::now();
            let parsed = syntax::parse_query::parse_file(&full, b).clone();
            (cached, parsed, inc, start.elapsed().as_nanos())
        } else {
            let start = Instant::now();
            let parsed = syntax::parse_query::parse_file(&full, b).clone();
            let all = start.elapsed().as_nanos();
            let start = Instant::now();
            let cached = syntax::parse_query::parse_file(&incremental, a).clone();
            (cached, parsed, start.elapsed().as_nanos(), all)
        };
        assert_eq!(cached.green, parsed.green);
        assert_eq!(cached.tokens, parsed.tokens);
        assert_eq!(cached.errors, parsed.errors);
        if i >= 20 {
            inc_times.push(inc);
            full_times.push(all);
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "source_bytes": source.len(), "measured_edits":inc_times.len(), "warmup_edits":20,
            "incremental_ns":{"p50":percentile(&mut inc_times,50), "p95":percentile(&mut inc_times,95)},
            "full_ns":{"p50":percentile(&mut full_times,50), "p95":percentile(&mut full_times,95)},
            "all_results_equal":true
        })
    );
}
