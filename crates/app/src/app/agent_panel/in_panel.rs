//! Delegated tasks run in agent panels.
//!
//! The `task` call runs on a thread of its own, the panels live on the UI
//! thread: the call sends a [`PanelDelegation`] down a process-wide channel
//! and waits for the app's answer. On its tick the app takes the request to
//! an agent panel — the first one still showing the banner, with nothing
//! typed, that is not the one in focus, else (for `panel`) a new one beside
//! the others, without the focus — rebuilt in place on a session that names
//! the delegating one and runs on the delegate's agent, connection and mode.
//! The panel then reports what happens to the waiting call
//! ([`DelegateUpdate`]), which shows it in its block as a hidden delegate's
//! progress and returns the delegate's last words as its answer.
//!
//! The channel answers only while an app serves it: a headless run and the
//! tests have no panels, and their tasks run hidden.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use termide_agent_core::{
    AgentEvent, CancelToken, IntentLog, Message, Mode, Session, SessionParent, SubagentPlace,
    ToolUpdate, UserMessage,
};
use termide_agent_tools::SubagentOutcome;
use termide_panel_agent::{AgentPanel, DelegateEnd, DelegateUpdate, Delegation};

use super::{agent_setup, session_dir_of, SubagentReport, DEFAULT_AGENT};
use crate::app::App;

/// A delegated task to run in an agent panel.
pub(super) struct PanelDelegation {
    pub place: SubagentPlace,
    /// The project the delegating panel belongs to: only its panels take
    /// the task.
    pub project_root: PathBuf,
    pub cwd: PathBuf,
    /// The delegate.
    pub agent: String,
    /// The connection it runs on.
    pub connection: String,
    pub mode: Mode,
    /// Where the task comes from, for its session's header.
    pub lineage: SessionParent,
    /// The task, as the delegating agent's message.
    pub message: UserMessage,
    /// Cancelled when the delegating call stops waiting.
    pub cancel: CancelToken,
    /// What the user asked the delegating agent, for the `auto` reviewer of
    /// the task's calls.
    pub intent: IntentLog,
}

/// A request with where the app answers it: the task under way, or `None`
/// when no panel took it.
type Request = (PanelDelegation, Sender<Option<PanelTask>>);

/// A delegated task a panel took.
pub(super) struct PanelTask {
    pub updates: Receiver<DelegateUpdate>,
    /// Its session's log.
    pub session: Option<PathBuf>,
}

/// The channel from the `task` calls to the app.
fn port() -> &'static (Sender<Request>, Mutex<Receiver<Request>>) {
    static PORT: OnceLock<(Sender<Request>, Mutex<Receiver<Request>>)> = OnceLock::new();
    PORT.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        (sender, Mutex::new(receiver))
    })
}

/// Set once an app takes the requests.
static SERVED: AtomicBool = AtomicBool::new(false);

/// Where each panel taken for a task worked before, by its number: once the
/// task is over it is set up there anew.
fn homes() -> &'static Mutex<HashMap<u64, PathBuf>> {
    static HOMES: OnceLock<Mutex<HashMap<u64, PathBuf>>> = OnceLock::new();
    HOMES.get_or_init(Mutex::default)
}

/// How long a `task` call waits for the app to place its task before it
/// runs the task hidden: the app answers on its next tick.
const REPLY_WAIT: Duration = Duration::from_secs(10);

/// How often the waiting call looks at what came.
const POLL: Duration = Duration::from_millis(50);

