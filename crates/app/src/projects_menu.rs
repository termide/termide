//! Projects menu: the fixed actions, the projects open in this instance,
//! then the known projects as a tree of directory submenus.
//!
//! The tree mirrors the directories that hold projects. Chains of directories
//! with a single child and no project of their own are folded into one row
//! (`github.com/termide`), and the prefix common to every project is dropped,
//! so each level only shows the points where the paths actually branch.
//!
//! The open levels are addressed by `UiState::projects_nested`: entry `k` is
//! the selected row of the submenu opened from the row selected at level `k`
//! (level 0 being the dropdown itself, `UiState::projects_submenu`). Both the
//! renderer and the input handlers resolve rows through [`ProjectsMenuLevel`],
//! so a row index always means the same thing to all of them.

use std::path::{Component, Path, PathBuf, MAIN_SEPARATOR};

use ratatui::layout::Rect;
use termide_ui_render::{
    dropdown_geometry, get_menu_item_x_position, get_projects_items, DropdownItem,
    PROJECTS_MENU_INDEX, PROJECTS_SUBMENU_ITEM_COUNT,
};

use crate::open_projects::OpenProjectView;
use crate::AppState;

/// Marks the current project.
const CURRENT_MARK: &str = "●";
/// Marks a project open in the background.
const OPEN_MARK: &str = "○";
/// Marks a project with a panel that waits for the user.
const ATTENTION_MARK: &str = "🔔";

/// A directory on the way to one or more projects, or a project itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectNode {
    /// Directory name, or several names joined when a chain was folded.
    pub label: String,
    /// Project root when this directory is itself a project.
    pub project: Option<PathBuf>,
    /// Directories below it that lead to further projects.
    pub children: Vec<ProjectNode>,
}

impl ProjectNode {
    fn new(label: String) -> Self {
        Self {
            label,
            project: None,
            children: Vec::new(),
        }
    }

    /// Whether `path` is this project or lies anywhere below this node.
    fn contains(&self, path: &Path) -> bool {
        self.project.as_deref() == Some(path) || self.children.iter().any(|c| c.contains(path))
    }

    /// Every project at or below this node.
    fn collect_projects<'a>(&'a self, out: &mut Vec<&'a Path>) {
        out.extend(self.project.as_deref());
        for child in &self.children {
            child.collect_projects(out);
        }
    }

    /// Rows of the submenu this node opens. A directory that is a project
    /// and also holds projects lists itself first, set off by a separator.
    fn rows(&self) -> Vec<ProjectRow<'_>> {
        let mut rows = Vec::with_capacity(self.children.len() + 2);
        if self.project.is_some() {
            rows.push(ProjectRow::Open(self));
            rows.push(ProjectRow::Separator);
        }
        rows.extend(self.children.iter().map(ProjectRow::Node));
        rows
    }
}

/// One row of a projects menu level.
#[derive(Debug, Clone, Copy)]
pub enum ProjectRow<'a> {
    /// One of the fixed actions (New / Switch / Change root) by index.
    Action(usize),
    Separator,
    /// A directory: opens its submenu, or switches to it when it is a leaf.
    Node(&'a ProjectNode),
    /// The project of a directory whose submenu this row belongs to.
    Open(&'a ProjectNode),
    /// A project open in this instance, in the section above the tree.
    Opened(&'a OpenProjectView),
}

impl<'a> ProjectRow<'a> {
    /// The node whose submenu this row opens, if it opens one.
    pub fn submenu(&self) -> Option<&'a ProjectNode> {
        match self {
            Self::Node(node) if !node.children.is_empty() => Some(node),
            _ => None,
        }
    }

    /// The projects Delete removes on this row: a directory's whole subtree,
    /// or the single project of a leaf or self row.
    pub fn projects(&self) -> Vec<&'a Path> {
        let mut out = Vec::new();
        match self {
            Self::Node(node) => node.collect_projects(&mut out),
            Self::Open(node) => out.extend(node.project.as_deref()),
            // Delete closes an open project instead, see `open_project`.
            Self::Action(_) | Self::Separator | Self::Opened(_) => {}
        }
        out
    }

    /// The project this row switches to, if it switches to one.
    pub fn project(&self) -> Option<&'a Path> {
        match self {
            Self::Node(node) if node.children.is_empty() => node.project.as_deref(),
            Self::Open(node) => node.project.as_deref(),
            Self::Opened(view) => Some(&view.root),
            _ => None,
        }
    }

    /// The open project this row stands for, in the section above the tree.
    pub fn open_project(&self) -> Option<&'a Path> {
        match self {
            Self::Opened(view) => Some(&view.root),
            _ => None,
        }
    }
}

