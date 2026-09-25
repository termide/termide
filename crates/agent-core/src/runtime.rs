//! Worker-thread host for an [`Agent`], following termide's background
//! pipeline convention: `std::thread` plus `mpsc`, polled from `tick()`.
//!
//! Prompts travel over a channel and run one at a time. Steering, follow-up
//! and abort must reach a loop that is blocked inside `run`, so they go
//! out-of-band through the shared [`QueueHandle`] and [`CancelToken`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::agent::{Agent, AgentEvent, Hooks, QueueHandle};
use crate::cancel::CancelToken;
use crate::compaction::CompactionReason;
use crate::message::UserMessage;
use crate::permissions::{ChannelPrompter, PermissionRules, PersistRule};
use crate::provider::ModelSpec;
use std::path::PathBuf;

/// Why a prompt was not accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptError {
    /// A run is in progress; queue the text with `steer` or `follow_up`.
    Busy,
    /// The worker thread is gone.
    Stopped,
    /// The backend has no such knob: an external agent takes no model,
    /// tool or prompt changes from the panel.
    Unsupported,
}

impl std::fmt::Display for PromptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("agent is busy"),
            Self::Stopped => f.write_str("agent runtime has stopped"),
            Self::Unsupported => f.write_str("not available for an external agent"),
        }
    }
}

impl std::error::Error for PromptError {}

enum WorkerCommand {
    Prompt(UserMessage),
    /// Continue a paused run on the existing transcript (no new message).
    Resume,
    /// Applied to the agent between runs.
    Update(Box<dyn FnOnce(&mut Agent) + Send>),
    /// Summarise the older part of the transcript now, on the user's word.
    Compact(Option<String>),
    /// Judge whether the autonomous goal is reached (`/goal`), read-only.
    Judge(String),
    /// Write a handoff brief of the unfinished work (`/handoff`), read-only.
    Handoff,
    Shutdown,
}

/// A model an external backend offers for the panel's picker. The built-in
/// loop lists its models through its [`crate::Provider`] instead; these come
/// from an ACP agent that advertises its models.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendModel {
    /// Id the backend expects back in [`Backend::select_model`].
    pub id: String,
    /// Human label for the picker; falls back to the id.
    pub name: String,
}

/// What the panel hands an external backend when it starts it.
pub struct BackendSetup {
    pub cwd: PathBuf,
    /// Permission prompts go to the user through this.
    pub prompter: ChannelPrompter,
    /// Set by the panel's abort.
    pub cancel: CancelToken,
    /// The same rules the built-in agent runs under (config plus this
    /// session's grants): read-only commands and rules an external agent's
    /// permission requests match are answered without troubling the user.
    pub rules: PermissionRules,
    /// Persists an "allow always" grant to the configuration, as for the
    /// built-in agent; `None` when there is nowhere to write it.
    pub persist: Option<PersistRule>,
}

