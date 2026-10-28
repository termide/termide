//! The autonomous-goal judge's texts: the system prompt of the judge call
//! that decides whether a `/goal` has been reached, and the user turn that
//! asks for the verdict. Both come from `system/goal.md` in the agent
//! directory, the seed being a data file in `assets/`, the way the compaction
//! and plan prompts are: no prompt text lives in code.

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalOutcome {
    /// Not reached yet: keep working.
    Continue,
    /// Reached: the goal ends.
    Done,
    /// Cannot be reached from here: the goal ends, with the reason.
    Impossible,
    /// The agent waits on the user or an outside event: the goal pauses,
    /// still set, instead of sending it back to repeat the wait.
    Waiting,
}

/// The judge's decision and a one-line reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalVerdict {
    pub outcome: GoalOutcome,
    pub reason: String,
}

/// Words that open an instruction about a verdict rather than a reason for
/// one: "`DONE` if the goal is fully achieved" is the prompt echoed back.
const INSTRUCTION_WORDS: &[&str] = &[
    "if",
    "when",
    "whenever",
    "unless",
    "means",
    "meaning",
    "otherwise",
    "or",
];

/// The outcome a verdict word names, in any case. `YES` is not one: "Yes,
/// but the tests fail" would read as done.
fn outcome_of(word: &str) -> Option<GoalOutcome> {
    match word.to_ascii_uppercase().as_str() {
        "DONE" | "ACHIEVED" | "COMPLETE" => Some(GoalOutcome::Done),
        "IMPOSSIBLE" | "UNREACHABLE" => Some(GoalOutcome::Impossible),
        "WAITING" | "BLOCKED" => Some(GoalOutcome::Waiting),
        "CONTINUE" => Some(GoalOutcome::Continue),
        _ => None,
    }
}

/// Read a verdict from the judge's reply, strictly: the first non-empty line
/// (past a closed `<think>` block) is the verdict word alone — `DONE`,
/// `CONTINUE`, `WAITING` or `IMPOSSIBLE`, or `ACHIEVED`/`COMPLETE`,
/// `UNREACHABLE`, `BLOCKED` — in any case, optionally after a `Verdict:`
/// label, wrapped in markup (`**DONE**`, `` `DONE` ``) and followed by
/// trailing punctuation, or by a separator (`:`, `—`, `-`, `.`) and the
/// reason. The reason is that rest of the line, or else the next non-empty
/// line.
///
/// Anything else is no verdict, and an `Err` saying what came instead: a
/// weak judge echoing its instructions ("`DONE` if the goal is fully
/// achieved", "The first line: `CONTINUE`") must not end or steer a goal —
/// the caller treats it as a failed check.
pub fn parse_verdict(reply: &str) -> Result<GoalVerdict, String> {
    let body = reply
        .rfind("</think>")
        .map_or(reply, |at| &reply[at + "</think>".len()..]);
    let mut lines = body.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return Err("the judge gave an empty answer".to_string());
    };
    let no_verdict = || {
        let mut shown: String = first.chars().take(80).collect();
        if shown.len() < first.len() {
            shown.push('…');
        }
        format!("the judge gave no verdict: \"{shown}\"")
    };
    let (outcome, tail) = verdict_line(first).ok_or_else(no_verdict)?;
    let reason = if tail.is_empty() {
        lines.next().unwrap_or_default().to_string()
    } else {
        tail.to_string()
    };
    Ok(GoalVerdict { outcome, reason })
}

