//! A delegated task running out of sight, followed in a panel.
//!
//! A hidden delegate runs on its own thread and writes its session log as it
//! goes. A panel watching it only reads that log: it rebuilds the transcript
//! whenever the log grows, so closing the panel takes nothing from the task.
//! While the task runs the panel is the delegating agent's: the input takes
//! nothing and names whose delegate it is, and the keys and menu entries
//! that act on a session do nothing. `Esc` stops the task, `Ctrl+T` takes it over. Once the
//! task is over the panel opens its session as any other, to go on with.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use termide_agent_core::{CancelToken, PauseGate, Session};
use termide_core::{ConfirmAction, PanelEvent};

use crate::runtime::push_history;
use crate::{AgentPanel, NoticeKind, Transcript, STOP_WATCHED_ACTION, TAKE_OVER_ACTION};

/// What the user can do to a delegate running out of sight, from a panel
/// watching it, and what the delegate tells it back. Shared between the
/// task's thread and the panels.
#[derive(Debug, Clone, Default)]
pub struct DelegateControl {
    /// Cancelled when the user stops the task, or takes it over.
    stop: CancelToken,
    /// The user took the task over: it is theirs to go on with.
    taken: Arc<AtomicBool>,
    /// The task is over.
    done: Arc<AtomicBool>,
    /// Held while the panel that delegated the task is paused: the delegate's
    /// loop waits at its step boundaries and goes on where it stood once the
    /// pause is lifted.
    pause: PauseGate,
}

impl DelegateControl {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The gate the delegate's loop waits at; handed to the loop when the
    /// task starts.
    #[must_use]
    pub fn gate(&self) -> PauseGate {
        self.pause.clone()
    }

    /// Hold the task at its next step boundary.
    pub fn pause(&self) {
        self.pause.pause();
    }

    /// Lift the pause: the task goes on where it stood.
    pub fn resume(&self) {
        self.pause.resume();
    }

    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.pause.is_paused()
    }

    /// Cancelled once the user stopped the task or took it over: the task
    /// stops its run on it.
    #[must_use]
    pub fn stop_token(&self) -> &CancelToken {
        &self.stop
    }

    /// Stop the task. A task held at a pause is let go of first: stopped,
    /// it is over, and its log must not read as paused, waiting to be
    /// carried on.
    pub fn stop(&self) {
        // Cancelled first, so a gate it waits at gives up rather than lets
        // it take one more step.
        self.stop.cancel();
        self.pause.resume();
    }

    /// Stop the task with the panel that delegated it, which is closing: a
    /// pause holding it stays in its log, as the panel's own pause stays in
    /// the panel's, so both are offered to carry on together.
    pub fn close(&self) {
        self.stop.cancel();
    }

    /// Take the task over: it stops, and its session is the user's.
    pub fn take_over(&self) {
        self.taken.store(true, Ordering::Release);
        self.stop.cancel();
        self.pause.resume();
    }

    /// Why the task ended, when the user ended it: what the delegating
    /// agent is told along with what the delegate said so far.
    #[must_use]
    pub fn ended_by_user(&self) -> Option<&'static str> {
        if !self.stop.is_cancelled() {
            None
        } else if self.taken.load(Ordering::Acquire) {
            Some("the user took the task over")
        } else {
            Some("the user stopped the subagent")
        }
    }

    /// Mark the task over.
    pub fn finish(&self) {
        self.done.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

/// A delegated task this panel follows.
pub struct Watch {
    /// The delegating agent's name.
    from: String,
    /// The task's first line, for the input's placeholder.
    task: String,
    /// The task's session log.
    session: PathBuf,
    control: DelegateControl,
    /// The panel the task came from.
    by_panel: u64,
    /// The id of the `task` call that handed it over.
    call: String,
    /// How long the log was when last read.
    seen: Option<u64>,
    /// When the log was last looked at.
    looked: Option<Instant>,
}

/// How often a watched log is looked at: a step of the task is seconds, so
/// a stat a few times a second keeps up without work on every frame.
const LOOK_EVERY: Duration = Duration::from_millis(250);

impl Watch {
    /// The task the `task` call `call` of panel `by_panel`, an agent
    /// `from`'s, handed over, logged at `session`.
    #[must_use]
    pub fn new(
        from: impl Into<String>,
        task: &str,
        session: PathBuf,
        control: DelegateControl,
        by_panel: u64,
        call: impl Into<String>,
    ) -> Self {
        Self {
            from: from.into(),
            task: task
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default()
                .trim()
                .to_string(),
            session,
            control,
            by_panel,
            call: call.into(),
            seen: None,
            looked: None,
        }
    }

    /// Whether the task is held at the pause of the panel that delegated it:
    /// it writes nothing until that run is resumed.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.control.is_paused()
    }
}

impl Drop for Watch {
    /// No panel follows the task any more: its cards go back to the panel
    /// that delegated it.
    fn drop(&mut self) {
        crate::live::set_viewed(self.by_panel, &self.call, false);
    }
}

/// The log's length, which only grows while the task writes it.
fn log_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|meta| meta.len())
}

impl AgentPanel {
    /// Follow `watch`'s task here.
    pub fn start_watch(&mut self, watch: Watch) {
        crate::live::set_viewed(watch.by_panel, &watch.call, true);
        self.chat_focus = true;
        self.watch = Some(watch);
        self.refresh_watch();
    }

