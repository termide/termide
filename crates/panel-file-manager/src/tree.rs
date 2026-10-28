//! Tree view model for the file manager.
//!
//! Provides a flat tree structure where directories can be expanded in-place,
//! similar to the git status panel tree but with lazy loading of subdirectories.

use std::path::PathBuf;

use super::FileEntry;

/// A node in the flat file tree, wrapping FileEntry with tree metadata.
#[derive(Debug, Clone)]
pub(crate) struct TreeEntry {
    /// The underlying file entry with all metadata.
    pub file_entry: FileEntry,
    /// Absolute path of this entry.
    pub full_path: PathBuf,
    /// Nesting depth (0 = top-level in current directory).
    pub depth: usize,
    /// `Some(true)` = expanded dir, `Some(false)` = collapsed dir, `None` = file or "..".
    pub expanded: Option<bool>,
    /// Synthetic placeholder shown under a remote directory while its
    /// async `list_dir` is in flight. The placeholder is removed and
    /// replaced by real children once the VFS operation resolves.
    pub is_loading: bool,
}

/// Compute indices of visible nodes, skipping children of collapsed directories.
///
/// Same algorithm as `panel-git-status/src/tree.rs::compute_visible_nodes`.
pub(crate) fn compute_visible(tree: &[TreeEntry]) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut skip_below_depth: Option<usize> = None;

    for (i, entry) in tree.iter().enumerate() {
        if let Some(max_depth) = skip_below_depth {
            if entry.depth > max_depth {
                continue;
            }
            // Exited the collapsed subtree
            skip_below_depth = None;
        }

        visible.push(i);

        if entry.expanded == Some(false) {
            skip_below_depth = Some(entry.depth);
        }
    }

    visible
}

/// Compute tree-drawing prefixes for visible nodes in O(n) time.
///
/// Only generates prefixes for depth > 0 nodes. Depth 0 nodes get empty prefixes.
/// Same traversal as `panel-git-status/src/tree.rs::compute_tree_prefixes`, but
/// uses a two-column-per-level connector (`├─`, no leading space) because this
/// panel renders an icon column the connector must line up under.
pub(crate) fn compute_prefixes(tree: &[TreeEntry], visible: &[usize]) -> Vec<String> {
    if visible.is_empty() {
        return Vec::new();
    }

    let max_depth = visible
        .iter()
        .map(|&idx| tree[idx].depth)
        .max()
        .unwrap_or(0);

    if max_depth == 0 {
        // No nested items, all prefixes are empty
        return vec![String::new(); visible.len()];
    }

    // has_next_at_level[lvl] is true when a later visible node exists at that depth
    let mut has_next_at_level = vec![false; max_depth + 1];

    // Build prefixes in reverse, then reverse the result
    let mut prefixes: Vec<String> = Vec::with_capacity(visible.len());

    for &tree_idx in visible.iter().rev() {
        let depth = tree[tree_idx].depth;

        if depth == 0 {
            // Root-level node: reset all levels, no prefix
            has_next_at_level.fill(false);
            has_next_at_level[0] = true;
            prefixes.push(String::new());
            continue;
        }

        // Two columns per level so the connector sits directly under the
        // parent's icon: the row layout renders `[attr][prefix][icon]`, where
        // the attr gutter occupies the first column and `├─`/`└─` is exactly
        // as wide as the gap from a connector to the next level's icon. A
        // leading space here would push the whole tree one column off the
        // top-level glyph.
        let mut prefix = String::with_capacity(depth * 2);
        for (lvl, has_next) in has_next_at_level[1..=depth].iter().enumerate() {
            let lvl = lvl + 1; // offset since we sliced from index 1
            if lvl == depth {
                if *has_next {
                    prefix.push_str("├─");
                } else {
                    prefix.push_str("└─");
                }
            } else if *has_next {
                prefix.push_str("│ ");
            } else {
                prefix.push_str("  ");
            }
        }
        prefixes.push(prefix);

        // This node "occupies" its depth: deeper levels no longer have siblings
        for val in &mut has_next_at_level[(depth + 1)..] {
            *val = false;
        }
        // Nodes processed earlier (which appear before this one) will see this as a sibling
        has_next_at_level[depth] = true;
    }

    prefixes.reverse();
    prefixes
}

