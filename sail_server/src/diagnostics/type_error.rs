use crate::diagnostics::error_format::{Message, MessageSeverity};
use sail_parser::Span;

// This mirrors the relevant upstream Type_error surface; not every constructor
// is emitted by the current local analyses yet.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VectorOrder {
    Dec,
    Inc,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModuleScope {
    pub name: String,
    pub span: Span,
}

#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TypeError {
    NoOverloading {
        id: String,
        errors: Vec<(String, Span, Box<TypeError>)>,
    },
    UnresolvedQuants {
        id: String,
        quants: Vec<String>,
    },
    FailedConstraint {
        constraint: String,
        derived_from: Vec<Span>,
    },
    Subtype {
        lhs: String,
        rhs: String,
        constraint: Option<String>,
    },
    NoNumIdent {
        id: String,
    },
    Other(String),
    Inner {
        primary: Box<TypeError>,
        span: Span,
        prefix: String,
        secondary: Box<TypeError>,
    },
    NotInScope {
        explanation: Option<String>,
        location: Option<Span>,
        item_scope: Option<ModuleScope>,
        into_scope: Option<ModuleScope>,
        is_opened: bool,
        is_private: bool,
    },
    InstantiationInfo {
        heuristic: i32,
        error: Box<TypeError>,
    },
    FunctionArg {
        span: Span,
        ty: String,
        error: Box<TypeError>,
    },
    NoFunctionType {
        id: String,
        functions: Vec<String>,
    },
    UnboundId {
        id: String,
        locals: Vec<String>,
        have_function: bool,
    },
    VectorSubrange {
        first: String,
        second: String,
        order: VectorOrder,
    },
    Hint(String),
    WithHint {
        hint: String,
        error: Box<TypeError>,
    },
    Alternate {
        primary: Box<TypeError>,
        reasons: Vec<(String, Span, Box<TypeError>)>,
    },
}

