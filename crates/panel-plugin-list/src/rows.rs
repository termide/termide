//! The rows a list is drawn as: each group's heading, its rows without a
//! `path` in the plugin's order, then those with one in a directory tree,
//! as git status shows changed files.

use std::collections::HashSet;

use crate::{ItemStatus, PanelList};

/// A row on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    /// A group's heading.
    Group(usize),
    /// A directory of the tree, by its index in [`Layout::dirs`].
    Dir(usize),
    /// An item of a group.
    Item(usize, usize),
}

/// A directory of a group's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Dir {
    pub(crate) group: usize,
    /// Its path in the tree, `/`-separated.
    pub(crate) path: String,
    /// Its own name.
    pub(crate) name: String,
    /// What its files are: their one status, or modified when they differ.
    pub(crate) status: Option<ItemStatus>,
    pub(crate) expanded: bool,
}

/// What [`layout`] gives: the visible rows with their depths, and every
/// directory, the hidden ones included.
#[derive(Debug, Default)]
pub(crate) struct Layout {
    pub(crate) rows: Vec<Row>,
    pub(crate) depths: Vec<usize>,
    pub(crate) dirs: Vec<Dir>,
}

/// Lay `list` out. `collapsed` holds the folded directories as (group
/// label, directory path).
pub(crate) fn layout(list: &PanelList, collapsed: &HashSet<(String, String)>) -> Layout {
    let mut out = Layout::default();
    for (g, group) in list.groups.iter().enumerate() {
        out.push(Row::Group(g), 0);
        for (i, _) in group
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.path.is_none())
        {
            out.push(Row::Item(g, i), 1);
        }
        let mut in_tree: Vec<(usize, Vec<&str>)> = group
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| Some((i, components(item.path.as_deref()?))))
            .filter(|(_, parts)| !parts.is_empty())
            .collect();
        in_tree.sort_by(|a, b| a.1.cmp(&b.1));
        // The directories the last item sat in, outermost first.
        let mut stack: Vec<usize> = Vec::new();
        for (i, parts) in in_tree {
            let dir_parts = &parts[..parts.len() - 1];
            let common = stack
                .iter()
                .zip(dir_parts.iter().enumerate())
                .take_while(|(&d, (k, _))| out.dirs[d].path == dir_parts[..=*k].join("/"))
                .count();
            stack.truncate(common);
            for k in common..dir_parts.len() {
                let path = dir_parts[..=k].join("/");
                let expanded = !collapsed.contains(&(group.label.clone(), path.clone()));
                let visible = stack.iter().all(|&d| out.dirs[d].expanded);
                out.dirs.push(Dir {
                    group: g,
                    path,
                    name: dir_parts[k].to_string(),
                    status: None,
                    expanded,
                });
                let index = out.dirs.len() - 1;
                if visible {
                    out.push(Row::Dir(index), k + 1);
                }
                stack.push(index);
            }
            if let Some(status) = group.items[i].status {
                for &d in &stack {
                    let dir = &mut out.dirs[d];
                    dir.status = match dir.status {
                        None => Some(status),
                        Some(seen) if seen == status => Some(seen),
                        Some(_) => Some(ItemStatus::Modified),
                    };
                }
            }
            if stack.iter().all(|&d| out.dirs[d].expanded) {
                out.push(Row::Item(g, i), stack.len() + 1);
            }
        }
    }
    out
}

impl Layout {
    fn push(&mut self, row: Row, depth: usize) {
        self.rows.push(row);
        self.depths.push(depth);
    }
}

/// The parts of a `/`-separated path, empty ones and `.` dropped.
pub(crate) fn components(path: &str) -> Vec<&str> {
    path.split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ListGroup, ListItem};

    fn item(label: &str, path: Option<&str>, status: Option<ItemStatus>) -> ListItem {
        ListItem {
            label: label.into(),
            path: path.map(str::to_string),
            status,
            ..ListItem::default()
        }
    }

    fn list() -> PanelList {
        PanelList {
            groups: vec![ListGroup {
                label: "Default".into(),
                items: vec![
                    item("src/b.rs", Some("src/b.rs"), Some(ItemStatus::Added)),
                    item("notes", None, None),
                    item("README", Some("README"), Some(ItemStatus::Modified)),
                    item("src/a/x.rs", Some("src/a/x.rs"), Some(ItemStatus::Added)),
                ],
                ..ListGroup::default()
            }],
            ..PanelList::default()
        }
    }

    #[test]
    fn paths_make_a_tree_after_the_rows_without_one() {
        let out = layout(&list(), &HashSet::new());
        assert_eq!(
            out.rows,
            [
                Row::Group(0),
                Row::Item(0, 1),
                Row::Item(0, 2),
                Row::Dir(0),
                Row::Dir(1),
                Row::Item(0, 3),
                Row::Item(0, 0),
            ]
        );
        assert_eq!(out.depths, [0, 1, 1, 1, 2, 3, 2]);
        let dirs: Vec<_> = out
            .dirs
            .iter()
            .map(|d| (d.path.as_str(), d.status))
            .collect();
        assert_eq!(
            dirs,
            [
                ("src", Some(ItemStatus::Added)),
                ("src/a", Some(ItemStatus::Added))
            ]
        );
    }

    #[test]
    fn a_folded_directory_hides_what_is_under_it_but_still_counts_it() {
        let mut items = list();
        items.groups[0].items[0].status = Some(ItemStatus::Deleted);
        let collapsed = HashSet::from([("Default".to_string(), "src".to_string())]);
        let out = layout(&items, &collapsed);
        assert_eq!(
            out.rows,
            [Row::Group(0), Row::Item(0, 1), Row::Item(0, 2), Row::Dir(0)]
        );
        assert!(!out.dirs[0].expanded);
        assert_eq!(out.dirs[0].status, Some(ItemStatus::Modified));
    }
}
