//! WorkDoneProgress helper.
//!
//! `$/progress` LSP notification with `Begin/Report/End` payloads.

use crossbeam_channel::Sender;
use lsp_server::{Message, Notification};
use lsp_types::notification::Notification as _;
use lsp_types::{
    ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressEnd, WorkDoneProgressReport,
};

/// Manages a single progress token's lifecycle (Begin → Report* → End).
pub struct ProgressReporter {
    sender: Sender<Message>,
    token: lsp_types::ProgressToken,
    cancellable: bool,
}

impl ProgressReporter {
    /// Start a new progress sequence. Sends `WorkDoneProgressBegin`.
    ///
    /// The token must already have been created with
    /// `window/workDoneProgress/create`: a server that reports progress
    /// under a token the client never agreed to is free to be ignored, and
    /// this used to invent one from the title. `GlobalState::progress_token`
    /// asks first — which needs the main thread, so the token is made there
    /// and carried to whichever worker reports.
    /// `cancellable` is what the client shows a cancel button for, and what
    /// makes it send `window/workDoneProgress/cancel`. Every reporter said
    /// `false`, so the one operation that *is* cancellable —
    /// `prime_caches`, which already carries a `CancellationToken` and
    /// restarts itself — could never be cancelled by the user.
    pub fn begin(
        sender: &Sender<Message>,
        token: lsp_types::ProgressToken,
        title: &str,
        cancellable: bool,
    ) -> Self {
        let begin = WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: title.to_string(),
            cancellable: Some(cancellable),
            message: None,
            percentage: Some(0),
        });
        send_progress(sender, &token, begin);
        Self { sender: sender.clone(), token, cancellable }
    }

    /// Report intermediate progress.
    pub fn report(&self, message: &str, percentage: Option<u32>) {
        let report = WorkDoneProgress::Report(WorkDoneProgressReport {
            cancellable: Some(self.cancellable),
            message: Some(message.to_string()),
            percentage,
        });
        send_progress(&self.sender, &self.token, report);
    }

    /// End the progress sequence. Sends `WorkDoneProgressEnd`.
    pub fn end(self, message: &str) {
        let end = WorkDoneProgress::End(WorkDoneProgressEnd { message: Some(message.to_string()) });
        send_progress(&self.sender, &self.token, end);
    }
}

fn send_progress(
    sender: &Sender<Message>,
    token: &lsp_types::ProgressToken,
    value: WorkDoneProgress,
) {
    let params =
        ProgressParams { token: token.clone(), value: ProgressParamsValue::WorkDone(value) };
    let notif = Notification::new(lsp_types::notification::Progress::METHOD.to_string(), params);
    let _ = sender.send(Message::Notification(notif));
}

// `client_supports_progress` was here, asking the same question as
// `ClientCapabilities::work_done_progress_support` — which had no caller.
