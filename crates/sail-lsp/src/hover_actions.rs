//! Hover actions on the wire — rust-analyzer's `experimental.hoverActions`.
//!
//! LSP's `Hover` carries contents and a range. rust-analyzer extends the
//! response with an `actions` array of command groups (`lsp_ext::Hover`,
//! `CommandLinkGroup`, `CommandLink`) that the client renders as links
//! under the popup, and sends it only to a client that declared
//! `experimental.hoverActions`. The `ide` layer has attached
//! `HoverAction`s to every hover for a long time; `to_proto::hover`
//! dropped them.

use ide::hover::{HoverAction, HoverResult};
use serde::{Deserialize, Serialize};

/// `textDocument/hover` with the extended result type.
pub(crate) enum HoverRequest {}

impl lsp_types::request::Request for HoverRequest {
    type Params = lsp_types::HoverParams;
    type Result = Option<Hover>;
    const METHOD: &'static str =
        <lsp_types::request::HoverRequest as lsp_types::request::Request>::METHOD;
}

#[derive(Debug, PartialEq, Clone, Deserialize, Serialize)]
pub(crate) struct Hover {
    #[serde(flatten)]
    pub(crate) hover: lsp_types::Hover,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) actions: Vec<CommandLinkGroup>,
}

#[derive(Debug, PartialEq, Clone, Default, Deserialize, Serialize)]
pub(crate) struct CommandLinkGroup {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<String>,
    pub(crate) commands: Vec<CommandLink>,
}

/// LSP's `Command` has no tooltip; VS Code's rendering takes one.
#[derive(Debug, PartialEq, Clone, Default, Deserialize, Serialize)]
pub(crate) struct CommandLink {
    #[serde(flatten)]
    pub(crate) command: lsp_types::Command,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tooltip: Option<String>,
}

/// Where an action's command goes: the hovered declaration and the
/// locations the client lists in its peek view.
pub(crate) struct ActionTarget {
    pub(crate) uri: lsp_types::Url,
    pub(crate) position: lsp_types::Position,
    pub(crate) locations: Vec<lsp_types::Location>,
}

/// The hover response with its action groups.
///
/// `supported` is the client's `experimental.hoverActions`; without it the
/// `actions` field is left out and the response is a plain LSP hover.
/// `locate` is what the handler asks the analysis for each action — the
/// implementations of a `val`, the references of a function.
pub(crate) fn hover(
    line_index: &ide_db::line_index::LineIndex,
    result: &HoverResult,
    markdown: bool,
    supported: bool,
    locate: impl Fn(&HoverAction) -> Option<ActionTarget>,
) -> Hover {
    let actions = if supported {
        result.actions.iter().filter_map(|action| command_link_group(action, &locate)).collect()
    } else {
        Vec::new()
    };
    Hover { hover: crate::to_proto::hover(line_index, result, markdown), actions }
}

fn command_link_group(
    action: &HoverAction,
    locate: &impl Fn(&HoverAction) -> Option<ActionTarget>,
) -> Option<CommandLinkGroup> {
    let (title, tooltip): (fn(usize) -> String, &str) = match action {
        HoverAction::Implementation(_) => (implementation_title, "Go to implementations"),
        HoverAction::Reference(_) => (reference_title, "Go to references"),
        // Nothing in `ide::hover` builds one of these yet.
        HoverAction::GoToType(_) => return None,
    };
    let target = locate(action)?;
    let command = show_references(title(target.locations.len()), target);
    Some(CommandLinkGroup {
        title: None,
        commands: vec![CommandLink { command, tooltip: Some(tooltip.to_owned()) }],
    })
}

/// `editor.action.showReferences` wants VS Code types, so the client
/// converts the arguments in its own `sail-lsp.showReferences` command.
fn show_references(title: String, target: ActionTarget) -> lsp_types::Command {
    lsp_types::Command {
        title,
        command: "sail-lsp.showReferences".to_owned(),
        arguments: Some(vec![
            serde_json::to_value(target.uri).expect("a Url serializes"),
            serde_json::to_value(target.position).expect("a Position serializes"),
            serde_json::to_value(target.locations).expect("Locations serialize"),
        ]),
    }
}

fn implementation_title(count: usize) -> String {
    if count == 1 {
        "1 implementation".to_owned()
    } else {
        format!("{count} implementations")
    }
}

