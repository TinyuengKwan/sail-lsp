//! `inactive-code`: the regions the preprocessor did not take, as the
//! editor sees them through `Analysis::file_diagnostics`.

use ide_diagnostics::DiagnosticsConfig;

fn inactive(source: &str, config: &DiagnosticsConfig) -> Vec<ide_diagnostics::Diagnostic> {
    let (analysis, files) = crate::fixture::project_analysis(source);
    analysis
        .file_diagnostics(files[0].0, config)
        .expect("not cancelled")
        .into_iter()
        .filter(|d| d.code.as_str() == "inactive-code")
        .collect()
}

fn covered<'a>(source: &'a str, d: &ide_diagnostics::Diagnostic) -> &'a str {
    &source[base_db::range_start(d.range.range)..base_db::range_end(d.range.range)]
}

/// With `X` undefined the `$ifdef` side is inactive; with it defined, the
/// `$else` side is. The range is the lines inside the region, never a
/// directive line.
#[test]
fn the_untaken_branch_is_reported_without_its_directives() {
    let source =
        "$ifdef X\nval a : int\nval a2 : int\n$else\nval b : int\n$endif\nval after : int\n";
    let diagnostics = inactive(source, &DiagnosticsConfig::default());
    let [d] = diagnostics.as_slice() else { panic!("one region, got {diagnostics:?}") };
    assert_eq!(covered(source, d), "val a : int\nval a2 : int");
    assert_eq!(d.message, "code is inactive: `$ifdef X` is false");
    assert_eq!(d.severity, hir::diagnostics::Severity::Hint);
    assert!(d.unused, "tagged Unnecessary so the editor fades it");

    let defined = format!("$define X\n{source}");
    let diagnostics = inactive(&defined, &DiagnosticsConfig::default());
    let [d] = diagnostics.as_slice() else { panic!("one region, got {diagnostics:?}") };
    assert_eq!(covered(&defined, d), "val b : int");
    assert_eq!(d.message, "code is inactive: `$ifdef X` is true, this is its `$else` branch");
}

#[test]
fn an_ifndef_reads_the_other_way() {
    let source = "$define X\n$ifndef X\nval a : int\n$endif\n";
    let diagnostics = inactive(source, &DiagnosticsConfig::default());
    let [d] = diagnostics.as_slice() else { panic!("one region, got {diagnostics:?}") };
    assert_eq!(covered(source, d), "val a : int");
    assert_eq!(d.message, "code is inactive: `$ifndef X` is false");
}

/// A nested conditional inside an inactive region is part of that one
/// region: one diagnostic, covering the inner directives too, and nothing
/// reported again for the inner `$ifdef`.
#[test]
fn a_nested_region_is_one_contiguous_report() {
    let source = "$ifdef X\nval a : int\n$ifdef Y\nval c : int\n$endif\nval d : int\n$endif\nval after : int\n";
    let diagnostics = inactive(source, &DiagnosticsConfig::default());
    let [d] = diagnostics.as_slice() else { panic!("one region, got {diagnostics:?}") };
    assert_eq!(covered(source, d), "val a : int\n$ifdef Y\nval c : int\n$endif\nval d : int");

    // Active outer, inactive inner: only the inner region.
    let source =
        "$define X\n$ifdef X\nval a : int\n$ifdef Y\nval c : int\n$endif\nval d : int\n$endif\n";
    let diagnostics = inactive(source, &DiagnosticsConfig::default());
    let [d] = diagnostics.as_slice() else { panic!("one region, got {diagnostics:?}") };
    assert_eq!(covered(source, d), "val c : int");
    assert_eq!(d.message, "code is inactive: `$ifdef Y` is false");
}

#[test]
fn an_empty_region_and_an_active_file_report_nothing() {
    assert!(inactive("$ifdef X\n$endif\nval a : int\n", &DiagnosticsConfig::default()).is_empty());
    assert!(inactive("$define X\n$ifdef X\nval a : int\n$endif\n", &DiagnosticsConfig::default())
        .is_empty());
}

#[test]
fn the_code_can_be_disabled_like_any_other() {
    let source = "$ifdef X\nval a : int\n$endif\n";
    assert_eq!(inactive(source, &DiagnosticsConfig::default()).len(), 1);
    let config = DiagnosticsConfig {
        disabled: std::iter::once("inactive-code".to_owned()).collect(),
        ..Default::default()
    };
    assert!(inactive(source, &config).is_empty());
}
