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
}

impl ProgressReporter {
    /// Start a new progress sequence. Sends `WorkDoneProgressBegin`.
    pub fn begin(sender: &Sender<Message>, title: &str) -> Self {
        let token =
            lsp_types::ProgressToken::String(format!("sail-lsp/{}", title.replace(' ', "-")));
        let begin = WorkDoneProgress::Begin(WorkDoneProgressBegin {
            title: title.to_string(),
            cancellable: Some(false),
            message: None,
            percentage: Some(0),
        });
        send_progress(sender, &token, begin);
        Self { sender: sender.clone(), token }
    }

    /// Report intermediate progress.
    pub fn report(&self, message: &str, percentage: Option<u32>) {
        let report = WorkDoneProgress::Report(WorkDoneProgressReport {
            cancellable: Some(false),
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

/// Check whether the client supports `window/workDoneProgress`.
pub fn client_supports_progress(caps: &lsp_types::ClientCapabilities) -> bool {
    caps.window.as_ref().and_then(|w| w.work_done_progress).unwrap_or(false)
}
