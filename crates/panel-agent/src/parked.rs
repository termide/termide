//! The delegates this panel parked at a limit: the card that asks what to do
//! with them, and the answers.
//!
//! A delegated task stopped by a limit that passes on its own answers nothing:
//! it waits, and goes again at the reset — or at once when the panel moves to
//! another connection, which is how a spent subscription is got past. `ask` and
//! `ask_else_wait` raise this panel's card over them; `wait` leaves the rows
//! alone to say where each stands. A card left unanswered does what `wait`
//! would: the task goes again at the moment the row already names.

use termide_agent_core::{LimitPolicy, MessageOrigin, OriginKind, UserMessage};
use termide_ui::ChoiceForm;

use crate::failure::{clock, left_until, unix_now};
use crate::live::{self, Parked};
use crate::pending::Pending;
use crate::{AgentPanel, NoticeKind};

impl AgentPanel {
    /// The parked delegates: their card when the panel's policy asks, and a
    /// redraw once their countdown's text moves. `true` when the panel is to
    /// be redrawn.
    pub(crate) fn poll_parked(&mut self) -> bool {
        let parked = live::parked_of(self.panel_id);
        if parked.is_empty() {
            self.parked_asked.clear();
            return false;
        }
        // One card at a time: the panel's own failure card, a question or a
        // permission waits ahead of these, and asks again next tick. A panel
        // mid-run takes none either — its cards belong to the run.
        if self.pending.is_none()
            && !self.is_busy()
            && self.on_limit != LimitPolicy::Stop
            && self.on_limit != LimitPolicy::Wait
        {
            if let Some(parked) = parked.iter().find(|p| {
                !self
                    .parked_asked
                    .iter()
                    .any(|(call, attempts)| call == &p.call && *attempts == p.attempts)
            }) {
                let parked = parked.clone();
                self.ask_parked(&parked);
                return true;
            }
        }
        // The text moves by whole seconds, so it is compared once a second.
        let text: String = parked
            .iter()
            .map(|p| format!("{}{}", p.call, p.wake_at))
            .collect();
        if text != self.parked_shown {
            self.parked_shown = text;
            return true;
        }
        false
    }

    /// The card for one parked task: what waits there, when it goes again on
    /// its own, and the two things the user can still do.
    fn ask_parked(&mut self, parked: &Parked) {
        let t = termide_i18n::t();
        let now = unix_now();
        let mut detail = format!("{} · {}", parked.agent, single_line(&parked.task));
        if let Some(at) = parked.failure.resets_at.filter(|at| *at > now) {
            detail.push('\n');
            detail.push_str(&t.agent_failure_resets_fmt(&clock(at), &left_until(at)));
        }
        if self.on_limit == LimitPolicy::AskElseWait {
            detail.push('\n');
            detail.push_str(
                &t.agent_failure_auto_retry_fmt(
                    &clock(parked.wake_at),
                    &left_until(parked.wake_at),
                ),
            );
        }
        let form = ChoiceForm::new(
            t.agent_parked_title(),
            vec![
                t.agent_failure_retry_now().to_string(),
                t.agent_parked_give_up().to_string(),
            ],
        )
        .with_detail(detail)
        .with_cancel(t.agent_parked_keep_waiting());
        self.pending = Some(Pending::DelegateLimit {
            form,
            call: parked.call.clone(),
        });
        self.parked_asked
            .push((parked.call.clone(), parked.attempts));
        self.raise_attention(true);
    }

    /// The card's first row: the task goes again now, on the connection the
    /// panel stands on. The tick starts it; the row stays until it answers.
    pub(crate) fn try_parked_now(&mut self, call: &str) {
        live::ask_now(self.panel_id, call);
        self.notice(termide_i18n::t().agent_notice_retrying(), NoticeKind::Info);
    }

    /// The task parked under `call` is given up: nothing runs it again, and
    /// the panel's own agent is told it did not finish — otherwise it waits on
    /// an answer that will never come.
    pub(crate) fn give_up_parked(&mut self, call: &str) {
        let Some(parked) = live::abandon(self.panel_id, call) else {
            return;
        };
        let text = termide_i18n::t().agent_parked_given_up_fmt(&parked.agent);
        let hops = self.agent_hops + 1;
        if let Err(error) = self.deliver(UserMessage::text(text).with_origin(MessageOrigin {
            session: String::new(),
            agent: parked.agent.clone(),
            kind: OriginKind::Reply,
            panel: None,
            hops,
        })) {
            log::warn!("the parked task's giving-up found no ear: {error}");
        }
    }
}

/// The task's first line, cut to one line for the card's detail.
fn single_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .chars()
        .take(80)
        .collect()
}
