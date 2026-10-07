//! A limit on how many requests a connection serves at once, shared by every
//! provider built for it in the process.
//!
//! A local server answers one request at a time, or a few (llama.cpp's
//! `--parallel` slots); one more request either waits on the server or takes
//! a slot whose prompt cache another conversation needed. So the agents of
//! every panel, their subagents and their side calls on one connection queue
//! here, in the order they asked, and a waiting request can be stopped.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use termide_agent_core::{
    AssistantMessage, CancelToken, ModelInfo, Provider, Request, StopReason, StreamEvent,
    ThinkingLevel,
};

/// How often a waiting request looks at its cancel token.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// The request slots of one connection.
#[derive(Debug, Default)]
pub struct Slots {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct State {
    /// How many requests may run at once; 0 sets no limit.
    limit: usize,
    /// Requests running now.
    busy: usize,
    /// The ticket the next request to wait gets.
    next: u64,
    /// The tickets of the requests waiting, first come first.
    waiting: VecDeque<u64>,
}

impl State {
    fn has_room(&self) -> bool {
        self.limit == 0 || self.busy < self.limit
    }
}

/// A slot taken: freed when dropped.
pub struct Permit<'a> {
    slots: &'a Slots,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut state = self.slots.lock();
        state.busy = state.busy.saturating_sub(1);
        drop(state);
        self.slots.changed.notify_all();
    }
}

