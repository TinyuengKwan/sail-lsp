//! LSP ↔ internal type conversion functions.
//!
//! All LSP-specific type conversions live here so the IDE crates
//! (ide, ide-db, etc.) can be framework-independent.

use ide_db::line_index::{LineCol, LineIndex, TextRange};
use lsp_types::{Position, Range};

/// Convert an LSP Position to an internal LineCol.
pub fn line_col_from_position(pos: Position) -> LineCol {
    LineCol { line: pos.line, col: pos.character }
}

/// Convert an LSP Position to a byte offset using a LineIndex.
#[allow(dead_code)]
pub fn offset_from_position(line_index: &LineIndex, pos: Position) -> usize {
    line_index.offset(line_col_from_position(pos))
}

/// Convert an LSP Range to an internal TextRange using a LineIndex.
#[allow(dead_code)]
pub fn text_range_from_range(line_index: &LineIndex, range: Range) -> TextRange {
    base_db::text_range(
        offset_from_position(line_index, range.start),
        offset_from_position(line_index, range.end),
    )
}

/// Convert an internal LineCol to an LSP Position.
pub fn position_from_line_col(lc: LineCol) -> Position {
    Position::new(lc.line, lc.col)
}

/// Convert a byte offset to an LSP Position using a LineIndex.
#[allow(dead_code)]
pub fn position_from_offset(line_index: &LineIndex, offset: usize) -> Position {
    position_from_line_col(line_index.line_col(offset))
}

/// Convert an internal TextRange to an LSP Range using a LineIndex.
#[allow(dead_code)]
pub fn range_from_text_range(line_index: &LineIndex, range: TextRange) -> Range {
    Range::new(
        position_from_offset(line_index, base_db::range_start(range)),
        position_from_offset(line_index, base_db::range_end(range)),
    )
}

/// Convert a parser::Span to an LSP Range using a LineIndex.
#[allow(dead_code)]
pub fn range_from_span(line_index: &LineIndex, span: parser::Span) -> Range {
    range_from_text_range(line_index, base_db::span_to_text_range(&span))
}

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

/// Convert ItemKind to LSP SymbolKind for workspace index entries.
pub fn item_kind_to_symbol_kind(kind: hir_def::item_tree::ItemKind) -> lsp_types::SymbolKind {
    use hir_def::item_tree::ItemKind;
    use lsp_types::SymbolKind as Lsp;
    match kind {
        ItemKind::Function | ItemKind::Mapping => Lsp::FUNCTION,
        ItemKind::ValSpec | ItemKind::MappingSpec => Lsp::FUNCTION,
        ItemKind::TypeAlias => Lsp::TYPE_PARAMETER,
        ItemKind::Struct | ItemKind::Bitfield | ItemKind::Newtype => Lsp::STRUCT,
        ItemKind::Union => Lsp::ENUM,
        ItemKind::Enum => Lsp::ENUM,
        ItemKind::Register => Lsp::VARIABLE,
        ItemKind::Let | ItemKind::Var => Lsp::VARIABLE,
        ItemKind::Overload => Lsp::FUNCTION,
        ItemKind::ScatteredHead | ItemKind::ScatteredClause => Lsp::FUNCTION,
        ItemKind::Constraint => Lsp::TYPE_PARAMETER,
        ItemKind::TerminationMeasure | ItemKind::EndMarker | ItemKind::Instantiation => Lsp::NULL,
    }
}

/// Convert a NavigationTarget to an LSP DocumentSymbol.
#[allow(deprecated, dead_code)]
pub fn document_symbol_from_nav(
    line_index: &LineIndex,
    nav: &ide_db::ide_types::NavigationTarget,
) -> lsp_types::DocumentSymbol {
    let range = range_from_text_range(line_index, nav.full_range);
    let selection_range = range_from_text_range(line_index, nav.focus_range);
    let children: Vec<lsp_types::DocumentSymbol> =
        nav.children.iter().map(|child| document_symbol_from_nav(line_index, child)).collect();

    lsp_types::DocumentSymbol {
        name: nav.name.clone(),
        detail: nav.detail.clone(),
        kind: lsp_symbol_kind(nav.kind),
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: if children.is_empty() { Some(Vec::new()) } else { Some(children) },
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
    use super::*;

    #[test]
    fn position_roundtrip() {
        let text = "val x : int\nfunction foo(x) = x + 1\n";
        let idx = LineIndex::new(text);

        // line 0, col 4 = byte offset 4
        let pos = Position::new(0, 4);
        let offset = offset_from_position(&idx, pos);
        assert_eq!(offset, 4);

        let back = position_from_offset(&idx, offset);
        assert_eq!(back, pos);
    }

    #[test]
    fn range_roundtrip() {
        let text = "val x : int\nfunction foo(x) = x + 1\n";
        let idx = LineIndex::new(text);

        let range = Range::new(Position::new(0, 0), Position::new(0, 11));
        let tr = text_range_from_range(&idx, range);
        assert_eq!(base_db::range_start(tr), 0);
        assert_eq!(base_db::range_end(tr), 11);

        let back = range_from_text_range(&idx, tr);
        assert_eq!(back, range);
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

/// Parameters for `experimental/ssr`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SsrParams {
    pub query: String,
}

/// Custom request: `experimental/ssr`.
///
/// Structural search-replace across the workspace.
pub enum Ssr {}

impl lsp_types::request::Request for Ssr {
    type Params = SsrParams;
    type Result = lsp_types::WorkspaceEdit;
    const METHOD: &'static str = "experimental/ssr";
}

/// `$/cancelRequest` notification — handles request cancellation.
pub enum CancelRequest {}

impl lsp_types::notification::Notification for CancelRequest {
    type Params = lsp_types::CancelParams;
    const METHOD: &'static str = "$/cancelRequest";
}
