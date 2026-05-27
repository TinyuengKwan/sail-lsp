use std::collections::HashSet;

use crate::type_error::TypeError;
use crate::types::DiagnosticCode;
use hir_def::diagnostics::{Diagnostic, Severity};
use ide_db::FileDb;
use parser::Span;

// Re-export Message + MessageSeverity through this module so the
// legacy `diagnostics::reporting::Message` path keeps working in
// sail_server callers (semantic.rs, etc.).
pub use crate::message::{Message, MessageSeverity};

fn short_span(file: &dyn FileDb, span: Span) -> String {
    let start = file.position_at(span.start);
    let end = file.position_at(span.end);
    format!("{}:{}-{}:{}", start.line + 1, start.col + 1, end.line + 1, end.col + 1)
}

fn push_non_empty(lines: &mut Vec<String>, indent: &str, text: &str) {
    if text.is_empty() {
        lines.push(String::new());
    } else {
        lines.push(format!("{indent}{text}"));
    }
}

fn render_into(file: &dyn FileDb, message: &Message, indent: &str, lines: &mut Vec<String>) {
    match message {
        Message::Location { prefix, hint, span, message } => {
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

pub fn render_message(file: &dyn FileDb, message: &Message) -> String {
    let mut lines = Vec::new();
    render_into(file, message, "", &mut lines);

    while matches!(lines.last(), Some(last) if last.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    Syntax { span: Span, message: String },
    Lex { span: Span, message: String },
    Type { span: Span, error: TypeError },
}

fn range(_file: &dyn FileDb, span: Span) -> ide_db::line_index::TextRange {
    base_db::text_range(span.start, span.end)
}

fn message_with_hint(message: String, hint: Option<String>) -> String {
    match hint {
        Some(hint) if !hint.is_empty() && !message.is_empty() => format!("{message}\n\n{hint}"),
        Some(hint) => hint,
        None => message,
    }
}

pub fn diagnostic_for_error(file: &dyn FileDb, code: DiagnosticCode, error: Error) -> Diagnostic {
    match error {
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

pub fn diagnostic_for_message(
    file: &dyn FileDb,
    code: DiagnosticCode,
    span: Span,
    severity: Severity,
    message: Message,
) -> Diagnostic {
    Diagnostic::new(code, render_message(file, &message), range(file, span), severity)
}

pub fn diagnostic_for_warning(
    file: &dyn FileDb,
    code: DiagnosticCode,
    span: Span,
    explanation: Message,
) -> Diagnostic {
    diagnostic_for_message(file, code, span, Severity::Warning, explanation)
}

pub fn unnecessary_warning(
    file: &dyn FileDb,
    code: DiagnosticCode,
    span: Span,
    explanation: Message,
    severity: Severity,
) -> Diagnostic {
    diagnostic_for_message(file, code, span, severity, explanation)
        .with_tags(vec![hir_def::diagnostics::DiagnosticTag::Unnecessary])
}

pub struct WarningEmitter {
    seen: HashSet<(DiagnosticCode, usize, usize, String)>,
}

impl WarningEmitter {
    pub fn new() -> Self {
        Self { seen: HashSet::new() }
    }

    pub fn warn(
        &mut self,
        file: &dyn FileDb,
        diagnostics: &mut Vec<Diagnostic>,
        code: DiagnosticCode,
        short: impl Into<String>,
        span: Span,
        explanation: Message,
    ) {
        let short = short.into();
        if !self.seen.insert((code.clone(), span.start, span.end, short)) {
            return;
        }
        diagnostics.push(diagnostic_for_warning(file, code, span, explanation));
    }
}
