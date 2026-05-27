//! LSP request and notification handlers.
//! - `dispatch.rs` — RequestDispatcher with on_sync/on async pattern
//! - `request.rs` — Individual request handler functions
//! - `notification.rs` — Notification handler functions

pub(crate) mod dispatch;
pub(crate) mod notification;
pub(crate) mod request;
