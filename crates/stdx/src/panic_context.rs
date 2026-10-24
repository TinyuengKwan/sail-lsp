//! A micro-crate to enhance panic messages with context info.

use std::{cell::RefCell, panic, sync::Once};

/// Dummy for leveraging RAII cleanup to pop frames.
#[must_use]
pub struct PanicContext {
    // prevent arbitrary construction
    _priv: (),
}

impl Drop for PanicContext {
    fn drop(&mut self) {
        with_ctx(|ctx| assert!(ctx.pop().is_some()));
    }
}

pub fn enter(frame: String) -> PanicContext {
    #[allow(clippy::print_stderr)]
    fn set_hook() {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |panic_info| {
            with_ctx(|ctx| {
                if !ctx.is_empty() {
                    eprintln!("Panic context:");
                    for frame in ctx.iter() {
                        eprintln!("> {frame}\n");
                    }
                }
            });
            default_hook(panic_info);
        }));
    }

    static SET_HOOK: Once = Once::new();
    SET_HOOK.call_once(set_hook);

    with_ctx(|ctx| ctx.push(frame));
    PanicContext { _priv: () }
}

fn with_ctx(f: impl FnOnce(&mut Vec<String>)) {
    thread_local! {
        static CTX: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }
    CTX.with(|ctx| f(&mut ctx.borrow_mut()));
}

#[cfg(test)]
mod tests {
    /// A frame entered is a frame the hook prints, and dropping pops it.
    ///
    /// This module was complete and had no caller anywhere in the
    /// workspace, which is the only way a panic-reporting aid can fail:
    /// silently, at the moment it was supposed to help.
    #[test]
    fn a_frame_is_pushed_and_popped() {
        super::with_ctx(|ctx| assert!(ctx.is_empty(), "a test leaked a frame"));
        {
            let _outer = super::enter("request: textDocument/hover".to_string());
            super::with_ctx(|ctx| assert_eq!(ctx.len(), 1));
            {
                let _inner = super::enter("file: main.sail".to_string());
                super::with_ctx(|ctx| {
                    assert_eq!(ctx.len(), 2);
                    assert!(ctx[0].contains("hover"));
                    assert!(ctx[1].contains("main.sail"));
                });
            }
            super::with_ctx(|ctx| assert_eq!(ctx.len(), 1));
        }
        super::with_ctx(|ctx| assert!(ctx.is_empty()));
    }
}
