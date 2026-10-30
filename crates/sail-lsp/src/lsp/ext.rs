//! LSP ↔ internal type conversion functions.
//!
//! All LSP-specific type conversions live here so the IDE crates
//! (ide, ide-db, etc.) can be framework-independent.
//!
//! Five position converters used to sit here with no caller, each the
//! same conversion as one in `from_proto` or `to_proto`
//! (`offset_from_position` / `from_proto::offset`,
//! `range_from_text_range` / `to_proto::range`, and so on). Two doors is
//! how one of them stops being maintained.

// The two conversions that used to be here assigned
// `Position::character` to a byte column and back:
//
//     LineCol { line: pos.line, col: pos.character }
//     Position::new(lc.line, lc.col)
//
// `character` is a UTF-16 column. Twenty-seven handlers called this pair,
// so the conversion fixed in `from_proto`/`to_proto` reached almost
// nothing — and `protocol_check`, which exercises those two modules,
// reported zero the whole time. They are `from_proto::line_col` and
// `to_proto::position_from_line_col` now, both of which need a
// `LineIndex`, which is what makes the wrong version unwritable.

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

/// Parameters for `sail-lsp/effectAnnotations`.
///
/// Sail-specific: returns per-function effect annotations for the
/// given file, used for the effect code lens overlay.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectAnnotationsParams {
    pub text_document: lsp_types::TextDocumentIdentifier,
}

/// A single effect annotation for a callable.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectAnnotationItem {
    pub name: String,
    pub range: lsp_types::Range,
    pub effects: Vec<String>,
}

/// Custom request: `sail-lsp/effectAnnotations`.
///
/// Returns effect annotations (inferred effects: throw, wreg, etc.)
/// for all callables in a file.
pub enum EffectAnnotations {}

impl lsp_types::request::Request for EffectAnnotations {
    type Params = EffectAnnotationsParams;
    type Result = Vec<EffectAnnotationItem>;
    const METHOD: &'static str = "sail-lsp/effectAnnotations";
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
pub enum OnEnter {}

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
impl lsp_types::request::Request for Ssr {
    type Params = SsrParams;
    type Result = lsp_types::WorkspaceEdit;
    const METHOD: &'static str = "experimental/ssr";
}
