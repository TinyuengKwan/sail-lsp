use std::collections::HashSet;

use tower_lsp::lsp_types::{DiagnosticTag, Range};

use crate::diagnostics::error_format::{render_message, Message};
use crate::diagnostics::type_error::TypeError;
use crate::diagnostics::{Diagnostic, DiagnosticCode, Severity};
use crate::state::File;
use sail_parser::Span;

// Keep the full upstream-style error surface even though the current LSP
// only constructs a subset of these variants.
#[allow(dead_code)]
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
