//! The autonomous-goal judge's texts: the system prompt of the judge call
//! that decides whether a `/goal` has been reached, and the user turn that
//! asks for the verdict. Both come from `system/goal.md` in the agent
//! directory, the seed being a data file in `assets/`, the way the compaction
//! and plan prompts are: no prompt text lives in code.

use crate::layers::split_front_matter;

/// The seed of `system/goal.md`.
pub const SEED_GOAL: &str = include_str!("../assets/system/goal.md");

/// What the goal judge is told, and what the verdict call asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalPrompt {
    /// The judge's system prompt; `{{goal}}` takes the goal text.
    pub instructions: String,
    /// The user turn appended to the transcript to ask for the verdict.
    pub request: String,
}

impl Default for GoalPrompt {
    fn default() -> Self {
        Self::from_file(SEED_GOAL)
    }
}

impl GoalPrompt {
    /// Parse `goal.md`: front matter `request:` plus the judge instructions.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        let (fields, body) = split_front_matter(text);
        Self {
            instructions: body.trim().to_string(),
            request: fields.get("request").cloned().unwrap_or_default(),
        }
    }

    /// The judge's system prompt with `goal` in place of `{{goal}}`, blank
    /// runs collapsed.
    #[must_use]
    pub fn system_prompt(&self, goal: &str) -> String {
        let mut text = self.instructions.replace("{{goal}}", goal.trim());
        text = text.trim_end().to_string();
        while text.contains("\n\n\n") {
            text = text.replace("\n\n\n", "\n\n");
        }
        text
    }
}

/// What the judge decided about a goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalOutcome {
    /// Not reached yet: keep working.
    Continue,
    /// Reached: the goal ends.
    Done,
    /// Cannot be reached from here: the goal ends, with the reason.
    Impossible,
}

/// The judge's decision and a one-line reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalVerdict {
    pub outcome: GoalOutcome,
    pub reason: String,
}

/// Read a verdict from the judge's reply. The first word decides: an explicit
/// `DONE` (or `ACHIEVED`/`COMPLETE`/`YES`) means done, `IMPOSSIBLE` (or
/// `UNREACHABLE`) means the goal cannot be reached; anything else — an empty
/// or confused reply included — means keep working: a goal that only spins
/// is caught by the panel's idle-turn check. The reason is the rest of the
/// reply, trimmed to a single line.
#[must_use]
pub fn parse_verdict(reply: &str) -> GoalVerdict {
    let mut lines = reply.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or_default();
    let token: String = first
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase();
    let outcome = match token.as_str() {
        "DONE" | "ACHIEVED" | "COMPLETE" | "YES" => GoalOutcome::Done,
        "IMPOSSIBLE" | "UNREACHABLE" => GoalOutcome::Impossible,
        _ => GoalOutcome::Continue,
    };
    // The reason is the first non-empty line after the verdict word, or, when
    // the verdict word shared its line with the reason, the rest of that line.
    let tail = first[token.len()..]
        .trim_start_matches([':', '.', '-', ' '])
        .trim();
    let reason = if tail.is_empty() {
        lines.next().unwrap_or_default().to_string()
    } else {
        tail.to_string()
    };
    GoalVerdict { outcome, reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_parses_and_takes_the_goal() {
        let goal = GoalPrompt::default();
        assert!(goal.request.starts_with("Has the goal been achieved?"));
        assert!(goal.instructions.starts_with("You are the judge"));
        assert!(!goal.instructions.contains("request:"));
        let prompt = goal.system_prompt("  ship the release  ");
        assert!(prompt.contains("ship the release"));
        assert!(!prompt.contains("{{goal}}"));
        assert!(!prompt.contains("\n\n\n"));
    }

    #[test]
    fn a_custom_file_overrides_both_texts() {
        let custom = GoalPrompt::from_file("---\nrequest: Done yet?\n---\nJudge {{goal}} now.\n");
        assert_eq!(custom.request, "Done yet?");
        assert_eq!(custom.system_prompt("X"), "Judge X now.");
    }

    #[test]
    fn a_verdict_reads_the_first_word_and_the_reason() {
        let done = |reply: &str| parse_verdict(reply).outcome == GoalOutcome::Done;
        let v = parse_verdict("DONE\nThe tests pass.");
        assert_eq!(v.outcome, GoalOutcome::Done);
        assert_eq!(v.reason, "The tests pass.");

        let v = parse_verdict("CONTINUE\nThe build still fails.");
        assert_eq!(v.outcome, GoalOutcome::Continue);
        assert_eq!(v.reason, "The build still fails.");

        // The word may share its line with the reason.
        let v = parse_verdict("DONE: everything compiles");
        assert_eq!(v.outcome, GoalOutcome::Done);
        assert_eq!(v.reason, "everything compiles");

        // Lowercase and trailing punctuation are tolerated.
        assert!(done("done."));
        assert!(done("Achieved — all green"));

        // A goal that cannot be reached says so, with the reason.
        let v = parse_verdict("IMPOSSIBLE\nThe API it needs was removed.");
        assert_eq!(v.outcome, GoalOutcome::Impossible);
        assert_eq!(v.reason, "The API it needs was removed.");

        // An empty or confused reply keeps working.
        assert_eq!(parse_verdict("").outcome, GoalOutcome::Continue);
        assert_eq!(
            parse_verdict("I think maybe").outcome,
            GoalOutcome::Continue
        );
    }
}