/// What the panel drives: the built-in agent on its worker thread, or an
/// external agent speaking ACP. Events come out through [`Backend::drain`]
/// from the panel's `tick()` either way.
pub trait Backend: Send {
    /// Start a run; [`PromptError::Busy`] while one is active.
    fn prompt(&self, message: UserMessage) -> Result<(), PromptError>;
    /// Queue a message for the running turn's next boundary.
    fn steer(&self, message: UserMessage);
    /// Queued messages: `(steering, follow_up)`.
    fn queue_lens(&self) -> (usize, usize);
    /// Take back every message still queued (not yet delivered), oldest
    /// first, so the UI can put the text back in its input to edit; the
    /// default has no queue to take from.
    fn take_queued(&self) -> Vec<UserMessage> {
        Vec::new()
    }
    /// Ask the active run to stop.
    fn abort(&self);
    /// Ask the active run to pause gracefully at the next step boundary; the
    /// default is a no-op (an external agent has no such control).
    fn pause(&self) {}
    /// Withdraw a pause asked for that the run has not reached yet; the
    /// default is a no-op, like [`Self::pause`].
    fn cancel_pause(&self) {}
    /// Continue a paused run; the default reports it is unsupported.
    fn resume(&self) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn is_busy(&self) -> bool;
    /// Everything that happened since the last call, without blocking.
    fn drain(&self) -> Vec<AgentEvent>;
    /// Change the built-in agent between runs; [`PromptError::Unsupported`]
    /// for an external one.
    fn update(&self, update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError>;
    /// Summarise the older part of the transcript now (`/compact`), with an
    /// optional focus; [`PromptError::Unsupported`] for an external agent.
    fn compact(&self, focus: Option<String>) -> Result<(), PromptError>;
    /// Judge whether the autonomous goal is reached (`/goal`); the default
    /// reports it is unsupported (an external agent has no such call).
    fn judge(&self, _goal: String) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    /// Write a handoff brief of the unfinished work (`/handoff`); the default
    /// reports it is unsupported (an external agent has no such call).
    fn handoff(&self) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    /// Models the backend offers for a picker, current first when known. Empty
    /// means it cannot enumerate them (the built-in loop lists via its
    /// provider instead); an ACP agent that advertises models returns them.
    fn available_models(&self) -> Vec<BackendModel> {
        Vec::new()
    }
    /// The backend's current model id, when it tracks one (an ACP agent).
    fn current_model(&self) -> Option<String> {
        None
    }
    /// Switch the backend's model for the runs that follow, reporting the
    /// agent's own error on failure. The built-in loop changes model through
    /// [`Backend::update`] instead; the default here says it is unsupported.
    fn select_model(&self, _model_id: String) -> Result<(), String> {
        Err("model selection is not supported".to_string())
    }
    /// Stop and hand the built-in agent back, when there is one.
    fn into_agent(self: Box<Self>) -> Option<Agent>;
}

impl Backend for AgentRuntime {
    fn prompt(&self, message: UserMessage) -> Result<(), PromptError> {
        AgentRuntime::prompt(self, message)
    }
    fn steer(&self, message: UserMessage) {
        AgentRuntime::steer(self, message);
    }
    fn queue_lens(&self) -> (usize, usize) {
        self.queues().lens()
    }
    fn take_queued(&self) -> Vec<UserMessage> {
        let (mut steering, follow_up) = self.clear_queue();
        steering.extend(follow_up);
        steering
    }
    fn abort(&self) {
        AgentRuntime::abort(self);
    }
    fn pause(&self) {
        AgentRuntime::pause(self);
    }
    fn cancel_pause(&self) {
        AgentRuntime::cancel_pause(self);
    }
    fn resume(&self) -> Result<(), PromptError> {
        AgentRuntime::resume(self)
    }
    fn is_busy(&self) -> bool {
        AgentRuntime::is_busy(self)
    }
    fn drain(&self) -> Vec<AgentEvent> {
        AgentRuntime::drain(self)
    }
    fn update(&self, update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        AgentRuntime::update(self, update)
    }
    fn compact(&self, focus: Option<String>) -> Result<(), PromptError> {
        AgentRuntime::compact(self, focus)
    }
    fn judge(&self, goal: String) -> Result<(), PromptError> {
        AgentRuntime::judge(self, goal)
    }
    fn handoff(&self) -> Result<(), PromptError> {
        AgentRuntime::handoff(self)
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        (*self).shutdown()
    }
}

/// Owns the worker thread that runs the agent.
pub struct AgentRuntime {
    commands: Sender<WorkerCommand>,
    events: Receiver<AgentEvent>,
    queues: QueueHandle,
    cancel: CancelToken,
    busy: Arc<AtomicBool>,
    worker: Option<JoinHandle<Agent>>,
}

impl AgentRuntime {
    /// Move `agent` onto a new thread. `hooks` run on that thread.
    #[must_use]
    pub fn spawn(agent: Agent, hooks: Box<dyn Hooks>) -> Self {
        Self::spawn_with_cancel(agent, hooks, CancelToken::new())
    }

