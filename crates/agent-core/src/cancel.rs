//! Cooperative cancellation shared between the UI thread and a running agent.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How long a run held by a [`PauseGate`] sleeps between checks.
const PAUSE_POLL: Duration = Duration::from_millis(50);

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

/// A graceful pause a run waits out at its step boundaries.
///
/// Where a [`CancelToken`] ends a run, a pause holds it: no call starts and
/// no request goes out while it is set, and the run goes on where it stood
/// once the pause is lifted. A panel that delegated a task holds its
/// delegates with one of these, so that pausing the panel pauses the work it
/// handed over too.
#[derive(Debug, Clone, Default)]
pub struct PauseGate {
    paused: Arc<AtomicBool>,
}

impl PauseGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hold the run at its next boundary.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Release);
    }

    /// Lift the pause: a run held at a boundary goes on from where it stood.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Release);
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Sleep out a pause, giving up when `cancel` says the run is stopped.
    /// `false` means the caller should not go on: the run was stopped while
    /// it waited.
    pub fn wait(&self, cancel: &CancelToken) -> bool {
        while self.is_paused() {
            if cancel.is_cancelled() {
                return false;
            }
            std::thread::sleep(PAUSE_POLL);
        }
        !cancel.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::{CancelToken, PauseGate};

    #[test]
    fn a_gate_holds_until_lifted_or_stopped() {
        let gate = PauseGate::new();
        let cancel = CancelToken::new();
        assert!(gate.wait(&cancel), "no pause: the run goes straight on");

        gate.pause();
        let held = {
            let (gate, cancel) = (gate.clone(), cancel.clone());
            std::thread::spawn(move || gate.wait(&cancel))
        };
        std::thread::sleep(std::time::Duration::from_millis(120));
        assert!(!held.is_finished(), "the pause holds the run");
        gate.resume();
        assert!(held.join().unwrap(), "lifted, it goes on");

        gate.pause();
        cancel.cancel();
        assert!(
            !gate.wait(&cancel),
            "stopped while it waits: it does not go on"
        );
    }

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
