//! LSP extensions and conversions for feature-specific protocol types.
//!
//! Position/range conversions live in `from_proto` and `to_proto` and require
//! a `LineIndex` to translate between UTF-16 columns and internal byte offsets.

/// Convert an internal SymbolKind to an LSP SymbolKind.
pub fn lsp_symbol_kind(kind: ide_db::ide_types::SymbolKind) -> lsp_types::SymbolKind {
    use ide_db::ide_types::SymbolKind as Ide;
    use lsp_types::SymbolKind as Lsp;
    match kind {
        Ide::Function => Lsp::FUNCTION,
        Ide::Variable => Lsp::VARIABLE,
        Ide::Constant => Lsp::CONSTANT,
        Ide::TypeParameter => Lsp::TYPE_PARAMETER,
        Ide::Struct => Lsp::STRUCT,
        Ide::Enum => Lsp::ENUM,
        Ide::EnumMember => Lsp::ENUM_MEMBER,
        Ide::Module => Lsp::MODULE,
        Ide::Field => Lsp::FIELD,
        Ide::Property => Lsp::PROPERTY,
        Ide::Event => Lsp::EVENT,
        Ide::Operator => Lsp::OPERATOR,
        Ide::TypeAlias => Lsp::TYPE_PARAMETER,
        Ide::Other => Lsp::NULL,
    }
}

/// Build the SemanticTokensOptions from the ide token/modifier legends.
/// This is LSP-protocol metadata, so it lives in the binary crate.
pub fn semantic_tokens_options() -> lsp_types::SemanticTokensOptions {
    use lsp_types::{
        SemanticTokenModifier, SemanticTokenType, SemanticTokensFullOptions, SemanticTokensLegend,
        SemanticTokensOptions, WorkDoneProgressOptions,
    };

    let token_types: Vec<SemanticTokenType> =
        ide::syntax_highlighting::TOKEN_TYPES.iter().map(|s| SemanticTokenType::new(s)).collect();
    let token_modifiers: Vec<SemanticTokenModifier> = ide::syntax_highlighting::TOKEN_MODIFIERS
        .iter()
        .map(|s| SemanticTokenModifier::new(s))
        .collect();

    SemanticTokensOptions {
        work_done_progress_options: WorkDoneProgressOptions { work_done_progress: Some(false) },
        legend: SemanticTokensLegend { token_types, token_modifiers },
        range: Some(true),
        full: Some(SemanticTokensFullOptions::Delta { delta: Some(true) }),
    }
}

#[cfg(test)]
mod tests {
    use ide_db::line_index::LineIndex;
    use lsp_types::{Position, Range};

    #[test]
    fn position_roundtrip() {
        let text = "val x : int\nfunction foo(x) = x + 1\n";
        let idx = LineIndex::new(text);

        // line 0, col 4 = byte offset 4
        let pos = Position::new(0, 4);
        let offset = crate::lsp::from_proto::offset(&idx, pos);
        assert_eq!(offset, 4);

        let back = crate::to_proto::position_in(&idx, offset);
        assert_eq!(back, pos);
    }

    #[test]
    fn range_roundtrip() {
        let text = "val x : int\nfunction foo(x) = x + 1\n";
        let idx = LineIndex::new(text);

        let range = Range::new(Position::new(0, 0), Position::new(0, 11));
        let tr = crate::lsp::from_proto::text_range(&idx, range);
        assert_eq!(base_db::range_start(tr), 0);
        assert_eq!(base_db::range_end(tr), 11);

        let back = crate::to_proto::range(&idx, tr);
        assert_eq!(back, range);
    }

    #[test]
    fn position_roundtrip_between_wide_characters() {
        let text = "étarget🦀\n";
        let idx = LineIndex::new(text);
        let client = Position::new(0, 4);
        let internal = crate::lsp::from_proto::line_col(&idx, client);
        assert_eq!(internal, ide_db::LineCol { line: 0, col: 5 });
        assert_eq!(crate::to_proto::position_from_line_col(&idx, internal), client);
    }
}

/// Server health status.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Health {
    Ok,
    Warning,
    Error,
}