    /// Like [`AgentRuntime::spawn`], sharing `cancel` with anything else that
    /// must notice an abort (a blocking permission prompt, for example).
    #[must_use]
    pub fn spawn_with_cancel(
        mut agent: Agent,
        mut hooks: Box<dyn Hooks>,
        cancel: CancelToken,
    ) -> Self {
        let (commands, command_rx) = mpsc::channel::<WorkerCommand>();
        let (event_tx, events) = mpsc::channel::<AgentEvent>();
        let queues = agent.queues();
        let busy = Arc::new(AtomicBool::new(false));

        let worker_cancel = cancel.clone();
        let worker_busy = busy.clone();
        let worker = std::thread::Builder::new()
            .name("termide-agent".into())
            .spawn(move || {
                while let Ok(command) = command_rx.recv() {
                    match command {
                        WorkerCommand::Prompt(prompt) => {
                            agent.run(prompt, hooks.as_mut(), &worker_cancel, &mut |event| {
                                // A closed receiver means the UI dropped the
                                // runtime; the run finishes on its own.
                                let _ = event_tx.send(event);
                            });
                            worker_busy.store(false, Ordering::Release);
                        }
                        WorkerCommand::Resume => {
                            agent.resume(hooks.as_mut(), &worker_cancel, &mut |event| {
                                let _ = event_tx.send(event);
                            });
                            worker_busy.store(false, Ordering::Release);
                        }
                        WorkerCommand::Update(update) => update(&mut agent),
                        WorkerCommand::Compact(focus) => {
                            let _ = agent.compact(
                                CompactionReason::Manual,
                                focus.as_deref(),
                                &worker_cancel,
                                &mut |event| {
                                    let _ = event_tx.send(event);
                                },
                            );
                        }
                        WorkerCommand::Judge(goal) => {
                            let _ = agent.judge(&goal, &worker_cancel, &mut |event| {
                                let _ = event_tx.send(event);
                            });
                        }
                        WorkerCommand::Handoff => {
                            agent.handoff(&worker_cancel, &mut |event| {
                                let _ = event_tx.send(event);
                            });
                        }
                        WorkerCommand::Shutdown => break,
                    }
                }
                agent
            })
            .expect("spawn agent worker thread");

        Self {
            commands,
            events,
            queues,
            cancel,
            busy,
            worker: Some(worker),
        }
    }

    /// Start a run. Fails with [`PromptError::Busy`] while one is active;
    /// callers then choose between `steer` and `follow_up`.
    pub fn prompt(&self, message: UserMessage) -> Result<(), PromptError> {
        if self.worker.is_none() {
            return Err(PromptError::Stopped);
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(PromptError::Busy);
        }
        self.cancel.reset();
        self.commands
            .send(WorkerCommand::Prompt(message))
            .map_err(|_| {
                self.busy.store(false, Ordering::Release);
                PromptError::Stopped
            })
    }

    /// Change the agent for the runs that follow (model, system prompt,
    /// tools). Refused while a run is active: the worker reads commands only
    /// between runs, so the change would otherwise land silently after the
    /// current one.
    pub fn update(
        &self,
        update: impl FnOnce(&mut Agent) + Send + 'static,
    ) -> Result<(), PromptError> {
        if self.worker.is_none() {
            return Err(PromptError::Stopped);
        }
        if self.is_busy() {
            return Err(PromptError::Busy);
        }
        self.commands
            .send(WorkerCommand::Update(Box::new(update)))
            .map_err(|_| PromptError::Stopped)
    }

