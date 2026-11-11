//! The agent's side of the review of its changes in the diff panel (see
//! `termide_git::Review`): the working tree is taken as a snapshot when a
//! request starts and when it ends, whatever made the changes — the edit
//! tools, the shell, or an external agent — and what the user rejected
//! meanwhile is told the agent with its next request.

use termide_agent_core::{UserContent, UserMessage};
use termide_core::PanelEvent;
use termide_git::Review;

use crate::{AgentPanel, NoticeKind};

impl AgentPanel {
    /// The review of this panel's session, in the repository it works in.
    fn review(&self) -> Option<Review> {
        Review::open(&self.cwd, &self.session_id()?)
    }

    /// Open the review of the session's changes in the diff panel.
    pub(crate) fn open_review(&mut self) -> Vec<PanelEvent> {
        match (self.review(), self.session_id()) {
            (Some(review), Some(session)) => vec![PanelEvent::OpenAgentReview {
                repo_path: review.root().to_path_buf(),
                session,
            }],
            _ => {
                self.notice(termide_i18n::t().git_no_repo(), NoticeKind::Info);
                vec![PanelEvent::NeedsRedraw]
            }
        }
    }

    /// A request starts with `message`: the working tree is taken as it is,
    /// and the message carries what the user rejected since the last one.
    pub(crate) fn begin_review(&self, mut message: UserMessage) -> UserMessage {
        let Some(review) = self.review() else {
            return message;
        };
        match review.begin_request() {
            Ok(notes) if !notes.is_empty() => message.content.push(UserContent::Text {
                text: rejection_note(&notes),
            }),
            Ok(_) => {}
            Err(error) => log::warn!("cannot take the working tree for review: {error}"),
        }
        message
    }

    /// A request ended: the working tree is taken as the agent left it.
    pub(crate) fn end_review(&self) {
        if let Some(Err(error)) = self.review().map(|review| review.end_request()) {
            log::warn!("cannot take the working tree for review: {error}");
        }
    }
}

/// What the agent reads of the user's rejections, after the user's words.
fn rejection_note(notes: &[String]) -> String {
    let mut text = String::from(
        "\n\n<review-note>\nThe user reviewed your changes and rejected these; \
         they are undone in the working tree. Do not make them again unless asked.",
    );
    for note in notes {
        text.push_str("\n- ");
        text.push_str(&note.replace('\n', "\n  "));
    }
    text.push_str("\n</review-note>");
    text
}

#[cfg(test)]
mod tests {
    use super::rejection_note;

    #[test]
    fn the_note_lists_each_rejection_indented_under_its_file() {
        let note = rejection_note(&["f @@ -1 +1 @@\n-a\n+b".into(), "g: every change".into()]);
        assert!(note.starts_with("\n\n<review-note>\n"));
        assert!(note.contains("\n- f @@ -1 +1 @@\n  -a\n  +b\n- g: every change\n</review-note>"));
    }
}