/// Server status notification parameters.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatusParams {
    pub health: Health,
    pub quiescent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Custom notification: `sail-lsp/serverStatus`.
pub enum ServerStatusNotification {}

impl lsp_types::notification::Notification for ServerStatusNotification {
    type Params = ServerStatusParams;
    const METHOD: &'static str = "sail-lsp/serverStatus";
}

// Each custom request follows the:
//   1. Parameter struct (serde)
//   2. Result type alias
//   3. Empty enum implementing `lsp_types::request::Request`

/// Parameters for `sail-lsp/viewSyntaxTree`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewSyntaxTreeParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
}

/// Custom request: `sail-lsp/viewSyntaxTree`.
///
/// Returns the CST (concrete syntax tree) for the given file as
/// pretty-printed text. Used for debugging the parser output.
pub enum ViewSyntaxTree {}

impl lsp_types::request::Request for ViewSyntaxTree {
    type Params = ViewSyntaxTreeParams;
    type Result = String;
    const METHOD: &'static str = "sail-lsp/viewSyntaxTree";
}

/// Parameters for `sail-lsp/viewHir`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewHirParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub position: lsp_types::Position,
}

/// Custom request: `sail-lsp/viewHir`.
///
/// Shows the HIR expression and inferred type at a cursor position.
/// Used for debugging semantic analysis.
pub enum ViewHir {}

impl lsp_types::request::Request for ViewHir {
    type Params = ViewHirParams;
    type Result = String;
    const METHOD: &'static str = "sail-lsp/viewHir";
}

/// Parameters for `sail-lsp/viewItemTree`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewItemTreeParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
}

/// Custom request: `sail-lsp/viewItemTree`.
///
/// Pretty-prints the ItemTree (public surface: signatures, kinds,
/// doc comments) for a file. Used for debugging name resolution.
pub enum ViewItemTree {}

impl lsp_types::request::Request for ViewItemTree {
    type Params = ViewItemTreeParams;
    type Result = String;
    const METHOD: &'static str = "sail-lsp/viewItemTree";
}

/// Parameters for the project navigation requests: the file asked about.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectNavigationParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
}

/// Custom request: `sail-lsp/openProjectManifest`, rust-analyzer's
/// `experimental/openCargoToml`.
///
/// The manifest of the project the file belongs to, at the `files` entry
/// that lists it (at the start of the manifest when the file is reached
/// through `$include`). `null` outside every project.
pub enum OpenProjectManifest {}

impl lsp_types::request::Request for OpenProjectManifest {
    type Params = ProjectNavigationParams;
    type Result = Option<lsp_types::Location>;
    const METHOD: &'static str = "sail-lsp/openProjectManifest";
}

/// Custom request: `sail-lsp/parentModule`, rust-analyzer's
/// `experimental/parentModule`.
///
/// What brings the file into the project: the name of each manifest module
/// whose `files` lists it, and each `$include` that names it.
pub enum ParentModule {}

impl lsp_types::request::Request for ParentModule {
    type Params = ProjectNavigationParams;
    type Result = Vec<lsp_types::Location>;
    const METHOD: &'static str = "sail-lsp/parentModule";
}

/// Parameters for `sail-lsp/expandInclude`.
///
/// Expand includes for a Sail file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandIncludeParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
}

/// Result of expanding includes.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpandIncludeResult {
    pub root: String,
    pub expansion: String,
    pub context: Option<String>,
    pub project: Option<serde_json::Value>,
    pub mappings: Vec<ExpansionMapping>,
}

/// A rendered source/directive segment and its original physical location.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpansionMapping {
    pub range: lsp_types::Range,
    pub origin: lsp_types::Location,
    pub kind: String,
}

/// Custom request: `sail-lsp/expandInclude`.
///
/// Shows the transitive closure of `$include` directives for a file,
/// displaying included content inline.
pub enum ExpandInclude {}

impl lsp_types::request::Request for ExpandInclude {
    type Params = ExpandIncludeParams;
    type Result = Option<ExpandIncludeResult>;
    const METHOD: &'static str = "sail-lsp/expandInclude";
}

