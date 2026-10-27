//! Cancellation ownership for blocking validation work.
use crate::cancel::CancellationToken;
use tokio::task::AbortHandle;

/// Request pipeline-worker cancellation when the RPC exits through any path.
///
/// A queued `spawn_blocking` job is aborted before it starts; a running job
/// cannot be aborted and instead observes the cancellation signal at its next
/// cooperative checkpoint.
#[must_use = "the guard must be held for the lifetime of the RPC pipeline work"]
pub(super) struct WorkerGuard {
    abort: AbortHandle,
    cancellation: CancellationToken,
}

impl WorkerGuard {
    pub(super) const fn new(abort: AbortHandle, cancellation: CancellationToken) -> Self {
        Self {
            abort,
            cancellation,
        }
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.abort.abort();
        self.cancellation.cancel();
    }
}
