//! The panel as the other agents of the project see it: its number, what it
//! is doing, and the messages they send it.
//!
//! A message from another agent goes in as that agent's words — the
//! transcript heads it with the sender, the reviewer never takes it for the
//! user's — and starts a turn when the panel is idle or joins the one under
//! way. The panel keeps count of how many messages agents passed one another
//! to get its run going, so a chain of them that runs on without the user can
//! be cut off; anything the user sends starts the count again.

use std::path::Path;

use termide_agent_core::{OriginKind, UserMessage};

use crate::AgentPanel;

/// What a panel is doing, as the `agents` tool lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerState {
    /// Nothing in its session yet.
    Free,
    Idle,
    Working,
    /// A card waits for the user.
    Waiting,
    /// It follows another agent's task, and takes no messages.
    Delegate,
    /// Its session is open in another termide; it takes no messages.
    Held,
}

impl PeerState {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Free => "free (nothing asked yet)",
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting for the user",
            Self::Delegate => "following another agent's task",
            Self::Held => "showing a session another termide holds",
        }
    }
}

impl AgentPanel {
    /// The panel's number, which other agents address it by.
    #[must_use]
    pub fn panel_id(&self) -> u64 {
        self.panel_id
    }

    /// The agent the panel runs.
    #[must_use]
    pub fn agent_name(&self) -> &str {
        &self.agent
    }

    /// Where the panel's agent works.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// How many messages agents passed one another, without the user, to
    /// get the panel's current run going.
    #[must_use]
    pub fn agent_hops(&self) -> u32 {
        self.agent_hops
    }

    /// What the panel is doing.
    #[must_use]
    pub fn peer_state(&self) -> PeerState {
        if self.is_delegate() {
            PeerState::Delegate
        } else if self.held.is_some() {
            PeerState::Held
        } else if self.pending.is_some() {
            PeerState::Waiting
        } else if self.is_busy() {
            PeerState::Working
        } else if self.banner_shown() {
            PeerState::Free
        } else {
            PeerState::Idle
        }
    }

    /// Take `message`, another agent's: as the next request when idle, at
    /// the run's next step when busy. A panel following another agent's task
    /// takes none — its session is that agent's — and a free one takes no
    /// question: it has nothing to answer from.
    pub fn deliver(&mut self, message: UserMessage) -> Result<(), String> {
        if self.is_delegate() {
            return Err(format!(
                "panel {} follows another agent's task and takes no messages until it ends",
                self.panel_id
            ));
        }
        if self.held.is_some() {
            return Err(format!(
                "panel {} shows a session another termide holds and takes no messages until it is taken up here",
                self.panel_id
            ));
        }
        // A panel with nothing in its session knows nothing of the work so
        // far: a question would only start a session there to say so.
        let question = message
            .origin
            .as_ref()
            .is_some_and(|origin| origin.kind == OriginKind::Question);
        if question && self.peer_state() == PeerState::Free {
            return Err(format!(
                "panel {} has nothing in its session yet: its agent knows nothing of the work so far, \
                 so it cannot answer; ask the panels that have worked, or the user",
                self.panel_id
            ));
        }
        self.send_message(message);
        Ok(())
    }

    /// Count `message` into the messages passed without the user: one of
    /// the user's starts the count again; another agent's carries its own,
    /// the highest counting while a run takes several.
    pub(crate) fn count_hops(&mut self, message: &UserMessage) {
        self.agent_hops = match &message.origin {
            None => 0,
            Some(origin) if self.is_busy() => self.agent_hops.max(origin.hops),
            Some(origin) => origin.hops,
        };
    }
}