    /// Compact the transcript between runs; refused while a run is active,
    /// like [`AgentRuntime::update`]. Progress arrives as `CompactionStart`,
    /// `Compacted` or `CompactionFailed` events.
    pub fn compact(&self, focus: Option<String>) -> Result<(), PromptError> {
        if self.worker.is_none() {
            return Err(PromptError::Stopped);
        }
        if self.is_busy() {
            return Err(PromptError::Busy);
        }
        self.commands
            .send(WorkerCommand::Compact(focus))
            .map_err(|_| PromptError::Stopped)
    }

    /// Judge whether the autonomous goal is reached, between runs; refused
    /// while a run is active, like [`AgentRuntime::compact`]. The verdict
    /// arrives as a `GoalJudged` or `GoalJudgeFailed` event.
    pub fn judge(&self, goal: String) -> Result<(), PromptError> {
        if self.worker.is_none() {
            return Err(PromptError::Stopped);
        }
        if self.is_busy() {
            return Err(PromptError::Busy);
        }
        self.commands
            .send(WorkerCommand::Judge(goal))
            .map_err(|_| PromptError::Stopped)
    }

    /// Write a handoff brief between runs; refused while a run is active, like
    /// [`AgentRuntime::compact`]. The brief arrives as a `Handoff` event.
    pub fn handoff(&self) -> Result<(), PromptError> {
        if self.worker.is_none() {
            return Err(PromptError::Stopped);
        }
        if self.is_busy() {
            return Err(PromptError::Busy);
        }
        self.commands
            .send(WorkerCommand::Handoff)
            .map_err(|_| PromptError::Stopped)
    }

    /// Switch the model for the runs that follow; see [`AgentRuntime::update`].
    pub fn set_model(&self, model: ModelSpec) -> Result<(), PromptError> {
        self.update(move |agent| agent.set_model(model))
    }

    /// Queue a message for the next turn boundary of the active run.
    pub fn steer(&self, message: UserMessage) {
        self.queues.steer(message);
    }

    /// Queue a message for when the active run would otherwise stop.
    pub fn follow_up(&self, message: UserMessage) {
        self.queues.follow_up(message);
    }

    /// Drop queued messages, returning `(steering, follow_up)`.
    pub fn clear_queue(&self) -> (Vec<UserMessage>, Vec<UserMessage>) {
        self.queues.clear()
    }

    /// Ask the active run to stop at its next check. No-op while idle.
    pub fn abort(&self) {
        if self.is_busy() {
            self.cancel.cancel();
        }
    }

    /// Ask the active run to pause gracefully at its next turn boundary — the
    /// current step finishes and the run can be resumed. No-op while idle.
    pub fn pause(&self) {
        if self.is_busy() {
            self.queues.pause();
        }
    }

    /// Withdraw a pause asked for that the run has not reached yet. Once the
    /// run has stopped at it, the pause stands and `resume` continues it.
    pub fn cancel_pause(&self) {
        self.queues.clear_pause();
    }

    /// Continue a paused run on the existing transcript. Fails with
    /// [`PromptError::Busy`] while a run is active, [`PromptError::Stopped`]
    /// once the worker is gone.
    pub fn resume(&self) -> Result<(), PromptError> {
        if self.worker.is_none() {
            return Err(PromptError::Stopped);
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(PromptError::Busy);
        }
        self.cancel.reset();
        self.commands.send(WorkerCommand::Resume).map_err(|_| {
            self.busy.store(false, Ordering::Release);
            PromptError::Stopped
        })
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.busy.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn queues(&self) -> &QueueHandle {
        &self.queues
    }

    /// The token `abort` sets; shared with prompters that block the run.
    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Non-blocking: the next event, if any. Call from `tick()` until it
    /// returns `None`.
    #[must_use]
    pub fn try_recv(&self) -> Option<AgentEvent> {
        // Disconnected means the worker is gone; there is nothing more to
        // read either way.
        self.events.try_recv().ok()
    }

    /// Everything queued so far, without blocking.
    #[must_use]
    pub fn drain(&self) -> Vec<AgentEvent> {
        std::iter::from_fn(|| self.try_recv()).collect()
    }

    /// Stop the worker after the current run and get the agent back with its
    /// transcript. Returns `None` if the worker panicked.
    pub fn shutdown(mut self) -> Option<Agent> {
        self.cancel.cancel();
        let _ = self.commands.send(WorkerCommand::Shutdown);
        self.worker.take().and_then(|worker| worker.join().ok())
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        // Let the worker exit on its own; joining here could block the UI
        // thread behind a long tool call.
        self.cancel.cancel();
        let _ = self.commands.send(WorkerCommand::Shutdown);
    }
}

impl std::fmt::Debug for AgentRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRuntime")
            .field("busy", &self.is_busy())
            .field("queues", &self.queues.lens())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::agent::test_support::*;
    use crate::agent::NoHooks;
    use crate::message::{AssistantMessage, StopReason};
    use crate::provider::{Provider, Request, StreamEvent};
    use crate::tool::ToolRegistry;