/// Custom request: `sail-lsp/analyzerStatus`.
///
/// Returns a human-readable status string with server internals
/// (file count, index size, include graph stats, memory usage).
pub enum SailLspStatus {}

impl lsp_types::request::Request for SailLspStatus {
    type Params = ();
    type Result = String;
    const METHOD: &'static str = "sail-lsp/analyzerStatus";
}

/// Passive, structured diagnostic report. Includes no document contents or environment values.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReportParams {
    pub text_document: Option<lsp_types::TextDocumentIdentifier>,
}
pub enum DiagnosticReport {}
impl lsp_types::request::Request for DiagnosticReport {
    type Params = DiagnosticReportParams;
    type Result = serde_json::Value;
    const METHOD: &'static str = "sail-lsp/diagnosticReport";
}

/// Parameters for `sail-lsp/viewIncludeGraph`.
///
/// Sail-specific: renders the $include dependency graph.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewIncludeGraphParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
}

/// Custom request: `sail-lsp/viewIncludeGraph`.
///
/// Renders the $include dependency graph as readable text.
/// Sail-specific debugging aid.
pub enum ViewIncludeGraph {}

impl lsp_types::request::Request for ViewIncludeGraph {
    type Params = ViewIncludeGraphParams;
    type Result = String;
    const METHOD: &'static str = "sail-lsp/viewIncludeGraph";
}

/// Custom notification: `sail-lsp/reloadWorkspace`.
///
/// Triggers a full workspace reload (re-scan files, rebuild index).
pub enum ReloadWorkspace {}

impl lsp_types::notification::Notification for ReloadWorkspace {
    type Params = ();
    const METHOD: &'static str = "sail-lsp/reloadWorkspace";
}

/// Parameters for `experimental/matchingBrace`.
///
/// rust-analyzer's shape: several positions in one round trip, because the
/// editor asks for every cursor it has.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchingBraceParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub positions: Vec<lsp_types::Position>,
}

/// Custom request: `experimental/matchingBrace`.
///
/// The position of the bracket matching the one at each given position.
/// A position that is not on a bracket comes back unchanged, so the reply
/// lines up with the request one for one.
pub enum MatchingBrace {}

impl lsp_types::request::Request for MatchingBrace {
    type Params = MatchingBraceParams;
    type Result = Vec<lsp_types::Position>;
    const METHOD: &'static str = "experimental/matchingBrace";
}

/// Parameters for `experimental/joinLines`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinLinesParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub ranges: Vec<lsp_types::Range>,
}

/// Custom request: `experimental/joinLines`.
pub enum JoinLines {}

impl lsp_types::request::Request for JoinLines {
    type Params = JoinLinesParams;
    type Result = Vec<lsp_types::TextEdit>;
    const METHOD: &'static str = "experimental/joinLines";
}

/// Custom request: `experimental/onEnter`.
///
/// What to insert when Enter is pressed: continue a doc comment, keep the
/// indentation of the block just opened.
///
/// Asked *after* the newline is in the buffer, with the cursor on the fresh
/// line — the same question the `"\n"` on-type formatting trigger asks,
/// answered by the same code. rust-analyzer's is asked before, and its
/// edits carry the newline; a client written against that shape gets
/// `null` here, since the line before the cursor is not empty.
pub enum OnEnter {}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplainFormattingParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub options: Option<lsp_types::FormattingOptions>,
    pub range: Option<lsp_types::Range>,
}
pub enum ExplainFormatting {}
impl lsp_types::request::Request for ExplainFormatting {
    type Params = ExplainFormattingParams;
    type Result = serde_json::Value;
    const METHOD: &'static str = "sail-lsp/explainFormatting";
}

impl lsp_types::request::Request for OnEnter {
    type Params = lsp_types::TextDocumentPositionParams;
    type Result = Option<Vec<SnippetTextEdit>>;
    const METHOD: &'static str = "experimental/onEnter";
}

/// Which way `experimental/moveItem` moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MoveItemDirection {
    Up,
    Down,
}

/// Parameters for `experimental/moveItem`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveItemParams {
    pub direction: MoveItemDirection,
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub range: lsp_types::Range,
}

