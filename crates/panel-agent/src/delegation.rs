//! A task another agent delegated to this panel: the panel runs it where the
//! user sees it, answers its permission requests through its cards, and
//! reports what happens to the agent waiting for the answer.
//!
//! While the task runs the panel is the delegating agent's: the input takes
//! nothing — whatever is typed would go out as the user's words, in the
//! middle of someone else's task — and shows whose delegate it is instead.
//! The cards, stop and pause still work. Once the run is over the panel tells
//! the delegating agent and goes back to the banner on a fresh session; the
//! task's session stays in the list. `Ctrl+T` takes the panel over: the task
//! ends for the delegating agent with what was said so far, and the
//! conversation is the user's to go on with.

use std::sync::mpsc::Sender;

use termide_agent_core::{AgentEvent, CancelToken, UserMessage};
use termide_core::{ConfirmAction, PanelEvent};

use crate::{AgentPanel, NoticeKind, TAKE_OVER_ACTION};

/// What the panel reports to the agent that delegated its task.
#[derive(Debug, Clone)]
pub enum DelegateUpdate {
    /// Something that happened in the run, as the panel took it from its
    /// agent.
    Event(AgentEvent),
    /// The task is over for the delegating agent.
    Ended(DelegateEnd),
}

/// How a delegated task ended for the agent that handed it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegateEnd {
    /// The run is over; what the delegate said last is its answer.
    Done,
    /// The run was stopped, by the user or by the delegating agent.
    Stopped,
    /// The user took the panel over; the task is theirs now.
    TakenOver,
}

/// A task this panel runs for another agent.
pub struct Delegation {
    /// The delegating agent's name.
    from: String,
    /// The task's first line, for the input's placeholder.
    task: String,
    updates: Sender<DelegateUpdate>,
    /// Cancelled by the delegating side when it stops waiting.
    cancel: CancelToken,
    /// The run was stopped here.
    stopped: bool,
    /// The panel the task came from, which the answer goes back to.
    by_panel: Option<u64>,
    /// The id of the `task` call that handed it over.
    call: Option<String>,
}

impl Delegation {
    /// A task from the agent `from`, whose updates go to `updates`; the
    /// delegating side stops it through `cancel`.
    #[must_use]
    pub fn new(
        from: impl Into<String>,
        task: &str,
        updates: Sender<DelegateUpdate>,
        cancel: CancelToken,
    ) -> Self {
        Self {
            from: from.into(),
            task: task
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or_default()
                .trim()
                .to_string(),
            updates,
            cancel,
            stopped: false,
            by_panel: None,
            call: None,
        }
    }

    /// The task as the `task` call `call` handed it over, so the panel
    /// that made it can find this one.
    #[must_use]
    pub fn with_call(mut self, call: impl Into<String>) -> Self {
        self.call = Some(call.into());
        self
    }
}

impl AgentPanel {
    /// Whether the panel can take a delegated task: nothing in its session
    /// yet, nothing typed, nothing running or waiting for an answer — a panel
    /// the user is about to type in is not taken. Focus is the caller's to
    /// judge.
    #[must_use]
    pub fn takes_delegate(&self) -> bool {
        self.delegation.is_none()
            && self.watch.is_none()
            && self.banner_shown()
            && self.input_area().is_empty()
            && !self.shell_mode
            && !self.is_busy()
            && self.pending.is_none()
    }

    /// Whether the panel runs a task for another agent, or follows one
    /// running out of sight.
    #[must_use]
    pub fn is_delegate(&self) -> bool {
        self.delegation.is_some() || self.watch.is_some()
    }

    /// Whether the panel runs the task the `task` call `call` of the panel
    /// `by` handed over.
    #[must_use]
    pub fn runs_task(&self, by: u64, call: &str) -> bool {
        self.watches_task(by, call)
            || self.delegation.as_ref().is_some_and(|delegation| {
                delegation.by_panel == Some(by) && delegation.call.as_deref() == Some(call)
            })
    }

    /// The panel whose task this one runs, when it says.
    #[must_use]
    pub fn delegated_by(&self) -> Option<u64> {
        self.delegation
            .as_ref()
            .and_then(|delegation| delegation.by_panel)
            .or_else(|| self.watch_by_panel())
    }

