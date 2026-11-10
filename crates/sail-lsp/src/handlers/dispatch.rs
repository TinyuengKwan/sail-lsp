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
///
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

/// A handler whose answer comes with facts about the analysis that produced
/// it; see [`RequestDispatcher::on_reporting`].
type ReportingHandler<R> =
    fn(
        GlobalStateSnapshot,
        <R as LspRequest>::Params,
    ) -> anyhow::Result<(<R as LspRequest>::Result, crate::analysis_health::PullReport)>;

impl RequestDispatcher<'_> {
    /// Handle request synchronously on the main thread with mutable state.
    /// Use for state-modifying requests (workspace reload, config change).
    /// NOT wrapped in catch_unwind — handler has full &mut GlobalState.
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
            Ok(it) => it,
            Err(response) => {
                self.global_state.respond(response);
                return self;
            }
        };

        let result = f(self.global_state, params);
        let response = result_to_response::<R>(id.clone(), result).unwrap_or_else(|_| Response {
            id,
            result: None,
            error: Some(on_cancelled()),
        });
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

        let uri = crate::request_batch::document_uri(&req);
        let (id, params) = match parse::<R>(req) {
            Ok(it) => it,
            Err(response) => {
                self.global_state.respond(response);
                return self;
            }
        };

        // The client may have cancelled while this sat in the queue; the
        // cancellation already answered it.
        if self.global_state.is_completed(&id) {
            return self;
        }
        let snap = self.global_state.snapshot_for_uri(uri.as_ref());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ctx = stdx::panic_context::enter(panic_frame::<R>(&id));
            f(snap, params)
        }));
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

    /// Dispatch ordinary background work. Retry policy is independent of priority.
    pub(crate) fn on<const ALLOW_RETRYING: bool, R>(
        &mut self,
        f: fn(GlobalStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: LspRequest,
        R::Params: for<'de> serde::Deserialize<'de> + Send + 'static,
        R::Result: serde::Serialize + 'static,
    {
        self.on_with_thread_intent::<ALLOW_RETRYING, R, R::Result>(
            stdx::thread::ThreadIntent::Worker,
            f,
            |result| (result, None),
        )
    }

    /// Dispatch interactive work, including requests which must not be retried.
    pub(crate) fn on_latency_sensitive<const ALLOW_RETRYING: bool, R>(
        &mut self,
        f: fn(GlobalStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
    ) -> &mut Self
    where
        R: LspRequest,
        R::Params: for<'de> serde::Deserialize<'de> + Send + 'static,
        R::Result: serde::Serialize + 'static,
    {
        self.on_with_thread_intent::<ALLOW_RETRYING, R, R::Result>(
            stdx::thread::ThreadIntent::LatencySensitive,
            f,
            |result| (result, None),
        )
    }

    /// Background work whose answer also carries facts about the analysis
    /// that produced it. The facts reach the main loop as a task of their
    /// own, ahead of the response — the way a diagnostics wave reports what
    /// it saw — so a handler holding a read-only snapshot still records
    /// what a pushing client's wave would have.
    pub(crate) fn on_reporting<const ALLOW_RETRYING: bool, R>(
        &mut self,
        f: ReportingHandler<R>,
    ) -> &mut Self
    where
        R: LspRequest,
        R::Params: for<'de> serde::Deserialize<'de> + Send + 'static,
        R::Result: serde::Serialize + Send + 'static,
    {
        self.on_with_thread_intent::<ALLOW_RETRYING, R, (R::Result, crate::analysis_health::PullReport)>(
            stdx::thread::ThreadIntent::Worker,
            f,
            |(result, report)| {
                (result, Some(crate::main_loop::Task::PullDiagnosticsHealth(report)))
            },
        )
    }

    /// `finish` splits what the handler computed into the answer and, for
    /// `on_reporting`, the task carrying its side facts.
    fn on_with_thread_intent<const ALLOW_RETRYING: bool, R, T>(
        &mut self,
        intent: stdx::thread::ThreadIntent,
        f: fn(GlobalStateSnapshot, R::Params) -> anyhow::Result<T>,
        finish: fn(T) -> (R::Result, Option<crate::main_loop::Task>),
    ) -> &mut Self
    where
        R: LspRequest,
        R::Params: for<'de> serde::Deserialize<'de> + Send + 'static,
        R::Result: serde::Serialize + 'static,
        T: 'static,
    {
        let req = match self.req.as_ref() {
            Some(req) if req.method == R::METHOD => self.req.take().unwrap(),
            _ => return self,
        };

        let (id, params) = match parse::<R>(req.clone()) {
            Ok(it) => it,
            Err(response) => {
                self.global_state.respond(response);
                return self;
            }
        };

        if self.global_state.is_completed(&id) {
            return self;
        }
        let snapshot_started = std::time::Instant::now();
        let uri = crate::request_batch::document_uri(&req);
        let snap = self.global_state.snapshot_for_uri(uri.as_ref());
        let snapshot_us = snapshot_started.elapsed().as_micros();
        let queued = std::time::Instant::now();
        self.global_state.task_pool.spawn_with_sender(intent, move |sender| {
            let queue_us = queued.elapsed().as_micros();
            let handler_started = std::time::Instant::now();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ctx = stdx::panic_context::enter(panic_frame::<R>(&id));
                f(snap, params)
            }));
            let handler_us = handler_started.elapsed().as_micros();
            tracing::debug!(target: "sail_lsp::latency",
                "request_worker id={id} method={} snapshot_us={snapshot_us} queue_us={queue_us} handler_us={handler_us} panicked={}",
                R::METHOD, result.is_err());
            let (result, facts) = match result {
                Ok(Ok(computed)) => {
                    let (result, facts) = finish(computed);
                    (Ok(Ok(result)), facts)
                }
                Ok(Err(error)) => (Ok(Err(error)), None),
                Err(payload) => (Err(payload), None),
            };
            if let Some(facts) = facts {
                let _ = sender.send(facts);
            }
            let task = match thread_result_to_response::<R>(id.clone(), result) {
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
            };
            let _ = sender.send(task);
        });
        self
    }

    /// Finish dispatching. If no handler matched, respond with MethodNotFound.
    pub(crate) fn finish(&mut self) {
        if let Some(req) = self.req.take() {
            log::error!("unknown request: {}", req.method);
            let error = lsp_server::ResponseError {
                code: lsp_server::ErrorCode::MethodNotFound as i32,
                message: format!("unknown request method: {}", req.method),
                data: None,
            };
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
/// What a crash report should say beside the backtrace.
///
/// `catch_unwind` turns a handler panic into an error response, which keeps
/// the server alive and tells nobody *which* request died: the log gets a
/// bare `panicked at ..` and the user gets an error code. rust-analyzer
/// enters a `DbPanicContext` naming the method before calling the handler
/// (`handlers/dispatch.rs:92`) and prints the frames from its panic hook.
///
/// `stdx::panic_context` is that mechanism, complete — RAII frames and the
/// hook that prints them — and it had no caller anywhere in this workspace.
///
/// The method and the request id, not the parameters: a request body can
/// carry a whole document, and this ends up in a log a user may paste.
fn panic_frame<R: LspRequest>(id: &lsp_server::RequestId) -> String {
    format!("request: {} (id {id})", R::METHOD)
}

fn parse<R: LspRequest>(req: Request) -> Result<(lsp_server::RequestId, R::Params), Response>
where
    R::Params: for<'de> serde::Deserialize<'de>,
{
    // Use crate::from_json for deserialization with clear error messages.
    match crate::from_json::<R::Params>(R::METHOD, &req.params) {
        Ok(params) => Ok((req.id, params)),
        Err(err) => {
            log::error!("failed to deserialize request {}: {err}", R::METHOD);
            Err(Response::new_err(
                req.id,
                lsp_server::ErrorCode::InvalidParams as i32,
                err.to_string(),
            ))
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
    /// The salsa unwind, when the cancellation arrived as one. An
    /// `Analysis` method that runs inside `with_db` catches it and returns
    /// `CancelledError` instead, and that arrives here as an ordinary
    /// `Err` with nothing to carry.
    inner: Option<salsa::Cancelled>,
}

impl std::error::Error for HandlerCancelledError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.inner.as_ref().map(|it| it as &(dyn std::error::Error + 'static))
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
                if matches!(*cancelled, salsa::Cancelled::PropagatedPanic { .. }) {
                    return Ok(Response::new_err(
                        id,
                        lsp_server::ErrorCode::InternalError as i32,
                        "dependent query panicked".into(),
                    ));
                }
                return Err(HandlerCancelledError { inner: Some(*cancelled) });
            };

            Ok(Response::new_err(id, lsp_server::ErrorCode::InternalError as i32, message))
        }
    }
}

/// A protocol error with an intentional code, distinct from a server failure.
#[derive(Debug)]
pub(crate) struct RequestError {
    pub code: lsp_server::ErrorCode,
    pub message: String,
}
impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RequestError {}

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
        // A cancellation reaches here two ways: as a salsa unwind that got
        // this far, or — now that `Analysis` catches its own — as the
        // `CancelledError` a handler propagated with `?`. Both are
        // ContentModified, not InternalError.
        Err(e) if e.downcast_ref::<RequestError>().is_some() => {
            let error = e.downcast::<RequestError>().unwrap();
            Response::new_err(id, error.code as i32, error.message.to_owned())
        }
        Err(e)
            if e.downcast_ref::<ide_db::ide_types::CancelledError>()
                == Some(&ide_db::ide_types::CancelledError::PendingWrite) =>
        {
            return Err(HandlerCancelledError { inner: None })
        }
        Err(e) => match e.downcast::<salsa::Cancelled>() {
            Ok(salsa::Cancelled::PropagatedPanic { .. }) => Response::new_err(
                id,
                lsp_server::ErrorCode::InternalError as i32,
                "dependent query panicked".into(),
            ),
            Ok(cancelled) => return Err(HandlerCancelledError { inner: Some(cancelled) }),
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

    enum Probe {}
    impl LspRequest for Probe {
        type Params = bool;
        type Result = ();
        const METHOD: &'static str = "test/dispatch-policy";
    }

    fn probe(_snapshot: GlobalStateSnapshot, cancel: bool) -> anyhow::Result<()> {
        if cancel {
            Err(ide_db::ide_types::CancelledError::PendingWrite.into())
        } else {
            Ok(())
        }
    }

    #[test]
    fn cancellation_retry_is_independent_of_dispatch_priority() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let mut id = 0;
        for latency_sensitive in [false, true] {
            for retry in [false, true] {
                for cancel in [false, true] {
                    id += 1;
                    let request = Request::new(id.into(), Probe::METHOD.into(), cancel);
                    state.register_request(&request);
                    let mut dispatcher =
                        RequestDispatcher { req: Some(request), global_state: &mut state };
                    match (latency_sensitive, retry) {
                        (false, false) => dispatcher.on::<false, Probe>(probe),
                        (false, true) => dispatcher.on::<true, Probe>(probe),
                        (true, false) => dispatcher.on_latency_sensitive::<false, Probe>(probe),
                        (true, true) => dispatcher.on_latency_sensitive::<true, Probe>(probe),
                    }
                    .finish();
                    let task = tasks
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .expect("worker must answer");
                    match task {
                        crate::main_loop::Task::Retry(request) => {
                            assert!(cancel && retry);
                            assert_eq!(request.id, id.into());
                        }
                        crate::main_loop::Task::Response(response) => {
                            assert!(!cancel || !retry);
                            assert_eq!(response.id, id.into());
                            assert_eq!(
                                response.error.map(|e| e.code),
                                cancel.then_some(lsp_server::ErrorCode::ServerCancelled as i32)
                            );
                        }
                        _ => panic!("unexpected worker result"),
                    }
                }
            }
        }
    }

    #[test]
    fn cancelled_feature_queries_do_not_become_empty_successes() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let uri = lsp_types::Url::from_file_path(
            std::env::temp_dir().join("sail-lsp-cancelled-features.sail"),
        )
        .unwrap();
        crate::handlers::notification::handle_did_open_text_document(
            &mut state,
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "sail".into(),
                    version: 1,
                    text: "val answer : unit -> int\nfunction answer() = 42\n".into(),
                },
            },
        )
        .unwrap();
        state.process_changes();
        let snap = state.snapshot();
        let file = snap.analysis.file_id(&uri).unwrap();
        assert!(snap.analysis.file_by_id(file).is_some(), "fixture must be loaded");
        let mut host = std::mem::take(&mut state.analysis_host);
        let writer = std::thread::spawn(move || host.trigger_cancellation());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if snap.analysis.semantic_tokens(file).is_err() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "writer did not cancel the snapshot");
            std::thread::yield_now();
        }

        fn check<R: LspRequest>(
            snap: GlobalStateSnapshot,
            handler: fn(GlobalStateSnapshot, R::Params) -> anyhow::Result<R::Result>,
            params: serde_json::Value,
        ) -> bool
        where
            R::Params: serde::de::DeserializeOwned,
            R::Result: serde::Serialize,
        {
            let params = serde_json::from_value(params).unwrap();
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler(snap, params)));
            thread_result_to_response::<R>(1.into(), result).is_err()
        }
        let text_document = serde_json::json!({ "uri": uri });
        let cancelled = [
            check::<lsp_types::request::RangeFormatting>(
                snap.clone(),
                crate::handlers::request::handle_range_formatting,
                serde_json::json!({"textDocument": text_document, "range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}},"options":{"tabSize":2,"insertSpaces":true}}),
            ),
            check::<crate::lsp_ext::ExplainFormatting>(
                snap.clone(),
                crate::formatting::explain,
                serde_json::json!({"textDocument":text_document}),
            ),
            check::<lsp_types::request::OnTypeFormatting>(
                snap.clone(),
                crate::handlers::request::handle_on_type_formatting,
                serde_json::json!({"textDocument":text_document,"position":{"line":1,"character":0},"ch":"}","options":{"tabSize":2,"insertSpaces":true}}),
            ),
            check::<lsp_types::request::Formatting>(
                snap.clone(),
                crate::handlers::request::handle_formatting,
                serde_json::json!({"textDocument": text_document, "options": {"tabSize": 2, "insertSpaces": true}}),
            ),
            check::<lsp_types::request::SemanticTokensFullRequest>(
                snap.clone(),
                crate::handlers::request::handle_semantic_tokens_full,
                serde_json::json!({"textDocument": text_document}),
            ),
            check::<lsp_types::request::SemanticTokensFullDeltaRequest>(
                snap.clone(),
                crate::handlers::request::handle_semantic_tokens_full_delta,
                serde_json::json!({"textDocument": text_document, "previousResultId": "old"}),
            ),
            check::<lsp_types::request::CodeLensRequest>(
                snap.clone(),
                crate::handlers::request::handle_code_lens,
                serde_json::json!({"textDocument": text_document}),
            ),
        ];
        drop(snap);
        writer.join().unwrap();
        assert_eq!(cancelled, [true; 7], "a cancelled query must reach the dispatch policy");
    }

    #[test]
    fn parse_returns_invalid_params_on_bad_json() {
        let req = Request {
            id: 1.into(),
            method: lsp_types::request::GotoDefinition::METHOD.to_string(),
            params: serde_json::json!("not an object"),
        };
        let result = parse::<lsp_types::request::GotoDefinition>(req);
        assert_eq!(
            result.unwrap_err().error.unwrap().code,
            lsp_server::ErrorCode::InvalidParams as i32
        );
    }

    #[test]
    fn on_cancelled_has_correct_code() {
        let err = on_cancelled();
        assert_eq!(err.code, lsp_server::ErrorCode::ServerCancelled as i32);
    }
}