/// What a Projects menu row does, detached from the menu it was read from.
pub enum ProjectsTarget {
    /// One of the fixed actions, by index.
    Action(usize),
    /// A directory that opens a submenu.
    Submenu,
    /// A project to switch to.
    Project(PathBuf),
    /// A separator.
    None,
}

impl ProjectsTarget {
    pub fn of(row: Option<ProjectRow<'_>>) -> Self {
        match row {
            Some(ProjectRow::Action(index)) => Self::Action(index),
            Some(row) if row.submenu().is_some() => Self::Submenu,
            Some(row) => row
                .project()
                .map_or(Self::None, |path| Self::Project(path.to_path_buf())),
            None => Self::None,
        }
    }
}

/// One open level of the projects menu, as drawn and hit-tested.
pub struct ProjectsMenuLevel<'a> {
    pub rows: Vec<ProjectRow<'a>>,
    pub items: Vec<DropdownItem>,
    pub selected: usize,
    /// Requested top-left corner (fitted to the screen when drawn).
    pub x: u16,
    pub y: u16,
}

impl<'a> ProjectsMenuLevel<'a> {
    /// Indices of the rows that cannot be selected.
    pub fn separators(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_separator)
            .map(|(index, _)| index)
            .collect()
    }

    pub fn selected_row(&self) -> Option<ProjectRow<'a>> {
        self.rows.get(self.selected).copied()
    }
}

/// Build the directory tree over `projects`. Paths under `home` are shown
/// relative to `~`, as everywhere else in the UI.
pub fn build_project_tree(projects: &[PathBuf], home: Option<&Path>) -> Vec<ProjectNode> {
    let mut root = ProjectNode::new(String::new());
    for project in projects {
        let mut node = &mut root;
        for segment in path_segments(project, home) {
            let index = match node.children.iter().position(|c| c.label == segment) {
                Some(index) => index,
                None => {
                    node.children.push(ProjectNode::new(segment));
                    node.children.len() - 1
                }
            };
            node = &mut node.children[index];
        }
        node.project = Some(project.clone());
    }

    for child in &mut root.children {
        fold(child);
    }
    sort(&mut root.children);

    // After folding, a lone top node that is not a project branches into
    // several children: the common prefix itself carries no choice.
    match root.children.as_slice() {
        [only] if only.project.is_none() => root.children.pop().unwrap().children,
        _ => root.children,
    }
}

/// Split `path` into the directory names shown in the tree.
fn path_segments(path: &Path, home: Option<&Path>) -> Vec<String> {
    let (mut segments, rest) = match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => (vec!["~".to_string()], rest),
        None => (Vec::new(), path),
    };
    let mut root = String::new();
    for component in rest.components() {
        match component {
            // A Windows drive prefix and the root after it name one place.
            Component::Prefix(prefix) => root.push_str(&prefix.as_os_str().to_string_lossy()),
            Component::RootDir => root.push(MAIN_SEPARATOR),
            Component::Normal(name) => {
                if !root.is_empty() {
                    segments.push(std::mem::take(&mut root));
                }
                segments.push(name.to_string_lossy().into_owned());
            }
            Component::CurDir | Component::ParentDir => {}
        }
    }
    if !root.is_empty() {
        segments.push(root);
    }
    segments
}

