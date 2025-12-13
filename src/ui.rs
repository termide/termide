use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
    Frame,
};
use std::any::Any;

use termide_app::state::ActiveModal;
use termide_app::AppState;
use termide_layout::LayoutManager;
use termide_panel_editor::Editor;
use termide_panel_file_manager::FileManager;
use termide_panel_terminal::Terminal;
use termide_ui_render::{
    render_collapsed_panel, render_expanded_panel, render_menu, ExpandedPanelParams,
    MenuRenderParams,
};

use termide_modal::Modal;
use termide_ui_render::{StatusBar, StatusBarParams};

/// Render modal windows
fn render_dropdowns_and_modals(frame: &mut Frame, state: &mut AppState) {
    // Render active modal window if it's open
    // Copy theme before getting mutable modal reference to avoid borrow checker issues
    let theme = state.theme;

    if let Some(modal) = state.get_active_modal_mut() {
        let area = frame.area();
        match modal {
            ActiveModal::Confirm(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Input(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Select(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Overwrite(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Conflict(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Info(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::RenamePattern(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::EditableSelect(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Search(m) => m.render(area, frame.buffer_mut(), theme),
            ActiveModal::Replace(m) => m.render(area, frame.buffer_mut(), theme),
        }
    }
}

/// Render the main application layout with accordion support
pub fn render_layout_with_accordion(
    frame: &mut Frame,
    state: &mut AppState,
    layout_manager: &mut LayoutManager,
) {
    let size = frame.area();

    // Set application background
    let background = Block::default().style(Style::default().bg(state.theme.bg));
    frame.render_widget(background, size);

    // Split screen into menu (1 line), main area, and status bar (1 line)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Menu
            Constraint::Min(0),    // Main area
            Constraint::Length(1), // Status bar
        ])
        .split(size);

    // Render menu
    let (ram_value, ram_unit) = state.system_monitor.format_ram();
    let menu_params = MenuRenderParams {
        theme: state.theme,
        selected_menu_item: state.ui.selected_menu_item,
        menu_open: state.ui.menu_open,
        cpu_usage: state.system_monitor.cpu_usage(),
        ram_percent: state.system_monitor.ram_usage_percent(),
        ram_value,
        ram_unit,
    };
    render_menu(frame, main_chunks[0], &menu_params);

    // Render main area with accordion support
    render_main_area_with_accordion(frame, main_chunks[1], state, layout_manager);

    // Render status bar for active panel
    render_status_bar_for_active(frame, main_chunks[2], state, layout_manager);

    // Render dropdowns and modals
    render_dropdowns_and_modals(frame, state);
}

/// Render main area with panel groups and accordion
fn render_main_area_with_accordion(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    layout_manager: &mut LayoutManager,
) {
    if layout_manager.panel_groups.is_empty() {
        // No panels at all - do nothing
        return;
    }

    // Render panel groups
    if !layout_manager.panel_groups.is_empty() {
        let groups_area = area;

        // Calculate horizontal constraints for groups (distribute all space)
        // Группы могут иметь фиксированную ширину (width = Some(n)) или auto-width (width = None)
        let group_constraints: Vec<Constraint> = layout_manager
            .panel_groups
            .iter()
            .map(|g| {
                // Для auto-width групп использовать всю доступную ширину
                let width = g.width.unwrap_or(groups_area.width);
                Constraint::Length(width.max(20))
            })
            .collect();

        let group_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(group_constraints)
            .split(groups_area);

        // Get active group index before borrowing panel_groups
        let active_group_idx = layout_manager.active_group_index();

        // Render each group
        for (group_idx, group) in layout_manager.panel_groups.iter_mut().enumerate() {
            let group_area = group_chunks[group_idx];
            let is_active_group = active_group_idx == Some(group_idx);

            render_panel_group(frame, group_area, state, group, group_idx, is_active_group);
        }
    }
}

/// Render a single panel group with accordion (vertical stack)
fn render_panel_group(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    group: &mut termide_layout::PanelGroup,
    group_idx: usize,
    is_active_group: bool,
) {
    if group.is_empty() || area.height == 0 {
        return;
    }

    let expanded_idx = group.expanded_index();

    // Build vertical constraints: collapsed panels = 1 line, expanded = Min(0)
    let vertical_constraints: Vec<Constraint> = (0..group.len())
        .map(|i| {
            if i == expanded_idx {
                Constraint::Min(0) // Expanded panel takes all remaining space
            } else {
                Constraint::Length(1) // Collapsed panels are 1 line
            }
        })
        .collect();

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vertical_constraints)
        .split(area);

    // Get group size for conditional icon rendering
    let group_size = group.len();

    // Render each panel in the group
    for (panel_idx, panel) in group.panels_mut().iter_mut().enumerate() {
        let panel_area = vertical_chunks[panel_idx];
        let is_expanded = panel_idx == expanded_idx;
        let is_focused = is_active_group && is_expanded;

        // Calculate global panel index for rendering
        // (не используется сейчас, но может понадобиться для совместимости)
        let global_panel_index = group_idx * 100 + panel_idx;

        if is_expanded {
            // Render expanded panel with full border
            let params = ExpandedPanelParams {
                tab_size: state.config.editor.tab_size,
                word_wrap: state.config.editor.word_wrap,
                terminal_width: state.terminal.width,
                terminal_height: state.terminal.height,
            };
            render_expanded_panel(
                panel,
                panel_area,
                frame.buffer_mut(),
                is_focused,
                global_panel_index,
                state.theme,
                &state.config,
                params,
                group_size,
            );
        } else {
            // Render collapsed panel (only title bar)
            render_collapsed_panel(
                &**panel,
                panel_area,
                frame.buffer_mut(),
                is_focused,
                state.theme,
                group_size,
            );
        }
    }
}

/// Render status bar for the active panel
fn render_status_bar_for_active(
    frame: &mut Frame,
    area: Rect,
    state: &AppState,
    layout_manager: &mut LayoutManager,
) {
    // Get active panel
    let active_panel = layout_manager.active_panel_mut();

    if let Some(panel) = active_panel {
        // Get information depending on panel type
        let (selected_count, file_info, disk_space, editor_info, terminal_info) = if let Some(fm) =
            (&mut **panel as &mut dyn Any).downcast_mut::<FileManager>()
        {
            (
                Some(fm.get_selected_count()),
                fm.get_current_file_info(),
                fm.get_disk_space_info(),
                None,
                None,
            )
        } else if let Some(editor) = (&mut **panel as &mut dyn Any).downcast_mut::<Editor>() {
            (None, None, None, Some(editor.get_editor_info()), None)
        } else if let Some(terminal) = (&mut **panel as &mut dyn Any).downcast_mut::<Terminal>() {
            (None, None, None, None, Some(terminal.get_terminal_info()))
        } else {
            (None, None, None, None, None)
        };

        let params = StatusBarParams {
            theme: state.theme,
            status_message: state.ui.status_message.as_ref(),
            terminal_width: state.terminal.width,
            terminal_height: state.terminal.height,
            recommended_layout: state.get_recommended_layout(),
        };
        StatusBar::render(
            frame.buffer_mut(),
            area,
            &params,
            &panel.title(),
            selected_count,
            file_info.as_ref(),
            disk_space.as_ref(),
            editor_info.as_ref(),
            terminal_info.as_ref(),
        );
    }
}
