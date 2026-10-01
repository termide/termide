//! What the model reads when a tool call is refused: the texts of
//! `system/permissions.md`, one front-matter key per case, the seed being a
//! data file in `assets/` like the other service prompts. No refusal text
//! lives in code.

use crate::layers::split_front_matter;

/// The seed of `system/permissions.md`.
pub const SEED_PERMISSIONS: &str = include_str!("../assets/system/permissions.md");

/// The refusal texts. `{{reason}}` in `reviewer_blocked` and
/// `user_denied_reason` takes the reviewer's reason or the user's words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusals {
    pub rule_denied: String,
    pub plan_mode: String,
    pub reviewer_blocked: String,
    pub user_denied: String,
    pub user_denied_session: String,
    pub user_denied_reason: String,
    pub unattended_subagent: String,
    pub unattended_headless: String,
}

impl Default for Refusals {
    fn default() -> Self {
        Self::parse(SEED_PERMISSIONS, None)
    }
}

impl Refusals {
    /// Parse `permissions.md`; a key the file leaves out or empty keeps the
    /// shipped text.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        Self::parse(text, Some(&Self::default()))
    }

    fn parse(text: &str, fallback: Option<&Refusals>) -> Self {
        let (fields, _) = split_front_matter(text);
        let pick = |key: &str, shipped: Option<&String>| {
            fields
                .get(key)
                .filter(|value| !value.is_empty())
                .cloned()
                .or_else(|| shipped.cloned())
                .unwrap_or_default()
        };
        let f = fallback;
        Self {
            rule_denied: pick("rule_denied", f.map(|f| &f.rule_denied)),
            plan_mode: pick("plan_mode", f.map(|f| &f.plan_mode)),
            reviewer_blocked: pick("reviewer_blocked", f.map(|f| &f.reviewer_blocked)),
            user_denied: pick("user_denied", f.map(|f| &f.user_denied)),
            user_denied_session: pick("user_denied_session", f.map(|f| &f.user_denied_session)),
            user_denied_reason: pick("user_denied_reason", f.map(|f| &f.user_denied_reason)),
            unattended_subagent: pick("unattended_subagent", f.map(|f| &f.unattended_subagent)),
            unattended_headless: pick("unattended_headless", f.map(|f| &f.unattended_headless)),
        }
    }

    /// `template` with `reason` in place of `{{reason}}`.
    #[must_use]
    pub fn with_reason(template: &str, reason: &str) -> String {
        template.replace("{{reason}}", reason.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_seed_has_every_text_and_a_file_overrides_some() {
        let shipped = Refusals::default();
        for text in [
            &shipped.rule_denied,
            &shipped.plan_mode,
            &shipped.reviewer_blocked,
            &shipped.user_denied,
            &shipped.user_denied_session,
            &shipped.user_denied_reason,
            &shipped.unattended_subagent,
            &shipped.unattended_headless,
        ] {
            assert!(!text.is_empty());
        }
        assert!(shipped.reviewer_blocked.contains("{{reason}}"));
        assert_eq!(
            Refusals::with_reason(&shipped.user_denied_reason, " use the fixture "),
            "denied by the user: use the fixture"
        );

        let custom =
            Refusals::from_file("---\nrule_denied: no: the rules say so\nplan_mode:\n---\n");
        assert_eq!(custom.rule_denied, "no: the rules say so");
        // Left empty or out, a key keeps the shipped text.
        assert_eq!(custom.plan_mode, shipped.plan_mode);
        assert_eq!(custom.user_denied, shipped.user_denied);
    }
}
