//! Commands the model offers the user to run by hand.
//!
//! The `suggest_command` tool puts one command to the user and waits: the
//! panel shows it in full on a card with `[Run]` on it, and nothing runs
//! until the user picks that row. This is the path a blocked call takes —
//! the reviewer's refusal tells the model to say what it needs run, and this
//! is how that becomes a thing the user does rather than a line of text they
//! must retype.
//!
//! The provenance is the point: a command reaches the shell only through a
//! card the user answered, so it is recorded as theirs. The channel works
//! like [`crate::ask`]'s — the call blocks on the agent thread and the wait
//! wakes regularly to notice an abort. A run with no one to ask — a subagent,
//! headless mode — has no [`CommandSuggester`] in its
//! [`ToolContext`](crate::ToolContext), and the tool says so to the model.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::cancel::CancelToken;

/// What the model offered, and what the user made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    /// The command line, as the model would run it.
    pub command: String,
    /// Why it is offered; may be empty. Shown to the user, not run.
    pub why: String,
}

/// How an offered command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionReply {
    /// The user confirmed it; run it.
    Run,
    /// The user took the text to the input to edit before running.
    Edit,
    /// The user copied it and did not run it.
    Copied,
    /// The user dismissed the card, or the run stopped before they answered.
    Declined,
}

/// One outstanding command and the channel for the user's choice.
pub struct SuggestionEnvelope {
    pub suggestion: Suggestion,
    /// Why the card withholds `[Run]` — plan mode, or a `deny` rule of the
    /// user's own, which the card cannot walk through; `None` when `[Run]` is
    /// on offer. The panel fills it in before showing the card.
    pub denied: Option<String>,
    pub reply: Sender<SuggestionReply>,
}

/// Puts commands to the panel's user. Cloneable and shared by the calls of
/// one run; each ask blocks until its answer comes back.
#[derive(Clone)]
pub struct CommandSuggester {
    tx: Sender<SuggestionEnvelope>,
    cancel: CancelToken,
}

impl std::fmt::Debug for CommandSuggester {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSuggester").finish_non_exhaustive()
    }
}

/// Build a suggester and the receiver the panel polls from `tick()`.
#[must_use]
pub fn suggestion_channel(cancel: CancelToken) -> (CommandSuggester, Receiver<SuggestionEnvelope>) {
    let (tx, rx) = mpsc::channel();
    (CommandSuggester { tx, cancel }, rx)
}

impl CommandSuggester {
    /// Show `suggestion` and wait for the user's choice. A stopped run, or a
    /// panel that went away, counts as declined.
    #[must_use]
    pub fn suggest(&self, suggestion: Suggestion) -> SuggestionReply {
        let (reply, answer) = mpsc::channel();
        let envelope = SuggestionEnvelope {
            suggestion,
            denied: None,
            reply,
        };
        if self.tx.send(envelope).is_err() {
            return SuggestionReply::Declined;
        }
        loop {
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(reply) => return reply,
                Err(RecvTimeoutError::Timeout) if self.cancel.is_cancelled() => {
                    return SuggestionReply::Declined;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return SuggestionReply::Declined,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggestion() -> Suggestion {
        Suggestion {
            command: "gh issue comment 59 --repo termide/termide".into(),
            why: "post the reply".into(),
        }
    }

    #[test]
    fn the_choice_comes_back_to_the_asking_thread() {
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        let worker = std::thread::spawn(move || suggester.suggest(suggestion()));
        let envelope = rx.recv().unwrap();
        assert_eq!(envelope.suggestion, suggestion());
        assert_eq!(envelope.denied, None);
        envelope.reply.send(SuggestionReply::Run).unwrap();
        assert_eq!(worker.join().unwrap(), SuggestionReply::Run);
    }

    #[test]
    fn a_dropped_card_or_a_stop_declines() {
        let (suggester, rx) = suggestion_channel(CancelToken::new());
        let worker = std::thread::spawn(move || suggester.suggest(suggestion()));
        drop(rx.recv().unwrap());
        assert_eq!(worker.join().unwrap(), SuggestionReply::Declined);

        let cancel = CancelToken::new();
        let (suggester, _rx) = suggestion_channel(cancel.clone());
        let worker = std::thread::spawn(move || suggester.suggest(suggestion()));
        cancel.cancel();
        assert_eq!(worker.join().unwrap(), SuggestionReply::Declined);
    }
}
