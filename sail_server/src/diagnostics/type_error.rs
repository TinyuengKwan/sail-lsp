use crate::diagnostics::reporting::{Message, MessageSeverity};
use sail_parser::Span;

// This mirrors the relevant upstream Type_error surface; not every constructor
// is emitted by the current local analyses yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VectorOrder {
    Dec,
    Inc,
}

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
    Other(String),
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
    Alternate {
        primary: Box<TypeError>,
        reasons: Vec<(String, Span, Box<TypeError>)>,
    },
}

impl TypeError {
    pub fn other(message: impl Into<String>) -> Self {
        Self::Other(message.into())
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
            TypeError::Other(message) => (Message::line(message.clone()), None),
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