impl Slots {
    /// Slots that let `limit` requests run at once, 0 for any number.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(State {
                limit,
                ..State::default()
            }),
            changed: Condvar::new(),
        }
    }

    /// The slots of connection `key`, shared across the process, now letting
    /// `limit` requests run at once: the settings may have changed it.
    #[must_use]
    pub fn shared(key: &str, limit: usize) -> Arc<Self> {
        static SHARED: Mutex<Option<HashMap<String, Arc<Slots>>>> = Mutex::new(None);
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        let slots = shared
            .get_or_insert_with(HashMap::new)
            .entry(key.to_string())
            .or_default();
        slots.set_limit(limit);
        Arc::clone(slots)
    }

    /// Let `limit` requests run at once from now on, 0 for any number; a
    /// raised limit lets the first waiting ones in.
    pub fn set_limit(&self, limit: usize) {
        let mut state = self.lock();
        if state.limit != limit {
            state.limit = limit;
            drop(state);
            self.changed.notify_all();
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Take a slot, waiting for one behind those who asked first. While it
    /// waits, `on_wait` hears how many requests are waiting ahead of it,
    /// each time that changes. `None` when `cancel` stops the wait.
    pub fn acquire(
        &self,
        cancel: &CancelToken,
        on_wait: &mut dyn FnMut(usize),
    ) -> Option<Permit<'_>> {
        let mut state = self.lock();
        let ticket = state.next;
        state.next += 1;
        state.waiting.push_back(ticket);
        let mut told = None;
        loop {
            let ahead = state
                .waiting
                .iter()
                .position(|waiting| *waiting == ticket)
                .expect("a waiting request keeps its ticket");
            if ahead == 0 && state.has_room() {
                state.waiting.pop_front();
                state.busy += 1;
                drop(state);
                // The next in line may fit too.
                self.changed.notify_all();
                return Some(Permit { slots: self });
            }
            if cancel.is_cancelled() {
                state.waiting.remove(ahead);
                drop(state);
                self.changed.notify_all();
                return None;
            }
            if told != Some(ahead) {
                told = Some(ahead);
                drop(state);
                on_wait(ahead);
                state = self.lock();
                continue;
            }
            state = self
                .changed
                .wait_timeout(state, CANCEL_POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// A provider whose requests take a slot of its connection first.
pub struct SlottedProvider {
    inner: Arc<dyn Provider>,
    slots: Arc<Slots>,
}

impl SlottedProvider {
    #[must_use]
    pub fn new(inner: Arc<dyn Provider>, slots: Arc<Slots>) -> Self {
        Self { inner, slots }
    }
}

impl Provider for SlottedProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    /// Waits for a slot, telling the caller with [`StreamEvent::Queued`]
    /// and, once it has one, [`StreamEvent::Admitted`]; stopped while it
    /// waits, the request is aborted unsent.
    fn stream(
        &self,
        request: &Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage {
        let mut waited = false;
        let mut on_wait = |ahead| {
            waited = true;
            on_event(StreamEvent::Queued { ahead });
        };
        let permit = self.slots.acquire(cancel, &mut on_wait);
        if waited && permit.is_some() {
            on_event(StreamEvent::Admitted);
        }
        let Some(_permit) = permit else {
            return AssistantMessage::failed(
                self.inner.name(),
                request.model.id.clone(),
                StopReason::Aborted,
                "stopped while waiting for a free slot of the connection",
            );
        };
        self.inner.stream(request, on_event, cancel)
    }

    fn endpoint(&self) -> Option<String> {
        self.inner.endpoint()
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        self.inner.list_models()
    }

    fn thinking_levels(&self, model: &str) -> Vec<ThinkingLevel> {
        self.inner.thinking_levels(model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    /// Wait until `slots` has `n` requests waiting.
    fn until_waiting(slots: &Slots, n: usize) {
        for _ in 0..500 {
            if slots.lock().waiting.len() == n {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("never {n} waiting");
    }

    #[test]
    fn requests_beyond_the_limit_wait_their_turn_in_order() {
        let slots = Arc::new(Slots::new(1));
        let cancel = CancelToken::new();
        let first = slots
            .acquire(&cancel, &mut |_| panic!("room for one"))
            .unwrap();
        let (done, order) = mpsc::channel();
        let mut waiters = Vec::new();
        for n in 0..2 {
            let (shared, done) = (Arc::clone(&slots), done.clone());
            waiters.push(thread::spawn(move || {
                let mut heard = Vec::new();
                let permit = shared.acquire(&CancelToken::new(), &mut |ahead| heard.push(ahead));
                done.send(n).unwrap();
                drop(permit);
                heard
            }));
            // The second asks only once the first waits.
            until_waiting(&slots, n + 1);
        }
        drop(first);
        assert_eq!(order.recv().unwrap(), 0);
        assert_eq!(order.recv().unwrap(), 1);
        let heard: Vec<Vec<usize>> = waiters.into_iter().map(|w| w.join().unwrap()).collect();
        // The first waited at the head, the second behind it (and maybe at
        // the head after, if it got there before the first was done).
        assert_eq!(heard[0], [0]);
        assert_eq!(heard[1][0], 1);
    }

    #[test]
    fn no_limit_never_waits_and_a_raised_limit_lets_waiters_in() {
        let unlimited = Slots::new(0);
        let cancel = CancelToken::new();
        let permits: Vec<_> = (0..5)
            .map(|_| {
                unlimited
                    .acquire(&cancel, &mut |_| panic!("no limit"))
                    .unwrap()
            })
            .collect();
        assert_eq!(unlimited.lock().busy, 5);
        drop(permits);
        assert_eq!(unlimited.lock().busy, 0);

        let slots = Arc::new(Slots::new(1));
        let held = slots.acquire(&cancel, &mut |_| {}).unwrap();
        let waiter = {
            let slots = Arc::clone(&slots);
            thread::spawn(move || slots.acquire(&CancelToken::new(), &mut |_| {}).is_some())
        };
        until_waiting(&slots, 1);
        slots.set_limit(2);
        assert!(waiter.join().unwrap());
        drop(held);
    }

    #[test]
    fn a_stopped_wait_leaves_the_queue() {
        let slots = Arc::new(Slots::new(1));
        let held = slots.acquire(&CancelToken::new(), &mut |_| {}).unwrap();
        let cancel = CancelToken::new();
        let waiter = {
            let (slots, cancel) = (Arc::clone(&slots), cancel.clone());
            thread::spawn(move || slots.acquire(&cancel, &mut |_| {}).is_none())
        };
        until_waiting(&slots, 1);
        cancel.cancel();
        assert!(waiter.join().unwrap());
        assert!(slots.lock().waiting.is_empty());
        drop(held);
        assert!(slots.acquire(&CancelToken::new(), &mut |_| {}).is_some());
    }

    #[test]
    fn the_same_connection_shares_its_slots_and_takes_the_latest_limit() {
        let a = Slots::shared("slots-test-connection", 1);
        let b = Slots::shared("slots-test-connection", 3);
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(a.lock().limit, 3);
        assert!(!Arc::ptr_eq(&a, &Slots::shared("slots-test-other", 1)));
    }
}
