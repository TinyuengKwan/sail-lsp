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

/// Ask for something the server is expected to refuse, and get the refusal.
fn ask_for_error(
    server: &mut ServerFixture,
    method: &str,
    params: serde_json::Value,
) -> lsp_server::ResponseError {
    let id = server.request(method, params);
    let response = server.wait_for_response(id);
    response.error.unwrap_or_else(|| panic!("{method} was answered: {:?}", response.result))
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

/// Signature help over the wire: the declaration's doc comment travels as
/// its documentation, parameters go as text to a client that declared no
/// `labelOffsetSupport` (the fixture's), and the `signatureInfo` settings
/// narrow it to the parameter list and drop the documentation.
///
/// A settings change is not itself a recomputation, so the file is edited
/// after configuring.
#[test]
fn signature_help_carries_the_docs_and_follows_the_signature_info_settings() {
    let source = "/// Adds two numbers.\nval add : (int, int) -> int\n\
                  function add(x : int, y : int) -> int = x + y\nfunction g() -> int = add(1, 2)\n";
    let mut server = project("signature-info-settings", source);
    let at_second_argument = serde_json::json!({
        "textDocument": {"uri": server.url("main.sail")},
        "position": {"line": 3, "character": 29},
    });
    let answer: lsp_types::SignatureHelp = serde_json::from_value(
        server.ask("textDocument/signatureHelp", at_second_argument.clone()),
    )
    .unwrap();
    assert_eq!(answer.active_parameter, Some(1));
    let [signature] = answer.signatures.as_slice() else { panic!("{answer:?}") };
    assert_eq!(signature.label, "val add : (int, int) -> int");
    assert_eq!(
        signature.documentation,
        Some(lsp_types::Documentation::String("Adds two numbers.".to_owned()))
    );
    let labels: Vec<_> =
        signature.parameters.as_ref().unwrap().iter().map(|p| p.label.clone()).collect();
    assert_eq!(
        labels,
        [
            lsp_types::ParameterLabel::Simple("arg1: int".to_owned()),
            lsp_types::ParameterLabel::Simple("arg2: int".to_owned()),
        ]
    );

    server.configure(
        serde_json::json!({"signatureInfo": {"detail": "parameters", "documentation": false}}),
    );
    server.did_change_appending("main.sail", 4, "val after : int\n");
    let answer: lsp_types::SignatureHelp =
        serde_json::from_value(server.ask("textDocument/signatureHelp", at_second_argument))
            .unwrap();
    let [signature] = answer.signatures.as_slice() else { panic!("{answer:?}") };
    assert_eq!(signature.label, "arg1: int, arg2: int");
    assert_eq!(signature.documentation, None);
    assert_eq!(answer.active_parameter, Some(1));
}

/// A rename the server will not do is refused with the ide's reason and the
/// protocol's code for it, `RequestFailed` (-32803). `prepareRename` used to
/// answer `null`, which editors show as a generic "cannot be renamed", and a
/// refused `rename` went out as `InternalError`, which they show as a server
/// failure.
#[test]
fn a_refused_rename_says_why_with_request_failed() {
    let source = "function f(x : int) -> int = x\n\n";
    let mut server = project("rename-refusals", source);
    let uri = server.url("main.sail");
    let at = |line: u32, character: u32| {
        serde_json::json!({
            "textDocument": {"uri": uri},
            "position": {"line": line, "character": character},
        })
    };
    let failed = lsp_server::ErrorCode::RequestFailed as i32;

    let keyword = ask_for_error(&mut server, "textDocument/prepareRename", at(0, 2));
    assert_eq!((keyword.code, keyword.message.as_str()), (failed, "Cannot rename a keyword"));

    let blank = ask_for_error(&mut server, "textDocument/prepareRename", at(1, 0));
    assert_eq!((blank.code, blank.message.as_str()), (failed, "No references found at position"));

    let mut builtin = at(0, 15);
    builtin["newName"] = serde_json::json!("integer");
    let refused = ask_for_error(&mut server, "textDocument/rename", builtin);
    assert_eq!(
        (refused.code, refused.message.as_str()),
        (failed, "Compiler builtin types cannot be renamed")
    );

    let mut to_keyword = at(0, 9);
    to_keyword["newName"] = serde_json::json!("function");
    let refused = ask_for_error(&mut server, "textDocument/rename", to_keyword);
    assert_eq!(
        (refused.code, refused.message.as_str()),
        (failed, "Invalid name: `function` is a keyword")
    );

    // What can be renamed still is.
    let prepared = server.ask("textDocument/prepareRename", at(0, 9));
    assert_eq!(prepared["placeholder"], "f");
}
