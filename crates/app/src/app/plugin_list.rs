//! The `list` a plugin sends to `panel/open` and `panel/update`, read into
//! what the list panel draws: text cleaned and cut as a plugin's titles
//! are, malformed entries left out, and the whole held to a size.

use serde_json::Value;
use termide_panel_plugin_list::{
    ActionKey, ItemStatus, ListAction, ListGroup, ListItem, PanelList, MAX_ROW_ACTIONS,
};
use termide_plugins::protocol::{clean_text, MAX_LABEL, MAX_MENU_ITEMS};

/// The longest a row's label or detail may be, in characters.
const MAX_ROW_TEXT: usize = 200;
/// The most rows a list holds, headings included.
const MAX_ROWS: usize = 10_000;

/// The `list` param.
pub(super) fn plugin_list(value: &Value) -> Result<PanelList, String> {
    let groups = value["groups"]
        .as_array()
        .ok_or("`list.groups` must be an array")?;
    let mut rows = 0;
    let mut list = PanelList {
        empty: text(&value["empty"], MAX_ROW_TEXT),
        refresh: value["refresh"]
            .as_str()
            .filter(|action| !action.is_empty())
            .map(str::to_string),
        groups: Vec::new(),
        buttons: actions(&value["buttons"], MAX_MENU_ITEMS)
            .into_iter()
            .map(|button| ListAction {
                key: None,
                ..button
            })
            .collect(),
    };
    for group in groups {
        let label = text(&group["label"], MAX_ROW_TEXT);
        if label.is_empty() {
            continue;
        }
        rows += 1;
        if rows > MAX_ROWS {
            break;
        }
        let mut items = Vec::new();
        for item in group["items"].as_array().into_iter().flatten() {
            let label = text(&item["label"], MAX_ROW_TEXT);
            if label.is_empty() {
                continue;
            }
            rows += 1;
            if rows > MAX_ROWS {
                break;
            }
            items.push(ListItem {
                label,
                path: item["path"].as_str().and_then(path),
                detail: text(&item["detail"], MAX_ROW_TEXT),
                status: item["status"].as_str().and_then(ItemStatus::from_name),
                actions: actions(&item["actions"], MAX_ROW_ACTIONS),
            });
        }
        list.groups.push(ListGroup {
            label,
            detail: text(&group["detail"], MAX_ROW_TEXT),
            actions: actions(&group["actions"], MAX_ROW_ACTIONS),
            items,
        });
    }
    Ok(list)
}

fn text(value: &Value, max: usize) -> String {
    value
        .as_str()
        .map_or_else(String::new, |text| clean_text(text, max))
}

/// A row's `path`, each part cleaned as text is; `None` when nothing is
/// left of it.
fn path(text: &str) -> Option<String> {
    let parts: Vec<String> = text
        .split(['/', '\\'])
        .map(|part| clean_text(part, MAX_ROW_TEXT))
        .filter(|part| !part.is_empty() && part != "." && part != "..")
        .collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A row's `actions`, `[{ "label", "action", "key"? }]`: at most `max`,
/// each key taken by the first that names it.
fn actions(value: &Value, max: usize) -> Vec<ListAction> {
    let mut taken = Vec::new();
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let label = text(&entry["label"], MAX_LABEL);
            let action = entry["action"].as_str()?.to_string();
            if label.is_empty() || action.is_empty() {
                return None;
            }
            let key = entry["key"]
                .as_str()
                .and_then(ActionKey::from_name)
                .filter(|key| !taken.contains(key));
            taken.extend(key);
            Some(ListAction { label, action, key })
        })
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_list_is_read_with_its_text_cleaned() {
        let list = plugin_list(&json!({
            "groups": [{
                "label": "Change\u{202E} 7",
                "detail": "fix\nit",
                "actions": [{ "label": "Submit", "action": "submit-7" }],
                "items": [
                    { "label": "a.c", "status": "added", "actions": [
                        { "label": "Diff", "action": "diff-0", "key": "view" },
                        { "label": "Again", "action": "diff-1", "key": "view" },
                        { "label": "Odd", "action": "x", "key": "stage" },
                        { "label": "", "action": "none" },
                    ]},
                    { "detail": "no label" },
                    { "label": "b.c", "status": "renamed", "path": "./src//\u{0007}b.c" },
                ],
            }, { "items": [] }],
            "buttons": [
                { "label": "Sync", "action": "sync", "key": "view" },
                { "label": "No action" },
            ],
            "empty": "Nothing",
            "refresh": "refresh",
        }))
        .unwrap();
        assert_eq!(list.groups.len(), 1, "a group needs a label");
        let group = &list.groups[0];
        assert_eq!(group.label, "Change 7");
        assert_eq!(group.detail, "fix it");
        assert_eq!(group.actions[0].action, "submit-7");
        assert_eq!(group.items.len(), 2);
        let a = &group.items[0];
        assert_eq!(a.status, Some(ItemStatus::Added));
        let keys: Vec<_> = a.actions.iter().map(|action| action.key).collect();
        assert_eq!(keys, [Some(ActionKey::View), None, None], "a key once");
        assert_eq!(group.items[1].status, None);
        assert_eq!(a.path, None);
        assert_eq!(group.items[1].path.as_deref(), Some("src/b.c"));
        let buttons: Vec<_> = list.buttons.iter().map(|b| (&*b.action, b.key)).collect();
        assert_eq!(buttons, [("sync", None)], "a button takes no key");
        assert_eq!(list.empty, "Nothing");
        assert_eq!(list.refresh.as_deref(), Some("refresh"));
    }

    #[test]
    fn a_list_needs_its_groups() {
        assert!(plugin_list(&json!({ "empty": "x" })).is_err());
        assert!(plugin_list(&json!({ "groups": [] }))
            .unwrap()
            .groups
            .is_empty());
    }

    #[test]
    fn a_row_offers_a_handful_of_actions() {
        let many: Vec<Value> = (0..10)
            .map(|i| json!({ "label": format!("A{i}"), "action": format!("a{i}") }))
            .collect();
        let list = plugin_list(&json!({ "groups": [{ "label": "g", "actions": many }] })).unwrap();
        assert_eq!(list.groups[0].actions.len(), MAX_ROW_ACTIONS);
    }
}
