//! Work the panel or termide closed on: the run under way, a `/goal` or a
//! `/loop`. The session log records where runs start and end and what
//! unattended work is going, so a reopened session asks whether to carry it
//! on. Nothing goes on by itself: the card's Continue (or `/continue`, `[▶]`)
//! carries it on, Drop it (or `[■]`) gives it up, and Decide later leaves the
//! panel waiting at the pause.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use termide_agent_core::{
    now_millis, unanswered_calls, Agent, AssistantContent, AssistantMessage, Autorun, Failure,
    GoalRecord, LastRun, LoggedMessage, LoopRecord, Message, RunMark, Session, StopReason,
    ToolCall, ToolResultMessage, Usage, ACP_PROVIDER,
};
use termide_core::{DelegateRestart, PanelEvent};
use termide_ui::ChoiceForm;

use crate::pending::Pending;
use crate::runtime::{hms_from_millis, push_history};
use crate::transcript::Item;
use crate::{single_line, truncate_title, AgentPanel, GoalTask, LoopTask, LOOP_MAX_ITERATIONS};

/// What a call a cut-off run left without a result is told: it may have run
/// in part, so the model looks before it runs it again.
const CUT_OFF_CALL: &str = "Not finished: termide closed during this run, while this call ran or before it started. It may have run in part — check its effects before running it again.";

/// The message that carries on a run an external agent was cut off in.
const CARRY_ON_PROMPT: &str = "termide closed while you were working on the request above, so your last run was cut off. Carry on from where you stopped; check the effects of any step that may have run only in part before repeating it.";

/// The run a reopened delegate's session carries on answers a task another
/// session delegated: the answer goes back there once the run is over, as a
/// background task's answer does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarryBack {
    /// The delegating session's id: panel numbers are born anew each run of
    /// termide, so the session is what still names the panel.
    pub parent_session: String,
    /// The delegate's own name, for the answer's heading.
    pub agent: String,
    /// The run that carries the task on is under way; its end reports back.
    pub running: bool,
}

