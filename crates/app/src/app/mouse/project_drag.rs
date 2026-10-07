//! Dragging open projects into another order: a project button along the
//! menu bar, or an open project's row in the Projects menu.
//!
//! The order changes as the cursor crosses other projects, so the bar and
//! the menu show the drag themselves. A press released on its own project
//! without having moved it is a click; `Esc` puts the project back.

use std::path::PathBuf;

use anyhow::Result;
use termide_ui_render::dropdown_geometry;
use termide_ui_render::menu::PROJECT_BUTTON_BASE;

use crate::app::App;
use crate::projects_menu::{ProjectRow, ProjectsTarget};

/// Where a project is dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ProjectDragSurface {
    /// The project buttons of the menu bar.
    Bar,
    /// The open projects of the Projects menu.
    Menu,
}

/// A project held by the mouse.
pub(in crate::app) struct ProjectDrag {
    surface: ProjectDragSurface,
    root: PathBuf,
    /// Its place when the press began, for `Esc` to restore.
    original: usize,
    /// The cursor on the last event: a project moves only towards where
    /// the cursor heads, or a short button swapped past a long one would
    /// land under the cursor again and swap straight back.
    last_x: u16,
    last_y: u16,
    moved: bool,
}

impl App {
    /// Hold open project `index` on a press over it on `surface`.
    pub(in crate::app) fn begin_project_drag(
        &mut self,
        surface: ProjectDragSurface,
        index: usize,
        x: u16,
        y: u16,
    ) {
        let Some(root) = self.listed_open_roots().get(index).cloned() else {
            return;
        };
        if surface == ProjectDragSurface::Bar {
            self.state.held_project_button = Some(index);
        }
        self.project_drag = Some(ProjectDrag {
            surface,
            root,
            original: index,
            last_x: x,
            last_y: y,
            moved: false,
        });
    }

    pub(in crate::app) fn is_dragging_project(&self) -> bool {
        self.project_drag.is_some()
    }

    /// The open project under (`x`, `y`) on `surface`, by its place.
    fn open_project_at(&self, surface: ProjectDragSurface, x: u16, y: u16) -> Option<usize> {
        match surface {
            ProjectDragSurface::Bar => (y == 0).then(|| self.menu_bar().project_at(x))?,
            ProjectDragSurface::Menu => {
                let menu = self.state.projects_menu();
                let geometry = dropdown_geometry(
                    &menu.items,
                    menu.selected,
                    menu.x,
                    menu.y,
                    self.screen_rect(),
                );
                let row = geometry.item_at(x, y)?;
                match menu.rows.get(row)? {
                    ProjectRow::Project(project) if project.open => {
                        let first = first_project_row(&menu.rows)?;
                        Some(row - first)
                    }
                    _ => None,
                }
            }
        }
    }

    /// Follow the cursor on `Drag(Left)`: crossing another open project
    /// moves the held one to its place.
    pub(in crate::app) fn handle_project_drag_move(&mut self, x: u16, y: u16) {
        let Some(drag) = self.project_drag.as_ref() else {
            return;
        };
        let (surface, root) = (drag.surface, drag.root.clone());
        let heading = match surface {
            ProjectDragSurface::Bar => i32::from(x) - i32::from(drag.last_x),
            ProjectDragSurface::Menu => i32::from(y) - i32::from(drag.last_y),
        };
        let hovered = self.open_project_at(surface, x, y);
        let current = self.open_projects.position(&root);
        if let Some(place) = current.and_then(|current| drop_place(current, hovered?, heading)) {
            if self.open_projects.move_to(&root, place) {
                self.sync_open_projects();
                self.follow_moved_project(surface, place);
                if let Some(drag) = self.project_drag.as_mut() {
                    drag.moved = true;
                }
            }
        }
        if let Some(drag) = self.project_drag.as_mut() {
            drag.last_x = x;
            drag.last_y = y;
        }
    }

    /// Keep the selection on the moved project, now at `place`.
    fn follow_moved_project(&mut self, surface: ProjectDragSurface, place: usize) {
        match surface {
            ProjectDragSurface::Bar => {
                self.state.held_project_button = Some(place);
                let selected = self.state.ui.selected_menu_item;
                if selected.is_some_and(|item| item >= PROJECT_BUTTON_BASE) {
                    self.state.ui.selected_menu_item = Some(PROJECT_BUTTON_BASE + place);
                }
            }
            ProjectDragSurface::Menu => {
                if let Some(first) = first_project_row(&self.state.projects_menu().rows) {
                    self.state.ui.projects_submenu.selected = first + place;
                }
            }
        }
        self.state.needs_redraw = true;
    }

    /// Let go on `Up(Left)`. A press that moved nothing and ends on its own
    /// project is a click on it: the project is switched to.
    pub(in crate::app) fn handle_project_drag_end(&mut self, x: u16, y: u16) -> Result<()> {
        self.state.held_project_button = None;
        let Some(drag) = self.project_drag.take() else {
            return Ok(());
        };
        if drag.moved {
            return Ok(());
        }
        let place = self.open_projects.position(&drag.root);
        if place.is_none() || self.open_project_at(drag.surface, x, y) != place {
            return Ok(());
        }
        match drag.surface {
            ProjectDragSurface::Bar => {
                self.state.close_indicator_modal();
                self.state.close_menu();
                if drag.root != self.project_root {
                    self.switch_to_project(drag.root)?;
                }
                Ok(())
            }
            ProjectDragSurface::Menu => {
                self.activate_projects_target(ProjectsTarget::Project(drag.root))
            }
        }
    }

    /// Let go of a held project where it is, without a click.
    pub(in crate::app) fn release_project_drag(&mut self) {
        self.project_drag = None;
        self.state.held_project_button = None;
    }

    /// `Esc` while holding a project: put it back where it was.
    pub(in crate::app) fn cancel_project_drag(&mut self) {
        let Some(drag) = self.project_drag.take() else {
            return;
        };
        if drag.moved && self.open_projects.move_to(&drag.root, drag.original) {
            self.sync_open_projects();
            self.follow_moved_project(drag.surface, drag.original);
        }
        self.state.held_project_button = None;
        self.state.needs_redraw = true;
    }
}

/// Where a project at `current` moves with the cursor over the project at
/// `hovered`, heading `heading` (positive: right or down): there, when the
/// cursor heads towards it.
fn drop_place(current: usize, hovered: usize, heading: i32) -> Option<usize> {
    let towards = (hovered > current && heading > 0) || (hovered < current && heading < 0);
    towards.then_some(hovered)
}

/// The row of the first listed project: the open ones come first.
fn first_project_row(rows: &[ProjectRow]) -> Option<usize> {
    rows.iter()
        .position(|row| matches!(row, ProjectRow::Project(_)))
}

#[cfg(test)]
mod tests {
    use super::drop_place;

    #[test]
    fn a_project_moves_only_towards_where_the_cursor_heads() {
        assert_eq!(drop_place(1, 3, 2), Some(3), "jumps over several at once");
        assert_eq!(drop_place(2, 0, -1), Some(0));
        assert_eq!(drop_place(1, 1, 5), None, "over itself");
        // A short button swapped right past a long one leaves the cursor on
        // the long one, now to its left: still heading right, it stays.
        assert_eq!(drop_place(1, 0, 1), None);
        assert_eq!(drop_place(0, 1, 0), None, "no movement");
    }
}
