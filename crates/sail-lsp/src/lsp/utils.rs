//! LSP utility functions.

use lsp_server::Notification;

/// Check if a notification is of a specific type.
pub(crate) fn notification_is<N: lsp_types::notification::Notification>(
    notification: &Notification,
) -> bool {
    notification.method == N::METHOD
}

/// Create an LSP invalid-params error.
pub(crate) fn invalid_params_error(message: String) -> lsp_server::ResponseError {
    lsp_server::ResponseError {
        code: lsp_server::ErrorCode::InvalidParams as i32,
        message,
        data: None,
    }
}
