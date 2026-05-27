//! Type-safe request/notification dispatcher.
//!
//! - `RequestDispatcher` holds `&mut GlobalState`, creates snapshots internally
//! - `on_sync_mut`: run on main thread with `fn(&mut GlobalState, Params) -> Result<R>`
//! - `on_sync`: run on main thread with `fn(GlobalStateSnapshot, Params) -> Result<R>`
//! - `on`:      run on thread pool with same signature, `ALLOW_RETRYING` const param
//! - `NotificationDispatcher` holds `&mut GlobalState`
//! - `on_sync_mut`: run on main thread with `fn(&mut GlobalState, Params) -> Result<()>`
//!
//! Handler functions are plain `fn` pointers (not closures), living in
//! `handlers/request.rs` and `handlers/notification.rs`.

use std::fmt;

use lsp_server::{Notification, Request, Response};
use lsp_types::request::Request as LspRequest;

use crate::global_state::GlobalState;
use crate::global_state::GlobalStateSnapshot;

/// Type-safe request dispatcher.
/// - holds `&mut GlobalState` (not pool or snapshot)
/// - creates `GlobalStateSnapshot` inside each dispatch method
/// - handler functions are `fn` pointers (not closures)
/// Usage in main_loop `on_request()`:
/// ```ignore
/// RequestDispatcher { req: Some(req), global_state: &mut *self }
///     .on_sync_mut::<ReloadWorkspace>(handle_workspace_reload)
///     .on_sync::<DocumentSymbolRequest>(handle_document_symbol)
///     .on::<false, Completion>(handle_completion)
///     .on::<true, HoverRequest>(handle_hover)  // retryable
///     .finish();
/// ```
pub(crate) struct RequestDispatcher<'a> {
    pub(crate) req: Option<Request>,
    pub(crate) global_state: &'a mut GlobalState,
}

