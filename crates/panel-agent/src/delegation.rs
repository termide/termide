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
use termide_core::PanelEvent;

use crate::{AgentPanel, NoticeKind};

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
        }
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
            && self.banner_shown()
            && self.input_area().is_empty()
            && !self.shell_mode
            && !self.is_busy()
            && self.pending.is_none()
    }

    /// Whether the panel runs a task for another agent.
    #[must_use]
    pub fn is_delegate(&self) -> bool {
        self.delegation.is_some()
    }

    /// Run `message`, another agent's task, as this panel's next request.
    pub fn start_delegation(&mut self, message: UserMessage, delegation: Delegation) {
        self.chat_focus = false;
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
        self.delegation.as_ref().map(|delegation| {
            termide_i18n::t()
                .agent_input_placeholder_delegate_fmt(&delegation.from, &delegation.task)
        })
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
        // next one.
        self.switch_session(None);
        true
    }

    /// Take the panel over from the delegating agent: the task ends for it
    /// with what was said so far, and the conversation is the user's.
    pub(crate) fn take_over(&mut self) -> Vec<PanelEvent> {
        let Some(delegation) = self.delegation.take() else {
            return Vec::new();
        };
        let _ = delegation
            .updates
            .send(DelegateUpdate::Ended(DelegateEnd::TakenOver));
        self.notice(
            termide_i18n::t().agent_notice_taken_over_fmt(&delegation.from),
            NoticeKind::Info,
        );
        vec![PanelEvent::NeedsRedraw]
    }
}
