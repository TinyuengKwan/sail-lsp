//! Scenarios about what a diagnostic, a signature or a refused rename says
//! on the wire, driven through the real server as `server_check` does.
//!
//! Split from `server_check` by subject: that file is about which findings
//! reach which document and when; this one is about the fields of what
//! arrives — the attribution, the link, the tags, the severity a setting
//! produces — and about the error a request that cannot be served returns.

use crate::server_check::ServerFixture;

/// One project with the file open, so semantic diagnostics are computed.
fn project(name: &str, source: &str) -> ServerFixture {
    let mut server = ServerFixture::new(
        name,
        &[("main.sail", source), ("sail.proj", "Main { files main.sail }")],
    );
    server.did_open("main.sail", source);
    server
}

fn with_code<'a>(
    diagnostics: &'a [lsp_types::Diagnostic],
    code: &str,
) -> Option<&'a lsp_types::Diagnostic> {
    diagnostics.iter().find(|d| d.code == Some(lsp_types::NumberOrString::String(code.to_owned())))
}

/// A native finding arrives attributed to `sail-lsp`, with its code linked
/// to the page that documents it, with the tags it carries, and at the
/// severity the settings ask for: `diagnostics.warningsAsHint` makes it a
/// HINT on the wire — it used to stay a WARNING, so the setting did
/// nothing a client could see.
///
/// A settings change is not itself a republish (`update_configuration`
/// leaves diagnostics to the next request), so the file is edited after
/// configuring to have them recomputed.
#[test]
fn a_published_diagnostic_is_attributed_linked_tagged_and_resettable() {
    let source = "function f() -> int = { let x = 1; 2 }\n";
    let mut server = project("diagnostic-wire-fields", source);
    server.wait_for_diagnostics("main.sail", "the unused variable", |d| {
        with_code(d, "unused-variable").is_some()
    });
    let shown = server.shown("main.sail");
    let unused = with_code(&shown, "unused-variable").unwrap();
    assert_eq!(unused.source.as_deref(), Some("sail-lsp"));
    assert_eq!(
        unused.code_description.as_ref().map(|c| c.href.as_str()),
        Some(format!("{}#unused-variable", crate::to_proto::DIAGNOSTICS_DOCS_URL).as_str())
    );
    assert_eq!(unused.severity, Some(lsp_types::DiagnosticSeverity::WARNING));
    assert_eq!(unused.tags, Some(vec![lsp_types::DiagnosticTag::UNNECESSARY]));

    server.configure(serde_json::json!({
        "diagnostics": { "warningsAsHint": ["unused-variable"] }
    }));
    server.did_change_appending("main.sail", 1, "val after : int\n");
    server.wait_for_diagnostics("main.sail", "the settings to take effect", |d| {
        with_code(d, "unused-variable")
            .is_some_and(|u| u.severity == Some(lsp_types::DiagnosticSeverity::HINT))
    });
    let shown = server.shown("main.sail");
    let unused = with_code(&shown, "unused-variable").unwrap();
    assert_eq!(unused.tags, Some(vec![lsp_types::DiagnosticTag::UNNECESSARY]));
    assert_eq!(unused.source.as_deref(), Some("sail-lsp"));
}
