//! Projects menu: the fixed actions, then every project worked on as one
//! flat list — the projects open in this instance first, in the order the
//! user keeps them, then the others, the most recently used first.
//!
//! The project switcher (`Alt+\`) lists the same projects in the same
//! order, see [`listed_projects`].

use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use termide_ui_render::{
    get_menu_item_x_position, get_projects_items, DropdownItem, ProjectButton, PROJECTS_MENU_INDEX,
    PROJECTS_SUBMENU_ITEM_COUNT,
};

use crate::open_projects::OpenProjectView;
use crate::AppState;

pub use termide_modal::projects::{CURRENT_MARK, OPEN_MARK};

/// A project as the Projects menu and the project switcher list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedProject {
    pub root: PathBuf,
    /// Open in this instance (the current project is).
    pub open: bool,
    /// A panel of this project, open in the background, waits for the user.
    pub attention: bool,
    /// When a project not open was last worked on. An open project's layout
    /// is saved all the time, so its time would always read "now": `None`.
    pub modified: Option<SystemTime>,
}

/// The projects to list: the `open` ones in their order — the user's, and
/// it numbers them for `goto_project_N` — then the `known` ones not open
/// (root and last use), the most recently used first.
pub fn listed_projects(
    open: &[OpenProjectView],
    known: &[(PathBuf, SystemTime)],
) -> Vec<ListedProject> {
    let mut listed: Vec<ListedProject> = open
        .iter()
        .map(|view| ListedProject {
            root: view.root.clone(),
            open: true,
            attention: view.attention,
            modified: None,
        })
        .collect();
    let mut others: Vec<ListedProject> = known
        .iter()
        .filter(|(root, _)| !open.iter().any(|view| same_project(&view.root, root)))
        .map(|(root, modified)| ListedProject {
            root: root.clone(),
            open: false,
            attention: false,
            modified: Some(*modified),
        })
        .collect();
    others.sort_by_key(|project| std::cmp::Reverse(project.modified));
    listed.extend(others);
    listed
}

/// Whether `a` and `b` name the same project. The list of projects worked
/// on is rebuilt from where their layouts are stored, which keeps no drive
/// prefix on Windows, so only what follows it is compared.
fn same_project(a: &Path, b: &Path) -> bool {
    fn tail(path: &Path) -> Vec<Component<'_>> {
        path.components()
            .filter(|c| !matches!(c, Component::Prefix(_) | Component::RootDir))
            .collect()
    }
    tail(a) == tail(b)
}

/// The projects worked on and when, from their saved layouts.
pub fn known_projects() -> Vec<(PathBuf, SystemTime)> {
    termide_project::list_all_projects()
        .unwrap_or_default()
        .into_iter()
        .map(|info| (info.project_path, info.modified))
        .collect()
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
    /// Reopen the projects open together in the last run.
    Reopen,
    Separator,
    Project(ListedProject),
}

/// What a Projects menu row does, detached from the menu it was read from.
pub enum ProjectsTarget {
    /// One of the fixed actions, by index.
    Action(usize),
    /// A project to switch to.
    Project(PathBuf),
    /// Reopen the projects of the last run.
    Reopen,
    /// A separator.
    None,
}