/// Ask the app for a panel for `request`, and follow the task there to its
/// end. `None` when no panel took it — none free, no app serving, the app
/// slow to answer — so the task runs hidden; a stop while waiting is a
/// stopped task.
pub(super) fn run_in_panel(
    request: PanelDelegation,
    cwd: &Path,
    max_turns: usize,
    cancel: &CancelToken,
    on_update: &mut dyn FnMut(ToolUpdate),
) -> Option<SubagentOutcome> {
    if !SERVED.load(Ordering::Acquire) {
        return None;
    }
    let (reply, answer) = mpsc::channel();
    let stop = request.cancel.clone();
    port().0.send((request, reply)).ok()?;
    let started = Instant::now();
    let task = loop {
        match answer.recv_timeout(POLL) {
            Ok(task) => break task?,
            Err(RecvTimeoutError::Timeout) if cancel.is_cancelled() => {
                stop.cancel();
                return Some(Err("the subagent was stopped".to_string()).into());
            }
            Err(RecvTimeoutError::Timeout) if started.elapsed() < REPLY_WAIT => {}
            Err(_) => {
                // A panel that takes it later sees it stopped at once.
                stop.cancel();
                return None;
            }
        }
    };
    let mut outcome = follow(&task.updates, cwd, max_turns, cancel, &stop, on_update);
    outcome.session = task.session;
    Some(outcome)
}

/// Follow a task a panel runs, showing its progress through `on_update`,
/// to its end; `stop` stops the panel's run when the delegating call is
/// stopped or the task takes more model calls than it may.
fn follow(
    updates: &Receiver<DelegateUpdate>,
    cwd: &Path,
    max_turns: usize,
    cancel: &CancelToken,
    stop: &CancelToken,
    on_update: &mut dyn FnMut(ToolUpdate),
) -> SubagentOutcome {
    let mut report = SubagentReport::new(max_turns);
    let end = loop {
        if cancel.is_cancelled() {
            stop.cancel();
        }
        match updates.recv_timeout(POLL) {
            Ok(DelegateUpdate::Event(event)) => match event {
                AgentEvent::MessageEnd(Message::Assistant(message)) => {
                    if report.turn() {
                        stop.cancel();
                    }
                    report.said(&message, on_update);
                }
                AgentEvent::ToolExecutionStart { call } => report.called(&call, cwd, on_update),
                AgentEvent::ToolExecutionEnd { result } => report.call_ended(&result, on_update),
                _ => {}
            },
            Ok(DelegateUpdate::Ended(end)) => break Some(end),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break None,
        }
    };
    match end {
        Some(DelegateEnd::Done) => report.into_outcome(cancel.is_cancelled()),
        _ if cancel.is_cancelled() => report.into_outcome(true),
        Some(DelegateEnd::Stopped) => report.ended_by_user("the user stopped the subagent"),
        Some(DelegateEnd::TakenOver) => {
            report.ended_by_user("the user took the task over in the subagent's panel")
        }
        None => report.ended_by_user("the subagent's panel was closed"),
    }
}

impl App {
    /// Place the delegated tasks waiting for a panel. Called on every tick:
    /// from the first call on, `task` calls ask for panels.
    pub(in crate::app) fn serve_delegations(&mut self) {
        SERVED.store(true, Ordering::Release);
        self.reset_released_panels();
        let requests: Vec<Request> = port()
            .1
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_iter()
            .collect();
        for (request, reply) in requests {
            let task = self.place_delegation(&request);
            if task.is_some() {
                self.state.needs_redraw = true;
            }
            let _ = reply.send(task);
        }
    }

