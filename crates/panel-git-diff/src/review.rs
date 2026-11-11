//! The review of an agent's changes (see `termide_git::Review`): the diff
//! is the working tree's difference from what the user kept so far. The
//! staging keys act on it instead: `Space` keeps what the cursor stands for,
//! the discard key rejects it — the file goes back there, and the agent
//! hears of it with its next request — and the undo key takes the last
//! rejection back. Staging or committing a change elsewhere keeps it too.

use std::path::PathBuf;

use termide_core::{ConfirmAction, PanelEvent};
use termide_git::{self as git, Rejected};

use crate::stage::{error_text, first_line, Target};
use crate::GitDiffPanel;

/// The answers `PanelCommand::Confirmed` brings back.
pub(crate) const REJECT_ACTION: &str = "git-diff:reject";
pub(crate) const REJECT_ALL_ACTION: &str = "git-diff:reject-all";

/// What a panel reviewing an agent's changes keeps of the review.
pub(crate) struct ReviewMode {
    /// The agent session whose changes are reviewed.
    pub(crate) session: String,
    /// Files both the user and the agent edited.
    pub(crate) both: Vec<String>,
    /// The last rejection, for the undo key.
    last_reject: Option<Rejected>,
    /// A rejection waiting for the user's confirmation.
    pending_reject: Option<Target>,
}

impl GitDiffPanel {
    /// A panel reviewing the changes the agent of `session` made in the
    /// repository at `repo_path`.
    pub fn new_for_review(repo_path: PathBuf, session: String) -> Self {
        let mut panel = Self::blank(repo_path, None, None, false, None);
        panel.review = Some(ReviewMode {
            session,
            both: Vec::new(),
            last_reject: None,
            pending_reject: None,
        });
        panel.refresh();
        panel
    }

    /// The agent session whose changes the panel reviews, if it does.
    pub fn review_session(&self) -> Option<&str> {
        self.review.as_ref().map(|review| review.session.as_str())
    }

    /// Whether both the user and the agent edited `path`.
    pub(crate) fn edited_by_both(&self, path: &str) -> bool {
        self.review
            .as_ref()
            .is_some_and(|review| review.both.iter().any(|p| p == path))
    }

    fn open_review(&self) -> Option<git::Review> {
        git::Review::open(&self.repo_path, &self.review.as_ref()?.session)
    }

    /// Keep what the cursor stands for.
    pub(crate) fn keep(&mut self) -> Vec<PanelEvent> {
        let (Some(target), Some(review)) = (self.target(), self.open_review()) else {
            return vec![];
        };
        let result = review.keep(&target.path, target.part.as_ref());
        self.status_message = result.as_ref().err().map(error_text);
        self.after_action()
    }

    /// Reject what the cursor stands for; in a file the user edited too,
    /// whose own edits may go with it, only once they confirm.
    pub(crate) fn ask_reject(&mut self) -> Vec<PanelEvent> {
        let Some(target) = self.target() else {
            return vec![];
        };
        let path = target.path.to_string_lossy().into_owned();
        if !self.edited_by_both(&path) {
            self.reject(&target);
            return self.after_action();
        }
        if let Some(review) = self.review.as_mut() {
            review.pending_reject = Some(target);
        }
        vec![PanelEvent::ShowConfirm {
            message: termide_i18n::t().git_diff_reject_both_confirm_fmt(&path),
            on_confirm: ConfirmAction::Custom(REJECT_ACTION.to_string()),
        }]
    }

    /// The rejection the user confirmed.
    pub(crate) fn confirmed_reject(&mut self) {
        let pending = self.review.as_mut().and_then(|r| r.pending_reject.take());
        if let Some(target) = pending {
            self.reject(&target);
            self.after_action();
        }
    }

    fn reject(&mut self, target: &Target) {
        let Some(review) = self.open_review() else {
            return;
        };
        let result = review.reject(&target.path, target.part.as_ref());
        self.status_message = result.as_ref().err().map(error_text);
        if let (Ok(rejected), Some(mode)) = (result, self.review.as_mut()) {
            mode.last_reject = Some(rejected);
        }
    }

    /// Take the last rejection back.
    pub(crate) fn undo_reject(&mut self) -> Vec<PanelEvent> {
        let last = self.review.as_mut().and_then(|r| r.last_reject.take());
        let (Some(rejected), Some(review)) = (last, self.open_review()) else {
            self.status_message = Some(termide_i18n::t().git_diff_nothing_to_undo().to_string());
            return vec![];
        };
        self.status_message = Some(match review.undo_reject(&rejected) {
            Ok(()) => termide_i18n::t().git_diff_restored_fmt(&rejected.path.to_string_lossy()),
            Err(error) => first_line(&error),
        });
        self.after_action()
    }

    /// Keep everything that waits.
    pub(crate) fn keep_all(&mut self) -> Vec<PanelEvent> {
        let Some(review) = self.open_review() else {
            return vec![];
        };
        self.status_message = review.keep_all().err().map(|e| first_line(&e));
        self.after_action()
    }

    /// Ask before rejecting everything that waits.
    pub(crate) fn ask_reject_all(&mut self) -> Vec<PanelEvent> {
        if self.diffs.is_empty() {
            return vec![];
        }
        vec![PanelEvent::ShowConfirm {
            message: termide_i18n::t().git_diff_reject_all_confirm().to_string(),
            on_confirm: ConfirmAction::Custom(REJECT_ALL_ACTION.to_string()),
        }]
    }

    /// The rejection of everything the user confirmed; files they edited
    /// too stay as they are.
    pub(crate) fn confirmed_reject_all(&mut self) {
        let Some(review) = self.open_review() else {
            return;
        };
        self.status_message = match review.reject_all() {
            Ok(skipped) if skipped.is_empty() => None,
            Ok(skipped) => Some(termide_i18n::t().git_diff_reject_all_skipped_fmt(skipped.len())),
            Err(error) => Some(first_line(&error)),
        };
        if let Some(mode) = self.review.as_mut() {
            mode.last_reject = None;
        }
        self.after_action();
    }

    fn after_action(&mut self) -> Vec<PanelEvent> {
        self.anchor = None;
        self.refresh();
        vec![PanelEvent::NeedsRedraw]
    }
}

#[cfg(test)]
mod tests;
