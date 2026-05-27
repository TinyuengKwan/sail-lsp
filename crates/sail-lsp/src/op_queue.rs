//! Operation queue for deduplicating and serializing background tasks.
//! Each `OpQueue` manages a single at-most-one-in-flight operation.
//! If a new request arrives while the operation is in progress, it overwrites
//! the previous pending request. This means rapid-fire edits collapse into
//! a single recomputation when the current operation completes.

pub(crate) type Cause = String;

/// A queue that holds at most one pending operation and at most one in-flight operation.
/// `Args` is the type of the arguments passed to the operation.
/// `Output` is the type of the result from the operation.
#[derive(Debug)]
pub(crate) struct OpQueue<Args = (), Output = ()> {
    op_requested: Option<(Cause, Args)>,
    op_in_progress: bool,
    last_op_result: Option<Output>,
}

impl<Args, Output> Default for OpQueue<Args, Output> {
    fn default() -> Self {
        Self { op_requested: None, op_in_progress: false, last_op_result: None }
    }
}

impl<Args, Output> OpQueue<Args, Output> {
    /// Request a new operation with a given cause and arguments.
    /// If there's already a pending request, it is overwritten (last-writer-wins).
    pub(crate) fn request_op(&mut self, reason: Cause, args: Args) {
        self.op_requested = Some((reason, args));
    }

    /// Check if an operation should be started.
    /// Returns `Some((cause, args))` if an operation was requested and no operation
    /// is currently in progress. Sets the in-progress flag.
    pub(crate) fn should_start_op(&mut self) -> Option<(Cause, Args)> {
        if self.op_in_progress {
            return None;
        }
        let request = self.op_requested.take()?;
        self.op_in_progress = true;
        Some(request)
    }

    /// Mark the current operation as completed with a result.
    pub(crate) fn op_completed(&mut self, result: Output) {
        assert!(self.op_in_progress);
        self.op_in_progress = false;
        self.last_op_result = Some(result);
    }

    /// Get the result of the last completed operation.
    #[allow(dead_code)]
    pub(crate) fn last_op_result(&self) -> Option<&Output> {
        self.last_op_result.as_ref()
    }

    /// Whether an operation is currently in progress.
    pub(crate) fn op_in_progress(&self) -> bool {
        self.op_in_progress
    }

    /// Whether a new operation has been requested (but not yet started).
    #[allow(dead_code)]
    pub(crate) fn op_requested(&self) -> bool {
        self.op_requested.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_lifecycle() {
        let mut q: OpQueue<i32, String> = OpQueue::default();

        // Nothing requested yet
        assert_eq!(q.should_start_op(), None);
        assert!(!q.op_in_progress());
        assert!(!q.op_requested());

        // Request an op
        q.request_op("edit".into(), 42);
        assert!(q.op_requested());

        // Start it
        let (cause, args) = q.should_start_op().unwrap();
        assert_eq!(cause, "edit");
        assert_eq!(args, 42);
        assert!(q.op_in_progress());

        // While in progress, new request doesn't start
        q.request_op("another edit".into(), 99);
        assert_eq!(q.should_start_op(), None);

        // Complete the first op
        q.op_completed("done".into());
        assert!(!q.op_in_progress());
        assert_eq!(q.last_op_result(), Some(&"done".to_string()));

        // Now the pending request can start
        let (cause, args) = q.should_start_op().unwrap();
        assert_eq!(cause, "another edit");
        assert_eq!(args, 99);
    }

    #[test]
    fn last_writer_wins() {
        let mut q: OpQueue = OpQueue::default();

        q.request_op("first".into(), ());
        q.request_op("second".into(), ());
        q.request_op("third".into(), ());

        // Only the last request survives
        let (cause, ()) = q.should_start_op().unwrap();
        assert_eq!(cause, "third");
    }

    #[test]
    #[should_panic]
    fn op_completed_without_in_progress_panics() {
        let mut q: OpQueue = OpQueue::default();
        q.op_completed(());
    }
}
