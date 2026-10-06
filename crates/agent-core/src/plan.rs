//! Plan mode's texts: the instructions appended to the system prompt while
//! the mode is on, and the request that carries the plan out once the user
//! accepts it. Both come from `system/plan.md` in the agent directory, the
//! seed being a data file in `assets/`, the way the compaction prompts are.

use crate::layers::split_front_matter;

/// The seed of `system/plan.md`.
pub const SEED_PLAN: &str = include_str!("../assets/system/plan.md");

/// What plan mode tells the model, and what accepting the plan sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanPrompt {
    /// Appended to the system prompt while plan mode is on.
    pub instructions: String,
    /// The user turn sent when the plan is accepted.
    pub request: String,
    /// The user turn sent when the plan is carried out from a clean
    /// context; empty means [`Self::request`] serves.
    pub clean_request: String,
    /// Told to an agent whose system prompt cannot change mid-session (Claude
    /// Code) when plan mode goes off after its prompt held the instructions.
    pub leave: String,
}

impl Default for PlanPrompt {
    fn default() -> Self {
        Self::from_file(SEED_PLAN)
    }
}

impl PlanPrompt {
    /// Parse `plan.md`: front matter `request:`, `clean_request:` and
    /// `leave:` plus the instructions.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        let (fields, body) = split_front_matter(text);
        let field = |key: &str| fields.get(key).cloned().unwrap_or_default();
        Self {
            instructions: body.trim().to_string(),
            request: field("request"),
            clean_request: field("clean_request"),
            leave: field("leave"),
        }
    }

    /// What to tell an agent whose system prompt was fixed when its session
    /// started, before a turn in which plan mode is `on` while its prompt was
    /// built with the mode `prompt_has_plan`: the instructions when the mode
    /// came on, `leave` when it went off; `None` when nothing changed or the
    /// text for the change is empty.
    #[must_use]
    pub fn switch_note(&self, prompt_has_plan: bool, on: bool) -> Option<&str> {
        let note = match (prompt_has_plan, on) {
            (false, true) => &self.instructions,
            (true, false) => &self.leave,
            _ => return None,
        };
        let note = note.trim();
        (!note.is_empty()).then_some(note)
    }

    /// The request that carries the plan out, from a clean context or not.
    #[must_use]
    pub fn request(&self, clean: bool) -> &str {
        if clean && !self.clean_request.trim().is_empty() {
            &self.clean_request
        } else {
            &self.request
        }
    }

    /// `base` with the plan-mode instructions after it; `base` alone when
    /// the instructions are empty.
    #[must_use]
    pub fn apply(&self, base: &str) -> String {
        if self.instructions.is_empty() {
            return base.to_string();
        }
        if base.trim().is_empty() {
            return self.instructions.clone();
        }
        format!("{}\n\n{}", base.trim_end(), self.instructions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_parses_and_is_appended_to_the_prompt() {
        let plan = PlanPrompt::default();
        assert!(plan.request.starts_with("Carry out the plan"));
        assert!(plan.instructions.starts_with("# Plan mode"));
        assert!(!plan.instructions.contains("request:"));
        let full = plan.apply("You are an agent.\n");
        assert!(full.starts_with("You are an agent.\n\n# Plan mode"));
        assert_eq!(plan.apply(""), plan.instructions);

        assert!(plan.request(true).contains("cleared from your context"));
        assert_eq!(plan.request(false), plan.request);

        let bare = PlanPrompt::from_file("Only instructions.");
        assert_eq!(bare.request, "");
        let old = PlanPrompt::from_file("---\nrequest: go\n---\nPlan.");
        assert_eq!(old.request(true), "go");
        assert_eq!(bare.apply("base"), "base\n\nOnly instructions.");
        let empty = PlanPrompt::from_file("---\nrequest: go\n---\n");
        assert_eq!(empty.apply("base"), "base");
    }

    #[test]
    fn a_switch_note_tells_only_a_change_of_mode() {
        let plan = PlanPrompt::default();
        assert!(plan.leave.contains("Plan mode is off"));
        assert_eq!(
            plan.switch_note(false, true),
            Some(plan.instructions.as_str())
        );
        assert_eq!(plan.switch_note(true, false), Some(plan.leave.as_str()));
        assert_eq!(plan.switch_note(true, true), None);
        assert_eq!(plan.switch_note(false, false), None);
        // A file without `leave:` says nothing when the mode goes off.
        let bare = PlanPrompt::from_file("Plan.");
        assert_eq!(bare.switch_note(true, false), None);
    }
}