/// Join a folded chain of names, without doubling a root's separator.
fn join_label(parent: &str, child: &str) -> String {
    if parent.ends_with(['/', '\\']) {
        format!("{parent}{child}")
    } else {
        format!("{parent}{MAIN_SEPARATOR}{child}")
    }
}

/// Fold every chain of single-child, non-project directories into one node.
fn fold(node: &mut ProjectNode) {
    while node.project.is_none() && node.children.len() == 1 {
        let child = node.children.pop().unwrap();
        node.label = join_label(&node.label, &child.label);
        node.project = child.project;
        node.children = child.children;
    }
    for child in &mut node.children {
        fold(child);
    }
}

fn sort(nodes: &mut [ProjectNode]) {
    nodes.sort_by(|a, b| {
        a.label
            .to_lowercase()
            .cmp(&b.label.to_lowercase())
            .then_with(|| a.label.cmp(&b.label))
    });
    for node in nodes {
        sort(&mut node.children);
    }
}

/// `label` with the marks of the project at `project`, if it is open: ● for
/// the current one, ○ for one open in the background, 🔔 when it waits for
/// the user. Whether it waits is returned too.
fn marked_label(
    label: &str,
    project: Option<&Path>,
    current: &Path,
    open: &[OpenProjectView],
) -> (String, bool) {
    let Some(view) = project.and_then(|p| open.iter().find(|view| view.root == p)) else {
        return (label.to_string(), false);
    };
    let mark = if view.root == current {
        CURRENT_MARK
    } else {
        OPEN_MARK
    };
    let mut marked = format!("{label} {mark}");
    if view.attention {
        marked.push(' ');
        marked.push_str(ATTENTION_MARK);
    }
    (marked, view.attention)
}

