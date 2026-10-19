//! Questions the model asks the user in the middle of a run.
//!
//! The `question` tool blocks on the agent thread until the user answers, the
//! way a permission prompt does: the questions travel to the panel over a
//! channel and the wait wakes regularly to notice an abort. A run with no one
//! to ask — a subagent, headless mode — has no [`UserAsker`] in its
//! [`ToolContext`](crate::ToolContext), and the tool says so to the model.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::cancel::CancelToken;

/// One choice offered for a question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    /// What the user picks, and what the answer reports.
    pub label: String,
    /// What picking it means; may be empty.
    pub description: String,
}

/// One question: its text, a short header naming its topic, the choices, and
/// whether several of them can be picked. An answer of the user's own is
/// always on offer besides the choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub header: String,
    pub question: String,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
}

/// The user's answer to one question: the labels picked, in the order the
/// options were offered, and the text they typed, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuestionAnswer {
    pub chosen: Vec<String>,
    pub custom: Option<String>,
}

/// How a set of questions ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionReply {
    /// One answer per question, in their order.
    Answered(Vec<QuestionAnswer>),
    /// The user dismissed the questions, or the run stopped before they
    /// answered.
    Declined,
}

/// One outstanding set of questions and the channel for its reply.
pub struct QuestionEnvelope {
    pub questions: Vec<Question>,
    pub reply: Sender<QuestionReply>,
    /// Cancelled once no one waits for the reply any more — the run
    /// stopped — so a set still waiting its turn is never shown.
    pub withdrawn: CancelToken,
    /// The id of the `task` call that handed the asking delegate its task,
    /// so a panel following that task can take the card.
    pub task: Option<String>,
}

/// Asks the panel's user. Cloneable and shared by the calls of one run; each
/// ask blocks until its reply comes back.
#[derive(Clone)]
pub struct UserAsker {
    tx: Sender<QuestionEnvelope>,
    cancel: CancelToken,
    /// Who asks, when it is not the panel's own agent: a delegate's
    /// questions reach its parent's panel headed with its name.
    on_behalf_of: Option<String>,
    /// The `task` call that handed the delegate its task.
    task: Option<String>,
    /// Ends the wait besides `cancel`: a delegate's own stop.
    stop: Option<CancelToken>,
}

impl std::fmt::Debug for UserAsker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserAsker").finish_non_exhaustive()
    }
}

/// Build an asker and the receiver the panel polls from `tick()`.
#[must_use]
pub fn question_channel(cancel: CancelToken) -> (UserAsker, Receiver<QuestionEnvelope>) {
    let (tx, rx) = mpsc::channel();
    (
        UserAsker {
            tx,
            cancel,
            on_behalf_of: None,
            task: None,
            stop: None,
        },
        rx,
    )
}

impl UserAsker {
    /// Put `questions` to the user and wait for the reply. A stopped run, or
    /// a panel that went away, counts as declined.
    #[must_use]
    pub fn ask(&self, mut questions: Vec<Question>) -> QuestionReply {
        if let Some(agent) = &self.on_behalf_of {
            for question in &mut questions {
                question.question = format!("& {agent} · {}", question.question);
            }
        }
        let (reply, answer) = mpsc::channel();
        let withdrawn = CancelToken::new();
        let envelope = QuestionEnvelope {
            questions,
            reply,
            withdrawn: withdrawn.clone(),
            task: self.task.clone(),
        };
        if self.tx.send(envelope).is_err() {
            return QuestionReply::Declined;
        }
        let stopped = || {
            self.cancel.is_cancelled() || self.stop.as_ref().is_some_and(CancelToken::is_cancelled)
        };
        loop {
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(reply) => return reply,
                Err(RecvTimeoutError::Timeout) if stopped() => {
                    withdrawn.cancel();
                    return QuestionReply::Declined;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return QuestionReply::Declined,
            }
        }
    }

    /// This asker for the delegate `agent`: its questions reach the same
    /// user, each headed `& <agent> · ` so they never read as the panel's
    /// own agent's.
    #[must_use]
    pub fn for_delegate(&self, agent: &str) -> Self {
        Self {
            on_behalf_of: Some(agent.to_string()),
            ..self.clone()
        }
    }

    /// This asker for the task the `task` call `call` handed over: its cards
    /// say so, for a panel following the task to take them.
    #[must_use]
    pub fn for_task(&self, call: &str) -> Self {
        Self {
            task: Some(call.to_string()),
            ..self.clone()
        }
    }

    /// This asker for a delegate the panel's run does not wait for — one
    /// handed over in the background: a stop of that run leaves its
    /// questions up, and only `stop`, its own, takes them back.
    #[must_use]
    pub fn detached(&self, stop: CancelToken) -> Self {
        Self {
            cancel: CancelToken::new(),
            stop: Some(stop),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn question() -> Question {
        Question {
            header: "Scope".into(),
            question: "Which crates?".into(),
            options: vec![QuestionOption {
                label: "core".into(),
                description: String::new(),
            }],
            multi_select: false,
        }
    }

    #[test]
    fn the_reply_comes_back_to_the_asking_thread() {
        let (asker, rx) = question_channel(CancelToken::new());
        let worker = std::thread::spawn(move || asker.ask(vec![question()]));
        let envelope = rx.recv().unwrap();
        assert_eq!(envelope.questions, vec![question()]);
        let answer = QuestionAnswer {
            chosen: vec!["core".into()],
            custom: None,
        };
        envelope
            .reply
            .send(QuestionReply::Answered(vec![answer.clone()]))
            .unwrap();
        assert_eq!(
            worker.join().unwrap(),
            QuestionReply::Answered(vec![answer])
        );
    }

    #[test]
    fn a_delegates_question_says_who_asks() {
        let (asker, rx) = question_channel(CancelToken::new());
        let asker = asker.for_delegate("search");
        let worker = std::thread::spawn(move || asker.ask(vec![question()]));
        let envelope = rx.recv().unwrap();
        assert_eq!(envelope.questions[0].question, "& search · Which crates?");
        envelope.reply.send(QuestionReply::Declined).unwrap();
        assert_eq!(worker.join().unwrap(), QuestionReply::Declined);
    }

    #[test]
    fn a_dropped_question_or_a_stop_declines() {
        let (asker, rx) = question_channel(CancelToken::new());
        let worker = std::thread::spawn(move || asker.ask(vec![question()]));
        drop(rx.recv().unwrap());
        assert_eq!(worker.join().unwrap(), QuestionReply::Declined);

        let cancel = CancelToken::new();
        let (asker, _rx) = question_channel(cancel.clone());
        let worker = std::thread::spawn(move || asker.ask(vec![question()]));
        cancel.cancel();
        assert_eq!(worker.join().unwrap(), QuestionReply::Declined);
    }

    #[test]
    fn a_background_delegates_question_outlives_the_panels_stop() {
        let panel_run = CancelToken::new();
        let (asker, rx) = question_channel(panel_run.clone());
        let own = CancelToken::new();
        let delegate = asker.for_delegate("search").detached(own.clone());
        let worker = std::thread::spawn(move || delegate.ask(vec![question()]));
        let envelope = rx.recv().unwrap();
        // The panel's run stops: the question stays up.
        panel_run.cancel();
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert!(!envelope.withdrawn.is_cancelled());
        // Its own stop takes it back, and marks it so.
        own.cancel();
        assert_eq!(worker.join().unwrap(), QuestionReply::Declined);
        assert!(envelope.withdrawn.is_cancelled());
    }
}