impl RequestDispatcher<'_> {
    /// Handle request synchronously on the main thread with mutable state.
    /// Use for state-modifying requests (workspace reload, config change).
    /// NOT wrapped in catch_unwind — handler has full &mut GlobalState.
    #[allow(dead_code)] // WIP: will be used for workspace reload request
    pub(crate) fn on_sync_mut<R>(
        &mut self,
        f: fn(&mut GlobalState, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: LspRequest,
        R::Params: for<'de> serde::Deserialize<'de>,
        R::Result: serde::Serialize,
    {
        let req = match self.req.as_ref() {
            Some(req) if req.method == R::METHOD => self.req.take().unwrap(),
            _ => return self,
        };

        let (id, params) = match parse::<R>(req) {
            Some(it) => it,
            None => return self,
        };

        let result = f(self.global_state, params);
        let response = match result {
            Ok(value) => match serde_json::to_value(value) {
                Ok(json) => Response::new_ok(id, json),
                Err(err) => Response::new_err(
                    id,
                    lsp_server::ErrorCode::InternalError as i32,
                    format!("serialization error: {err}"),
                ),
            },
            Err(err) => Response::new_err(
                id,
                lsp_server::ErrorCode::InternalError as i32,
                format!("{err:#}"),
            ),
        };
        self.global_state.respond(response);
        self
    }

    /// Handle request synchronously on the calling (main) thread.
    /// (formatting, semantic tokens, document symbols).
    /// Creates snapshot internally, calls handler, sends response.
    /// Wrapped in `catch_unwind` (salsa::Cancelled panic → ContentModified).
    pub(crate) fn on_sync<R>(
        &mut self,
        f: fn(GlobalStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: LspRequest,
        R::Params: for<'de> serde::Deserialize<'de>,
        R::Result: serde::Serialize,
    {
        let req = match self.req.as_ref() {
            Some(req) if req.method == R::METHOD => self.req.take().unwrap(),
            _ => return self,
        };

        let (id, params) = match parse::<R>(req) {
            Some(it) => it,
            None => return self,
        };

        let snap = self.global_state.snapshot();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(snap, params)));
        let response = match thread_result_to_response::<R>(id.clone(), result) {
            Ok(response) => response,
            Err(_cancelled) => {
                // Sync handlers are never retried — just return ContentModified.
                Response { id, result: None, error: Some(on_cancelled()) }
            }
        };
        self.global_state.respond(response);
        self
    }

    /// Dispatch request to the thread pool for async execution.
    /// Creates snapshot, moves it to worker thread, response flows back
    /// via `Task::Response` or `Task::Retry`.
    /// `ALLOW_RETRYING`:
    /// - `true`: on cancellation, the request is re-queued via `Task::Retry`
    ///   (used for latency-sensitive requests like completion/hover)
    /// - `false`: on cancellation, returns ServerCancelled error
    pub(crate) fn on<const ALLOW_RETRYING: bool, R>(
        &mut self,
        f: fn(GlobalStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: LspRequest,
        R::Params: for<'de> serde::Deserialize<'de> + Send + 'static,
        R::Result: serde::Serialize + 'static,
    {
        let req = match self.req.as_ref() {
            Some(req) if req.method == R::METHOD => self.req.take().unwrap(),
            _ => return self,
        };

        let (id, params) = match parse::<R>(req.clone()) {
            Some(it) => it,
            None => return self,
        };

        let snap = self.global_state.snapshot();
        self.global_state.task_pool.spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(snap, params)));
            match thread_result_to_response::<R>(id.clone(), result) {
                Ok(response) => crate::main_loop::Task::Response(response),
                // (dispatch.rs:268): retry on cancellation if allowed.
                Err(_cancelled) if ALLOW_RETRYING => crate::main_loop::Task::Retry(req),
                Err(_cancelled) => {
                    let error = on_cancelled();
                    crate::main_loop::Task::Response(Response {
                        id,
                        result: None,
                        error: Some(error),
                    })
                }
            }
        });
        self
    }

    /// Finish dispatching. If no handler matched, respond with MethodNotFound.
    pub(crate) fn finish(&mut self) {
        if let Some(req) = self.req.take() {
            log::error!("unknown request: {}", req.method);
            let error = crate::lsp::utils::invalid_params_error(format!(
                "unknown request method: {}",
                req.method
            ));
            let response = Response { id: req.id, result: None, error: Some(error) };
            self.global_state.respond(response);
        }
    }
}

use lsp_types::notification::Notification as LspNotification;

/// Type-safe notification dispatcher.
/// - holds `&mut GlobalState`
/// - `on_sync_mut` takes `fn(&mut GlobalState, Params) -> Result<()>`
pub(crate) struct NotificationDispatcher<'a> {
    pub(crate) not: Option<Notification>,
    pub(crate) global_state: &'a mut GlobalState,
}

impl NotificationDispatcher<'_> {
    /// Handle notification synchronously on the main thread.
    /// handlers run on main thread with mutable access to GlobalState.
    pub(crate) fn on_sync_mut<N>(
        &mut self,
        f: fn(&mut GlobalState, N::Params) -> anyhow::Result<()>,
    ) -> &mut Self
    where
        N: LspNotification,
        N::Params: for<'de> serde::Deserialize<'de>,
    {
        let not = match self.not.as_ref() {
            Some(not) if not.method == N::METHOD => self.not.take().unwrap(),
            _ => return self,
        };

        let params = match serde_json::from_value::<N::Params>(not.params) {
            Ok(p) => p,
            Err(err) => {
                log::error!("failed to deserialize notification {}: {err}", N::METHOD);
                return self;
            }
        };

        if let Err(err) = f(self.global_state, params) {
            log::error!("notification handler failed for {}: {err}", N::METHOD);
        }
        self
    }

    /// Finish dispatching. Logs unhandled notifications (except `$/` methods).
    pub(crate) fn finish(&mut self) {
        if let Some(not) = &self.not {
            if !not.method.starts_with("$/") {
                log::warn!("unhandled notification: {}", not.method);
            }
        }
    }
}