    /// Run `message`, another agent's task, as this panel's next request.
    pub fn start_delegation(&mut self, message: UserMessage, mut delegation: Delegation) {
        self.chat_focus = false;
        delegation.by_panel = message.origin.as_ref().and_then(|origin| origin.panel);
        self.delegation = Some(delegation);
        self.send_message(message);
    }

    /// Hand `event` to the agent waiting for the task.
    pub(crate) fn forward_to_delegator(&self, event: &AgentEvent) {
        if let Some(delegation) = &self.delegation {
            let _ = delegation
                .updates
                .send(DelegateUpdate::Event(event.clone()));
        }
    }

    /// Note that the run was stopped here.
    pub(crate) fn note_delegate_stopped(&mut self) {
        if let Some(delegation) = &mut self.delegation {
            delegation.stopped = true;
        }
    }

    /// The placeholder the input shows while a task runs here.
    pub(crate) fn delegate_placeholder(&self) -> Option<String> {
        self.delegation
            .as_ref()
            .map(|delegation| {
                termide_i18n::t()
                    .agent_input_placeholder_delegate_fmt(&delegation.from, &delegation.task)
            })
            .or_else(|| self.watch_placeholder())
    }

    /// Follow the task: stop it when the delegating agent stopped waiting,
    /// and once nothing runs or waits any more tell that agent and go back to
    /// the banner. `true` when the panel changed.
    pub(crate) fn settle_delegation(&mut self) -> bool {
        let Some(delegation) = &self.delegation else {
            return false;
        };
        if delegation.cancel.is_cancelled() && self.is_busy() {
            self.abort();
        }
        let working =
            self.is_busy() || self.paused || self.retry_wait.is_some() || self.pending.is_some();
        if working {
            return false;
        }
        let Some(delegation) = self.delegation.take() else {
            return false;
        };
        let end = if delegation.stopped || delegation.cancel.is_cancelled() {
            DelegateEnd::Stopped
        } else {
            DelegateEnd::Done
        };
        let _ = delegation.updates.send(DelegateUpdate::Ended(end));
        // The task's session stays in the list; the panel is free for the
        // next one, judged by nothing the delegating agent was asked.
        self.inherited_intent = None;
        self.switch_session(None);
        // On the delegate's agent, connection and mode until the app sets
        // it up as the user would find a new one.
        self.released = true;
        true
    }

    /// Whether a delegated task ended here since the last look, leaving the
    /// panel free for the app to set up anew; looking clears it. A panel the
    /// user took the task over in is theirs, and never released.
    pub fn take_released(&mut self) -> bool {
        std::mem::take(&mut self.released)
    }

    /// What closing the panel asks while it runs another agent's task.
    pub(crate) fn delegate_close_confirmation(&self) -> Option<String> {
        self.delegation
            .as_ref()
            .map(|delegation| termide_i18n::t().agent_close_confirm_delegate_fmt(&delegation.from))
    }

    /// `Ctrl+T`: ask before taking the task over; the answer comes back as
    /// `PanelCommand::Confirmed(TAKE_OVER_ACTION)`.
    pub(crate) fn ask_take_over(&self) -> Vec<PanelEvent> {
        let Some(from) = self
            .delegation
            .as_ref()
            .map(|delegation| delegation.from.as_str())
            .or_else(|| self.watch_from())
        else {
            return Vec::new();
        };
        vec![PanelEvent::ShowConfirm {
            message: termide_i18n::t().agent_take_over_confirm_fmt(from),
            on_confirm: ConfirmAction::Custom(TAKE_OVER_ACTION.to_string()),
        }]
    }

    /// Take the panel over from the delegating agent: the task ends for it
    /// with what was said so far, and the conversation is the user's.
    pub(crate) fn take_over(&mut self) -> Vec<PanelEvent> {
        // A task followed here stops, and its session opens once it has.
        if self.watch.is_some() {
            return self.take_over_watched();
        }
        let Some(delegation) = self.delegation.take() else {
            return Vec::new();
        };
        let _ = delegation
            .updates
            .send(DelegateUpdate::Ended(DelegateEnd::TakenOver));
        // The running agent keeps it for this conversation; a rebuilt one
        // starts from the user's words here alone.
        self.inherited_intent = None;
        self.notice(
            termide_i18n::t().agent_notice_taken_over_fmt(&delegation.from),
            NoticeKind::Info,
        );
        vec![PanelEvent::NeedsRedraw]
    }
}