/// A delegated task left unfinished in its own log when termide closed: it
/// stands where it stopped, with its transcript, and waits to be carried on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoredDelegate {
    /// The delegate's name.
    pub agent: String,
    /// Its session log, which holds the task's transcript.
    pub session: std::path::PathBuf,
    /// The `task` call that handed it over.
    pub call: String,
    /// The task's first line, for the card.
    pub task: String,
}

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
    /// The tasks this panel delegated and left unfinished in their own logs:
    /// each was running when termide closed, and stands with its transcript
    /// where it stopped. Newest first, as the session lists are.
    pub(crate) fn unfinished_delegates(&self) -> Vec<RestoredDelegate> {
        let (Some(dir), Some(id)) = (
            self.session_dir.as_deref(),
            self.session
                .as_ref()
                .map(|session| session.id().to_string()),
        ) else {
            return Vec::new();
        };
        let Ok(summaries) = Session::list(dir) else {
            return Vec::new();
        };
        summaries
            .into_iter()
            .filter_map(|summary| {
                // Read what is taken out of the summary before taking it:
                // the label and the agent's name need the whole there.
                let task = summary.label();
                // The delegate's own name, as its log has it: the parent
                // names the agent that delegated, not the one that ran.
                let agent = summary.agent;
                let parent = summary.parent?;
                // A log open in a panel — or carried on out of sight — is
                // already in hand there.
                if !summary.unfinished
                    || parent.session.as_deref() != Some(id.as_str())
                    || crate::live::is_running(self.panel_id, &parent.call)
                    || Session::is_open(&summary.path)
                {
                    return None;
                }
                Some(RestoredDelegate {
                    agent: agent.unwrap_or(parent.agent),
                    session: summary.path,
                    call: parent.call,
                    task,
                })
            })
            .collect()
    }

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

    /// The failed request the panel still holds: one a card asks about, or one
    /// waited on. `None` when neither — the request went, was retried or was
    /// given up.
    fn held_failure(&self) -> Option<Failure> {
        match &self.pending {
            Some(Pending::Failure { failure, .. }) => Some(*failure),
            _ => self.retry_wait.map(|wait| wait.failure),
        }
    }

    /// Write the held failed request to the log when it changed since the log
    /// last recorded it: a card raised, a wait scheduled, the request taken
    /// back. So a session reopened after termide closed asks the same again.
    pub(crate) fn sync_retry(&mut self) {
        let held = self.held_failure();
        if self.logged_retry == Some(held) {
            return;
        }
        if let Some(session) = &mut self.session {
            if let Err(error) = session.append_retry(held) {
                log::warn!("agent session write failed: {error}");
            }
        }
        // Written or not, it is not tried again on every tick.
        self.logged_retry = Some(held);
    }

    /// Take the log's record of a held request as what the panel now knows,
    /// once it moved onto `session`'s log.
    pub(crate) fn adopt_logged_retry(&mut self) {
        self.logged_retry = Some(self.session.as_ref().and_then(Session::pending_retry));
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

    /// The panel closes — or termide quits — on a run under way: the calls
    /// running now go into the log, each with a result saying it was cut
    /// off, so a reopened session shows what was cut off and asks to carry
    /// it on. An external agent logs a call only with its result: without
    /// this a run cut off in a long call leaves no trace of the call. The
    /// run's end stays unwritten, as for a run cut off any other way.
    pub(crate) fn log_cut_off_calls(&mut self) {
        // Nothing of a run whose first message is not logged yet.
        if !self.is_busy() || self.run_start_due {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut context = session.context_messages();
        let running: Vec<ToolCall> = {
            let logged: HashSet<&str> = context
                .iter()
                .filter_map(|message| match message {
                    Message::Assistant(reply) => Some(reply),
                    _ => None,
                })
                .flat_map(|reply| reply.tool_calls().map(|call| call.id.as_str()))
                .collect();
            self.transcript
                .items()
                .iter()
                .filter_map(|item| match item {
                    Item::Tool {
                        call, result: None, ..
                    } if self.tool_starts.contains_key(&call.id)
                        && !logged.contains(call.id.as_str()) =>
                    {
                        Some(call.clone())
                    }
                    _ => None,
                })
                .collect()
        };
        if !running.is_empty() {
            let message = Message::Assistant(AssistantMessage {
                content: running
                    .into_iter()
                    .map(AssistantContent::ToolCall)
                    .collect(),
                stop_reason: StopReason::ToolUse,
                usage: Usage::default(),
                provider: if self.external {
                    ACP_PROVIDER.to_string()
                } else {
                    self.model.provider.clone()
                },
                model: self.model.id.clone(),
                error_message: None,
                failure: None,
                timestamp: now_millis(),
            });
            if let Err(error) = session.append_message(&message) {
                log::warn!("agent session write failed: {error}");
                return;
            }
            context.push(message);
        }
        for call in unanswered_calls(&context) {
            let result = Message::ToolResult(ToolResultMessage::error(&call, CUT_OFF_CALL));
            if let Err(error) = session.append_message(&result) {
                log::warn!("agent session write failed: {error}");
            }
        }
    }

    /// A session just opened: when its log shows a run cut off or paused, or
    /// a goal or loop still going, the panel waits at a pause and asks on a
    /// card whether to carry the work on. A run that ended on a request a
    /// failure card asked about — termide closed while the card was up, or
    /// over a wait — brings that same card back, so the choice the user never
    /// made is asked again. Nothing runs by itself either way.
    pub(crate) fn restore_unfinished(&mut self) {
        self.restored = None;
        self.adopt_logged_autorun();
        self.adopt_logged_retry();
        let Some(mark) = self.session.as_ref().map(Session::unfinished_run) else {
            return;
        };
        let autorun = self.logged_autorun.clone();
        // A delegate's own session: whatever the run carrying it on ends on
        // is the answer the panel that delegated the task is waiting for.
        self.carry_back = self.carry_back_from_log();
        // A request a card asked about is asked again first, ahead of any
        // carry-on: it is the nearer decision, and a limit wants its retry,
        // wait or switch rather than a judge over work that never ran. The
        // goal or loop it belonged to comes back with it, waiting on the card.
        if let Some(failure) = self.session.as_ref().and_then(Session::pending_retry) {
            // The run's own line stays where it was left, so the transcript
            // reads the same as it did before termide closed.
            if let Some(mark) = mark {
                self.mark_run_end(mark);
            }
            self.restore_autorun_tasks(&autorun);
            // The request stands where it stood: ready for `/continue`, its
            // waits counting from the first try again.
            self.retry_attempts = 0;
            self.retry_ready = true;
            self.offer_failure(failure);
            // The card asks about the request; the delegates hold rows above
            // the input, where they can still be carried on.
            self.adopt_unfinished_delegates();
            return;
        }
        let restored = match mark {
            Some(_) if self.runtime.can_pause() => Restored::Resume,
            Some(_) if self.external => Restored::Prompt,
            _ if !autorun.is_empty() => Restored::Autorun,
            // Nothing of the panel's own is left unfinished. Its delegated
            // tasks may well be: they ran out of sight and their logs stand
            // where termide stopped them, so the panel asks about those alone.
            _ => {
                self.adopt_unfinished_delegates();
                if !self.restored_delegates.is_empty() {
                    self.ask_carrying_delegates();
                }
                return;
            }
        };
        if let Some(mark) = mark {
            if mark.state == LastRun::CutOff {
                self.close_cut_off_calls();
            }
            self.mark_run_end(mark);
        }
        self.restore_autorun_tasks(&autorun);
        self.paused = true;
        self.restored = Some(restored);
        self.adopt_unfinished_delegates();
        self.ask_carry_on(mark, &autorun);
    }

    /// What this session owes the panel that delegated it, when it is a
    /// delegate's own: the answer of the run that carries its task on.
    fn carry_back_from_log(&self) -> Option<CarryBack> {
        let session = self.session.as_ref()?;
        let parent = session.header().parent.as_ref()?;
        Some(CarryBack {
            parent_session: parent.session.clone()?,
            agent: session
                .current_agent()
                .unwrap_or_else(|| parent.agent.clone()),
            running: false,
        })
    }

    /// The run carrying a delegated task on is under way — resumed, told to
    /// go on or tried again: its end owes the delegating panel the answer.
    pub(crate) fn owe_carry_back(&mut self) {
        if let Some(carry) = &mut self.carry_back {
            carry.running = true;
        }
    }

    /// The run carrying a delegated task on is over: its answer goes back to
    /// the panel that delegated it, by that session's id.
    pub(crate) fn report_carried_back(&mut self) {
        let Some(carry) = self.carry_back.take().filter(|carry| carry.running) else {
            return;
        };
        let Some(from) = self.session_path().map(std::path::Path::to_path_buf) else {
            return;
        };
        let answer = self.last_answer();
        let t = termide_i18n::t();
        let text = if self.run_failed {
            t.agent_restored_failed_fmt(&carry.agent, &answer)
        } else {
            t.agent_restored_report_fmt(&carry.agent, &answer)
        };
        self.pending_events
            .push(PanelEvent::DelegateAnswerToParent {
                parent_session: carry.parent_session,
                from,
                agent: carry.agent,
                text,
            });
    }

    /// The run's last words, as its log has them: what the panel that
    /// delegated the task is told when this one finishes carrying it on.
    pub(crate) fn last_answer(&self) -> String {
        self.session
            .as_ref()
            .and_then(|session| {
                session
                    .branch()
                    .into_iter()
                    .rev()
                    .find_map(|entry| match &entry.kind {
                        termide_agent_core::EntryKind::Message {
                            message: Message::Assistant(reply),
                            ..
                        } => Some(reply.plain_text()),
                        _ => None,
                    })
            })
            .unwrap_or_default()
    }

    /// Take the delegated tasks this session left unfinished as the panel's
    /// own: each holds a row above the input until the user carries it on or
    /// leaves it. Nothing runs by itself here.
    fn adopt_unfinished_delegates(&mut self) {
        for delegate in self.unfinished_delegates() {
            if !self
                .restored_delegates
                .iter()
                .any(|kept| kept.session == delegate.session)
            {
                self.restored_delegates.push(delegate);
            }
        }
    }

    /// Carry the unfinished delegated tasks on: each runs again out of sight
    /// under the `task` call that handed it over, asking through this panel's
    /// prompter and answering back here, as any background task does. The
    /// rows they held go.
    pub(crate) fn restart_delegates(&mut self) -> Vec<PanelEvent> {
        let chosen = std::mem::take(&mut self.restored_delegates);
        if chosen.is_empty() {
            return vec![PanelEvent::NeedsRedraw];
        }
        let items: Vec<DelegateRestart> = chosen
            .iter()
            .map(|delegate| DelegateRestart {
                agent: delegate.agent.clone(),
                session: delegate.session.clone(),
                call: delegate.call.clone(),
                task: delegate.task.clone(),
            })
            .collect();
        vec![
            PanelEvent::RestartDelegates {
                panel: self.panel_id,
                items,
            },
            PanelEvent::NeedsRedraw,
        ]
    }

    /// The panel is idle and has nothing of its own to carry on, but it left
    /// delegated tasks unfinished: ask whether to carry those on. Continue
    /// runs them out of sight, and their answers come back here; Later leaves
    /// the rows, and the tasks wait in the session lists (`F6`), `⏸ &`.
    fn ask_carrying_delegates(&mut self) {
        let t = termide_i18n::t();
        let detail: Vec<String> = self
            .restored_delegates
            .iter()
            .map(|delegate| {
                t.agent_unfinished_delegate_fmt(
                    &delegate.agent,
                    &truncate_title(&single_line(&delegate.task)),
                )
            })
            .collect();
        let form = ChoiceForm::new(
            t.agent_unfinished_delegates_title(),
            vec![
                t.agent_unfinished_continue().to_string(),
                t.agent_unfinished_later().to_string(),
            ],
        )
        .with_detail(detail.join("\n"));
        self.pending = Some(Pending::Unfinished { form });
        self.raise_attention(true);
    }

    /// Take the log's `/goal` and `/loop` back as the panel's tasks, their
    /// clocks starting afresh, as they do on any reopen.
    fn restore_autorun_tasks(&mut self, autorun: &Autorun) {
        if let Some(goal) = autorun.goal.clone() {
            self.goal_task = Some(GoalTask {
                goal: goal.goal,
                iterations: goal.iterations,
                // The clock starts again on a reopen, as the idle count does.
                started: Instant::now(),
                used_tools: false,
                idle_turns: 0,
                judge_at: None,
                judging: false,
                judge_failures: 0,
                // Logged before its first turn went, it starts with that.
                first_turn: (goal.iterations == 0).then_some(None),
            });
        }
        if let Some(repeat) = autorun.repeat.clone() {
            self.loop_task = Some(LoopTask {
                prompt: repeat.prompt,
                interval: repeat.interval_ms.map(Duration::from_millis),
                next_at: None,
                due_ms: repeat.due_ms,
                iterations: repeat.iterations,
                started: Instant::now(),
            });
        }
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
            // An external agent keeps its own context, which its resume
            // brings back.
            if !self.runtime.can_pause() {
                continue;
            }
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
        let delegates = self.unfinished_delegates();
        let mut detail: Vec<String> = Vec::new();
        for delegate in &delegates {
            detail.push(t.agent_unfinished_delegate_fmt(
                &delegate.agent,
                &truncate_title(&single_line(&delegate.task)),
            ));
        }
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
            ));
        }
        if let Some(repeat) = &autorun.repeat {
            let interval = match repeat.interval_ms {
                Some(ms) => termide_i18n::duration(Duration::from_millis(ms)),
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
                line.push_str(&t.agent_unfinished_loop_due_fmt(&termide_i18n::duration(
                    Duration::from_millis(due - now),
                )));
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
        // Highlighted until seen, with the bell: the work stands still until
        // the user says, as at a restored failure card.
        self.raise_attention(true);
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
                self.owe_carry_back();
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