/// Dropdown rows for `rows`. The current project, and every directory on the
/// way to it, is drawn bold so the path to it can be followed. Open projects
/// carry their marks, see `marked_label`.
fn level_items(
    rows: &[ProjectRow<'_>],
    current: &Path,
    open: &[OpenProjectView],
) -> Vec<DropdownItem> {
    rows.iter()
        .map(|row| match row {
            ProjectRow::Action(_) => unreachable!("actions are built by get_projects_items"),
            ProjectRow::Opened(_) => unreachable!("open projects are built by open_items"),
            ProjectRow::Separator => DropdownItem::separator(),
            ProjectRow::Node(node) => {
                // A directory that opens a submenu lists its own project
                // inside, where the marks go.
                let project = if node.children.is_empty() {
                    node.project.as_deref()
                } else {
                    None
                };
                let (label, attention) = marked_label(&node.label, project, current, open);
                let mut item = DropdownItem::new(label, String::new());
                if !node.children.is_empty() {
                    item = item.with_submenu();
                }
                if node.contains(current) {
                    item = item.with_project();
                }
                if attention {
                    item = item.with_attention();
                }
                item
            }
            ProjectRow::Open(node) => {
                let (label, attention) =
                    marked_label(&node.label, node.project.as_deref(), current, open);
                let mut item = DropdownItem::new(label, String::new());
                if node.project.as_deref() == Some(current) {
                    item = item.with_project();
                }
                if attention {
                    item = item.with_attention();
                }
                item
            }
        })
        .collect()
}

/// Names for `roots` that tell them apart: as few trailing directory names
/// as make each one unique, the whole path when nothing shorter does.
pub fn short_names(roots: &[&Path]) -> Vec<String> {
    let tails = |root: &Path, count: usize| -> Option<PathBuf> {
        let names: Vec<_> = root
            .components()
            .filter_map(|c| match c {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        (count <= names.len()).then(|| names[names.len() - count..].iter().collect())
    };
    roots
        .iter()
        .map(|root| {
            (1..)
                .map_while(|count| tails(root, count))
                .find(|tail| {
                    roots
                        .iter()
                        .filter(|other| {
                            tails(other, tail.components().count()).as_ref() == Some(tail)
                        })
                        .count()
                        == 1
                })
                .map(|tail| tail.display().to_string())
                .unwrap_or_else(|| {
                    termide_core::util::shorten_home_path(&root.display().to_string())
                })
        })
        .collect()
}

/// Dropdown rows for the open projects, `rows`, in the section above the
/// tree: ● for the current one, ○ for the others, 🔔 for one that waits, and
/// the key that switches to it, when one is bound.
fn open_items(
    rows: &[ProjectRow<'_>],
    current: &Path,
    keybindings: &termide_config::GlobalKeybindings,
) -> Vec<DropdownItem> {
    let views: Vec<&OpenProjectView> = rows
        .iter()
        .filter_map(|row| match row {
            ProjectRow::Opened(view) => Some(*view),
            _ => None,
        })
        .collect();
    let roots: Vec<&Path> = views.iter().map(|view| view.root.as_path()).collect();
    let names = short_names(&roots);
    let goto = keybindings.goto_project();
    views
        .iter()
        .zip(names)
        .enumerate()
        .map(|(index, (view, name))| {
            let mark = if view.root == current {
                CURRENT_MARK
            } else {
                OPEN_MARK
            };
            let mut label = format!("{mark} {name}");
            if view.attention {
                label.push(' ');
                label.push_str(ATTENTION_MARK);
            }
            let shortcut = goto
                .get(index)
                .and_then(|binding| binding.as_ref())
                .map(|binding| binding.display().to_string());
            let mut item = DropdownItem::new(label, String::new()).with_shortcut(shortcut);
            if view.root == current {
                item = item.with_project();
            }
            if view.attention {
                item = item.with_attention();
            }
            item
        })
        .collect()
}

impl AppState {
    /// Every open level of the projects menu, from the dropdown itself to the
    /// deepest open submenu. Each submenu is placed to the right of its
    /// parent, level with the row that opened it.
    pub fn projects_menu_levels(&self, screen: Rect) -> Vec<ProjectsMenuLevel<'_>> {
        let mut rows: Vec<ProjectRow<'_>> = (0..PROJECTS_SUBMENU_ITEM_COUNT)
            .map(ProjectRow::Action)
            .collect();
        let mut items = get_projects_items(Some(&self.config.general.keybindings));
        // With one project open the section would only repeat the tree.
        if self.open_projects.len() > 1 {
            rows.push(ProjectRow::Separator);
            items.push(DropdownItem::separator());
            let open_rows: Vec<_> = self.open_projects.iter().map(ProjectRow::Opened).collect();
            items.extend(open_items(
                &open_rows,
                &self.project_root,
                &self.config.general.keybindings,
            ));
            rows.extend(open_rows);
        }
        if !self.cache.projects.is_empty() {
            rows.push(ProjectRow::Separator);
            items.push(DropdownItem::separator());
            let tree_rows: Vec<_> = self.cache.projects.iter().map(ProjectRow::Node).collect();
            items.extend(level_items(
                &tree_rows,
                &self.project_root,
                &self.open_projects,
            ));
            rows.extend(tree_rows);
        }

        let mut levels = vec![ProjectsMenuLevel {
            rows,
            items,
            selected: self.ui.projects_submenu.selected,
            x: get_menu_item_x_position(PROJECTS_MENU_INDEX),
            y: 1,
        }];
        for &selected in &self.ui.projects_nested {
            let parent = levels.last().unwrap();
            let Some(node) = parent.selected_row().and_then(|row| row.submenu()) else {
                break;
            };
            let geometry =
                dropdown_geometry(&parent.items, parent.selected, parent.x, parent.y, screen);
            let row_y = geometry.area.y + 1 + (parent.selected - geometry.scroll_offset) as u16;
            let rows = node.rows();
            let items = level_items(&rows, &self.project_root, &self.open_projects);
            levels.push(ProjectsMenuLevel {
                rows,
                items,
                selected,
                x: geometry.area.right(),
                y: row_y,
            });
        }
        levels
    }

    /// Select `selection` (the dropdown's row, then one per nested level)
    /// in a freshly loaded tree. A row index past the end of its level falls
    /// back to the last row, and the levels stop where the row no longer
    /// opens a submenu, so a deletion never leaves the cursor nowhere.
    pub fn restore_projects_selection(&mut self, selection: &[usize], screen: Rect) {
        let Some((&first, nested)) = selection.split_first() else {
            return;
        };
        self.ui.projects_nested.clear();
        self.ui.projects_submenu.selected = first;
        self.clamp_deepest_projects_row(screen);
        for &index in nested {
            self.ui.projects_nested.push(index);
            if self.projects_menu_levels(screen).len() <= self.ui.projects_nested.len() {
                self.ui.projects_nested.pop();
                break;
            }
            self.clamp_deepest_projects_row(screen);
        }
    }

    fn clamp_deepest_projects_row(&mut self, screen: Rect) {
        let levels = self.projects_menu_levels(screen);
        let level = levels.last().unwrap();
        let mut index = level.selected.min(level.items.len().saturating_sub(1));
        // Separators are never last in a level, so the row above is selectable.
        if level.items.get(index).is_some_and(|item| item.is_separator) {
            index = index.saturating_sub(1);
        }
        drop(levels);
        match self.ui.projects_nested.last_mut() {
            Some(selected) => *selected = index,
            None => self.ui.projects_submenu.selected = index,
        }
    }

    /// Load the project list the menu shows as a tree.
    pub(crate) fn load_projects_tree(&mut self) {
        let projects: Vec<PathBuf> = termide_project::list_all_projects()
            .unwrap_or_default()
            .into_iter()
            .map(|info| info.project_path)
            .collect();
        self.cache.projects = build_project_tree(&projects, dirs::home_dir().as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// State on the built-in defaults: `AppState::new` would read the
    /// developer's own config file.
    fn test_state() -> AppState {
        let mut config = termide_config::Config::default();
        config.normalize();
        let theme = termide_theme::Theme::get_by_name(&config.general.theme);
        AppState::with_config_and_theme(config.clone(), config, theme)
    }

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn labels(nodes: &[ProjectNode]) -> Vec<&str> {
        nodes.iter().map(|n| n.label.as_str()).collect()
    }

    fn sep() -> char {
        MAIN_SEPARATOR
    }

    #[test]
    fn drops_common_prefix_and_folds_chains() {
        let home = PathBuf::from("/home/u");
        let tree = build_project_tree(
            &paths(&[
                "/home/u/src/github.com/termide/termide",
                "/home/u/src/github.com/termide/termide-gitlog",
                "/home/u/src/work/api/server",
            ]),
            Some(&home),
        );
        assert_eq!(
            labels(&tree),
            vec![
                format!("github.com{}termide", sep()),
                format!("work{0}api{0}server", sep())
            ]
        );
        assert_eq!(labels(&tree[0].children), vec!["termide", "termide-gitlog"]);
        assert_eq!(
            tree[1].project.as_deref(),
            Some(Path::new("/home/u/src/work/api/server"))
        );
        assert!(tree[1].children.is_empty());
    }

    #[test]
    fn single_project_keeps_its_path() {
        let home = PathBuf::from("/home/u");
        let tree = build_project_tree(&paths(&["/home/u/a/b"]), Some(&home));
        assert_eq!(labels(&tree), vec![format!("~{0}a{0}b", sep())]);
    }

    #[test]
    fn project_with_nested_projects_lists_itself_first() {
        let home = PathBuf::from("/home/u");
        let tree = build_project_tree(
            &paths(&["/home/u/p", "/home/u/p/x", "/home/u/p/y", "/home/u/q"]),
            Some(&home),
        );
        assert_eq!(labels(&tree), vec!["p", "q"]);
        let rows = tree[0].rows();
        assert!(matches!(rows[0], ProjectRow::Open(n) if n.label == "p"));
        assert!(matches!(rows[1], ProjectRow::Separator));
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].project(), Some(Path::new("/home/u/p")));
        assert!(rows[2].submenu().is_none());
        // Delete on `p` takes its whole subtree; on its self row, only `p`.
        assert_eq!(
            ProjectRow::Node(&tree[0]).projects(),
            vec![
                Path::new("/home/u/p"),
                Path::new("/home/u/p/x"),
                Path::new("/home/u/p/y")
            ]
        );
        assert_eq!(rows[0].projects(), vec![Path::new("/home/u/p")]);
        assert_eq!(rows[2].project(), Some(Path::new("/home/u/p/x")));
    }

    #[cfg(unix)]
    #[test]
    fn paths_outside_home_start_at_root() {
        let home = PathBuf::from("/home/u");
        let tree = build_project_tree(&paths(&["/home/u/a", "/opt/b"]), Some(&home));
        assert_eq!(labels(&tree), vec!["/opt/b", "~/a"]);
    }

    #[test]
    fn marks_path_to_current_project_bold() {
        let home = PathBuf::from("/home/u");
        let tree = build_project_tree(
            &paths(&["/home/u/g/a/one", "/home/u/g/a/two", "/home/u/g/b"]),
            Some(&home),
        );
        let rows: Vec<_> = tree.iter().map(ProjectRow::Node).collect();
        let items = level_items(&rows, Path::new("/home/u/g/a/two"), &[]);
        assert!(items[0].is_project && items[0].has_submenu);
        assert!(!items[1].is_project && !items[1].has_submenu);
    }

    fn open(root: &str, attention: bool) -> OpenProjectView {
        OpenProjectView {
            root: PathBuf::from(root),
            attention,
        }
    }

    #[test]
    fn short_names_take_as_many_trailing_names_as_tell_projects_apart() {
        let roots = [
            Path::new("/home/u/src/termide"),
            Path::new("/home/u/work/api"),
            Path::new("/home/u/old/api"),
        ];
        assert_eq!(
            short_names(&roots),
            vec![
                "termide".to_string(),
                format!("work{}api", sep()),
                format!("old{}api", sep())
            ]
        );
        // One path is the tail of another: the whole path names it.
        let nested = [Path::new("/api"), Path::new("/x/api")];
        let names = short_names(&nested);
        assert_eq!(names[1], format!("x{}api", sep()));
        assert_ne!(names[0], "api");
    }

    #[test]
    fn open_projects_get_a_section_above_the_tree_from_two_on() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.cache.projects = build_project_tree(&paths(&["/p/one", "/p/two", "/p/three"]), None);
        let screen = Rect::new(0, 0, 120, 40);

        state.open_projects = vec![open("/p/one", false)];
        let levels = state.projects_menu_levels(screen);
        assert!(levels[0]
            .rows
            .iter()
            .all(|row| row.open_project().is_none()));

        state.open_projects = vec![open("/p/one", false), open("/p/two", true)];
        let levels = state.projects_menu_levels(screen);
        let first = PROJECTS_SUBMENU_ITEM_COUNT + 1;
        let (section, tree) = (
            &levels[0].items[first..first + 2],
            &levels[0].items[first + 3..],
        );
        assert!(levels[0].items[first - 1].is_separator && levels[0].items[first + 2].is_separator);
        assert_eq!(section[0].label, "● one");
        assert!(section[0].is_project && !section[0].attention);
        assert_eq!(section[1].label, "○ two 🔔");
        assert!(section[1].attention);
        assert_eq!(
            levels[0].rows[first + 1].open_project(),
            Some(Path::new("/p/two"))
        );
        assert!(
            levels[0].rows[first + 1].projects().is_empty(),
            "Delete closes an open project, it does not delete its layout"
        );

        let labels: Vec<&str> = tree.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, vec!["one ●", "three", "two ○ 🔔"]);
        assert!(tree[2].attention);
    }

    #[test]
    fn the_open_project_section_shows_bound_project_keys() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.open_projects = vec![open("/p/one", false), open("/p/two", false)];
        let mut config = (*state.config).clone();
        config.general.keybindings.goto_project_2 =
            Some(termide_config::KeyBinding::Single("Alt+F2".into()));
        state.config = std::sync::Arc::new(config);

        let levels = state.projects_menu_levels(Rect::new(0, 0, 120, 40));
        let first = PROJECTS_SUBMENU_ITEM_COUNT + 1;
        assert_eq!(levels[0].items[first].shortcut, None);
        assert_eq!(
            levels[0].items[first + 1].shortcut.as_deref(),
            Some("Alt+F2")
        );
    }

    #[test]
    fn nested_levels_open_beside_their_row() {
        let home = PathBuf::from("/home/u");
        let mut state = test_state();
        state.cache.projects = build_project_tree(
            &paths(&["/home/u/g/a/one", "/home/u/g/a/two", "/home/u/g/b"]),
            Some(&home),
        );
        let screen = Rect::new(0, 0, 120, 40);
        let first_tree_row = PROJECTS_SUBMENU_ITEM_COUNT + 1;
        state.ui.projects_submenu.selected = first_tree_row;
        state.ui.projects_nested = vec![1];

        let levels = state.projects_menu_levels(screen);
        assert_eq!(levels.len(), 2);
        assert!(levels[0].items[PROJECTS_SUBMENU_ITEM_COUNT].is_separator);
        let parent = dropdown_geometry(&levels[0].items, first_tree_row, levels[0].x, 1, screen);
        assert_eq!(levels[1].x, parent.area.right());
        assert_eq!(levels[1].y, 1 + 1 + first_tree_row as u16);
        assert_eq!(
            levels[1].selected_row().and_then(|row| row.project()),
            Some(Path::new("/home/u/g/a/two"))
        );

        // A stale level below a row without a submenu is not drawn.
        state.ui.projects_submenu.selected = first_tree_row + 1;
        assert_eq!(state.projects_menu_levels(screen).len(), 1);
    }

    #[test]
    fn restored_selection_falls_back_within_the_new_tree() {
        let home = PathBuf::from("/home/u");
        let mut state = test_state();
        let screen = Rect::new(0, 0, 120, 40);
        let first_tree_row = PROJECTS_SUBMENU_ITEM_COUNT + 1;

        // `p` still holds itself and `x` after `y` was deleted: the cursor
        // that was on `y` moves up to `x`.
        state.cache.projects = build_project_tree(
            &paths(&["/home/u/p", "/home/u/p/x", "/home/u/q"]),
            Some(&home),
        );
        state.restore_projects_selection(&[first_tree_row, 3], screen);
        assert_eq!(state.ui.projects_nested, vec![2]);

        // `p` has no submenu left: the cursor stays on `p` itself.
        state.cache.projects = build_project_tree(&paths(&["/home/u/p", "/home/u/q"]), Some(&home));
        state.restore_projects_selection(&[first_tree_row, 2], screen);
        assert_eq!(state.ui.projects_submenu.selected, first_tree_row);
        assert!(state.ui.projects_nested.is_empty());

        // The last top-level project went: the cursor moves up past the
        // separator to the last action.
        state.cache.projects.clear();
        state.restore_projects_selection(&[first_tree_row], screen);
        assert_eq!(
            state.ui.projects_submenu.selected,
            PROJECTS_SUBMENU_ITEM_COUNT - 1
        );
    }
}