    fn wait_for_end(runtime: &AgentRuntime) -> Vec<AgentEvent> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            events.extend(runtime.drain());
            if events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
                return events;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("agent did not finish: {events:?}");
    }

    fn wait_until_idle(runtime: &AgentRuntime) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while runtime.is_busy() {
            assert!(Instant::now() < deadline, "runtime stayed busy");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn prompt_runs_on_the_worker_and_events_arrive_over_the_channel() {
        let provider = Arc::new(ScriptedProvider::new(vec![text_reply("hi there")]));
        let agent = Agent::new(
            provider,
            ToolRegistry::new(),
            model(),
            PathBuf::from("/tmp"),
        );
        let runtime = AgentRuntime::spawn(agent, Box::new(NoHooks));

        runtime.prompt(UserMessage::text("hello")).unwrap();
        let events = wait_for_end(&runtime);
        wait_until_idle(&runtime);

        assert!(
            events.contains(&AgentEvent::MessageUpdate(StreamEvent::TextDelta(
                "hi there".into()
            )))
        );
        let agent = runtime.shutdown().expect("worker returns the agent");
        assert_eq!(roles(agent.messages()), vec!["user", "assistant"]);
    }

    /// Blocks inside `stream` until the test releases it, so the runtime is
    /// observably busy.
    struct GatedProvider {
        gate: Mutex<Option<mpsc::Receiver<()>>>,
        cancelled: Arc<AtomicBool>,
        /// Set once `stream` is entered, so a test can steer only after the
        /// loop has passed its pre-call queue check.
        entered: Arc<AtomicBool>,
    }

    fn wait_until(flag: &AtomicBool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !flag.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline, "provider was never called");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    impl Provider for GatedProvider {
        fn name(&self) -> &str {
            "gated"
        }
        fn stream(
            &self,
            request: &Request<'_>,
            _on_event: &mut dyn FnMut(StreamEvent),
            cancel: &CancelToken,
        ) -> AssistantMessage {
            self.entered.store(true, Ordering::Release);
            if let Some(gate) = self.gate.lock().unwrap().take() {
                let _ = gate.recv();
            }
            if cancel.is_cancelled() {
                self.cancelled.store(true, Ordering::Release);
                return AssistantMessage::failed(
                    "gated",
                    &request.model.id,
                    StopReason::Aborted,
                    "aborted",
                );
            }
            text_reply("released")
        }
    }

    #[test]
    fn second_prompt_while_busy_is_rejected_and_abort_reaches_the_provider() {
        let (release, gate) = mpsc::channel::<()>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let entered = Arc::new(AtomicBool::new(false));
        let provider = Arc::new(GatedProvider {
            gate: Mutex::new(Some(gate)),
            cancelled: cancelled.clone(),
            entered: entered.clone(),
        });
        let agent = Agent::new(
            provider,
            ToolRegistry::new(),
            model(),
            PathBuf::from("/tmp"),
        );
        let runtime = AgentRuntime::spawn(agent, Box::new(NoHooks));

        runtime.prompt(UserMessage::text("first")).unwrap();
        assert!(runtime.is_busy());
        assert_eq!(
            runtime.prompt(UserMessage::text("second")),
            Err(PromptError::Busy)
        );

        // Steering before the model call would be delivered at once; the
        // queue is only observable once the provider blocks.
        wait_until(&entered);
        runtime.steer(UserMessage::text("queued"));
        assert_eq!(runtime.queues().lens(), (1, 0));
        let (steering, _) = runtime.clear_queue();
        assert_eq!(steering.len(), 1);

        runtime.abort();
        release.send(()).unwrap();
        let events = wait_for_end(&runtime);
        wait_until_idle(&runtime);

        assert!(cancelled.load(Ordering::Acquire));
        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::MessageEnd(crate::Message::Assistant(a)) if a.stop_reason == StopReason::Aborted
        )));
        assert!(runtime.prompt(UserMessage::text("third")).is_ok());
        wait_for_end(&runtime);
    }

    #[test]
    fn abort_while_idle_does_not_poison_the_next_run() {
        let provider = Arc::new(ScriptedProvider::new(vec![text_reply("fine")]));
        let agent = Agent::new(
            provider,
            ToolRegistry::new(),
            model(),
            PathBuf::from("/tmp"),
        );
        let runtime = AgentRuntime::spawn(agent, Box::new(NoHooks));

        runtime.abort();
        runtime.prompt(UserMessage::text("go")).unwrap();
        let events = wait_for_end(&runtime);

        assert!(events.iter().any(|e| matches!(
            e,
            AgentEvent::MessageEnd(crate::Message::Assistant(a)) if a.stop_reason == StopReason::Stop
        )));
    }
    #[test]
    fn set_model_applies_between_runs_and_is_refused_during_one() {
        let (release, gate) = mpsc::channel::<()>();
        let provider = Arc::new(GatedProvider {
            gate: Mutex::new(Some(gate)),
            cancelled: Arc::new(AtomicBool::new(false)),
            entered: Arc::new(AtomicBool::new(false)),
        });
        let agent = Agent::new(
            provider,
            ToolRegistry::new(),
            model(),
            PathBuf::from("/tmp"),
        );
        let runtime = AgentRuntime::spawn(agent, Box::new(NoHooks));
        let other = ModelSpec {
            id: "other".into(),
            ..model()
        };

        runtime.prompt(UserMessage::text("first")).unwrap();
        assert_eq!(runtime.set_model(other.clone()), Err(PromptError::Busy));
        release.send(()).unwrap();
        wait_for_end(&runtime);
        wait_until_idle(&runtime);

        runtime.set_model(other.clone()).unwrap();
        runtime
            .update(|agent| agent.set_system_prompt("terse"))
            .unwrap();
        let agent = runtime.shutdown().expect("worker returns the agent");
        assert_eq!(agent.model(), &other);
        assert_eq!(agent.system_prompt(), "terse");
    }
    #[test]
    fn a_manual_compaction_reports_through_the_events() {
        let provider = Arc::new(ScriptedProvider::new(vec![text_reply("hi")]));
        let agent = Agent::new(
            provider,
            ToolRegistry::new(),
            model(),
            PathBuf::from("/tmp"),
        );
        let runtime = AgentRuntime::spawn(agent, Box::new(NoHooks));
        runtime.compact(Some("the tests".into())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let failed = loop {
            let events = runtime.drain();
            if let Some(AgentEvent::CompactionFailed { error }) = events
                .into_iter()
                .find(|e| matches!(e, AgentEvent::CompactionFailed { .. }))
            {
                break error;
            }
            assert!(Instant::now() < deadline, "no compaction event");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(failed.contains("too few messages"), "{failed}");
    }
}
