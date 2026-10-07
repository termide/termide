//! What the panel does when a run stops on a failure: try again at once,
//! wait for a limit's reset (or back off) and try again, restart an
//! external agent that died, or carry on with another agent — asked on a
//! card, or done unasked as `[ai] on_limit` says.

use std::sync::PoisonError;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use chrono::{Local, TimeZone};
use termide_agent_core::{Failure, FailureKind, LimitPolicy, PromptError, Session};
use termide_core::PanelEvent;
use termide_ui::ChoiceForm;

use crate::pending::Pending;
use crate::{transcript, AgentPanel, NoticeKind, FAILOVER_ACTION};

/// The state strip's mark for a request waiting to be tried again.
pub(crate) const WAIT_GLYPH: &str = "↻";

/// A failed request waiting to be tried again.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RetryWait {
    /// When the next try goes.
    pub(crate) at: Instant,
    /// The same moment on the wall clock, Unix seconds, for the display.
    pub(crate) wall: u64,
    pub(crate) failure: Failure,
    /// Start the external agent anew before the try.
    pub(crate) restart: bool,
}

/// A row of the card shown after a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FailureChoice {
    RetryNow,
    /// Wait for the reset, or back off, then try again.
    Wait,
    /// Start the external agent anew, then try again.
    Restart,
    /// Compact the context, then try again (the built-in loop).
    Compact,
    /// Pick another agent and try the request there.
    SwitchAgent,
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `21:50` today, or `2026-10-09 17:00` on another day, local time.
pub(crate) fn clock(wall: u64) -> String {
    let Some(at) = i64::try_from(wall)
        .ok()
        .and_then(|secs| Local.timestamp_opt(secs, 0).single())
    else {
        return String::new();
    };
    if at.date_naive() == Local::now().date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%Y-%m-%d %H:%M").to_string()
    }
}

/// How long until `wall`, as the transcript writes durations.
fn left_until(wall: u64) -> String {
    let secs = wall.saturating_sub(unix_now());
    transcript::fmt_dur(u32::try_from(secs.saturating_mul(1000)).unwrap_or(u32::MAX))
}

/// The card's title for a failure of `kind`.
fn title(kind: FailureKind) -> String {
    let t = termide_i18n::t();
    match kind {
        FailureKind::Transient => t.agent_failure_transient(),
        FailureKind::RateLimited => t.agent_failure_rate_limited(),
        FailureKind::QuotaExhausted => t.agent_failure_quota(),
        FailureKind::Auth => t.agent_failure_auth(),
        FailureKind::BadRequest => t.agent_failure_bad_request(),
        FailureKind::ContextExhausted => t.agent_failure_context(),
        FailureKind::AgentDied => t.agent_failure_agent_died(),
        FailureKind::Unknown => t.agent_failure_unknown(),
    }
    .to_string()
}

impl AgentPanel {
    /// A run ended on `failure` (`error` its text). Returns whether the
    /// request is still in hand — a retry, a wait or a card — so a goal or
    /// loop it belongs to is kept rather than stopped.
    pub(crate) fn on_run_failed(&mut self, failure: Failure, error: &str) -> bool {
        // An agent that died is started anew once per request, unasked —
        // from the next tick, once the runtime has let go of the run.
        if failure.kind == FailureKind::AgentDied && self.external && !self.restarted {
            self.notice(
                termide_i18n::t().agent_notice_agent_restarting(),
                NoticeKind::Warn,
            );
            self.retry_wait = Some(RetryWait {
                at: Instant::now(),
                wall: unix_now(),
                failure,
                restart: true,
            });
            return true;
        }
        if failure.kind.passes() {
            match self.on_limit {
                LimitPolicy::Wait => return self.schedule_retry(failure),
                LimitPolicy::Stop => return false,
                LimitPolicy::Ask => {}
            }
        }
        self.offer_failure(failure, error);
        true
    }

    /// The card after a failure: what happened, and what can be done.
    fn offer_failure(&mut self, failure: Failure, error: &str) {
        let t = termide_i18n::t();
        let mut rows: Vec<(String, FailureChoice)> = Vec::new();
        let wait = failure.retry_delay(self.retry_attempts, unix_now());
        if let (Some(_), Some(at)) = (wait, failure.resets_at) {
            rows.push((
                t.agent_failure_wait_until_fmt(&clock(at)),
                FailureChoice::Wait,
            ));
        }
        if failure.kind == FailureKind::AgentDied && self.external {
            rows.push((
                t.agent_failure_restart().to_string(),
                FailureChoice::Restart,
            ));
        } else {
            rows.push((
                t.agent_failure_retry_now().to_string(),
                FailureChoice::RetryNow,
            ));
        }
        if wait.is_some() && failure.resets_at.is_none() {
            rows.push((
                t.agent_failure_keep_trying().to_string(),
                FailureChoice::Wait,
            ));
        }
        if failure.kind == FailureKind::ContextExhausted && !self.external {
            rows.push((
                t.agent_failure_compact().to_string(),
                FailureChoice::Compact,
            ));
        }
        if self.catalog.list().len() > 1 {
            rows.push((
                t.agent_failure_switch_agent().to_string(),
                FailureChoice::SwitchAgent,
            ));
        }
        let mut detail = error.trim().to_string();
        if let Some(at) = failure.resets_at {
            detail.push('\n');
            detail.push_str(&t.agent_failure_resets_fmt(&clock(at), &left_until(at)));
        }
        if failure.kind == FailureKind::Auth {
            detail.push('\n');
            detail.push_str(t.agent_failure_auth_hint());
        }
        let (labels, choices): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
        let form = ChoiceForm::new(title(failure.kind), labels)
            .with_detail(detail)
            .with_cancel(t.agent_failure_stop());
        self.pending = Some(Pending::Failure {
            form,
            choices,
            failure,
        });
        self.raise_attention(true);
    }

