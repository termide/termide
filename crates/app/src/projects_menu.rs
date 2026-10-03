//! Projects menu: the fixed actions, then every project worked on as one
//! flat list — the projects open in this instance first, then the others,
//! each group sorted by path.
//!
//! The project switcher (`Alt+\`) lists the same projects in the same
//! order, see [`listed_projects`].

use std::path::{Component, Path, PathBuf};

use termide_ui_render::{
    get_menu_item_x_position, get_projects_items, DropdownItem, PROJECTS_MENU_INDEX,
    PROJECTS_SUBMENU_ITEM_COUNT,
};

use crate::open_projects::OpenProjectView;
use crate::AppState;

/// Marks the current project.
pub const CURRENT_MARK: &str = "●";
/// Marks a project open in the background.
pub const OPEN_MARK: &str = "○";
/// Marks a project with a panel that waits for the user.
pub const ATTENTION_MARK: &str = "🔔";

/// A project as the Projects menu and the project switcher list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedProject {
    pub root: PathBuf,
    /// Open in this instance (the current project is).
    pub open: bool,
    /// A panel of this project, open in the background, waits for the user.
    pub attention: bool,
}

/// How projects are ordered in a list: by path, ignoring case first.
pub fn path_order(a: &Path, b: &Path) -> std::cmp::Ordering {
    let (a, b) = (a.to_string_lossy(), b.to_string_lossy());
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(&b))
}

/// The projects to list: the `open` ones, then the `known` ones not open,
/// each group sorted by path.
pub fn listed_projects(open: &[OpenProjectView], known: &[PathBuf]) -> Vec<ListedProject> {
    let mut listed: Vec<ListedProject> = open
        .iter()
        .map(|view| ListedProject {
            root: view.root.clone(),
            open: true,
            attention: view.attention,
        })
        .collect();
    listed.sort_by(|a, b| path_order(&a.root, &b.root));
    let mut others: Vec<ListedProject> = known
        .iter()
        .filter(|root| !open.iter().any(|view| &view.root == *root))
        .map(|root| ListedProject {
            root: root.clone(),
            open: false,
            attention: false,
        })
        .collect();
    others.sort_by(|a, b| path_order(&a.root, &b.root));
    listed.extend(others);
    listed
}

/// The mark in front of a project: ● for the current one, ○ for another
/// open one, blank for the rest.
pub fn project_mark(project: &ListedProject, current: &Path) -> &'static str {
    if project.root == current {
        CURRENT_MARK
    } else if project.open {
        OPEN_MARK
    } else {
        " "
    }
}

/// One row of the Projects menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRow {
    /// One of the fixed actions (New / Switch / Change root) by index.
    Action(usize),
    Separator,
    Project(ListedProject),
}

/// What a Projects menu row does, detached from the menu it was read from.
pub enum ProjectsTarget {
    /// One of the fixed actions, by index.
    Action(usize),
    /// A project to switch to.
    Project(PathBuf),
    /// A separator.
    None,
}

impl ProjectsTarget {
    pub fn of(row: Option<&ProjectRow>) -> Self {
        match row {
            Some(ProjectRow::Action(index)) => Self::Action(*index),
            Some(ProjectRow::Project(project)) => Self::Project(project.root.clone()),
            Some(ProjectRow::Separator) | None => Self::None,
        }
    }
}

/// The Projects menu as drawn and hit-tested.
pub struct ProjectsMenu {
    pub rows: Vec<ProjectRow>,
    pub items: Vec<DropdownItem>,
    pub selected: usize,
    /// Requested top-left corner (fitted to the screen when drawn).
    pub x: u16,
    pub y: u16,
}

impl ProjectsMenu {
    /// Indices of the rows that cannot be selected.
    pub fn separators(&self) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_separator)
            .map(|(index, _)| index)
            .collect()
    }

    pub fn selected_row(&self) -> Option<&ProjectRow> {
        self.rows.get(self.selected)
    }
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

/// Dropdown rows for `projects`: the mark, a short name and 🔔 for one that
/// waits; an open project shows the key that switches to it, when one is
/// bound.
fn project_items(
    projects: &[ListedProject],
    current: &Path,
    keybindings: &termide_config::GlobalKeybindings,
) -> Vec<DropdownItem> {
    let roots: Vec<&Path> = projects.iter().map(|p| p.root.as_path()).collect();
    let goto = keybindings.goto_project();
    projects
        .iter()
        .zip(short_names(&roots))
        .enumerate()
        .map(|(index, (project, name))| {
            let mut label = format!("{} {name}", project_mark(project, current));
            if project.attention {
                label.push(' ');
                label.push_str(ATTENTION_MARK);
            }
            // Open projects come first, so their index is their number.
            let shortcut = if project.open {
                goto.get(index).and_then(|binding| binding.as_ref())
            } else {
                None
            }
            .map(|binding| binding.display().to_string());
            let mut item = DropdownItem::new(label, String::new()).with_shortcut(shortcut);
            if project.root == current {
                item = item.with_project();
            }
            if project.attention {
                item = item.with_attention();
            }
            item
        })
        .collect()
}