impl TypeError {
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
    }

    #[allow(dead_code)]
    pub fn with_hint(hint: impl Into<String>, error: TypeError) -> Self {
        Self::WithHint {
            hint: hint.into(),
            error: Box::new(error),
        }
    }

    pub fn message(&self) -> (Message, Option<String>) {
        match self {
            TypeError::NoOverloading { id, errors } => {
                let list = errors
                    .iter()
                    .map(|(candidate, span, error)| {
                        let (message, hint) = error.message();
                        (
                            candidate.clone(),
                            Message::location(String::new(), hint, *span, message),
                        )
                    })
                    .collect::<Vec<_>>();
                (
                    Message::seq([
                        Message::line(format!("No overloading for {id}, tried:")),
                        Message::List(list),
                    ]),
                    Some(id.clone()),
                )
            }
            TypeError::UnresolvedQuants { id, quants } => (
                Message::seq([
                    Message::line(format!("Could not resolve quantifiers for {id}")),
                    Message::line(format!("* {}", quants.join("\n* "))),
                ]),
                None,
            ),
            TypeError::FailedConstraint {
                constraint,
                derived_from,
            } => {
                let mut messages = vec![Message::line(format!(
                    "Failed to prove constraint: {constraint}"
                ))];
                for span in derived_from {
                    messages.push(Message::location(
                        "constraint from ",
                        Some("This type argument".to_string()),
                        *span,
                        Message::seq([]),
                    ));
                }
                (Message::Seq(messages), None)
            }
            TypeError::Subtype {
                lhs,
                rhs,
                constraint,
            } => {
                let mut messages = vec![Message::Severity(
                    MessageSeverity::Warning,
                    Box::new(Message::line(format!("{lhs} is not a subtype of {rhs}"))),
                )];
                if let Some(constraint) = constraint {
                    messages.push(Message::line(format!(
                        "as {constraint} could not be proven"
                    )));
                }
                (Message::Seq(messages), None)
            }
            TypeError::NoNumIdent { id } => {
                (Message::line(format!("No num identifier {id}")), None)
            }
            TypeError::Other(message) => (Message::line(message.clone()), None),
            TypeError::Inner {
                primary,
                span,
                prefix,
                secondary,
            } => {
                let (primary_message, primary_hint) = primary.message();
                let (secondary_message, secondary_hint) = secondary.message();
                if primary == secondary {
                    (primary_message, primary_hint)
                } else {
                    (
                        Message::seq([
                            primary_message,
                            Message::line(""),
                            Message::location(
                                prefix.clone(),
                                secondary_hint,
                                *span,
                                secondary_message,
                            ),
                        ]),
                        primary_hint,
                    )
                }
            }
            TypeError::NotInScope {
                explanation,
                location,
                item_scope,
                into_scope,
                is_opened,
                is_private,
            } => {
                let message = explanation.clone().unwrap_or_else(|| {
                    if *is_private {
                        "Cannot use private definition".to_string()
                    } else {
                        "Not in scope".to_string()
                    }
                });
                match location {
                    Some(span) => {
                        let scope_suffix = item_scope
                            .as_ref()
                            .map(|scope| format!(" in {}", scope.name))
                            .unwrap_or_default();
                        let mut messages = vec![Message::line(message), Message::line("")];
                        if *is_private && !*is_opened {
                            messages.push(Message::line(
                                "The module containing this definition is also not required in this context",
                            ));
                            messages.push(Message::line(""));
                        }
                        let hint = if *is_private {
                            format!("private definition here{scope_suffix}")
                        } else {
                            format!("definition here{scope_suffix}")
                        };
                        messages.push(Message::location("", Some(hint), *span, Message::seq([])));
                        if let Some(scope) = into_scope {
                            messages.push(Message::location(
                                "",
                                Some(format!("add requires here for {}", scope.name)),
                                scope.span,
                                Message::seq([]),
                            ));
                        }
                        (Message::Seq(messages), None)
                    }
                    None => (Message::line(message), None),
                }
            }
            TypeError::InstantiationInfo { error, .. } => error.message(),
            TypeError::FunctionArg { ty, error, .. } => {
                let (message, hint) = error.message();
                (
                    message,
                    hint.or_else(|| Some(format!("checking function argument has type {ty}"))),
                )
            }
            TypeError::NoFunctionType { id, .. } => {
                (Message::line(format!("Function {id} not found")), None)
            }
            TypeError::UnboundId {
                id, have_function, ..
            } => {
                if *have_function {
                    (
                        Message::seq([
                            Message::line(format!("Identifier {id} is unbound")),
                            Message::line(""),
                            Message::line(format!("There is also a function {id} in scope.")),
                        ]),
                        None,
                    )
                } else {
                    (Message::line(format!("Identifier {id} is unbound")), None)
                }
            }
            TypeError::VectorSubrange {
                first,
                second,
                order,
            } => {
                let message = match order {
                    VectorOrder::Dec => format!(
                        "First index {first} must be greater than or equal to second index {second} (when default Order dec)"
                    ),
                    VectorOrder::Inc => format!(
                        "First index {first} must be less than or equal to second index {second} (when default Order inc)"
                    ),
                };
                (Message::line(message), None)
            }
            TypeError::Hint(hint) => (Message::seq([]), Some(hint.clone())),
            TypeError::WithHint { hint, error } => {
                let (message, _) = error.message();
                (message, Some(hint.clone()))
            }
            TypeError::Alternate { primary, reasons } => {
                let (message, hint) = primary.message();
                let reasons = reasons
                    .iter()
                    .map(|(header, span, error)| {
                        let (message, hint) = error.message();
                        (
                            header.clone(),
                            Message::location(String::new(), hint, *span, message),
                        )
                    })
                    .collect::<Vec<_>>();
                (Message::seq([message, Message::List(reasons)]), hint)
            }
        }
    }
}