/// Parse a request into (id, params). Returns None on deserialization failure.
fn parse<R: LspRequest>(req: Request) -> Option<(lsp_server::RequestId, R::Params)>
where
    R::Params: for<'de> serde::Deserialize<'de>,
{
    // Use crate::from_json for deserialization with clear error messages.
    match crate::from_json::<R::Params>(R::METHOD, &req.params) {
        Ok(params) => Some((req.id, params)),
        Err(err) => {
            log::error!("failed to deserialize request {}: {err}", R::METHOD);
            None
        }
    }
}

/// Create a ServerCancelled error for the response.
fn on_cancelled() -> lsp_server::ResponseError {
    lsp_server::ResponseError {
        code: lsp_server::ErrorCode::ServerCancelled as i32,
        message: "server cancelled the request".to_owned(),
        data: None,
    }
}

/// Cancellation error wrapper for salsa::Cancelled.
#[derive(Debug)]
struct HandlerCancelledError {
    #[allow(dead_code)]
    inner: salsa::Cancelled,
}

impl std::error::Error for HandlerCancelledError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.inner)
    }
}

impl fmt::Display for HandlerCancelledError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Cancelled")
    }
}

/// Convert a thread result (possibly panicked) to an LSP response.
/// Catches panics from the handler thread:
/// - Normal result → delegate to `result_to_response`
/// - String/&str panic → InternalError response
/// - salsa::Cancelled panic → `Err(HandlerCancelledError)`
fn thread_result_to_response<R>(
    id: lsp_server::RequestId,
    result: std::thread::Result<anyhow::Result<R::Result>>,
) -> Result<Response, HandlerCancelledError>
where
    R: LspRequest,
    R::Params: for<'de> serde::Deserialize<'de>,
    R::Result: serde::Serialize,
{
    match result {
        Ok(result) => result_to_response::<R>(id, result),
        Err(panic) => {
            let panic_message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied());

            let mut message = "request handler panicked".to_owned();
            if let Some(panic_message) = panic_message {
                message.push_str(": ");
                message.push_str(panic_message);
            } else if let Ok(cancelled) = panic.downcast::<salsa::Cancelled>() {
                log::error!("Cancellation propagated out of salsa! This is a bug");
                return Err(HandlerCancelledError { inner: *cancelled });
            };

            Ok(Response::new_err(id, lsp_server::ErrorCode::InternalError as i32, message))
        }
    }
}

/// Convert a handler result to an LSP Response.
/// Downcasts anyhow errors to check for `salsa::Cancelled`:
/// - Ok → serialize to JSON response
/// - Err(Cancelled) → `Err(HandlerCancelledError)`
/// - Err(other) → InternalError response
fn result_to_response<R>(
    id: lsp_server::RequestId,
    result: anyhow::Result<R::Result>,
) -> Result<Response, HandlerCancelledError>
where
    R: LspRequest,
    R::Params: for<'de> serde::Deserialize<'de>,
    R::Result: serde::Serialize,
{
    let res = match result {
        Ok(resp) => match serde_json::to_value(&resp) {
            Ok(json) => Response::new_ok(id, json),
            Err(err) => Response::new_err(
                id,
                lsp_server::ErrorCode::InternalError as i32,
                format!("serialization error: {err}"),
            ),
        },
        Err(e) => match e.downcast::<salsa::Cancelled>() {
            Ok(cancelled) => return Err(HandlerCancelledError { inner: cancelled }),
            Err(e) => {
                Response::new_err(id, lsp_server::ErrorCode::InternalError as i32, e.to_string())
            }
        },
    };
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_returns_none_on_bad_json() {
        let req = Request {
            id: 1.into(),
            method: lsp_types::request::GotoDefinition::METHOD.to_string(),
            params: serde_json::json!("not an object"),
        };
        let result = parse::<lsp_types::request::GotoDefinition>(req);
        assert!(result.is_none());
    }

    #[test]
    fn on_cancelled_has_correct_code() {
        let err = on_cancelled();
        assert_eq!(err.code, lsp_server::ErrorCode::ServerCancelled as i32);
    }
}
