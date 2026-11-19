//! The agent's checklist: the steps of the work at hand, each pending, in
//! progress or done, shown above the panel's input. Every report replaces
//! the whole list, as ACP's `plan` updates do; an empty list clears it.

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
    fn all_done_needs_a_step() {
        assert!(!all_done(&[]));
        assert!(all_done(&[ChecklistItem::new("a", ChecklistStatus::Done)]));
        assert!(!all_done(&[
            ChecklistItem::new("a", ChecklistStatus::Done),
            ChecklistItem::new("b", ChecklistStatus::Pending),
        ]));
    }
}
