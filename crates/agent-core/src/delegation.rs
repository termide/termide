//! Where a delegated task runs: out of sight, or in an agent panel of its
//! own, where the user watches it, answers its permission requests and can
//! stop it.

use serde::{Deserialize, Serialize};

/// Where a task handed to another agent runs: `subagent_run` in `[ai]`, or
/// in the delegate's `AGENT.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentPlace {
    /// Out of sight: the delegating call's block shows its progress, and
    /// what would need the user is refused.
    #[default]
    Hidden,
    /// In an agent panel already open and still empty, else out of sight:
    /// a task never changes the layout.
    ExistingPanel,
    /// In an agent panel: an empty one already open, else a new one.
    Panel,
}

impl SubagentPlace {
    pub const ALL: [SubagentPlace; 3] = [Self::Hidden, Self::ExistingPanel, Self::Panel];

    /// The value as the config file writes it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::ExistingPanel => "existing_panel",
            Self::Panel => "panel",
        }
    }

    /// Whether the task may run in a panel at all.
    #[must_use]
    pub fn in_panel(self) -> bool {
        self != Self::Hidden
    }

    /// Whether a panel may be opened for the task when no empty one is.
    #[must_use]
    pub fn opens_panel(self) -> bool {
        self == Self::Panel
    }
}
