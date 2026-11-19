//! The agent's checklist: the steps of the work at hand, each pending, in
//! progress or done, shown above the panel's input. Every report replaces
//! the whole list, as ACP's `plan` updates do; an empty list clears it.
//!
//! The list comes from the `checklist` tool or an external agent's plan.
//! The process keeps each session's current list ([`remember`]), so the
//! model hears of an unfinished one at each call ([`context_note`]) — at
//! the end of the request, never in the transcript, so it outlives a
//! compaction and leaves the cached prefix alone.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use serde::{Deserialize, Serialize};

/// Where a step of the list stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistStatus {
    Pending,
    InProgress,
    Done,
}

/// A step of the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub text: String,
    pub status: ChecklistStatus,
}

impl ChecklistItem {
    #[must_use]
    pub fn new(text: impl Into<String>, status: ChecklistStatus) -> Self {
        Self {
            text: text.into(),
            status,
        }
    }
}

/// Whether every step of `items` is done (and there is at least one).
#[must_use]
pub fn all_done(items: &[ChecklistItem]) -> bool {
    !items.is_empty()
        && items
            .iter()
            .all(|item| item.status == ChecklistStatus::Done)
}

/// The key of the tool result's `details` that carries the list.
pub const DETAILS_KEY: &str = "checklist";

/// Each session's current list, by the session's id.
fn lists() -> &'static Mutex<HashMap<String, Vec<ChecklistItem>>> {
    static LISTS: OnceLock<Mutex<HashMap<String, Vec<ChecklistItem>>>> = OnceLock::new();
    LISTS.get_or_init(Mutex::default)
}

/// Keep `items` as session `session`'s list (empty: it has none).
pub fn remember(session: &str, items: &[ChecklistItem]) {
    let mut lists = lists().lock().unwrap_or_else(PoisonError::into_inner);
    if items.is_empty() {
        lists.remove(session);
    } else {
        lists.insert(session.to_string(), items.to_vec());
    }
}

/// Session `session`'s list as last kept.
#[must_use]
pub fn current(session: &str) -> Vec<ChecklistItem> {
    lists()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(session)
        .cloned()
        .unwrap_or_default()
}

/// The list as the model reads it, a line per step:
/// `- [x] done`, `- [>] in progress`, `- [ ] pending`.
#[must_use]
pub fn render(items: &[ChecklistItem]) -> String {
    items
        .iter()
        .map(|item| {
            let mark = match item.status {
                ChecklistStatus::Done => "x",
                ChecklistStatus::InProgress => ">",
                ChecklistStatus::Pending => " ",
            };
            format!("- [{mark}] {}", item.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The note on an unfinished list; `None` for none, or one carried through.
#[must_use]
pub fn note(items: &[ChecklistItem]) -> Option<String> {
    if items.is_empty() || all_done(items) {
        return None;
    }
    Some(format!(
        "Your checklist (keep it current with `checklist`):\n{}",
        render(items)
    ))
}

/// The list the result of a `checklist` call carries, for the panel: `None`
/// for any other result, or one that failed.
#[must_use]
pub fn of_result(result: &crate::ToolResultMessage) -> Option<Vec<ChecklistItem>> {
    if result.is_error {
        return None;
    }
    let items = result.details.as_ref()?.get(DETAILS_KEY)?;
    serde_json::from_value(items.clone()).ok()
}

/// The [`ContextNote`](crate::ContextNote) that reminds the agent of its
/// unfinished list at each model call.
#[must_use]
pub fn context_note() -> crate::ContextNote {
    std::sync::Arc::new(|session| note(&current(session?)))
}

/// The list in an ACP `plan` update's `entries`: `content` and `status`
/// (`pending`, `in_progress`, `completed`) of each; entries with no text
/// are dropped, an unknown status reads as pending.
#[must_use]
pub fn from_acp_entries(entries: &serde_json::Value) -> Vec<ChecklistItem> {
    entries
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let text = entry["content"].as_str()?.trim();
            if text.is_empty() {
                return None;
            }
            let status = match entry["status"].as_str() {
                Some("in_progress") => ChecklistStatus::InProgress,
                Some("completed") => ChecklistStatus::Done,
                _ => ChecklistStatus::Pending,
            };
            Some(ChecklistItem::new(text, status))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acp_entries_become_the_list() {
        let entries = json!([
            {"content": "Read the code", "priority": "high", "status": "completed"},
            {"content": "Fix it", "priority": "medium", "status": "in_progress"},
            {"content": "  ", "status": "pending"},
            {"content": "Test", "status": "something_new"},
        ]);
        assert_eq!(
            from_acp_entries(&entries),
            vec![
                ChecklistItem::new("Read the code", ChecklistStatus::Done),
                ChecklistItem::new("Fix it", ChecklistStatus::InProgress),
                ChecklistItem::new("Test", ChecklistStatus::Pending),
            ]
        );
        assert!(from_acp_entries(&json!(null)).is_empty());
    }

    #[test]
    fn an_unfinished_list_is_noted_and_a_finished_one_is_not() {
        let session = "checklist-note-test";
        assert_eq!(context_note()(Some(session)), None);
        let mut items = vec![
            ChecklistItem::new("Read", ChecklistStatus::Done),
            ChecklistItem::new("Fix", ChecklistStatus::InProgress),
            ChecklistItem::new("Test", ChecklistStatus::Pending),
        ];
        remember(session, &items);
        let text = context_note()(Some(session)).unwrap();
        assert!(
            text.ends_with("- [x] Read\n- [>] Fix\n- [ ] Test"),
            "{text}"
        );
        assert_eq!(context_note()(None), None);
        for item in &mut items {
            item.status = ChecklistStatus::Done;
        }
        remember(session, &items);
        assert_eq!(context_note()(Some(session)), None);
        remember(session, &[]);
        assert!(current(session).is_empty());
    }

    #[test]
    fn all_done_needs_a_step() {
        assert!(!all_done(&[]));
        assert!(all_done(&[ChecklistItem::new("a", ChecklistStatus::Done)]));
        assert!(!all_done(&[
            ChecklistItem::new("a", ChecklistStatus::Done),
            ChecklistItem::new("b", ChecklistStatus::Pending),
        ]));
    }
}