fn reference_title(count: usize) -> String {
    if count == 1 {
        "1 reference".to_owned()
    } else {
        format!("{count} references")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide::hover::HoverFilePosition;

    fn result_with(actions: Vec<HoverAction>) -> HoverResult {
        HoverResult {
            markup: "**function** **f**".to_owned(),
            actions,
            range: base_db::text_range(0, 1),
        }
    }

    fn at(file_id: u32, offset: usize) -> HoverFilePosition {
        HoverFilePosition { file_id: base_db::FileId::from_raw(file_id), offset }
    }

    fn two_locations(_: &HoverAction) -> Option<ActionTarget> {
        let uri = lsp_types::Url::parse("file:///tmp/a.sail").unwrap();
        let location = |line| {
            lsp_types::Location::new(
                uri.clone(),
                lsp_types::Range::new(
                    lsp_types::Position::new(line, 0),
                    lsp_types::Position::new(line, 6),
                ),
            )
        };
        Some(ActionTarget {
            uri: uri.clone(),
            position: lsp_types::Position::new(0, 4),
            locations: vec![location(3), location(7)],
        })
    }

    fn caps(
        experimental: Option<serde_json::Value>,
    ) -> crate::lsp::capabilities::ClientCapabilities {
        crate::lsp::capabilities::ClientCapabilities(lsp_types::ClientCapabilities {
            experimental,
            ..Default::default()
        })
    }

    #[test]
    fn the_capability_is_experimental_hover_actions() {
        assert!(!caps(None).hover_actions_support());
        assert!(!caps(Some(serde_json::json!({"hoverActions": false}))).hover_actions_support());
        assert!(caps(Some(serde_json::json!({"hoverActions": true}))).hover_actions_support());
    }

    /// Without the capability the response is a plain LSP hover: no
    /// `actions` key at all, and `locate` is never asked.
    #[test]
    fn a_client_without_the_capability_gets_a_plain_hover() {
        let line_index = ide_db::line_index::LineIndex::new("f\n");
        let result = result_with(vec![HoverAction::Reference(at(0, 0))]);
        let asked = std::cell::Cell::new(false);
        let hover = hover(&line_index, &result, true, false, |_| {
            asked.set(true);
            two_locations(&HoverAction::Reference(at(0, 0)))
        });
        assert!(hover.actions.is_empty());
        assert!(!asked.get(), "no client to render them, so nothing is looked up");
        let json = serde_json::to_value(&hover).unwrap();
        assert!(json.get("actions").is_none(), "{json}");
        assert!(json.get("contents").is_some(), "{json}");
        assert!(json.get("range").is_some(), "{json}");
    }

    #[test]
    fn a_client_with_the_capability_gets_show_references_command_links() {
        let line_index = ide_db::line_index::LineIndex::new("f\n");
        let result = result_with(vec![
            HoverAction::Implementation(at(0, 0)),
            HoverAction::Reference(at(0, 0)),
        ]);
        let hover = hover(&line_index, &result, true, true, two_locations);
        assert_eq!(hover.actions.len(), 2);
        let [implementations, references] = hover.actions.as_slice() else { unreachable!() };
        assert_eq!(implementations.commands[0].command.title, "2 implementations");
        assert_eq!(implementations.commands[0].tooltip.as_deref(), Some("Go to implementations"));
        assert_eq!(references.commands[0].command.title, "2 references");
        assert_eq!(references.commands[0].tooltip.as_deref(), Some("Go to references"));
        for group in &hover.actions {
            let command = &group.commands[0].command;
            assert_eq!(command.command, "sail-lsp.showReferences");
            let arguments = command.arguments.as_ref().unwrap();
            assert_eq!(arguments.len(), 3, "uri, position, locations");
            assert_eq!(arguments[0], serde_json::json!("file:///tmp/a.sail"));
            assert_eq!(arguments[1], serde_json::json!({"line": 0, "character": 4}));
            assert_eq!(arguments[2].as_array().map(Vec::len), Some(2));
        }
        let json = serde_json::to_value(&hover).unwrap();
        assert_eq!(json["actions"].as_array().map(Vec::len), Some(2), "{json}");
        assert_eq!(json["actions"][0]["commands"][0]["command"], "sail-lsp.showReferences");
        assert_eq!(json["actions"][0]["commands"][0]["tooltip"], "Go to implementations");
    }

    /// An action whose target cannot be located is left out rather than
    /// rendered as a dead link.
    #[test]
    fn an_unlocatable_action_is_dropped() {
        let line_index = ide_db::line_index::LineIndex::new("f\n");
        let result = result_with(vec![HoverAction::Reference(at(0, 0))]);
        let hover = hover(&line_index, &result, true, true, |_| None);
        assert!(hover.actions.is_empty());
    }

    #[test]
    fn titles_agree_in_number() {
        assert_eq!(implementation_title(1), "1 implementation");
        assert_eq!(implementation_title(0), "0 implementations");
        assert_eq!(reference_title(1), "1 reference");
        assert_eq!(reference_title(3), "3 references");
    }
}