/// Custom request: `experimental/moveItem`.
///
/// Swap the definition, match arm or block item under the cursor with the
/// sibling above or below it.
pub enum MoveItem {}

impl lsp_types::request::Request for MoveItem {
    type Params = MoveItemParams;
    type Result = Vec<SnippetTextEdit>;
    const METHOD: &'static str = "experimental/moveItem";
}

/// `$/cancelRequest` notification — handles request cancellation.
pub enum CancelRequest {}

impl lsp_types::notification::Notification for CancelRequest {
    type Params = lsp_types::CancelParams;
    const METHOD: &'static str = "$/cancelRequest";
}

/// A text edit that may carry snippet syntax — `$0` for where the cursor
/// lands, `${1:name}` for something to type over.
///
/// rust-analyzer's extension, verbatim: the protocol has no snippet in a
/// workspace edit, so a client opts in with `experimental.snippetTextEdit`
/// and a client that does not gets the placeholders stripped.
#[derive(Debug, Eq, PartialEq, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetTextEdit {
    pub range: lsp_types::Range,
    pub new_text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insert_text_format: Option<lsp_types::InsertTextFormat>,
}

/// A workspace edit whose text edits may be snippets.
#[derive(Debug, Eq, PartialEq, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnippetWorkspaceEdit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_changes: Option<lsp_types::DocumentChanges>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<std::collections::HashMap<lsp_types::Url, Vec<SnippetTextEdit>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_annotations: Option<
        std::collections::HashMap<
            lsp_types::ChangeAnnotationIdentifier,
            lsp_types::ChangeAnnotation,
        >,
    >,
}

/// `textDocument/codeAction`'s result, with the edit widened to carry
/// snippets. rust-analyzer's, for the same reason: `lsp_types::CodeAction`
/// has a `WorkspaceEdit`, and a `WorkspaceEdit` has nowhere to say that an
/// edit's text is a snippet.
#[derive(Debug, PartialEq, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAction {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<lsp_types::Command>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<lsp_types::CodeActionKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<lsp_types::Diagnostic>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit: Option<SnippetWorkspaceEdit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_preferred: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub enum CodeActionRequest {}

impl lsp_types::request::Request for CodeActionRequest {
    type Params = lsp_types::CodeActionParams;
    type Result = Option<Vec<CodeAction>>;
    const METHOD: &'static str = "textDocument/codeAction";
}

pub enum CodeActionResolveRequest {}

impl lsp_types::request::Request for CodeActionResolveRequest {
    type Params = CodeAction;
    type Result = CodeAction;
    const METHOD: &'static str = "codeAction/resolve";
}

/// Preview semantic structural search/replace. Selections limit edits to the
/// context file; an empty list searches project sources.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SsrParams {
    pub query: String,
    #[serde(default)]
    pub parse_only: bool,
    #[serde(flatten)]
    pub position: lsp_types::TextDocumentPositionParams,
    #[serde(default)]
    pub selections: Vec<lsp_types::Range>,
}
pub enum Ssr {}
/// Read-only expression search; query contains a pattern without `==>>`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SsrSearchParams {
    pub query: String,
    #[serde(flatten)]
    pub position: lsp_types::TextDocumentPositionParams,
    #[serde(default)]
    pub selections: Vec<lsp_types::Range>,
}
pub enum SsrSearch {}
impl lsp_types::request::Request for SsrSearch {
    type Params = SsrSearchParams;
    type Result = Vec<lsp_types::Location>;
    const METHOD: &'static str = "sail-lsp/ssrSearch";
}
impl lsp_types::request::Request for Ssr {
    type Params = SsrParams;
    type Result = lsp_types::WorkspaceEdit;
    const METHOD: &'static str = "experimental/ssr";
}

/// Re-run checks for loaded projects and standalone open documents.
pub enum Recheck {}
impl lsp_types::notification::Notification for Recheck {
    type Params = ();
    const METHOD: &'static str = "sail-lsp/recheck";
}

/// Optional scope for Flycheck commands. With a document, only the loaded
/// projects (or standalone file) that contain it are affected. Without one,
/// the command applies to every check target.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlycheckControlParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_document: Option<lsp_types::TextDocumentIdentifier>,
}

