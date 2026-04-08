use std::collections::HashSet;

use tower_lsp::lsp_types::{DiagnosticTag, Range};

use crate::diagnostics::type_error::TypeError;
use crate::diagnostics::{Diagnostic, DiagnosticCode, Severity};
use crate::state::File;
use sail_parser::Span;

// ---------------------------------------------------------------------------
// Structured error messages (formerly error_format.rs)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Diagnostic construction helpers
// ---------------------------------------------------------------------------

// Keep the full upstream-style error surface even though the current LSP
// only constructs a subset of these variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    General { span: Span, message: String },
    Todo { span: Span, message: String },
    Syntax { span: Span, message: String },
    Lex { span: Span, message: String },
    Type { span: Span, error: TypeError },
}

fn range(file: &File, span: Span) -> Range {
    Range::new(
        file.source.position_at(span.start),
        file.source.position_at(span.end),
    )
}

fn message_with_hint(message: String, hint: Option<String>) -> String {
    match hint {
        Some(hint) if !hint.is_empty() && !message.is_empty() => format!("{message}\n\n{hint}"),
        Some(hint) => hint,
        None => message,
    }
}

pub(crate) fn diagnostic_for_error(file: &File, code: DiagnosticCode, error: Error) -> Diagnostic {
    match error {
        Error::General { span, message } | Error::Todo { span, message } => {
            Diagnostic::new(code, message, range(file, span), Severity::Error)
        }
        Error::Syntax { span, message } => {
            Diagnostic::new(code, message, range(file, span), Severity::Error)
        }
        Error::Lex { span, message } => {
            Diagnostic::new(code, message, range(file, span), Severity::Error)
        }
        Error::Type { span, error } => {
            let (message, hint) = error.message();
            let message = render_message(file, &message);
            Diagnostic::new(
                code,
                message_with_hint(message, hint),
                range(file, span),
                Severity::Error,
            )
        }
    }
}

pub(crate) fn diagnostic_for_message(
    file: &File,
    code: DiagnosticCode,
    span: Span,
    severity: Severity,
    message: Message,
) -> Diagnostic {
    Diagnostic::new(
        code,
        render_message(file, &message),
        range(file, span),
        severity,
    )
}

pub(crate) fn diagnostic_for_warning(
    file: &File,
    code: DiagnosticCode,
    span: Span,
    explanation: Message,
) -> Diagnostic {
    diagnostic_for_message(file, code, span, Severity::Warning, explanation)
}

pub(crate) fn unnecessary_warning(
    file: &File,
    code: DiagnosticCode,
    span: Span,
    explanation: Message,
    severity: Severity,
) -> Diagnostic {
    diagnostic_for_message(file, code, span, severity, explanation)
        .with_tags(vec![DiagnosticTag::UNNECESSARY])
}

pub(crate) struct WarningEmitter {
    seen: HashSet<(DiagnosticCode, usize, usize, String)>,
}

impl WarningEmitter {
    pub(crate) fn new() -> Self {
        Self {
            seen: HashSet::new(),
        }
    }

    pub(crate) fn warn(
        &mut self,
        file: &File,
        diagnostics: &mut Vec<Diagnostic>,
        code: DiagnosticCode,
        short: impl Into<String>,
        span: Span,
        explanation: Message,
    ) {
        let short = short.into();
        if !self
            .seen
            .insert((code.clone(), span.start, span.end, short))
        {
            return;
        }
        diagnostics.push(diagnostic_for_warning(file, code, span, explanation));
    }
}
