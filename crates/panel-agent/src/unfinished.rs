//! Work the panel or termide closed on: the run under way, a `/goal` or a
//! `/loop`. The session log records where runs start and end and what
//! unattended work is going, so a reopened session asks whether to carry it
//! on. Nothing goes on by itself: the card's Continue (or `/continue`, `[▶]`)
//! carries it on, Drop it (or `[■]`) gives it up, and Decide later leaves the
//! panel waiting at the pause.

use std::time::{Duration, Instant};

use termide_agent_core::{
    now_millis, unanswered_calls, Agent, Autorun, GoalRecord, LastRun, LoggedMessage, LoopRecord,
    Message, RunMark, Session, ToolResultMessage,
};
use termide_ui::ChoiceForm;

use crate::pending::Pending;
use crate::runtime::{hms_from_millis, push_history};
use crate::submit::fmt_secs;
use crate::transcript::Item;
use crate::{
    single_line, truncate_title, AgentPanel, GoalTask, LoopTask, GOAL_MAX_ITERATIONS,
    LOOP_MAX_ITERATIONS,
};

/// What a call a cut-off run left without a result is told: it may have run
/// in part, so the model looks before it runs it again.
const CUT_OFF_CALL: &str = "Not finished: termide closed during this run, while this call ran or before it started. It may have run in part — check its effects before running it again.";

/// The message that carries on a run an external agent was cut off in.
const CARRY_ON_PROMPT: &str = "termide closed while you were working on the request above, so your last run was cut off. Carry on from where you stopped; check the effects of any step that may have run only in part before repeating it.";

/// How work taken up from the log goes on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Restored {
    /// The run is resumed on the built-in loop, from the transcript.
    Resume,
    /// The external agent, which cannot be resumed between steps, is told
    /// to carry on.
    Prompt,
    /// No run to carry on: the goal or loop takes its next step.
    Autorun,
}

impl AgentPanel {
    /// The unattended work going now, as the log records it.
    fn current_autorun(&self) -> Autorun {
        Autorun {
            goal: self.goal_task.as_ref().map(|task| GoalRecord {
                goal: task.goal.clone(),
                iterations: task.iterations,
            }),
            repeat: self.loop_task.as_ref().map(|task| LoopRecord {
                prompt: task.prompt.clone(),
                interval_ms: task
                    .interval
                    .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                iterations: task.iterations,
                due_ms: task.due_ms,
            }),
        }
    }