impl ProjectsTarget {
    pub fn of(row: Option<&ProjectRow>) -> Self {
        match row {
            Some(ProjectRow::Action(index)) => Self::Action(*index),
            Some(ProjectRow::Project(project)) => Self::Project(project.root.clone()),
            Some(ProjectRow::Reopen) => Self::Reopen,
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

/// Dropdown rows for `projects`, as the project switcher shows them: the
/// mark, the dimmed time a project not open was last worked on, the path —
/// losing its start when too long — and 🔔 for one that waits. An open
/// project shows the key that switches to it, when one is bound.
fn project_items(
    projects: &[ListedProject],
    current: &Path,
    keybindings: &termide_config::GlobalKeybindings,
) -> Vec<DropdownItem> {
    let goto = keybindings.goto_project();
    projects
        .iter()
        .enumerate()
        .map(|(index, project)| {
            let mut label =
                termide_core::util::shorten_home_path(&project.root.display().to_string());
            if project.attention {
                label.push(' ');
                label.push_str(termide_core::attention_mark());
            }
            let time = project
                .modified
                .map(|time| format!("{} ", termide_project::format_local_minute(time)))
                .unwrap_or_default();
            // Open projects come first, so their index is their number.
            let shortcut = if project.open {
                goto.get(index).and_then(|binding| binding.as_ref())
            } else {
                None
            }
            .map(|binding| binding.display().to_string());
            let mut item = DropdownItem::new(label, String::new())
                .with_prefix(format!("{} ", project_mark(project, current)), time)
                .cut_at_start()
                .with_shortcut(shortcut);
            if project.root == current {
                item = item.with_project();
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
        let reopenable = self
            .reopenable_projects
            .iter()
            .filter(|root| !self.open_projects.iter().any(|view| view.root == **root))
            .count();
        if reopenable > 0 {
            rows.push(ProjectRow::Reopen);
            items.push(DropdownItem::new(
                termide_i18n::t().projects_reopen_fmt(reopenable),
                "reopen_projects",
            ));
        }

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

    /// The open projects as the menu bar's buttons, in the order the menus
    /// list them (so button `i` is `switch_to_open_project(i)`).
    pub fn project_buttons(&self) -> Vec<ProjectButton> {
        let open: Vec<&OpenProjectView> = self.open_projects.iter().collect();
        let name = |root: &Path| {
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string())
        };
        open.iter()
            .map(|view| {
                let own = name(&view.root);
                let shared = open
                    .iter()
                    .any(|other| other.root != view.root && name(&other.root) == own);
                let label = match view.root.parent().and_then(Path::file_name) {
                    Some(parent) if shared => format!("{}/{own}", parent.to_string_lossy()),
                    _ => own,
                };
                ProjectButton {
                    name: label,
                    current: view.root == self.project_root,
                    attention: view.attention,
                }
            })
            .collect()
    }

    /// Load the projects worked on.
    pub(crate) fn load_known_projects(&mut self) {
        self.cache.projects = known_projects();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// State on the built-in defaults: `AppState::new` would read the
    /// developer's own config file.
    fn test_state() -> AppState {
        let mut config = termide_config::Config::default();
        config.normalize();
        let theme = termide_theme::Theme::get_by_name(&config.general.theme);
        AppState::with_config_and_theme(config.clone(), config, theme)
    }

    /// `(root, last use)` pairs, the first `seconds` after the epoch and
    /// so on.
    fn known(list: &[(&str, u64)]) -> Vec<(PathBuf, SystemTime)> {
        list.iter()
            .map(|(root, seconds)| {
                (
                    PathBuf::from(root),
                    SystemTime::UNIX_EPOCH + Duration::from_secs(*seconds),
                )
            })
            .collect()
    }

    fn open(root: &str, attention: bool) -> OpenProjectView {
        OpenProjectView {
            root: PathBuf::from(root),
            attention,
        }
    }

    /// Each project row as "lead|muted|label", separators as "---".
    fn rows(menu: &ProjectsMenu) -> Vec<String> {
        menu.items[PROJECTS_SUBMENU_ITEM_COUNT..]
            .iter()
            .map(|item| {
                if item.is_separator {
                    "---".to_string()
                } else {
                    format!("{}|{}|{}", item.lead, item.muted, item.label)
                }
            })
            .collect()
    }

    #[test]
    fn project_buttons_follow_the_menu_order_and_tell_twins_apart() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/w/b/app");
        state.open_projects = vec![
            open("/w/b/app", false),
            open("/w/termide", true),
            open("/w/a/app", false),
        ];
        let buttons = state.project_buttons();
        let names: Vec<_> = buttons.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["b/app", "termide", "a/app"]);
        let current: Vec<_> = buttons.iter().map(|b| b.current).collect();
        assert_eq!(current, [true, false, false]);
        assert!(buttons[1].attention);
    }

    #[test]
    fn open_projects_come_first_in_their_order_then_the_rest_most_recent_first() {
        let listed = listed_projects(
            &[open("/p/b", false), open("/p/a", true)],
            &known(&[("/p/c", 10), ("/p/a", 50), ("/p/d", 30)]),
        );
        let roots: Vec<_> = listed.iter().map(|p| p.root.to_str().unwrap()).collect();
        assert_eq!(roots, vec!["/p/b", "/p/a", "/p/d", "/p/c"]);
        assert!(listed[1].open && listed[1].attention);
        assert_eq!(listed[1].modified, None, "an open project has no time");
        assert!(!listed[2].open && listed[2].modified.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn a_known_project_without_its_drive_is_still_the_open_one() {
        let listed = listed_projects(
            &[open(r"C:\Users\u\a", false)],
            &known(&[(r"\Users\u\a", 10), (r"\Users\u\b", 5)]),
        );
        assert_eq!(listed.len(), 2);
        assert!(listed[0].open && !listed[1].open);
    }

    #[test]
    fn the_menu_shows_rows_as_the_switcher_does() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.open_projects = vec![open("/p/one", false), open("/p/two", true)];
        state.cache.projects = known(&[("/p/one", 40), ("/p/three", 10), ("/p/four", 20)]);

        let menu = state.projects_menu();
        let time = |seconds| {
            format!(
                "{} ",
                termide_project::format_local_minute(
                    SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
                )
            )
        };
        assert_eq!(
            rows(&menu),
            vec![
                "---".to_string(),
                "● ||/p/one".to_string(),
                format!("○ ||/p/two {}", termide_core::attention_mark()),
                "---".to_string(),
                format!("  |{}|/p/four", time(20)),
                format!("  |{}|/p/three", time(10)),
            ]
        );
        let first = PROJECTS_SUBMENU_ITEM_COUNT + 1;
        assert!(menu.items[first].is_project);
        assert!(menu.items[first..]
            .iter()
            .all(|item| item.is_separator || item.cut_start));
        assert!(matches!(
            ProjectsTarget::of(menu.rows.get(first + 3)),
            ProjectsTarget::Project(root) if root == Path::new("/p/four")
        ));

        // With only the current project open there is no second group.
        state.open_projects = vec![open("/p/one", false)];
        state.cache.projects = known(&[("/p/one", 40)]);
        assert_eq!(rows(&state.projects_menu()), vec!["---", "● ||/p/one"]);
    }

    #[test]
    fn the_projects_of_the_last_run_are_offered_until_open() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.open_projects = vec![open("/p/one", false)];
        assert!(!state.projects_menu().rows.contains(&ProjectRow::Reopen));

        state.reopenable_projects = vec![PathBuf::from("/p/two"), PathBuf::from("/p/three")];
        let menu = state.projects_menu();
        assert_eq!(
            menu.rows[PROJECTS_SUBMENU_ITEM_COUNT],
            ProjectRow::Reopen,
            "right after the fixed actions"
        );
        assert_eq!(
            menu.items[PROJECTS_SUBMENU_ITEM_COUNT].label,
            termide_i18n::t().projects_reopen_fmt(2)
        );
        assert!(matches!(
            ProjectsTarget::of(menu.rows.get(PROJECTS_SUBMENU_ITEM_COUNT)),
            ProjectsTarget::Reopen
        ));

        // The ones open already are not counted.
        state.open_projects.push(open("/p/two", false));
        let menu = state.projects_menu();
        assert_eq!(
            menu.items[PROJECTS_SUBMENU_ITEM_COUNT].label,
            termide_i18n::t().projects_reopen_fmt(1)
        );
        state.open_projects.push(open("/p/three", false));
        assert!(!state.projects_menu().rows.contains(&ProjectRow::Reopen));
    }

    #[test]
    fn open_projects_show_their_bound_keys() {
        let mut state = test_state();
        state.project_root = PathBuf::from("/p/one");
        state.open_projects = vec![open("/p/one", false), open("/p/two", false)];
        state.cache.projects = known(&[("/p/three", 10)]);
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
        state.cache.projects = known(&[("/p/one", 20), ("/p/two", 10)]);
        state.restore_projects_selection(first + 3);
        assert_eq!(state.ui.projects_submenu.selected, first + 2);

        // Only the current project is left; past the end falls back to it.
        state.cache.projects = known(&[("/p/one", 20)]);
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
