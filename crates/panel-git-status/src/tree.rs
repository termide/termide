//! Tree view types and algorithms for Git Status Panel.
//!
//! Groups files by directory into a collapsible tree, similar to VS Code Source Control.

use std::collections::HashSet;
use std::path::PathBuf;

/// A single node in the file tree (directory or file).
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// Display label ("src" for directories, "main.rs" for files)
    pub label: String,
    /// Full relative path
    pub full_path: PathBuf,
    /// Nesting depth (0 = top-level)
    pub depth: usize,
    /// Whether this is a directory or file, with associated data
    pub kind: TreeNodeKind,
}

/// Kind of tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeNodeKind {
    Directory {
        expanded: bool,
    },
    File {
        file_index: usize,
        status: char,
        untracked: bool,
    },
}

/// Input file info for tree building.
pub struct FileEntry {
    pub path: PathBuf,
    pub index: usize,
    pub status: char,
    pub untracked: bool,
}

/// Build a flat tree from a sorted list of files.
///
/// For each file, directory nodes are emitted for path segments that haven't been seen yet.
/// `collapsed_dirs` determines which directories start collapsed.
pub fn build_tree(files: &[FileEntry], collapsed_dirs: &HashSet<PathBuf>) -> Vec<TreeNode> {
    if files.is_empty() {
        return Vec::new();
    }

    let mut nodes = Vec::new();
    // Stack of (directory_path, depth) currently open
    let mut dir_stack: Vec<(PathBuf, usize)> = Vec::new();

    for file in files {
        let components: Vec<&str> = file
            .path
            .components()
            .filter_map(|c| {
                if let std::path::Component::Normal(s) = c {
                    s.to_str()
                } else {
                    None
                }
            })
            .collect();

        if components.is_empty() {
            continue;
        }

        // The last component is the filename
        let dir_components = &components[..components.len() - 1];
        let file_name = components[components.len() - 1];

        // Find common prefix with current dir_stack
        let mut common = 0;
        for (i, (dir_path, _)) in dir_stack.iter().enumerate() {
            if i < dir_components.len() {
                let expected: PathBuf = dir_components[..=i].iter().collect();
                if *dir_path == expected {
                    common = i + 1;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        // Pop directories that are no longer in the path
        dir_stack.truncate(common);

        // Push new directories
        for i in common..dir_components.len() {
            let dir_path: PathBuf = dir_components[..=i].iter().collect();
            let depth = i;
            let expanded = !collapsed_dirs.contains(&dir_path);
            nodes.push(TreeNode {
                label: dir_components[i].to_string(),
                full_path: dir_path.clone(),
                depth,
                kind: TreeNodeKind::Directory { expanded },
            });
            dir_stack.push((dir_path, depth));
        }

        // Add the file node
        let file_depth = dir_components.len();
        nodes.push(TreeNode {
            label: file_name.to_string(),
            full_path: file.path.clone(),
            depth: file_depth,
            kind: TreeNodeKind::File {
                file_index: file.index,
                status: file.status,
                untracked: file.untracked,
            },
        });
    }

    nodes
}

/// Compute indices of visible nodes (skipping children of collapsed directories).
pub fn compute_visible_nodes(tree: &[TreeNode]) -> Vec<usize> {
    let mut visible = Vec::new();
    let mut skip_below_depth: Option<usize> = None;

    for (i, node) in tree.iter().enumerate() {
        if let Some(max_depth) = skip_below_depth {
            if node.depth > max_depth {
                continue;
            }
            // We've exited the collapsed subtree
            skip_below_depth = None;
        }

        visible.push(i);

        if let TreeNodeKind::Directory { expanded: false } = node.kind {
            skip_below_depth = Some(node.depth);
        }
    }

    visible
}

/// Collect all file paths under a directory node (recursively).
pub(crate) fn collect_files_under(tree: &[TreeNode], dir_index: usize) -> Vec<PathBuf> {
    let dir_depth = tree[dir_index].depth;
    let mut files = Vec::new();

    for node in &tree[dir_index + 1..] {
        if node.depth <= dir_depth {
            break;
        }
        if let TreeNodeKind::File { .. } = node.kind {
            files.push(node.full_path.clone());
        }
    }

    files
}

/// Tree-drawing prefixes for the visible nodes.
pub fn compute_tree_prefixes(tree: &[TreeNode], visible: &[usize]) -> Vec<String> {
    let depths: Vec<usize> = visible.iter().map(|&idx| tree[idx].depth).collect();
    termide_ui::tree_prefixes(&depths)
}

/// Get the aggregate status for files under a directory.
///
/// The list only carries changed files, so the directory's own history is
/// inferred from them: a folder whose changes are all additions (staged or
/// untracked) is shown as added, one whose changes are all deletions as
/// deleted, and any mixture — a modified file next to a new one, a deletion
/// next to an addition — means the folder already existed and is shown as
/// modified, however many new files it gained.
pub(crate) fn aggregate_dir_status(tree: &[TreeNode], dir_index: usize) -> (char, bool) {
    let dir_depth = tree[dir_index].depth;
    let mut deleted = 0u32;
    let mut modified = 0u32;
    let mut added = 0u32;
    let mut untracked = 0u32;

    for node in &tree[dir_index + 1..] {
        if node.depth <= dir_depth {
            break;
        }
        if let TreeNodeKind::File {
            status,
            untracked: ut,
            ..
        } = node.kind
        {
            if ut {
                untracked += 1;
            } else {
                match status {
                    'D' => deleted += 1,
                    'M' => modified += 1,
                    'A' | 'R' => added += 1,
                    _ => {}
                }
            }
        }
    }

    let new = added + untracked;
    if modified == 0 && new == 0 && deleted > 0 {
        ('D', false)
    } else if modified == 0 && deleted == 0 && new > 0 {
        if added > 0 {
            ('A', false)
        } else {
            ('?', true)
        }
    } else if modified + deleted + new > 0 {
        ('M', false)
    } else {
        (' ', false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_file(path: &str, index: usize, status: char) -> FileEntry {
        FileEntry {
            path: PathBuf::from(path),
            index,
            status,
            untracked: false,
        }
    }

    #[test]
    fn test_build_flat_files() {
        let files = vec![make_file("a.rs", 0, 'M'), make_file("b.rs", 1, 'A')];
        let tree = build_tree(&files, &HashSet::new());
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].depth, 0);
        assert_eq!(tree[0].label, "a.rs");
        assert_eq!(tree[1].depth, 0);
        assert_eq!(tree[1].label, "b.rs");
    }

    #[test]
    fn test_build_nested_dirs() {
        let files = vec![
            make_file("src/foo/a.rs", 0, 'M'),
            make_file("src/foo/b.rs", 1, 'M'),
            make_file("src/bar.rs", 2, 'A'),
        ];
        let tree = build_tree(&files, &HashSet::new());
        // src/ (depth 0), foo/ (depth 1), a.rs (depth 2), b.rs (depth 2), bar.rs (depth 1)
        assert_eq!(tree.len(), 5);
        assert_eq!(tree[0].label, "src");
        assert_eq!(tree[0].depth, 0);
        assert!(matches!(
            tree[0].kind,
            TreeNodeKind::Directory { expanded: true }
        ));
        assert_eq!(tree[1].label, "foo");
        assert_eq!(tree[1].depth, 1);
        assert_eq!(tree[2].label, "a.rs");
        assert_eq!(tree[2].depth, 2);
        assert_eq!(tree[3].label, "b.rs");
        assert_eq!(tree[3].depth, 2);
        assert_eq!(tree[4].label, "bar.rs");
        assert_eq!(tree[4].depth, 1);
    }

    #[test]
    fn test_visibility_with_collapse() {
        let mut collapsed = HashSet::new();
        collapsed.insert(PathBuf::from("src/foo"));

        let files = vec![
            make_file("src/foo/a.rs", 0, 'M'),
            make_file("src/foo/b.rs", 1, 'M'),
            make_file("src/bar.rs", 2, 'A'),
        ];
        let tree = build_tree(&files, &collapsed);
        let visible = compute_visible_nodes(&tree);
        // src/, foo/ (collapsed), bar.rs — a.rs and b.rs are hidden
        assert_eq!(visible.len(), 3);
        assert_eq!(tree[visible[0]].label, "src");
        assert_eq!(tree[visible[1]].label, "foo");
        assert_eq!(tree[visible[2]].label, "bar.rs");
    }

    #[test]
    fn test_collect_files_under() {
        let files = vec![
            make_file("src/foo/a.rs", 0, 'M'),
            make_file("src/foo/b.rs", 1, 'M'),
            make_file("src/bar.rs", 2, 'A'),
        ];
        let tree = build_tree(&files, &HashSet::new());
        // dir index 0 = src/, should contain all 3 files
        let collected = collect_files_under(&tree, 0);
        assert_eq!(collected.len(), 3);
        // dir index 1 = foo/, should contain 2 files
        let collected = collect_files_under(&tree, 1);
        assert_eq!(collected.len(), 2);
    }

    #[test]
    fn test_tree_prefixes() {
        let files = vec![make_file("src/a.rs", 0, 'M'), make_file("src/b.rs", 1, 'M')];
        let tree = build_tree(&files, &HashSet::new());
        let visible = compute_visible_nodes(&tree);
        let prefixes = compute_tree_prefixes(&tree, &visible);
        // src/ (depth 0) -> no prefix
        assert_eq!(prefixes[0], "");
        // a.rs (depth 1, has sibling b.rs) -> "├─ "
        assert_eq!(prefixes[1], "├─ ");
        // b.rs (depth 1, last) -> "└─ "
        assert_eq!(prefixes[2], "└─ ");
    }

    fn dir_status(files: &[FileEntry]) -> (char, bool) {
        let tree = build_tree(files, &HashSet::new());
        let dir = tree
            .iter()
            .position(|node| matches!(node.kind, TreeNodeKind::Directory { .. }))
            .expect("directory node");
        aggregate_dir_status(&tree, dir)
    }

    /// A folder that gained more new files than it has modified ones still
    /// existed before, so it is modified — the majority of additions must not
    /// paint it as created.
    #[test]
    fn existing_directory_with_new_files_is_modified() {
        let files = vec![
            make_file("src/old.rs", 0, 'M'),
            make_file("src/new1.rs", 1, 'A'),
            make_file("src/new2.rs", 2, 'A'),
            make_file("src/new3.rs", 3, 'A'),
        ];
        assert_eq!(dir_status(&files), ('M', false));

        let mut files = vec![make_file("src/old.rs", 0, 'M')];
        for i in 1..4 {
            files.push(FileEntry {
                path: PathBuf::from(format!("src/new{i}.rs")),
                index: i,
                status: '?',
                untracked: true,
            });
        }
        assert_eq!(dir_status(&files), ('M', false));
    }

    /// Deletions next to additions mean the folder existed too.
    #[test]
    fn directory_with_deletions_and_additions_is_modified() {
        let files = vec![
            make_file("src/gone.rs", 0, 'D'),
            make_file("src/new1.rs", 1, 'A'),
            make_file("src/new2.rs", 2, 'A'),
        ];
        assert_eq!(dir_status(&files), ('M', false));
    }

    /// Only a folder whose every change is an addition reads as created, and
    /// only one whose every change is a deletion reads as deleted.
    #[test]
    fn homogeneous_directories_keep_their_status() {
        let files = vec![make_file("src/a.rs", 0, 'A'), make_file("src/b.rs", 1, 'R')];
        assert_eq!(dir_status(&files), ('A', false));

        let files = vec![
            FileEntry {
                path: PathBuf::from("src/a.rs"),
                index: 0,
                status: '?',
                untracked: true,
            },
            FileEntry {
                path: PathBuf::from("src/b.rs"),
                index: 1,
                status: '?',
                untracked: true,
            },
        ];
        assert_eq!(dir_status(&files), ('?', true));

        let files = vec![make_file("src/a.rs", 0, 'D'), make_file("src/b.rs", 1, 'D')];
        assert_eq!(dir_status(&files), ('D', false));
    }
}
