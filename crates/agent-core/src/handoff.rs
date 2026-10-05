//! The `/handoff` brief's texts: the system prompt of the model call that
//! distils the unfinished work into a forward-looking brief for a fresh
//! session or another agent, and the user turn that requests it. Both come
//! from `system/handoff.md` in the agent directory, the seed being a data file
//! in `assets/`, the way the compaction, plan and goal prompts are.

use crate::layers::split_front_matter;

/// The first line of the request that seeds a fresh session with a handoff
/// brief, the brief following after a blank line; `recall` recognises a
/// brief in a session log by it.
pub const CONTINUATION_LEAD: &str = "Continue the work described in this handoff brief:";

/// The seed of `system/handoff.md`.
pub const SEED_HANDOFF: &str = include_str!("../assets/system/handoff.md");

/// What the handoff call is told, and the user turn that closes the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandoffPrompt {
    /// The system prompt of the handoff call.
    pub instructions: String,
    /// The user turn appended to the transcript to ask for the brief.
    pub request: String,
}

impl Default for HandoffPrompt {
    fn default() -> Self {
        Self::from_file(SEED_HANDOFF)
    }
}

impl HandoffPrompt {
    /// Parse `handoff.md`: front matter `request:` plus the instructions.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        let (fields, body) = split_front_matter(text);
        Self {
            instructions: body.trim().to_string(),
            request: fields.get("request").cloned().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_parses() {
        let h = HandoffPrompt::default();
        assert!(h.request.starts_with("Write the handoff brief"));
        assert!(h
            .instructions
            .starts_with("You are writing a handoff brief"));
        assert!(!h.instructions.contains("request:"));
    }

    #[test]
    fn a_custom_file_overrides_both() {
        let custom = HandoffPrompt::from_file("---\nrequest: Hand off.\n---\nBe terse.\n");
        assert_eq!(custom.request, "Hand off.");
        assert_eq!(custom.instructions, "Be terse.");
    }
}
