use crate::diagnostics::Severity;
use crate::state::File;
use sail_parser::Span;

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
    Location {
        prefix: String,
        hint: Option<String>,
        span: Span,
        message: Box<Message>,
    },
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
        Self::Location {
            prefix: prefix.into(),
            hint,
            span,
            message: Box::new(message),
        }
    }
}

fn short_span(file: &File, span: Span) -> String {
    let start = file.source.position_at(span.start);
    let end = file.source.position_at(span.end);
    format!(
        "{}:{}-{}:{}",
        start.line + 1,
        start.character + 1,
        end.line + 1,
        end.character + 1
    )
}

fn push_non_empty(lines: &mut Vec<String>, indent: &str, text: &str) {
    if text.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(format!("{indent}{text}"));
    }
}

fn render_into(file: &File, message: &Message, indent: &str, lines: &mut Vec<String>) {
    match message {
        Message::Location {
            prefix,
            hint,
            span,
            message,
        } => {
            let mut header = String::new();
            if !prefix.is_empty() {
                header.push_str(prefix);
            }
            if let Some(hint) = hint {
                if !header.is_empty() {
                    header.push(' ');
                }
                header.push_str(hint);
            }
            if !header.is_empty() {
                header.push(' ');
            }
            header.push('(');
            header.push_str(&short_span(file, *span));
            header.push(')');
            push_non_empty(lines, indent, &header);
            render_into(file, message, &format!("{indent}  "), lines);
        }
        Message::Line(text) => push_non_empty(lines, indent, text),
        Message::List(items) => {
            for (header, message) in items {
                push_non_empty(lines, indent, &format!("* {header}"));
                render_into(file, message, &format!("{indent}  "), lines);
            }
        }
        Message::Seq(messages) => {
            for message in messages {
                render_into(file, message, indent, lines);
            }
        }
        Message::Severity(_, message) => render_into(file, message, indent, lines),
    }
}

pub(crate) fn render_message(file: &File, message: &Message) -> String {
    let mut lines = Vec::new();
    render_into(file, message, "", &mut lines);

    while matches!(lines.last(), Some(last) if last.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}
