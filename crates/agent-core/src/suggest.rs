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
//!
//! The `suggest_goal` and `suggest_loop` tools offer unattended work the
//! same way: a `/goal` or a `/loop` starts only from a card the user
//! answered, as if they had typed it.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::cancel::CancelToken;

/// What an offer would start.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SuggestionKind {
    /// A shell command (`suggest_command`).
    #[default]
    Command,
    /// A `/goal`, its text the goal (`suggest_goal`).
    Goal,
    /// A `/loop`, its text the prompt re-run every `interval`, or back to
    /// back without one (`suggest_loop`).
    Loop { interval: Option<Duration> },
}

/// What the model offered, and what the user made of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    /// The command line as the model would run it; for a goal, the goal; for
    /// a loop, its prompt.
    pub command: String,
    /// Why it is offered; may be empty. Shown to the user, not run.
    pub why: String,
}

impl Suggestion {
    /// What the user would type in the panel's prompt to start it: the
    /// command itself, or the `/goal …` or `/loop …` line.
    #[must_use]
    pub fn typed(&self) -> String {
        match self.kind {
            SuggestionKind::Command => self.command.clone(),
            SuggestionKind::Goal => format!("/goal {}", self.command),
            SuggestionKind::Loop { interval: None } => format!("/loop {}", self.command),
            SuggestionKind::Loop {
                interval: Some(every),
            } => format!("/loop {} {}", format_interval(every), self.command),
        }
    }
}

/// Parse an interval as `/loop` takes it: `30s`, `5m`, `2h`, or a bare count
/// of seconds; `None` for anything else or zero.
#[must_use]
pub fn parse_interval(token: &str) -> Option<Duration> {
    let (digits, unit) = match token.chars().last() {
        Some('s') => (&token[..token.len() - 1], 1),
        Some('m') => (&token[..token.len() - 1], 60),
        Some('h') => (&token[..token.len() - 1], 3600),
        _ => (token, 1),
    };
    let n: u64 = digits.parse().ok()?;
    (n > 0).then(|| Duration::from_secs(n * unit))
}

/// An interval in the largest unit [`parse_interval`] reads back exactly.
#[must_use]
pub fn format_interval(every: Duration) -> String {
    let secs = every.as_secs().max(1);
    if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else if secs.is_multiple_of(60) {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

/// How an offered command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionReply {
    /// The user confirmed it; run it. For a goal or a loop, the panel has
    /// started it.
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
            kind: SuggestionKind::Command,
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

    /// A goal or a loop is offered as the line that would start it, and an
    /// interval reads back as it was given.
    #[test]
    fn a_goal_or_a_loop_is_typed_as_its_command() {
        let offer = |kind| Suggestion {
            kind,
            command: "get the build green".into(),
            why: String::new(),
        };
        assert_eq!(
            offer(SuggestionKind::Goal).typed(),
            "/goal get the build green"
        );
        assert_eq!(
            offer(SuggestionKind::Loop { interval: None }).typed(),
            "/loop get the build green"
        );
        for token in ["30s", "5m", "2h", "90s"] {
            let interval = parse_interval(token);
            assert_eq!(
                offer(SuggestionKind::Loop { interval }).typed(),
                format!("/loop {token} get the build green")
            );
        }
        assert_eq!(parse_interval("90"), Some(Duration::from_secs(90)));
        assert_eq!(parse_interval("0m"), None);
        assert_eq!(parse_interval("soon"), None);
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