/// The outcome and the reason after it on a verdict line, or `None` when the
/// line is not one.
fn verdict_line(line: &str) -> Option<(GoalOutcome, &str)> {
    const MARKUP: [char; 5] = ['*', '`', '_', '"', '\''];
    let mut rest = line.trim_start_matches(|c: char| {
        MARKUP.contains(&c) || matches!(c, '#' | '>') || c.is_whitespace()
    });
    // A `Verdict:` label before the word.
    if let Some(label) = rest.get(..7).filter(|l| l.eq_ignore_ascii_case("verdict")) {
        let after = rest[label.len()..].trim_start_matches(MARKUP).trim_start();
        if let Some(after) = after.strip_prefix(':') {
            rest = after.trim_start_matches(|c: char| MARKUP.contains(&c) || c.is_whitespace());
        }
    }
    let end = rest
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(rest.len());
    let outcome = outcome_of(&rest[..end])?;
    let after = rest[end..].trim_start_matches(MARKUP).trim_start();
    // The word stands alone, or a separator leads to the reason: "DONE if
    // the goal…" is an instruction, not a verdict.
    let reason = match after.chars().next() {
        None => return Some((outcome, "")),
        Some(':' | '—' | '–' | '-' | '.' | '!') => after
            .trim_start_matches([':', '—', '–', '-', '.', '!'])
            .trim_start_matches(MARKUP)
            .trim(),
        Some(_) => return None,
    };
    let first_word = reason
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if INSTRUCTION_WORDS.contains(&first_word.as_str()) {
        return None;
    }
    // A line that names other verdicts lists the choices, it makes none.
    let names_another = reason
        .split(|c: char| !c.is_ascii_alphabetic())
        .any(|word| {
            word.len() > 1
                && word.chars().all(|c| c.is_ascii_uppercase())
                && outcome_of(word).is_some()
        });
    if names_another {
        return None;
    }
    Some((outcome, reason))
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
        let parse = |reply: &str| parse_verdict(reply).expect(reply);
        let done = |reply: &str| parse(reply).outcome == GoalOutcome::Done;
        let v = parse("DONE\nThe tests pass.");
        assert_eq!(v.outcome, GoalOutcome::Done);
        assert_eq!(v.reason, "The tests pass.");

        let v = parse("CONTINUE\nThe build still fails.");
        assert_eq!(v.outcome, GoalOutcome::Continue);
        assert_eq!(v.reason, "The build still fails.");

        // The word may share its line with the reason.
        let v = parse("DONE: everything compiles");
        assert_eq!(v.outcome, GoalOutcome::Done);
        assert_eq!(v.reason, "everything compiles");

        // Lowercase and trailing punctuation are tolerated.
        assert!(done("done."));
        assert!(done("Achieved — all green"));
        assert!(done("DONE - all green"));

        // A goal that cannot be reached says so, with the reason.
        let v = parse("IMPOSSIBLE\nThe API it needs was removed.");
        assert_eq!(v.outcome, GoalOutcome::Impossible);
        assert_eq!(v.reason, "The API it needs was removed.");

        // An agent stopped on the user is waiting, not to be sent back.
        let v = parse("WAITING\nThe agent asks which session to keep.");
        assert_eq!(v.outcome, GoalOutcome::Waiting);
        assert_eq!(v.reason, "The agent asks which session to keep.");
        assert_eq!(parse("Blocked: on approval").outcome, GoalOutcome::Waiting);

        // Markup round the word, a label before it or a closed reasoning
        // block above it does not hide it.
        let v = parse("**DONE**\nThe tests pass.");
        assert_eq!(v.outcome, GoalOutcome::Done);
        assert_eq!(v.reason, "The tests pass.");
        let v = parse("Verdict: CONTINUE\nThe IR slice 1e is not landed.");
        assert_eq!(v.outcome, GoalOutcome::Continue);
        assert_eq!(v.reason, "The IR slice 1e is not landed.");
        let v = parse("**Verdict:** `WAITING`\nOn the user's choice.");
        assert_eq!(v.outcome, GoalOutcome::Waiting);
        let v = parse("<think>the tests ran</think>\n\n`IMPOSSIBLE`: the API is gone");
        assert_eq!(v.outcome, GoalOutcome::Impossible);
        assert_eq!(v.reason, "the API is gone");
    }

    #[test]
    fn no_line_of_the_seed_ends_or_pauses_a_goal_when_echoed() {
        // A judge that repeats its prompt must not end a goal by it: at most
        // the example's `CONTINUE` reads, which only sends the agent on.
        let goal = GoalPrompt::default();
        let prompt = goal.system_prompt("ship it");
        for line in prompt.lines().chain([goal.request.as_str()]) {
            for reply in [line.to_string(), format!("{line}\nmore text")] {
                if let Ok(verdict) = parse_verdict(&reply) {
                    assert_eq!(verdict.outcome, GoalOutcome::Continue, "{line:?}");
                }
            }
        }
    }

    #[test]
    fn an_echoed_instruction_is_no_verdict() {
        let fails = |reply: &str| {
            assert!(
                parse_verdict(reply).is_err(),
                "{reply:?} read as {:?}",
                parse_verdict(reply)
            );
        };
        // The prompt's own lines, echoed by a weak judge.
        fails("`DONE` if the goal is fully achieved\nThe tests pass.");
        fails("- The first line is a single word: `DONE` if the goal is fully achieved");
        fails("The first line: `CONTINUE`");
        fails("DONE: if the goal is fully achieved");
        fails("DONE means the goal is reached");
        fails("CONTINUE — otherwise, keep working");
        // A line listing the choices makes none.
        fails("DONE / CONTINUE / WAITING / IMPOSSIBLE");
        fails("`DONE`, `IMPOSSIBLE`, `WAITING` or `CONTINUE`");
        fails("DONE: or CONTINUE, depending on the tests");
        // The verdict must open the answer: one further down is not taken.
        fails("Verdict:\nCONTINUE\nThe IR slice 1e is not landed.");
        fails("Let me look at the work.\nDONE\nThe tests pass.");
        // "Yes, but" is no verdict; nor is an empty or confused reply.
        fails("Yes, but the tests still fail");
        fails("");
        fails("I think maybe");
        fails("Doneness depends on the tests");
        // What came instead is shown in the error.
        let error = parse_verdict("The first line: `CONTINUE`").unwrap_err();
        assert!(error.contains("The first line: `CONTINUE`"), "{error}");
    }
}