impl AppState {
    /// The Projects menu: the actions, the open projects, then the others.
    pub fn projects_menu(&self) -> ProjectsMenu {
        let mut rows: Vec<ProjectRow> = (0..PROJECTS_SUBMENU_ITEM_COUNT)
            .map(ProjectRow::Action)
            .collect();
        let mut items = get_projects_items(Some(&self.config.general.keybindings));

        let projects = listed_projects(&self.open_projects, &self.cache.projects);
        let open_count = projects.iter().filter(|p| p.open).count();
        let project_items = project_items(
            &projects,
            &self.project_root,
            &self.config.general.keybindings,
        );
        for (index, (project, item)) in projects.into_iter().zip(project_items).enumerate() {
            if index == 0 || index == open_count {
                rows.push(ProjectRow::Separator);
                items.push(DropdownItem::separator());
            }
            rows.push(ProjectRow::Project(project));
            items.push(item);
        }

        ProjectsMenu {
            rows,
            items,
            selected: self.ui.projects_submenu.selected,
            x: get_menu_item_x_position(PROJECTS_MENU_INDEX),
            y: 1,
        }
    }

    /// Select row `selection` of a freshly loaded menu. A row past the end
    /// falls back to the last one, and a separator to the row above it, so
    /// a deletion never leaves the cursor nowhere.
    pub fn restore_projects_selection(&mut self, selection: usize) {
        let menu = self.projects_menu();
        let mut index = selection.min(menu.items.len().saturating_sub(1));
        // Separators are never last, so the row above is selectable.
        if menu.items.get(index).is_some_and(|item| item.is_separator) {
            index = index.saturating_sub(1);
        }
        self.ui.projects_submenu.selected = index;
    }

    /// Load the projects worked on.
    pub(crate) fn load_known_projects(&mut self) {
        self.cache.projects = termide_project::list_all_projects()
            .unwrap_or_default()
            .into_iter()
            .map(|info| info.project_path)
            .collect();
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

    fn sep() -> char {
        std::path::MAIN_SEPARATOR
    }

    fn open(root: &str, attention: bool) -> OpenProjectView {
        OpenProjectView {
            root: PathBuf::from(root),
            attention,
        }
    }

    fn labels(menu: &ProjectsMenu) -> Vec<&str> {
        menu.items[PROJECTS_SUBMENU_ITEM_COUNT..]
            .iter()
            .map(|item| {
                if item.is_separator {
                    "---"
                } else {
                    item.label.as_str()
                }
            })
            .collect()
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
    fn open_projects_come_first_then_the_rest_each_sorted_by_path() {
        let listed = listed_projects(
            &[open("/p/b", false), open("/p/a", true)],
            &paths(&["/p/D", "/p/a", "/p/c"]),
        );
        let roots: Vec<_> = listed.iter().map(|p| p.root.to_str().unwrap()).collect();
        assert_eq!(roots, vec!["/p/a", "/p/b", "/p/c", "/p/D"]);
        assert!(listed[0].open && listed[0].attention);
        assert!(!listed[2].open);
    }

    #[test]
    fn the_menu_lists_open_projects_then_the_others_below_a_separator() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.open_projects = vec![open("/p/one", false), open("/p/two", true)];
        state.cache.projects = paths(&["/p/three", "/p/one", "/p/two", "/p/four"]);

        let menu = state.projects_menu();
        assert_eq!(
            labels(&menu),
            vec!["---", "● one", "○ two 🔔", "---", "  four", "  three"]
        );
        let first = PROJECTS_SUBMENU_ITEM_COUNT + 1;
        assert!(menu.items[first].is_project && !menu.items[first].attention);
        assert!(menu.items[first + 1].attention);
        assert!(matches!(
            ProjectsTarget::of(menu.rows.get(first + 3)),
            ProjectsTarget::Project(root) if root == Path::new("/p/four")
        ));

        // With only the current project open there is no second group.
        state.open_projects = vec![open("/p/one", false)];
        state.cache.projects = paths(&["/p/one"]);
        assert_eq!(labels(&state.projects_menu()), vec!["---", "● one"]);
    }

    #[test]
    fn open_projects_show_their_bound_keys() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.open_projects = vec![open("/p/one", false), open("/p/two", false)];
        state.cache.projects = paths(&["/p/three"]);
        let mut config = (*state.config).clone();
        config.general.keybindings.goto_project_2 =
            Some(termide_config::KeyBinding::Single("Alt+F2".into()));
        config.general.keybindings.goto_project_3 =
            Some(termide_config::KeyBinding::Single("Alt+F3".into()));
        state.config = std::sync::Arc::new(config);

        let menu = state.projects_menu();
        let first = PROJECTS_SUBMENU_ITEM_COUNT + 1;
        assert_eq!(menu.items[first].shortcut, None);
        assert_eq!(menu.items[first + 1].shortcut.as_deref(), Some("Alt+F2"));
        assert_eq!(
            menu.items[first + 3].shortcut,
            None,
            "a project that is not open has no number"
        );
    }

    #[test]
    fn restored_selection_falls_back_within_the_new_list() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.open_projects = vec![open("/p/one", false)];
        let first = PROJECTS_SUBMENU_ITEM_COUNT + 1;

        // The last project went: the cursor moves to the one above.
        state.cache.projects = paths(&["/p/one", "/p/two"]);
        state.restore_projects_selection(first + 3);
        assert_eq!(state.ui.projects_submenu.selected, first + 2);

        // Only the current project is left; past the end falls back to it.
        state.cache.projects = paths(&["/p/one"]);
        state.restore_projects_selection(first + 2);
        assert_eq!(state.ui.projects_submenu.selected, first);

        // No projects at all: the cursor moves up past the separator to the
        // last action.
        state.open_projects.clear();
        state.cache.projects.clear();
        state.restore_projects_selection(first);
        assert_eq!(
            state.ui.projects_submenu.selected,
            PROJECTS_SUBMENU_ITEM_COUNT - 1
        );
    }
}
