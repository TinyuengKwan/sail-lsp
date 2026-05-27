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
