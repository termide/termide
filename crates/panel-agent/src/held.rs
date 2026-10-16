//! A session another termide holds. Two instances may restore one project —
//! one left running in a terminal window that is out of sight — and the
//! panel each restores would back the same session: both writing its log,
//! both carrying its run on, an external agent running twice. The panel of
//! the instance that came second shows the session read-only: no agent is
//! started, nothing is sent or written, no unfinished work is offered. It
//! looks again every few seconds, and takes the session up once the other
//! termide lets it go, offering its unfinished work then as any reopen does.

use std::time::{Duration, Instant};

use termide_agent_core::Session;

use crate::transcript::NoticeKind;
use crate::AgentPanel;

/// How often a held session is looked at again.
pub(crate) const HELD_RECHECK: Duration = Duration::from_secs(2);

/// The session another termide holds, shown here read-only.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeldSession {
    /// The process id the holder's lock file names.
    pub(crate) pid: Option<u32>,
    /// When to look whether it was let go.
    pub(crate) next_check: Instant,
}

impl HeldSession {
    /// The held session `session` is, when it was opened read-only.
    pub(crate) fn of(session: Option<&Session>) -> Option<Self> {
        let session = session.filter(|s| s.is_read_only())?;
        Some(Self {
            pid: Session::held_elsewhere(session.path()).flatten(),
            next_check: Instant::now() + HELD_RECHECK,
        })
    }

    /// The holder as the notices name it.
    fn holder(self) -> String {
        self.pid
            .map_or_else(|| "?".to_string(), |pid| pid.to_string())
    }
}

impl AgentPanel {
    /// Whether the session is another termide's, shown here read-only.
    #[must_use]
    pub fn is_held_elsewhere(&self) -> bool {
        self.held.is_some()
    }

    /// Say that the session is another termide's and read-only here.
    pub(crate) fn notice_held(&mut self) {
        if let Some(held) = self.held {
            self.notice(
                termide_i18n::t().agent_notice_session_held_fmt(&held.holder()),
                NoticeKind::Warn,
            );
        }
    }

    /// Refuse what would run the agent on a held session, with the notice
    /// that says why; `false` when the session is this panel's.
    pub(crate) fn refuse_held(&mut self) -> bool {
        if self.held.is_none() {
            return false;
        }
        self.notice_held();
        true
    }

    /// Look, when due, whether the other termide let the session go, and take
    /// it up if it did: the agent starts on the log as it now stands, and its
    /// unfinished work is offered. Whether anything changed.
    pub(crate) fn check_held(&mut self) -> bool {
        let Some(held) = self.held.as_mut() else {
            return false;
        };
        let now = Instant::now();
        if now < held.next_check {
            return false;
        }
        held.next_check = now + HELD_RECHECK;
        let Some(path) = self.session.as_ref().map(|s| s.path().to_path_buf()) else {
            self.held = None;
            return false;
        };
        if Session::held_elsewhere(&path).is_some() {
            return false;
        }
        let session = match Session::open_exclusive(&path) {
            Ok(session) => session,
            Err(error) => {
                log::debug!("cannot take up {} yet: {error}", path.display());
                return false;
            }
        };
        self.held = None;
        // The log as the other termide left it: the transcript, the agent and
        // its session are built anew from it.
        if self.switch_session(Some(session)) {
            self.notice(
                termide_i18n::t().agent_notice_session_released(),
                NoticeKind::Info,
            );
            self.restore_unfinished();
        }
        true
    }
}