    /// Start `request` in a panel: rebuilt in place of the first free one,
    /// else opened beside the others when its place allows. `None` when no
    /// panel takes it.
    fn place_delegation(&mut self, request: &PanelDelegation) -> Option<PanelTask> {
        if request.cancel.is_cancelled()
            || !super::peers::same_dir(&request.project_root, &self.project_root)
        {
            return None;
        }
        let focused = self.layout_manager.active_panel_position();
        let free = self
            .layout_manager
            .iter_all_panels_mut()
            .enumerate()
            .find(|(position, panel)| {
                Some(*position) != focused
                    && panel
                        .as_any()
                        .downcast_ref::<AgentPanel>()
                        .is_some_and(AgentPanel::takes_delegate)
            })
            .map(|(position, _)| position);
        if free.is_none() && !request.place.opens_panel() {
            return None;
        }
        // A panel rebuilt in place keeps its number, and where it worked,
        // to go back to once the task is over.
        let taken = free.and_then(|position| {
            self.layout_manager
                .iter_all_panels_mut()
                .nth(position)
                .and_then(|panel| panel.as_any().downcast_ref::<AgentPanel>())
                .map(|panel| (panel.panel_id(), panel.cwd().to_path_buf()))
        });
        let keep = taken.as_ref().map(|(id, _)| *id);
        let (mut panel, session) = self.delegate_panel(request, keep)?;
        let (updates, task_updates) = mpsc::channel();
        panel.start_delegation(
            request.message.clone(),
            Delegation::new(
                request.lineage.agent.clone(),
                &request.message.plain_text(),
                updates,
                request.cancel.clone(),
            )
            .with_call(request.lineage.call.clone()),
        );
        let home = taken.map_or_else(|| request.cwd.clone(), |(_, cwd)| cwd);
        homes()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(panel.panel_id(), home);
        match free {
            Some(position) => {
                self.layout_manager
                    .replace_panel_at(position, Box::new(panel));
            }
            None => self.add_panel_without_focus(Box::new(panel)),
        }
        self.auto_save_layout();
        Some(PanelTask {
            updates: task_updates,
            session,
        })
    }