    /// Rebuild the transcript from the log when it grew. `true` when it did.
    fn refresh_watch(&mut self) -> bool {
        let Some(watch) = &mut self.watch else {
            return false;
        };
        let len = log_len(&watch.session);
        if len.is_none() || len == watch.seen {
            return false;
        }
        let Ok(session) = Session::open_read_only(&watch.session) else {
            return false;
        };
        watch.seen = len;
        let mut transcript = Transcript::default();
        transcript.set_fold(self.fold);
        for logged in session.context_messages_with_times(&self.compaction_prompts) {
            push_history(&mut transcript, &logged);
        }
        self.transcript = transcript;
        self.selected = self
            .selected
            .min(self.transcript.items().len().saturating_sub(1));
        true
    }

    /// Follow the task: show what its log gained, and once it is over open
    /// its session here, as any other, to go on with. `true` when the panel
    /// changed.
    pub(crate) fn settle_watch(&mut self) -> bool {
        let Some(watch) = &mut self.watch else {
            return false;
        };
        let taken = watch.control.taken.load(Ordering::Acquire);
        // Over, its session opens here — but shown over the layout a task
        // that ended stays as it ended, to read, until the view moves into
        // a panel; one taken over moves there itself, to go on with.
        let opens = watch.control.is_done() && (!self.in_overlay || taken);
        if !opens && watch.looked.is_some_and(|at| at.elapsed() < LOOK_EVERY) {
            return false;
        }
        watch.looked = Some(Instant::now());
        let changed = self.refresh_watch();
        if !opens {
            return changed;
        }
        if self.in_overlay {
            self.pending_events.push(PanelEvent::DockOverlay);
            self.in_overlay = false;
        }
        let Some(watch) = self.watch.take() else {
            return changed;
        };
        match Session::open(&watch.session) {
            Ok(session) => {
                self.switch_session(Some(session));
            }
            Err(error) => log::warn!("cannot open the delegate's session: {error}"),
        }
        if watch.control.taken.load(Ordering::Acquire) {
            self.chat_focus = false;
            self.notice(
                termide_i18n::t().agent_notice_taken_over_fmt(&watch.from),
                NoticeKind::Info,
            );
        }
        true
    }

    /// `Esc` while watching: back to the panel that delegated the task,
    /// which goes on.
    pub(crate) fn leave_watched(&mut self) -> Vec<PanelEvent> {
        match &self.watch {
            Some(watch) => vec![PanelEvent::FocusAgentPanel(watch.by_panel)],
            None => Vec::new(),
        }
    }

    /// `[■]` while watching: ask before stopping the task; the answer comes
    /// back as `PanelCommand::Confirmed(STOP_WATCHED_ACTION)`.
    pub(crate) fn ask_stop_watched(&self) -> Vec<PanelEvent> {
        let Some(watch) = self.watch.as_ref().filter(|_| !self.watched_task_over()) else {
            return Vec::new();
        };
        vec![crate::live::ask_stop(
            watch.by_panel,
            &watch.call,
            STOP_WATCHED_ACTION.to_string(),
        )]
    }

    /// Stop the task, once confirmed.
    pub(crate) fn stop_watched(&mut self) -> Vec<PanelEvent> {
        if let Some(watch) = &self.watch {
            watch.control.stop();
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// `Ctrl+T`: ask before taking the task over; the answer comes back as
    /// `PanelCommand::Confirmed(TAKE_OVER_ACTION)`.
    pub(crate) fn ask_take_over(&self) -> Vec<PanelEvent> {
        let Some(watch) = self.watch.as_ref().filter(|_| !self.watched_task_over()) else {
            return Vec::new();
        };
        vec![PanelEvent::ShowConfirm {
            message: termide_i18n::t().agent_take_over_confirm_fmt(&watch.from),
            on_confirm: ConfirmAction::Custom(TAKE_OVER_ACTION.to_string()),
        }]
    }

    /// Take the task over, once confirmed: it stops, and its session opens
    /// here once it has.
    pub(crate) fn take_over(&mut self) -> Vec<PanelEvent> {
        if let Some(watch) = &self.watch {
            watch.control.take_over();
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// The placeholder the input shows while the task runs.
    pub(crate) fn delegate_placeholder(&self) -> Option<String> {
        self.watch.as_ref().map(|watch| {
            termide_i18n::t().agent_input_placeholder_delegate_fmt(&watch.from, &watch.task)
        })
    }

    /// The panel whose task this one follows.
    #[must_use]
    pub fn delegated_by(&self) -> Option<u64> {
        self.watch.as_ref().map(|watch| watch.by_panel)
    }

    /// Whether the panel follows the task the `task` call `call` of the
    /// panel `by` handed over.
    #[must_use]
    pub fn runs_task(&self, by: u64, call: &str) -> bool {
        self.watch
            .as_ref()
            .is_some_and(|watch| watch.by_panel == by && watch.call == call)
    }

    /// Whether the task the panel follows is over.
    pub(crate) fn watched_task_over(&self) -> bool {
        self.watch
            .as_ref()
            .is_some_and(|watch| watch.control.is_done())
    }

    /// Show the panel over the layout, or back in it: a view shown over the
    /// layout offers to move into a panel of its own.
    pub fn set_in_overlay(&mut self, in_overlay: bool) {
        self.in_overlay = in_overlay;
    }

    /// Whether the panel follows another agent's task: its session is that
    /// agent's until the task is over.
    #[must_use]
    pub fn is_delegate(&self) -> bool {
        self.watch.is_some()
    }
}