    /// Write the unattended work to the log when it changed since the log
    /// last recorded it: a goal or loop started, stepped or stopped.
    pub(crate) fn sync_autorun(&mut self) {
        let current = self.current_autorun();
        if current == self.logged_autorun {
            return;
        }
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_autorun(&current) {
                log::warn!("agent session write failed: {error}");
            }
        }
        // Written or not, it is not tried again on every tick.
        self.logged_autorun = current;
    }

    /// Take the log's record of the unattended work as what it now holds,
    /// once the panel moved onto `session`'s log.
    pub(crate) fn adopt_logged_autorun(&mut self) {
        self.logged_autorun = self
            .session
            .as_ref()
            .map(Session::autorun)
            .unwrap_or_default();
    }

    /// The run's first message is in the log: record after it that the run
    /// started, with the goal or loop it belongs to, so a run cut off from
    /// here is known as such.
    pub(crate) fn log_run_start(&mut self) {
        if !std::mem::take(&mut self.run_start_due) {
            return;
        }
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_run_start() {
                log::warn!("agent session write failed: {error}");
            }
        }
        self.sync_autorun();
    }

    /// Record in the log that the run ended, unless it logged nothing.
    pub(crate) fn log_run_end(&mut self) {
        if std::mem::take(&mut self.run_start_due) {
            return;
        }
        let paused = self.run_paused;
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_run_end(paused) {
                log::warn!("agent session write failed: {error}");
            }
        }
    }

    /// A session just opened: when its log shows a run cut off or paused, or
    /// a goal or loop still going, the panel waits at a pause and asks on a
    /// card whether to carry the work on.
    pub(crate) fn restore_unfinished(&mut self) {
        self.restored = None;
        self.adopt_logged_autorun();
        let Some(mark) = self.session.as_ref().map(Session::unfinished_run) else {
            return;
        };
        let autorun = self.logged_autorun.clone();
        let restored = match mark {
            Some(_) if self.runtime.can_pause() => Restored::Resume,
            Some(_) if self.external => Restored::Prompt,
            _ if !autorun.is_empty() => Restored::Autorun,
            _ => return,
        };
        if let Some(mark) = mark {
            if mark.state == LastRun::CutOff && restored == Restored::Resume {
                self.close_cut_off_calls();
            }
            self.mark_run_end(mark);
        }
        if let Some(goal) = autorun.goal.clone() {
            self.goal_task = Some(GoalTask {
                goal: goal.goal,
                iterations: goal.iterations,
                judge_at: None,
                judging: false,
            });
        }
        if let Some(repeat) = autorun.repeat.clone() {
            self.loop_task = Some(LoopTask {
                prompt: repeat.prompt,
                interval: repeat.interval_ms.map(Duration::from_millis),
                next_at: None,
                due_ms: repeat.due_ms,
                iterations: repeat.iterations,
            });
        }
        self.paused = true;
        self.restored = Some(restored);
        self.ask_carry_on(mark, &autorun);
    }

    /// The calls a cut-off run left without a result may have run in part:
    /// each gets a result saying so — in the log, the transcript and the
    /// agent's context — so the resume asks the model rather than running
    /// them again unseen.
    fn close_cut_off_calls(&mut self) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let calls = unanswered_calls(&session.context_messages());
        for call in calls {
            let message = Message::ToolResult(ToolResultMessage::error(&call, CUT_OFF_CALL));
            if let Err(error) = session.append_message(&message) {
                log::warn!("agent session write failed: {error}");
            }
            push_history(
                &mut self.transcript,
                &LoggedMessage {
                    message: message.clone(),
                    timestamp: now_millis(),
                    timing: None,
                },
            );
            let delivered = self.runtime.update(Box::new(move |agent: &mut Agent| {
                agent.append_context(message);
            }));
            if let Err(error) = delivered {
                log::debug!("cannot close a cut-off call in the context: {error}");
            }
        }
    }

    /// Close the restored transcript with the run's line, as a run that
    /// ended would have left it: `‖` for a pause, `✻ … cut off` for a run
    /// cut off, with how long it ran and when it stopped.
    fn mark_run_end(&mut self, mark: RunMark) {
        let elapsed = u32::try_from(mark.stopped.saturating_sub(mark.started)).unwrap_or(u32::MAX);
        let paused = mark.state == LastRun::Paused;
        self.transcript.push(Item::RunEnd {
            elapsed_ms: elapsed,
            at: hms_from_millis(mark.stopped),
            ok: paused,
            paused,
            live: false,
            cut: !paused,
        });
    }

    /// The card asking whether to carry the restored work on, naming what
    /// it is: the run and how long ago it stopped, the goal or loop and how
    /// far each has gone.
    fn ask_carry_on(&mut self, mark: Option<RunMark>, autorun: &Autorun) {
        let t = termide_i18n::t();
        let now = now_millis();
        let mut detail: Vec<String> = Vec::new();
        if let Some(mark) = mark {
            let age = termide_i18n::relative_age(now.saturating_sub(mark.stopped) / 1000);
            detail.push(match mark.state {
                LastRun::Paused => t.agent_unfinished_run_paused_fmt(&age),
                _ => t.agent_unfinished_run_cut_fmt(&age),
            });
        }
        if let Some(goal) = &autorun.goal {
            detail.push(t.agent_unfinished_goal_fmt(
                &truncate_title(&single_line(&goal.goal)),
                goal.iterations,
                GOAL_MAX_ITERATIONS,
            ));
        }
        if let Some(repeat) = &autorun.repeat {
            let interval = match repeat.interval_ms {
                Some(ms) => fmt_secs(ms / 1000),
                None => t.agent_unfinished_back_to_back().to_string(),
            };
            let mut line = t.agent_unfinished_loop_fmt(
                &interval,
                &truncate_title(&single_line(&repeat.prompt)),
                repeat.iterations,
                LOOP_MAX_ITERATIONS,
            );
            if let Some(due) = repeat.due_ms.filter(|&due| due > now) {
                line.push_str(", ");
                line.push_str(&t.agent_unfinished_loop_due_fmt(&fmt_secs((due - now) / 1000)));
            }
            detail.push(line);
        }
        let form = ChoiceForm::new(
            t.agent_unfinished_title(),
            vec![
                t.agent_unfinished_continue().to_string(),
                t.agent_unfinished_drop().to_string(),
            ],
        )
        .with_detail(detail.join("\n"))
        .with_cancel(t.agent_unfinished_later());
        self.pending = Some(Pending::Unfinished { form });
        // Highlighted until seen, without the bell: nothing is waiting on a
        // run, only on the user's word.
        self.raise_attention(false);
    }

    /// `/continue` on a pause taken up from the log that `resume` does not
    /// take itself: the external agent is told to carry on, or the goal is
    /// judged and the loop runs again — now, or once what is left of its
    /// wait has passed. `false` when the pause was an ordinary one.
    pub(crate) fn carry_on_restored(&mut self) -> bool {
        match self.restored {
            Some(Restored::Prompt) => {
                self.restored = None;
                self.paused = false;
                let events = self.send_as(CARRY_ON_PROMPT.to_string(), Some("/continue".into()));
                self.pending_events.extend(events);
                true
            }
            Some(Restored::Autorun) => {
                self.restored = None;
                self.paused = false;
                let now = Instant::now();
                let wall = now_millis();
                if let Some(task) = self.goal_task.as_mut() {
                    task.judge_at = Some(now);
                }
                if let Some(task) = self.loop_task.as_mut() {
                    let wait = task.due_ms.map_or(0, |due| due.saturating_sub(wall));
                    task.next_at = Some(now + Duration::from_millis(wait));
                }
                true
            }
            Some(Restored::Resume) => {
                self.restored = None;
                false
            }
            None => false,
        }
    }

    /// A restored pause that waited only on a goal or loop ends once they
    /// are stopped (`/goal stop`, `/loop stop`, Esc).
    pub(crate) fn settle_restored(&mut self) -> bool {
        if self.restored == Some(Restored::Autorun)
            && self.goal_task.is_none()
            && self.loop_task.is_none()
        {
            self.restored = None;
            self.paused = false;
            if matches!(self.pending, Some(Pending::Unfinished { .. })) {
                self.pending = None;
            }
            return true;
        }
        false
    }

    /// Whether closing the panel would cut work off: a run under way, or a
    /// goal or loop going that the user has not left at a restored pause.
    pub(crate) fn work_under_way(&self) -> bool {
        self.is_busy()
            || (self.restored.is_none() && (self.goal_task.is_some() || self.loop_task.is_some()))
    }
}
