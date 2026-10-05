//! The `recall` solver's texts: the system prompt of the one call that
//! answers a question from the tool's search results, and the user turn that
//! asks it. Both come from `system/recall.md` in the agent directory, the
//! seed being a data file in `assets/`, the way the compaction, handoff and
//! goal prompts are: no prompt text lives in code.

use crate::layers::split_front_matter;

/// The seed of `system/recall.md`.
pub const SEED_RECALL: &str = include_str!("../assets/system/recall.md");

/// What the solver is told, and the question it is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecallPrompt {
    /// The solver's system prompt.
    pub instructions: String,
    /// The user turn's first line; `{{question}}` takes the question.
    pub request: String,
}

impl Default for RecallPrompt {
    fn default() -> Self {
        Self::from_file(SEED_RECALL)
    }
}

impl RecallPrompt {
    /// Parse `recall.md`: front matter `request:` plus the instructions.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        let (fields, body) = split_front_matter(text);
        Self {
            instructions: body.trim().to_string(),
            request: fields.get("request").cloned().unwrap_or_default(),
        }
    }

    /// The user turn: the request with `question` in place, then the results.
    #[must_use]
    pub fn user_turn(&self, question: &str, results: &str) -> String {
        format!(
            "{}\n\n{results}",
            self.request.replace("{{question}}", question)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_parses_and_fills_the_question() {
        let prompt = RecallPrompt::default();
        assert!(prompt.instructions.starts_with("You answer a question"));
        assert!(!prompt.instructions.contains("request:"));
        let turn = prompt.user_turn("why no tokio?", "1. session:a#b …");
        assert!(turn.contains("Question: why no tokio?"));
        assert!(turn.ends_with("1. session:a#b …"));
        assert!(!turn.contains("{{question}}"));
    }
}