pub enum RunFlycheck {}
impl lsp_types::notification::Notification for RunFlycheck {
    type Params = FlycheckControlParams;
    const METHOD: &'static str = "sail-lsp/runFlycheck";
}

/// RA-compatible discovery shape. Native clients execute tasks directly;
/// standard source actions use the advertised server command fallback.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnablesParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub position: Option<lsp_types::Position>,
}

pub enum Runnables {}
impl lsp_types::request::Request for Runnables {
    type Params = RunnablesParams;
    type Result = Vec<Runnable>;
    const METHOD: &'static str = "experimental/runnables";
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Runnable {
    pub id: String,
    pub label: String,
    pub project: lsp_types::TextDocumentIdentifier,
    pub fingerprint: String,
    /// Revalidate discovery before execution; do not execute a disabled entry.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<String>,
    pub documents: Vec<lsp_types::Url>,
    /// Where the entry belongs in the source — rust-analyzer's
    /// `Runnable::location`: a `$[test]` function or `main`. A project task
    /// has none and is shown at the top of the file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<lsp_types::Location>,
    #[serde(flatten)]
    pub command: RunnableCommand,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "args")]
pub enum RunnableCommand {
    #[serde(rename = "sail-check")]
    Check(FlycheckControlParams),
    #[serde(rename = "shell")]
    Process(ProcessRunnableArgs),
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessRunnableArgs {
    pub kind: crate::config::ProjectTaskKind,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
    pub environment: std::collections::BTreeMap<String, String>,
    /// An interpreter script the program reads from a file: `args` end with
    /// the flag that names it (`--is`), and the client stores these contents
    /// in a file and appends its path. Upstream Sail has no test runner;
    /// running a `$[test]` function means evaluating it in `sail`'s
    /// interpreter, and the script is what the interpreter is told to do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

pub enum CancelFlycheck {}
impl lsp_types::notification::Notification for CancelFlycheck {
    type Params = FlycheckControlParams;
    const METHOD: &'static str = "sail-lsp/cancelFlycheck";
}

pub enum ClearFlycheck {}
impl lsp_types::notification::Notification for ClearFlycheck {
    type Params = FlycheckControlParams;
    const METHOD: &'static str = "sail-lsp/clearFlycheck";
}

/// Resolved module graph and build configurations, also used to select the
/// analysis context of source files shared by several loaded projects.
pub enum ProjectGraph {}
impl lsp_types::request::Request for ProjectGraph {
    type Params = ();
    type Result = serde_json::Value;
    const METHOD: &'static str = "sail-lsp/projectGraph";
}

pub enum ExplainAnalysis {}
impl lsp_types::request::Request for ExplainAnalysis {
    type Params = lsp_types::TextDocumentPositionParams;
    type Result = serde_json::Value;
    const METHOD: &'static str = "sail-lsp/explainAnalysis";
}

/// Emitted after a new project model is installed, including configuration
/// reloads. Consumers should request the graph again and discard older replies.
pub enum ProjectGraphChanged {}
impl lsp_types::notification::Notification for ProjectGraphChanged {
    type Params = ();
    const METHOD: &'static str = "sail-lsp/projectGraphChanged";
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectIncludeContextParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
    pub context: Option<String>,
}
pub enum IncludeContexts {}
impl lsp_types::request::Request for IncludeContexts {
    type Params = lsp_types::TextDocumentIdentifier;
    type Result = serde_json::Value;
    const METHOD: &'static str = "sail-lsp/includeContexts";
}
pub enum SelectIncludeContext {}
impl lsp_types::request::Request for SelectIncludeContext {
    type Params = SelectIncludeContextParams;
    type Result = ();
    const METHOD: &'static str = "sail-lsp/selectIncludeContext";
}

/// Includes the exact source used to compute each edit, including closed files.
pub enum SsrPreview {}
impl lsp_types::request::Request for SsrPreview {
    type Params = SsrParams;
    type Result = serde_json::Value;
    const METHOD: &'static str = "sail-lsp/ssrPreview";
}
