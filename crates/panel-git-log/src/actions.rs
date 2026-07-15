//! Commit actions: info modal, open-in-browser, view-diff, and modal handoff.

use std::path::PathBuf;

use termide_core::PanelEvent;
use termide_git::{self as git};
use termide_modal::{ActiveModal, InfoModal, ModalValue, SegmentStyle, StyledSegment};
use termide_state::PendingAction;

use crate::GitLogPanel;

impl GitLogPanel {
    /// Take pending modal request (called by the app via PanelExt).
    pub fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    /// Show commit info modal for the selected commit.
    pub(crate) fn show_commit_info(&mut self) {
        let Some(commit) = self.selected_commit() else {
            return;
        };
        if commit.hash.is_empty() {
            return;
        }
        let hash = commit.hash.clone();

        let Some(repo) = self.repo_manager.current() else {
            return;
        };
        let repo = repo.to_path_buf();

        let Some(details) = git::get_commit_details(&repo, &hash) else {
            return;
        };

        let t = termide_i18n::t();
        let short_hash = if details.hash.len() > 8 {
            &details.hash[..8]
        } else {
            &details.hash
        };
        let title = t.git_commit_info_title(short_hash);

        // Build colored file status segments (only non-zero counts)
        let mut file_segments = Vec::new();
        if details.files_modified > 0 {
            file_segments.push(StyledSegment {
                text: details.files_modified.to_string(),
                style: SegmentStyle::Warning,
            });
            file_segments.push(StyledSegment {
                text: format!(" {}", t.git_commit_files_modified()),
                style: SegmentStyle::Default,
            });
        }
        if details.files_added > 0 {
            if !file_segments.is_empty() {
                file_segments.push(StyledSegment {
                    text: "  ".to_string(),
                    style: SegmentStyle::Default,
                });
            }
            file_segments.push(StyledSegment {
                text: details.files_added.to_string(),
                style: SegmentStyle::Success,
            });
            file_segments.push(StyledSegment {
                text: format!(" {}", t.git_commit_files_added()),
                style: SegmentStyle::Default,
            });
        }
        if details.files_deleted > 0 {
            if !file_segments.is_empty() {
                file_segments.push(StyledSegment {
                    text: "  ".to_string(),
                    style: SegmentStyle::Default,
                });
            }
            file_segments.push(StyledSegment {
                text: details.files_deleted.to_string(),
                style: SegmentStyle::Error,
            });
            file_segments.push(StyledSegment {
                text: format!(" {}", t.git_commit_files_deleted()),
                style: SegmentStyle::Default,
            });
        }
        // Fallback if all zero
        if file_segments.is_empty() {
            file_segments.push(StyledSegment {
                text: "0".to_string(),
                style: SegmentStyle::Disabled,
            });
        }

        // Build colored lines segments (+N green, -N red)
        let lines_segments = vec![
            StyledSegment {
                text: format!("+{}", details.insertions),
                style: SegmentStyle::Success,
            },
            StyledSegment {
                text: " / ".to_string(),
                style: SegmentStyle::Default,
            },
            StyledSegment {
                text: format!("-{}", details.deletions),
                style: SegmentStyle::Error,
            },
        ];

        let data = vec![
            (
                t.git_commit_author().to_string(),
                ModalValue::Text(details.author),
            ),
            (
                t.git_commit_date().to_string(),
                ModalValue::Text(details.date),
            ),
            (
                t.git_commit_message().to_string(),
                ModalValue::Text(details.message),
            ),
            (
                t.git_commit_files().to_string(),
                ModalValue::Segments(file_segments),
            ),
            (
                t.git_commit_lines().to_string(),
                ModalValue::Segments(lines_segments),
            ),
        ];

        let modal = InfoModal::new_rich(title, data);
        self.modal_request = Some((
            PendingAction::VfsMessage,
            ActiveModal::Info(Box::new(modal)),
        ));
    }

    /// Open selected commit in browser via remote URL, or show fallback message.
    pub(crate) fn open_commit_external(&mut self) -> Vec<PanelEvent> {
        let Some(commit) = self.selected_commit() else {
            return vec![];
        };
        if commit.hash.is_empty() {
            return vec![];
        }
        let hash = commit.hash.clone();

        let Some(repo) = self.repo_manager.current() else {
            return vec![];
        };

        if let Some(url) = git::get_commit_web_url(repo, &hash) {
            return vec![PanelEvent::OpenExternal(PathBuf::from(url))];
        }

        let t = termide_i18n::t();
        self.status_message = Some(t.git_no_remote_url().to_string());
        vec![]
    }

    /// View diff for selected commit
    pub(crate) fn view_diff(&mut self) -> Vec<PanelEvent> {
        let Some(commit) = self.selected_commit() else {
            return vec![];
        };
        if commit.hash.is_empty() {
            return vec![];
        }

        let Some(repo) = self.repo_manager.current() else {
            return vec![];
        };

        // Open Git Diff panel for this commit
        vec![PanelEvent::OpenGitDiff {
            repo_path: repo.to_path_buf(),
            commit_hash: Some(commit.hash.clone()),
            file_path: None,
        }]
    }
}
