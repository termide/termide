//! The repositories of a project as the tree they live in.
//!
//! A project is often not a repository but a plane holding many: a folder of
//! checkouts grouped by host and team, `<project>/<host>/<group>/<repo>`. A flat
//! dropdown of folder names then shows `crm` twice and says nothing about which is
//! which, because the list sorts by the last component. These rows name each
//! repository by where it sits in the project and mark the directories between, so
//! the same list reads as the tree on disk.
//!
//! The rows are a *projection*: [`crate::RepoManager`] keeps its flat, sorted list
//! of repositories as the canonical set — the merge and prune of `poll` and the
//! selection by index stay as they are — and each row carries the index of the
//! repository it shows. A heading row carries `None` and is not selectable, so
//! ↑/↓ keep moving over repositories alone.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::get_repo_name;

/// One row of a repository dropdown: a repository, or the directory above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRow {
    /// Text to draw: the folder name of a repository, the segment of a directory.
    pub label: String,
    /// How deep in the project the row sits; 0 is directly under its root.
    pub depth: usize,
    /// Index into the flat list of repositories, or `None` for a directory row.
    pub repo: Option<usize>,
    /// The row's path inside the project, `/`-separated: what a collapsed
    /// directory is remembered by, and how a repository finds the directories
    /// above it. Empty for a row that has no place in the tree.
    pub key: String,
    /// For a directory: whether the rows under it are shown.
    pub expanded: bool,
}

impl RepoRow {
    /// A selectable row: the repository at `index` in the flat list.
    pub fn repo(label: impl Into<String>, depth: usize, index: usize) -> Self {
        Self {
            label: label.into(),
            depth,
            repo: Some(index),
            key: String::new(),
            expanded: true,
        }
    }

    /// A non-selectable row naming a directory the repositories below it sit in.
    pub fn heading(label: impl Into<String>, depth: usize) -> Self {
        Self {
            label: label.into(),
            depth,
            repo: None,
            key: String::new(),
            expanded: true,
        }
    }

