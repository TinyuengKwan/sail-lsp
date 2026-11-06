//! Discard obsolete reads before creating analysis snapshots. Notifications
//! still run in wire order; edits and commands are never discarded.
use lsp_server::{ErrorCode, Message, Request};
use lsp_types::Url;

pub(crate) const LIMIT: usize = 64;

/// lsp-server's stdio reader uses a rendezvous channel. A bounded forwarding
/// queue lets it receive subsequent edits while the main loop applies a change.
/// Shutdown interrupts both receive and send, including a full output queue.
pub(crate) struct BufferedMessages {
    pub receiver: crossbeam_channel::Receiver<Message>,
    stop: Option<crossbeam_channel::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl BufferedMessages {
    pub fn new(source: crossbeam_channel::Receiver<Message>) -> Self {
        let (sender, receiver) = crossbeam_channel::bounded(LIMIT);
        let (stop, stopped) = crossbeam_channel::bounded::<()>(0);
        let join = std::thread::Builder::new().name("sail-input".into()).spawn(move || loop {
            let message = crossbeam_channel::select! {
                recv(stopped) -> _ => break,
                recv(source) -> message => match message { Ok(message) => message, Err(_) => break },
            };
            crossbeam_channel::select! {
                recv(stopped) -> _ => break,
                send(sender, message) -> result => if result.is_err() { break },
            }
        }).expect("failed to start bounded LSP input reader");
        Self { receiver, stop: Some(stop), join: Some(join) }
    }
}

impl Drop for BufferedMessages {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub(crate) struct RequestState {
    pub document: Option<(Url, (i32, u64))>,
    pub retries: u8,
}

pub(crate) fn document_uri(request: &Request) -> Option<Url> {
    serde_json::from_value(request.params.get("textDocument")?.get("uri")?.clone()).ok()
}

pub(crate) fn superseded(
    request: &Request,
    version: Option<i32>,
    remaining: &[Message],
) -> Option<ErrorCode> {
    // Explicit client cancellation takes precedence over a newer revision.
    if remaining.iter().any(|message| match message {
        Message::Notification(n) if n.method == "$/cancelRequest" => {
            n.params.get("id") == serde_json::to_value(&request.id).ok().as_ref()
        }
        _ => false,
    }) {
        return Some(ErrorCode::RequestCanceled);
    }
    if !matches!(
        request.method.as_str(),
        "textDocument/hover"
            | "sail-lsp/ssrSearch"
            | "textDocument/completion"
            | "textDocument/signatureHelp"
            | "textDocument/definition"
            | "textDocument/references"
            | "textDocument/documentHighlight"
            | "textDocument/semanticTokens/full"
            | "textDocument/semanticTokens/full/delta"
            | "textDocument/semanticTokens/range"
            | "textDocument/inlayHint"
            | "textDocument/documentSymbol"
            | "textDocument/diagnostic"
    ) {
        return None;
    }
    let uri = document_uri(request)?;
    let version = version?;
    remaining.iter().find_map(|message| {
        let Message::Notification(n) = message else { return None };
        if n.method != "textDocument/didChange" {
            return None;
        }
        let change: lsp_types::DidChangeTextDocumentParams =
            serde_json::from_value(n.params.clone()).ok()?;
        (change.text_document.uri == uri && change.text_document.version > version)
            .then_some(ErrorCode::ServerCancelled)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stdio_reader_can_receive_a_burst_while_main_loop_is_busy() {
        let (sender, source) = crossbeam_channel::bounded(0);
        let reader = BufferedMessages::new(source);
        let (done, finished) = crossbeam_channel::bounded(1);
        let producer = std::thread::spawn(move || {
            let sent = (0..LIMIT).all(|id| {
                sender
                    .send(Message::Request(Request::new((id as i32).into(), "probe".into(), ())))
                    .is_ok()
            });
            let _ = done.send(sent);
        });
        // No main-loop receive: the rendezvous input must still accept a
        // bounded burst, making subsequent edits visible for coalescing.
        let sent = finished.recv_timeout(std::time::Duration::from_secs(5));
        drop(reader);
        producer.join().unwrap();
        assert_eq!(sent, Ok(true));
    }

    #[test]
    fn bounded_reader_preserves_order_and_stops_with_either_channel_blocked() {
        for count in [0, LIMIT * 2] {
            let (sender, source) = crossbeam_channel::unbounded();
            for id in 0..count {
                sender
                    .send(Message::Request(Request::new((id as i32).into(), "probe".into(), ())))
                    .unwrap();
            }
            let reader = BufferedMessages::new(source);
            if count > 0 {
                for id in 0..LIMIT {
                    let Message::Request(request) =
                        reader.receiver.recv_timeout(std::time::Duration::from_secs(5)).unwrap()
                    else {
                        panic!("expected request");
                    };
                    assert_eq!(request.id, (id as i32).into());
                }
            }
            // Sender is deliberately kept alive: drop must interrupt waiting
            // for input, as well as waiting for room in the bounded queue.
            drop(reader);
            drop(sender);
        }
    }

    #[test]
    fn only_obsolete_reads_are_skipped_and_client_cancellation_wins() {
        let params =
            json!({"textDocument":{"uri":"file:///main.sail"},"position":{"line":0,"character":0}});
        let mut request = Request::new(1.into(), "textDocument/hover".into(), params);
        let change = |uri: &str, version| {
            Message::Notification(lsp_server::Notification::new(
                "textDocument/didChange".into(),
                json!({"textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":"x"}]}),
            ))
        };
        assert!(superseded(&request, Some(2), &[change("file:///main.sail", 2)]).is_none());
        assert!(superseded(&request, Some(2), &[change("file:///other.sail", 3)]).is_none());
        assert_eq!(
            superseded(&request, Some(2), &[change("file:///main.sail", 3)])
                .map(|code| code as i32),
            Some(ErrorCode::ServerCancelled as i32)
        );
        request.method = "sail-lsp/ssrSearch".into();
        assert_eq!(
            superseded(&request, Some(2), &[change("file:///main.sail", 3)])
                .map(|code| code as i32),
            Some(ErrorCode::ServerCancelled as i32)
        );
        request.method = "textDocument/rename".into();
        assert!(superseded(&request, Some(2), &[change("file:///main.sail", 3)]).is_none());
        let cancel = Message::Notification(lsp_server::Notification::new(
            "$/cancelRequest".into(),
            json!({"id":1}),
        ));
        assert_eq!(
            superseded(&request, Some(2), &[change("file:///main.sail", 3), cancel])
                .map(|code| code as i32),
            Some(ErrorCode::RequestCanceled as i32)
        );
    }
}
