//! Structured diagnostic messages.
//!
//! Canonical location: hir-def (accessible to hir-ty without
//! depending on ide-diagnostics). Moved from ide-diagnostics
//! in to break the hir-ty → ide-diagnostics cycle.

use crate::diagnostics::Severity;
use parser::Span;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageSeverity {
    Warning,
    Error,
}

impl From<MessageSeverity> for Severity {
    fn from(value: MessageSeverity) -> Self {
        match value {
            MessageSeverity::Warning => Severity::Warning,
            MessageSeverity::Error => Severity::Error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    Location { prefix: String, hint: Option<String>, span: Span, message: Box<Message> },
    Line(String),
    List(Vec<(String, Message)>),
    Seq(Vec<Message>),
    Severity(MessageSeverity, Box<Message>),
}

impl Message {
    /// Render explanation text without embedding source offsets or styling.
    /// Locations are carried separately by IDE clients as navigable links.
    pub fn plain_text(&self) -> String {
        match self {
            Self::Line(text) => text.clone(),
            Self::Seq(messages) => messages
                .iter()
                .map(Self::plain_text)
                .filter(|text| !text.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
            Self::List(messages) => messages
                .iter()
                .map(|(label, message)| format!("{label}: {}", message.plain_text()))
                .collect::<Vec<_>>()
                .join("\n"),
            Self::Location { prefix, hint, message, .. } => {
                [prefix.clone(), hint.clone().unwrap_or_default(), message.plain_text()]
                    .into_iter()
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            Self::Severity(_, message) => message.plain_text(),
        }
    }

    pub fn line(text: impl Into<String>) -> Self {
        Self::Line(text.into())
    }

    pub fn seq(messages: impl IntoIterator<Item = Message>) -> Self {
        Self::Seq(messages.into_iter().collect())
    }

    pub fn location(
        prefix: impl Into<String>,
        hint: Option<String>,
        span: Span,
        message: Message,
    ) -> Self {
        Self::Location { prefix: prefix.into(), hint, span, message: Box::new(message) }
    }
}
