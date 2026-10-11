//! Cooperative cancellation shared between the UI thread and a running agent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A cloneable flag polled by providers and tools between units of work.
///
/// Cancellation is cooperative: setting the flag does not interrupt a
/// blocking read, it only asks the next check to stop. A provider must poll
/// it between stream chunks and a tool between output chunks.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
    /// The token this one was made from with [`Self::child`]: cancelling it
    /// cancels this one too, but not the other way round.
    parent: Option<Arc<CancelToken>>,
}

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A token that is cancelled when this one is, and can also be cancelled
    /// on its own without touching this one: the loop stops one model call
    /// this way while the run goes on.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            flag: Arc::default(),
            parent: Some(Arc::new(self.clone())),
        }
    }

    /// Request cancellation of the current run.
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.is_cancelled())
    }

    /// Clear the flag before a new run starts.
    pub fn reset(&self) {
        self.flag.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::CancelToken;

    #[test]
    fn a_child_follows_its_parent_but_not_the_reverse() {
        let parent = CancelToken::new();
        let child = parent.child();
        child.cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled(), "a child stops only itself");

        let child = parent.child();
        assert!(!child.is_cancelled());
        parent.cancel();
        assert!(child.is_cancelled(), "a stopped run stops its calls");
    }
}