    /// Set up anew the panels a delegated task left: the default agent, on
    /// the default connection and mode, where the panel worked before it was
    /// taken — as a panel the user opens — keeping its place and number.
    fn reset_released_panels(&mut self) {
        let released: Vec<(usize, u64, PathBuf)> = self
            .layout_manager
            .iter_all_panels_mut()
            .enumerate()
            .filter_map(|(position, panel)| {
                let agent = panel.as_any_mut().downcast_mut::<AgentPanel>()?;
                agent
                    .take_released()
                    .then(|| (position, agent.panel_id(), agent.cwd().to_path_buf()))
            })
            .collect();
        if released.is_empty() {
            return;
        }
        let settings = self.state.config.ai.clone();
        for (position, id, cwd) in released {
            let cwd = homes()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id)
                .unwrap_or(cwd);
            let mut setup = agent_setup(&settings, cwd, &self.project_root, DEFAULT_AGENT, None);
            setup.panel_id = Some(id);
            self.layout_manager
                .replace_panel_at(position, Box::new(AgentPanel::new(setup)));
        }
        self.state.needs_redraw = true;
        self.auto_save_layout();
    }

    /// A panel for `request`, on a session of its own that names where the
    /// task came from and starts on the delegate's connection, agent and
    /// mode; with the log's path.
    fn delegate_panel(
        &self,
        request: &PanelDelegation,
        panel_id: Option<u64>,
    ) -> Option<(AgentPanel, Option<PathBuf>)> {
        let dir = session_dir_of(&request.cwd)?;
        let mut session =
            match Session::create_delegate(&dir, &request.cwd, request.lineage.clone()) {
                Ok(session) => session,
                Err(error) => {
                    log::warn!("cannot start a delegate's session log: {error}");
                    return None;
                }
            };
        let written = session
            .append_connection_change(&request.connection)
            .and_then(|_| session.append_agent_change(&request.agent))
            .and_then(|_| session.append_mode_change(request.mode));
        if let Err(error) = written {
            log::warn!("delegate session write failed: {error}");
        }
        let path = session.path().to_path_buf();
        let settings = self.state.config.ai.clone();
        let mut setup = agent_setup(
            &settings,
            request.cwd.clone(),
            &self.project_root,
            &request.agent,
            Some(session),
        );
        setup.inherited_intent = Some(request.intent.clone());
        setup.panel_id = panel_id;
        let panel = AgentPanel::new(setup);
        Some((panel, Some(path)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_agent_core::{AssistantContent, AssistantMessage, StopReason, ToolCall, Usage};

    fn said(text: &str) -> DelegateUpdate {
        DelegateUpdate::Event(AgentEvent::MessageEnd(Message::Assistant(
            AssistantMessage {
                content: vec![AssistantContent::Text { text: text.into() }],
                stop_reason: StopReason::Stop,
                usage: Usage {
                    input: 10,
                    output: 2,
                    ..Usage::default()
                },
                provider: "p".into(),
                model: "m".into(),
                error_message: None,
                failure: None,
                timestamp: 0,
            },
        )))
    }

    fn run(updates: Vec<DelegateUpdate>, cancel: &CancelToken) -> (SubagentOutcome, Vec<String>) {
        let (sender, receiver) = mpsc::channel();
        for update in updates {
            sender.send(update).unwrap();
        }
        drop(sender);
        let mut shown = Vec::new();
        let outcome = follow(
            &receiver,
            Path::new("/proj"),
            50,
            cancel,
            &CancelToken::new(),
            &mut |update| {
                let ToolUpdate::Output(text) = update;
                shown.push(text);
            },
        );
        (outcome, shown)
    }

    #[test]
    fn a_task_in_a_panel_answers_with_the_delegates_last_words() {
        let read = ToolCall {
            id: "c1".into(),
            name: "read".into(),
            arguments: serde_json::json!({ "path": "/proj/src/lib.rs" }),
            extra_content: None,
        };
        let (outcome, shown) = run(
            vec![
                said("looking"),
                DelegateUpdate::Event(AgentEvent::ToolExecutionStart { call: read }),
                said("found it"),
                DelegateUpdate::Ended(DelegateEnd::Done),
            ],
            &CancelToken::new(),
        );
        assert_eq!(outcome.answer.as_deref(), Ok("found it"));
        assert_eq!(outcome.spent.output, 4);
        assert!(
            shown.last().unwrap().contains("· read src/lib.rs"),
            "{shown:?}"
        );
    }

    #[test]
    fn a_task_in_a_panel_says_how_the_user_ended_it() {
        let stopped = run(
            vec![said("half"), DelegateUpdate::Ended(DelegateEnd::Stopped)],
            &CancelToken::new(),
        )
        .0;
        let answer = stopped.answer.unwrap();
        assert!(
            answer.starts_with("half") && answer.contains("the user stopped"),
            "{answer}"
        );

        let taken = run(
            vec![DelegateUpdate::Ended(DelegateEnd::TakenOver)],
            &CancelToken::new(),
        )
        .0;
        assert!(taken.answer.unwrap_err().contains("took the task over"));

        // A panel closed under the task ends it too.
        let closed = run(vec![said("so far")], &CancelToken::new()).0;
        assert!(closed.answer.unwrap().contains("panel was closed"));

        // The delegating agent's own stop is a stopped task, whatever the
        // panel says.
        let cancel = CancelToken::new();
        cancel.cancel();
        let cancelled = run(vec![DelegateUpdate::Ended(DelegateEnd::Stopped)], &cancel).0;
        assert_eq!(
            cancelled.answer,
            Err("the subagent was stopped".to_string())
        );
    }

    #[test]
    fn without_an_app_to_serve_it_a_task_runs_hidden() {
        let request = PanelDelegation {
            place: SubagentPlace::Panel,
            project_root: PathBuf::from("/proj"),
            cwd: PathBuf::from("/proj"),
            agent: "search".into(),
            connection: "local".into(),
            mode: Mode::default(),
            lineage: SessionParent {
                session: None,
                agent: "lead".into(),
                call: "c1".into(),
            },
            message: UserMessage::text("find it"),
            cancel: CancelToken::new(),
            intent: IntentLog::default(),
        };
        assert!(run_in_panel(
            request,
            Path::new("/proj"),
            50,
            &CancelToken::new(),
            &mut |_| {}
        )
        .is_none());
    }
}
