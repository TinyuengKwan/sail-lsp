//! LSP utility functions.

// There is no `notification_is`. Its one caller was
// `is_cancel_notification`, an early filter that did not exist either —
// `$/cancelRequest` is dispatched by `on_sync_mut` like every other
// notification.

/// Create an LSP invalid-params error.
pub(crate) fn invalid_params_error(message: String) -> lsp_server::ResponseError {
    lsp_server::ResponseError {
        code: lsp_server::ErrorCode::InvalidParams as i32,
        message,
        data: None,
    }
}