/// Count children of a directory node in tree_entries (direct and nested).
/// Returns the number of entries immediately following `dir_idx` that have depth > dir's depth.
#[cfg(test)]
pub(crate) fn count_children(tree: &[TreeEntry], dir_idx: usize) -> usize {
    let dir_depth = tree[dir_idx].depth;
    tree[dir_idx + 1..]
        .iter()
        .take_while(|e| e.depth > dir_depth)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_git::GitStatus;

    fn make_file_entry(name: &str, is_dir: bool) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            os_name: None,
            is_dir,
            is_symlink: false,
            is_executable: false,
            is_readonly: false,
            git_status: GitStatus::Unmodified,
            size: None,
            modified: None,
        }
    }

    fn make_tree_entry(
        name: &str,
        is_dir: bool,
        depth: usize,
        expanded: Option<bool>,
    ) -> TreeEntry {
        TreeEntry {
            file_entry: make_file_entry(name, is_dir),
            full_path: PathBuf::from(format!("/test/{}", name)),
            depth,
            expanded,
            is_loading: false,
        }
    }

    #[test]
    fn test_compute_visible_all_top_level() {
        let tree = vec![
            make_tree_entry("..", true, 0, None),
            make_tree_entry("src", true, 0, Some(false)),
            make_tree_entry("Cargo.toml", false, 0, None),
        ];
        let visible = compute_visible(&tree);
        assert_eq!(visible, vec![0, 1, 2]);
    }

    #[test]
    fn test_compute_visible_with_expanded() {
        let tree = vec![
            make_tree_entry("..", true, 0, None),
            make_tree_entry("src", true, 0, Some(true)),
            make_tree_entry("main.rs", false, 1, None),
            make_tree_entry("lib.rs", false, 1, None),
            make_tree_entry("Cargo.toml", false, 0, None),
        ];
        let visible = compute_visible(&tree);
        assert_eq!(visible, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn test_compute_visible_with_collapsed() {
        let tree = vec![
            make_tree_entry("..", true, 0, None),
            make_tree_entry("src", true, 0, Some(false)),
            make_tree_entry("main.rs", false, 1, None),
            make_tree_entry("lib.rs", false, 1, None),
            make_tree_entry("Cargo.toml", false, 0, None),
        ];
        let visible = compute_visible(&tree);
        // main.rs and lib.rs should be hidden
        assert_eq!(visible, vec![0, 1, 4]);
    }

    #[test]
    fn test_compute_visible_nested_collapse() {
        let tree = vec![
            make_tree_entry("src", true, 0, Some(true)),
            make_tree_entry("components", true, 1, Some(false)),
            make_tree_entry("button.rs", false, 2, None),
            make_tree_entry("main.rs", false, 1, None),
        ];
        let visible = compute_visible(&tree);
        // button.rs hidden (inside collapsed components)
        assert_eq!(visible, vec![0, 1, 3]);
    }

    #[test]
    fn test_compute_prefixes_flat() {
        let tree = vec![
            make_tree_entry("src", true, 0, Some(false)),
            make_tree_entry("Cargo.toml", false, 0, None),
        ];
        let visible = compute_visible(&tree);
        let prefixes = compute_prefixes(&tree, &visible);
        assert_eq!(prefixes, vec!["", ""]);
    }

    #[test]
    fn test_compute_prefixes_expanded() {
        let tree = vec![
            make_tree_entry("src", true, 0, Some(true)),
            make_tree_entry("main.rs", false, 1, None),
            make_tree_entry("lib.rs", false, 1, None),
            make_tree_entry("Cargo.toml", false, 0, None),
        ];
        let visible = compute_visible(&tree);
        let prefixes = compute_prefixes(&tree, &visible);
        assert_eq!(prefixes[0], ""); // src (depth 0)
        assert_eq!(prefixes[1], "├─"); // main.rs (has sibling)
        assert_eq!(prefixes[2], "└─"); // lib.rs (last child)
        assert_eq!(prefixes[3], ""); // Cargo.toml (depth 0)
    }

    #[test]
    fn test_compute_prefixes_nested() {
        let tree = vec![
            make_tree_entry("src", true, 0, Some(true)),
            make_tree_entry("components", true, 1, Some(true)),
            make_tree_entry("button.rs", false, 2, None),
            make_tree_entry("main.rs", false, 1, None),
        ];
        let visible = compute_visible(&tree);
        let prefixes = compute_prefixes(&tree, &visible);
        assert_eq!(prefixes[0], ""); // src
        assert_eq!(prefixes[1], "├─"); // components (has sibling main.rs)
        assert_eq!(prefixes[2], "│ └─"); // button.rs (inside components, which has sibling)
        assert_eq!(prefixes[3], "└─"); // main.rs (last child of src)
    }

    #[test]
    fn test_count_children() {
        let tree = vec![
            make_tree_entry("src", true, 0, Some(true)),
            make_tree_entry("components", true, 1, Some(true)),
            make_tree_entry("button.rs", false, 2, None),
            make_tree_entry("main.rs", false, 1, None),
            make_tree_entry("Cargo.toml", false, 0, None),
        ];
        assert_eq!(count_children(&tree, 0), 3); // components, button.rs, main.rs
        assert_eq!(count_children(&tree, 1), 1); // button.rs
    }
}
