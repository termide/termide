//! Build the indented result tree (nodes + box-drawing prefixes) from matches.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use termide_git::GitStatus;

use super::{ContentMatch, ResultTreeNode};

pub(super) struct TreeBuildItem<'a> {
    pub(super) relative_path: &'a str,
    pub(super) full_path: &'a Path,
    pub(super) git_status: GitStatus,
    pub(super) is_dir: bool,
    pub(super) content_match: Option<ContentMatch>,
}

pub(super) fn build_tree_nodes(
    items: Vec<TreeBuildItem<'_>>,
) -> (Vec<ResultTreeNode>, Vec<String>) {
    if items.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut nodes: Vec<ResultTreeNode> = Vec::new();
    let mut added_dirs: HashSet<PathBuf> = HashSet::new();

    for item in &items {
        let rel_path = Path::new(item.relative_path);
        let components: Vec<&std::ffi::OsStr> = rel_path.iter().collect();

        // Add ancestor directories
        for depth in 0..components.len().saturating_sub(1) {
            let dir_path: PathBuf = components[..=depth].iter().collect();
            if !added_dirs.contains(&dir_path) {
                added_dirs.insert(dir_path.clone());
                let dir_name = components[depth].to_string_lossy().into_owned();
                nodes.push(ResultTreeNode {
                    name: dir_name,
                    full_path: Path::new(item.full_path)
                        .ancestors()
                        .nth(components.len() - 1 - depth)
                        .unwrap_or(item.full_path)
                        .to_path_buf(),
                    depth,
                    is_dir: true,
                    git_status: GitStatus::Unmodified,
                    content_match: None,
                    is_file_header: false,
                    match_count: 0,
                    collapsed: false,
                });
            }
        }

        // Add the item itself
        let depth = components.len().saturating_sub(1);
        let name = components
            .last()
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_default();

        nodes.push(ResultTreeNode {
            name,
            full_path: item.full_path.to_path_buf(),
            depth,
            is_dir: item.is_dir,
            git_status: item.git_status,
            content_match: item.content_match.clone(),
            is_file_header: false,
            match_count: 0,
            collapsed: false,
        });
    }

    // Sort by path to maintain tree structure
    nodes.sort_by(|a, b| a.full_path.cmp(&b.full_path));

    // Deduplicate dir nodes
    nodes.dedup_by(|a, b| a.is_dir && b.is_dir && a.full_path == b.full_path);

    let prefixes = compute_tree_prefixes(&nodes);
    (nodes, prefixes)
}

fn compute_tree_prefixes(nodes: &[ResultTreeNode]) -> Vec<String> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let max_depth = nodes.iter().map(|n| n.depth).max().unwrap_or(0);
    if max_depth == 0 {
        return vec![String::new(); nodes.len()];
    }

    let mut has_next_at_level = vec![false; max_depth + 1];
    let mut prefixes: Vec<String> = Vec::with_capacity(nodes.len());

    for node in nodes.iter().rev() {
        let depth = node.depth;

        if depth == 0 {
            has_next_at_level.fill(false);
            has_next_at_level[0] = true;
            prefixes.push(String::new());
            continue;
        }

        let mut prefix = String::with_capacity(depth * 3);
        for (lvl, has_next) in has_next_at_level[1..=depth].iter().enumerate() {
            let lvl = lvl + 1;
            if lvl == depth {
                if *has_next {
                    prefix.push_str("├─ ");
                } else {
                    prefix.push_str("└─ ");
                }
            } else if *has_next {
                prefix.push_str("│  ");
            } else {
                prefix.push_str("   ");
            }
        }
        prefixes.push(prefix);

        for val in &mut has_next_at_level[(depth + 1)..] {
            *val = false;
        }
        has_next_at_level[depth] = true;
    }

    prefixes.reverse();
    prefixes
}