    /// Carry out a row of the failure card.
    pub(crate) fn choose_after_failure(
        &mut self,
        choice: FailureChoice,
        failure: Failure,
    ) -> Vec<PanelEvent> {
        match choice {
            FailureChoice::RetryNow => {
                self.retry_now();
            }
            FailureChoice::Wait => {
                self.schedule_retry(failure);
            }
            FailureChoice::Restart => {
                self.restart_and_retry();
            }
            FailureChoice::Compact => {
                // The worker compacts before it takes the retry.
                match self.runtime.compact(None) {
                    Ok(()) => {
                        self.retry_now();
                    }
                    Err(error) => self.notice(
                        termide_i18n::t().agent_notice_cannot_retry_fmt(&error.to_string()),
                        NoticeKind::Warn,
                    ),
                }
            }
            FailureChoice::SwitchAgent => {
                let mut picker = self.agent_picker();
                if let PanelEvent::ShowSelect { on_select, .. } = &mut picker {
                    *on_select = termide_core::SelectAction::Custom(FAILOVER_ACTION.to_string());
                }
                return vec![picker, PanelEvent::NeedsRedraw];
            }
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// The failure card was dismissed: the request is given up, and a loop
    /// or goal it was part of ends with it.
    pub(crate) fn give_up_after_failure(&mut self) {
        self.retry_attempts = 0;
        self.retry_wait = None;
        if self.goal_task.take().is_some() {
            self.notice(
                termide_i18n::t().agent_notice_goal_stopped_failed(),
                NoticeKind::Warn,
            );
        }
        self.loop_task = None;
    }

    /// Wait as `failure` says — for its reset, or a backoff that grows with
    /// each try — then try again. `false` when waiting does not cure it.
    pub(crate) fn schedule_retry(&mut self, failure: Failure) -> bool {
        let now = unix_now();
        let Some(delay) = failure.retry_delay(self.retry_attempts, now) else {
            return false;
        };
        let wall = now + delay.as_secs();
        self.retry_wait = Some(RetryWait {
            at: Instant::now() + delay,
            wall,
            failure,
            restart: false,
        });
        self.notice(
            termide_i18n::t().agent_notice_retry_waiting_fmt(&clock(wall)),
            NoticeKind::Info,
        );
        true
    }

    /// Stop waiting to try again; the request is given up.
    pub(crate) fn cancel_retry_wait(&mut self) {
        if self.retry_wait.take().is_some() {
            self.retry_attempts = 0;
            self.loop_task = None;
            self.goal_task = None;
            self.notice(
                termide_i18n::t().agent_notice_retry_cancelled(),
                NoticeKind::Info,
            );
        }
    }

    /// Try the failed request again now.
    pub(crate) fn retry_now(&mut self) -> bool {
        self.retry_wait = None;
        self.retry_ready = false;
        // The files the new try touches are kept for /undo, as a request's.
        if let Some(store) = &self.checkpoints {
            let leaf = self
                .session
                .as_ref()
                .and_then(Session::undo_point)
                .map(str::to_string);
            store
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .begin_run(leaf);
        }
        match self.runtime.retry() {
            Ok(()) => {
                self.retry_attempts = self.retry_attempts.saturating_add(1);
                self.busy = true;
                self.goal_errored = false;
                self.follow = true;
                self.notice(termide_i18n::t().agent_notice_retrying(), NoticeKind::Info);
                true
            }
            // A dead external agent cannot take it: a new one is started.
            Err(PromptError::Stopped) if self.external && !self.restarted => {
                self.restart_and_retry()
            }
            Err(error) => {
                self.notice(
                    termide_i18n::t().agent_notice_cannot_retry_fmt(&error.to_string()),
                    NoticeKind::Warn,
                );
                false
            }
        }
    }

    /// Start the external agent anew on the same session, then try again; a
    /// goal or loop under way carries on.
    pub(crate) fn restart_and_retry(&mut self) -> bool {
        self.restarted = true;
        let (goal, task) = (self.goal_task.take(), self.loop_task.take());
        let session = self.session.take();
        self.switch_session(session);
        self.goal_task = goal;
        self.loop_task = task;
        self.retry_now()
    }

    /// The agent was switched from the failure card: the request goes to it.
    pub(crate) fn retry_on_switched_agent(&mut self, name: &str) -> bool {
        let (goal, task) = (self.goal_task.take(), self.loop_task.take());
        let switched = self.switch_agent(name);
        self.goal_task = goal;
        self.loop_task = task;
        if switched {
            self.retry_attempts = 0;
            self.retry_now();
        }
        switched
    }

    /// The wait is over: try again, once the panel is free.
    pub(crate) fn poll_retry_wait(&mut self) -> bool {
        let due = self.retry_wait.filter(|wait| wait.at <= Instant::now());
        if let Some(wait) = due.filter(|_| !self.is_busy() && self.pending.is_none()) {
            if wait.restart {
                self.restart_and_retry();
            } else {
                self.retry_now();
            }
            return true;
        }
        // Redrawn only when the countdown's text changes.
        let text = self.retry_wait_text();
        if text != self.retry_wait_shown {
            self.retry_wait_shown = text;
            return true;
        }
        false
    }

    /// The state strip's line for a wait: when the next try goes, and why.
    pub(crate) fn retry_wait_text(&self) -> Option<String> {
        let wait = self.retry_wait?;
        Some(termide_i18n::t().agent_state_retry_at_fmt(
            &title(wait.failure.kind),
            &clock(wait.wall),
            &left_until(wait.wall),
        ))
    }
}