    /// The same row, placed at `key` in the project.
    pub fn at(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// Whether the row takes the selection.
    pub fn is_selectable(&self) -> bool {
        self.repo.is_some()
    }

    /// Whether `self` is a directory that holds `other` somewhere below it.
    fn holds(&self, other: &RepoRow) -> bool {
        !self.is_selectable()
            && !self.key.is_empty()
            && other
                .key
                .strip_prefix(self.key.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// The folders a user closed in a repository tree, and the tree as it then
/// shows. Each panel keeps one for as long as it lives; nothing is saved.
#[derive(Debug, Default, Clone)]
pub struct RepoFolds {
    collapsed: HashSet<String>,
}

impl RepoFolds {
    /// The tree of `repos` in `project_root` as [`repo_tree`] builds it, the
    /// contents of the closed folders left out.
    pub fn rows(&self, repos: &[PathBuf], project_root: Option<&Path>) -> Vec<RepoRow> {
        collapse(repo_tree(repos, project_root), &self.collapsed)
    }

    /// Open the folders above the repository at `index` and return the row it
    /// then sits at: where a list opened on it puts its cursor.
    pub fn reveal(
        &mut self,
        repos: &[PathBuf],
        project_root: Option<&Path>,
        index: usize,
    ) -> usize {
        reveal(&mut self.collapsed, &repo_tree(repos, project_root), index);
        self.rows(repos, project_root)
            .iter()
            .position(|row| row.repo == Some(index))
            .unwrap_or(0)
    }

    /// Open or close the folder at `row` of `rows`, as this shows them; `open`
    /// says which, `None` turns it over. `false` when the row is no folder, or
    /// is already as asked.
    pub fn set_open(&mut self, rows: &[RepoRow], row: usize, open: Option<bool>) -> bool {
        let Some(dir) = rows.get(row) else {
            return false;
        };
        if dir.is_selectable() || open == Some(dir.expanded) {
            return false;
        }
        if dir.expanded {
            self.collapsed.insert(dir.key.clone());
        } else {
            self.collapsed.remove(&dir.key);
        }
        true
    }
}

/// The rows as a collapsible tree shows them: a directory in `collapsed` is
/// marked closed and the rows under it are left out.
fn collapse(rows: Vec<RepoRow>, collapsed: &HashSet<String>) -> Vec<RepoRow> {
    let mut shown = Vec::with_capacity(rows.len());
    // The depth of the closed directory whose subtree is being skipped.
    let mut hidden_below: Option<usize> = None;
    for mut row in rows {
        if let Some(depth) = hidden_below {
            if row.depth > depth {
                continue;
            }
            hidden_below = None;
        }
        if !row.is_selectable() && collapsed.contains(&row.key) {
            row.expanded = false;
            hidden_below = Some(row.depth);
        }
        shown.push(row);
    }
    shown
}

/// Open every directory above the repository at `index`, so a list opened on it
/// shows it instead of the closed folder it sits in.
fn reveal(collapsed: &mut HashSet<String>, rows: &[RepoRow], index: usize) {
    if let Some(repo) = rows.iter().find(|row| row.repo == Some(index)) {
        rows.iter().filter(|row| row.holds(repo)).for_each(|dir| {
            collapsed.remove(&dir.key);
        });
    }
}

/// The connector drawn before each row — `├─ `, `└─ `, `│  ` — the same lines the
/// file tree of the Git status panel draws. A row at depth 0 has none.
pub fn tree_prefixes(rows: &[RepoRow]) -> Vec<String> {
    let max_depth = rows.iter().map(|row| row.depth).max().unwrap_or(0);
    // Whether a later row sits at that depth before a shallower one cuts it off.
    let mut has_next = vec![false; max_depth + 1];
    let mut prefixes = Vec::with_capacity(rows.len());
    for row in rows.iter().rev() {
        let depth = row.depth;
        if depth == 0 {
            has_next.fill(false);
            has_next[0] = true;
            prefixes.push(String::new());
            continue;
        }
        let mut prefix = String::with_capacity(depth * 3);
        for (level, next) in has_next.iter().enumerate().take(depth + 1).skip(1) {
            prefix.push_str(match (level == depth, *next) {
                (true, true) => "├─ ",
                (true, false) => "└─ ",
                (false, true) => "│  ",
                (false, false) => "   ",
            });
        }
        prefixes.push(prefix);
        for next in &mut has_next[depth + 1..] {
            *next = false;
        }
        has_next[depth] = true;
    }
    prefixes.reverse();
    prefixes
}

/// The path of `repo` inside `project_root`, as the segments it is nested in.
///
/// `None` when the repository is not inside the project — a panel navigated
/// elsewhere keeps its repository, it simply has no place in this tree.
pub fn nested_segments(repo: &Path, project_root: &Path) -> Option<Vec<String>> {
    let rest = repo.strip_prefix(project_root).ok()?;
    let segments: Vec<String> = rest
        .components()
        .filter_map(|c| match c {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    (!segments.is_empty()).then_some(segments)
}

/// The path of `repo` inside `project_root`, canonicalising both sides first: the
/// panels hand over paths as they came from their own sources, and on macOS `/var`
/// is a symlink to `/private/var` — unnormalised, the strip would fail and every
/// repository would land outside the tree.
fn inside_project(repo: &Path, project_root: &Path) -> Option<Vec<String>> {
    let canonical_root =
        dunce::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let canonical_repo = dunce::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
    nested_segments(&canonical_repo, &canonical_root)
        // A repository whose `.git` vanished mid-walk still has its row.
        .or_else(|| nested_segments(repo, project_root))
}

/// The name a repository shows outside the tree: its path inside the project, so
/// two checkouts named `crm` on different hosts are told apart in the closed
/// selector too. The folder name when the repository is not inside the project,
/// and the folder name of the project itself when it *is* the repository.
pub fn relative_name(repo: &Path, project_root: Option<&Path>) -> String {
    let name = get_repo_name(repo);
    match project_root.and_then(|root| inside_project(repo, root)) {
        Some(segments) if segments.len() > 1 => segments.join("/"),
        _ => name,
    }
}

/// The repositories of a project as dropdown rows: directories as headings, each
/// repository under them, indented one step deeper than its parent, in the order
/// `find` would list them.
///
/// Flat when there is nothing to group — every repository a direct child of the
/// project root, or no project root at all — because headings would then add rows
/// without adding information.
pub fn repo_tree(repos: &[PathBuf], project_root: Option<&Path>) -> Vec<RepoRow> {
    let pairs: Vec<(usize, PathBuf)> = repos.iter().cloned().enumerate().collect();
    // The tree is read top to bottom, so it is sorted by where each repository
    // sits, not by the panel's order: a group's repositories stay together.
    let mut nested: Vec<(usize, Vec<String>)> = pairs
        .iter()
        .filter_map(|(index, repo)| {
            project_root
                .and_then(|root| inside_project(repo, root))
                .map(|segments| (*index, segments))
        })
        .collect();
    nested.sort_by(|a, b| a.1.cmp(&b.1));
    tree_rows(&nested, &pairs)
}

/// [`repo_tree`] for repositories handed over with the index they are chosen by,
/// in the order they should appear: a filtered list passes its repositories in
/// score order, and the tree keeps it — the group holding the best match comes
/// first, its repositories in their own order. Each row still carries the index
/// into the panel's full list, so what the user picks is the repository it names.
pub fn repo_tree_indexed(repos: &[(usize, PathBuf)], project_root: Option<&Path>) -> Vec<RepoRow> {
    let nested: Vec<(usize, Vec<String>)> = repos
        .iter()
        .filter_map(|(index, repo)| {
            project_root
                .and_then(|root| inside_project(repo, root))
                .map(|segments| (*index, segments))
        })
        .collect();
    tree_rows(&nested, repos)
}

/// Build the rows of a tree from repositories nested in the project, in the order
/// given, and put the repositories outside it at the top: they have no place in
/// the tree, but they belong in the list — a panel navigated elsewhere keeps its
/// repository.
fn tree_rows(nested: &[(usize, Vec<String>)], repos: &[(usize, PathBuf)]) -> Vec<RepoRow> {
    // Every repository a direct child of the root: nothing between them to name.
    if nested.iter().all(|(_, segments)| segments.len() == 1) {
        return repos
            .iter()
            .map(|(index, repo)| RepoRow::repo(get_repo_name(repo), 0, *index))
            .collect();
    }

    let mut tree = Node::default();
    for (index, segments) in nested {
        // The last segment is the repository itself; those before it are directories.
        let (name, dirs) = match segments.split_last() {
            Some((name, dirs)) => (name.clone(), dirs.to_vec()),
            None => continue,
        };
        tree.insert(
            &dirs,
            RepoRow::repo(name, dirs.len(), *index).at(segments.join("/")),
        );
    }

    let mut rows = Vec::new();
    tree.write("", 0, &mut rows);
    let placed: std::collections::HashSet<usize> = rows.iter().filter_map(|r| r.repo).collect();
    let orphans = repos
        .iter()
        .filter(|(index, _)| !placed.contains(index))
        .map(|(index, repo)| RepoRow::repo(get_repo_name(repo), 0, *index));
    orphans.chain(rows).collect()
}

/// Rows in which every line is a selectable repository: the flat list, and what a
/// dropdown of anything that is not a tree passes to the same renderer.
pub fn flat_rows(labels: &[String]) -> Vec<RepoRow> {
    labels
        .iter()
        .enumerate()
        .map(|(index, label)| RepoRow::repo(label.clone(), 0, index))
        .collect()
}

/// A directory of the tree: its subdirectories, then the repositories in it, each
/// kept in the order it was first met.
#[derive(Default)]
struct Node {
    dirs: Vec<(String, Node)>,
    repos: Vec<RepoRow>,
}

impl Node {
    fn insert(&mut self, dirs: &[String], repo: RepoRow) {
        match dirs.first() {
            Some(dir) => {
                let slot = match self.dirs.iter_mut().position(|(name, _)| name == dir) {
                    Some(at) => &mut self.dirs[at].1,
                    None => {
                        self.dirs.push((dir.clone(), Node::default()));
                        &mut self.dirs.last_mut().unwrap().1
                    }
                };
                slot.insert(&dirs[1..], repo);
            }
            None => self.repos.push(repo),
        }
    }

    /// The index of the one repository this directory holds, when that is all it
    /// holds and it carries the directory's own name — `github.com/owner/owner`,
    /// the common shape of a host's checkout. Its heading would name twice over
    /// what the row below it says.
    fn single_namesake(&self, name: &str) -> Option<usize> {
        (self.dirs.is_empty()
            && self.repos.len() == 1
            && self.repos[0].label == name
            && self.repos[0].repo.is_some())
        .then(|| self.repos[0].repo.unwrap())
    }

    /// Write the subtree, directories before the repositories beside them: the
    /// order a file manager lists them in.
    fn write(&self, parent: &str, depth: usize, rows: &mut Vec<RepoRow>) {
        for (name, node) in &self.dirs {
            let key = if parent.is_empty() {
                name.clone()
            } else {
                format!("{parent}/{name}")
            };
            match node.single_namesake(name) {
                // No heading; the repository takes the place of the folder that
                // adds nothing, at the folder's own depth, so the lines of the
                // tree run straight through it.
                Some(index) => {
                    rows.push(RepoRow::repo(name, depth, index).at(format!("{key}/{name}")))
                }
                None => {
                    rows.push(RepoRow::heading(name, depth).at(key.clone()));
                    node.write(&key, depth + 1, rows);
                }
            }
        }
        rows.extend(self.repos.iter().cloned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("termide-tree-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        dunce::canonicalize(&path).unwrap_or(path)
    }

    /// The shape of a tree: each row's depth, and its label when it is a heading.
    fn shape(rows: &[RepoRow]) -> Vec<(Option<&str>, usize)> {
        rows.iter()
            .map(|r| ((!r.is_selectable()).then_some(r.label.as_str()), r.depth))
            .collect()
    }

    // The layout a flat list of folder names could not read: repositories grouped by
    // host and team, two of them sharing a name.
    #[test]
    fn a_grouped_project_reads_as_a_tree() {
        let root = root("grouped");
        let repos: Vec<PathBuf> = [
            "git.example.com/devops/tardis",
            "git.example.com/sigma/crm",
            "github.com/termide/termide",
        ]
        .iter()
        .map(|rel| root.join(rel))
        .collect();

        let rows = repo_tree(&repos, Some(&root));
        let tree: Vec<(bool, &str, usize)> = rows
            .iter()
            .map(|r| (r.is_selectable(), r.label.as_str(), r.depth))
            .collect();
        assert_eq!(
            tree,
            vec![
                (false, "git.example.com", 0),
                (false, "devops", 1),
                (true, "tardis", 2),
                (false, "sigma", 1),
                (true, "crm", 2),
                (false, "github.com", 0),
                // `github.com/termide/termide`: the folder named like its repository
                // writes no heading — it would name twice over what the row below it
                // says — and the repository takes the folder's place in the tree.
                (true, "termide", 1),
            ],
            "{tree:?}"
        );
    }

    // Each row points at the repository it shows, in the order the panel's flat list
    // has them; a heading points at nothing.
    #[test]
    fn rows_carry_the_index_of_the_repository_they_show() {
        let root = root("indices");
        let repos: Vec<PathBuf> = ["a/repo-a", "b/repo-b", "repo-c"]
            .iter()
            .map(|rel| root.join(rel))
            .collect();

        let rows = repo_tree(&repos, Some(&root));
        let mut picked: Vec<(usize, &str)> = rows
            .iter()
            .filter_map(|r| r.repo.map(|i| (i, r.label.as_str())))
            .collect();
        picked.sort();
        assert_eq!(
            picked,
            vec![(0, "repo-a"), (1, "repo-b"), (2, "repo-c")],
            "a repository was placed at the wrong index or left out"
        );
        assert!(
            rows.iter().any(|r| !r.is_selectable()),
            "the tree has no headings: {rows:?}"
        );
    }

    // A filtered list keeps the order it was handed — best match first — while each
    // row still names its index into the panel's full list.
    #[test]
    fn a_filtered_list_keeps_its_order_and_its_indices() {
        let root = root("filtered");
        let crm = root.join("git.example.com/sigma/crm");
        let other = root.join("github.com/other/crm");

        // Score order: `other` first, though it is the second repository.
        let rows = repo_tree_indexed(&[(1, other.clone()), (0, crm.clone())], Some(&root));
        let picked: Vec<(usize, &str)> = rows
            .iter()
            .filter_map(|r| r.repo.map(|i| (i, r.label.as_str())))
            .collect();
        assert_eq!(picked, vec![(1, "crm"), (0, "crm")], "{rows:?}");
        assert_eq!(
            rows.iter()
                .filter(|r| !r.is_selectable())
                .map(|r| r.label.as_str())
                .collect::<Vec<_>>(),
            ["github.com", "other", "git.example.com", "sigma"],
            "each match keeps the headings above it: {rows:?}"
        );
    }

    // A host's checkout of a project named after its owner names that folder twice
    // over: the heading between them is dropped, the repository stays where it is.
    #[test]
    fn a_folder_named_like_the_repository_inside_it_writes_no_heading() {
        let root = root("same-name");
        let repos = vec![
            root.join("git.example.com/devops/tardis"),
            root.join("github.com/termide/termide"),
        ];

        let rows = repo_tree(&repos, Some(&root));
        let headings: Vec<&str> = rows
            .iter()
            .filter(|r| !r.is_selectable())
            .map(|r| r.label.as_str())
            .collect();
        assert_eq!(
            headings,
            ["git.example.com", "devops", "github.com"],
            "the host and team headings stay; only the redundant one goes: {rows:?}"
        );
        assert_eq!(
            rows.iter()
                .filter(|r| r.is_selectable() && r.label == "termide")
                .map(|r| r.depth)
                .collect::<Vec<_>>(),
            vec![1],
            "the repository did not take the place of its folder: {rows:?}"
        );
    }

    // Every repository a direct child of the project: headings would be pure noise.
    #[test]
    fn a_flat_project_stays_flat() {
        let root = root("flat");
        let repos: Vec<PathBuf> = ["alpha", "beta", "gamma"]
            .iter()
            .map(|rel| root.join(rel))
            .collect();

        let rows = repo_tree(&repos, Some(&root));
        assert_eq!(shape(&rows), vec![(None, 0); 3]);
        assert_eq!(
            rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
            ["alpha", "beta", "gamma"]
        );
    }

    // Without a project root there is no plane to nest in: the flat list stands.
    #[test]
    fn without_a_project_root_the_list_stays_flat() {
        let labels: Vec<String> = vec!["one".into(), "two".into()];
        let rows = flat_rows(&labels);
        assert_eq!(shape(&rows), vec![(None, 0), (None, 0)]);
        assert_eq!(rows[1].repo, Some(1));
    }

    // A repository the user reached outside the project keeps its row, at the top
    // of the tree and selectable — it is the panel's repository.
    #[test]
    fn a_repository_outside_the_project_keeps_its_row() {
        let root = root("outside");
        let outside = dunce::canonicalize(std::env::temp_dir()).unwrap();
        let repos = vec![outside.clone(), root.join("group/repo-a")];

        let rows = repo_tree(&repos, Some(&root));
        assert_eq!(rows.first().and_then(|r| r.repo), Some(0));
        assert_eq!(rows.first().map(|r| r.depth), Some(0));
        assert_eq!(
            rows.iter().filter_map(|r| r.repo).collect::<Vec<_>>(),
            vec![0, 1],
            "{rows:?}"
        );
    }

    #[test]
    fn relative_names_tell_apart_two_repositories_of_the_same_name() {
        let root = root("names");
        let crm = root.join("git.example.com/sigma/crm");
        assert_eq!(
            relative_name(&crm, Some(&root)),
            "git.example.com/sigma/crm",
            "the closed selector shows no more than the folder name"
        );
        // A repository directly in the project needs no path before its name.
        assert_eq!(relative_name(&root.join("crm"), Some(&root)), "crm");
        // The project that is its own repository has no path inside itself.
        assert_eq!(relative_name(&root, Some(&root)), get_repo_name(&root));
        // And one outside the project is named by its folder alone.
        assert_eq!(
            relative_name(Path::new("/elsewhere/repo"), Some(&root)),
            "repo"
        );
    }

    fn grouped(root: &Path) -> Vec<RepoRow> {
        let repos: Vec<PathBuf> = [
            "git.example.com/devops/multica",
            "git.example.com/devops/tardis",
            "git.example.com/sigma/crm",
            "github.com/termide/termide",
        ]
        .iter()
        .map(|rel| root.join(rel))
        .collect();
        repo_tree(&repos, Some(root))
    }

    // The lines are the ones the file tree of the status panel draws: a branch for
    // a row with a sibling below, a corner for the last, a bar through the levels
    // that still go on.
    #[test]
    fn the_tree_is_drawn_with_the_lines_of_the_file_tree() {
        let rows = grouped(&root("lines"));
        let drawn: Vec<String> = tree_prefixes(&rows)
            .iter()
            .zip(&rows)
            .map(|(prefix, row)| format!("{prefix}{}", row.label))
            .collect();
        assert_eq!(
            drawn,
            [
                "git.example.com",
                "├─ devops",
                "│  ├─ multica",
                "│  └─ tardis",
                "└─ sigma",
                "   └─ crm",
                "github.com",
                "└─ termide",
            ]
        );
    }

    // A closed directory keeps its row, marked closed, and hides everything under
    // it; its siblings and the rest of the tree stay.
    #[test]
    fn a_collapsed_directory_hides_what_is_under_it() {
        let rows = grouped(&root("collapse"));
        let collapsed: HashSet<String> = ["git.example.com/devops".to_string()].into();
        let shown = collapse(rows, &collapsed);
        let labels: Vec<(&str, bool)> = shown
            .iter()
            .map(|r| (r.label.as_str(), r.expanded))
            .collect();
        assert_eq!(
            labels,
            [
                ("git.example.com", true),
                ("devops", false),
                ("sigma", true),
                ("crm", true),
                ("github.com", true),
                ("termide", true),
            ]
        );
    }

    // Opening the list on a repository inside closed folders opens those folders,
    // and those alone.
    #[test]
    fn revealing_a_repository_opens_the_folders_above_it() {
        let rows = grouped(&root("reveal"));
        let crm = rows
            .iter()
            .find(|r| r.label == "crm")
            .and_then(|r| r.repo)
            .unwrap();
        let mut collapsed: HashSet<String> = [
            "git.example.com".to_string(),
            "git.example.com/sigma".to_string(),
            "git.example.com/devops".to_string(),
        ]
        .into();
        reveal(&mut collapsed, &rows, crm);
        assert_eq!(
            collapsed,
            ["git.example.com/devops".to_string()].into(),
            "only the folders holding the repository open"
        );
    }

    #[test]
    fn nested_segments_are_the_directories_between_root_and_repo() {
        let root = root("segments");
        assert_eq!(
            nested_segments(&root.join("a/b/c"), &root),
            Some(vec!["a".to_string(), "b".to_string(), "c".to_string()])
        );
        assert_eq!(nested_segments(&root, &root), None);
        assert_eq!(nested_segments(Path::new("/other/repo"), &root), None);
    }
}
