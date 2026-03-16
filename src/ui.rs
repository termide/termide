use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
    Frame,
};
use std::any::Any;

use termide_app::AppState;
use termide_layout::LayoutManager;
use termide_panel_editor::Editor;
use termide_panel_file_manager::FileManager;
use termide_panel_git_status::GitStatusPanel;
use termide_panel_terminal::Terminal;
use termide_theme::Theme;
use termide_ui_render::{
    get_bookmarks_group_items, get_bookmarks_items, get_menu_item_x_position, get_options_items,
    get_scripts_group_items, get_scripts_items, get_sessions_items, get_shell_items,
    get_tools_items, render_collapsed_panel, render_dividers, render_expanded_panel, render_menu,
    Dropdown, ExpandedPanelParams, LanguageDropdown, MenuRenderParams, ThemeDropdown,
    BOOKMARKS_MENU_INDEX, OPTIONS_MENU_INDEX, SCRIPTS_MENU_INDEX, SESSIONS_MENU_INDEX,
    WINDOWS_MENU_INDEX,
};

use termide_ui_render::{StatusBar, StatusBarParams};

/// Render dropdown submenus and modal windows
fn render_dropdowns_and_modals(frame: &mut Frame, state: &mut AppState) {
    let theme = state.theme;

    // Render Sessions submenu if open
    if state.ui.menu_open
        && state.ui.selected_menu_item == Some(SESSIONS_MENU_INDEX)
        && state.ui.sessions_submenu.open
    {
        // Calculate position of Sessions menu item
        let menu_x = get_menu_item_x_position(SESSIONS_MENU_INDEX);
        let dropdown_y = 1_u16; // Below menu bar

        // Render Sessions submenu
        let sessions_items = get_sessions_items();
        let dropdown = Dropdown::new(
            &sessions_items,
            state.ui.sessions_submenu.selected,
            menu_x,
            dropdown_y,
            theme,
        );
        dropdown.render(frame.buffer_mut());
    }

    // Render Tools submenu if open
    if state.ui.menu_open
        && state.ui.selected_menu_item == Some(WINDOWS_MENU_INDEX)
        && state.ui.tools_submenu.open
    {
        // Calculate position of Tools menu item
        let menu_x = get_menu_item_x_position(WINDOWS_MENU_INDEX);
        let dropdown_y = 1_u16; // Below menu bar

        // Render Tools submenu
        let tools_items = get_tools_items();
        let dropdown = Dropdown::new(
            &tools_items,
            state.ui.tools_submenu.selected,
            menu_x,
            dropdown_y,
            theme,
        );
        dropdown.render(frame.buffer_mut());

        // Render shell picker nested submenu if open (Terminal selected)
        if state.ui.tools_nested.open && state.ui.tools_submenu.selected == 1 {
            let shell_items = get_shell_items(
                &state.cached_shells,
                state.config.terminal.default_shell.as_deref(),
            );
            if !shell_items.is_empty() {
                let nested_x = menu_x + dropdown.width();
                // +1 for border, +selected for the item row
                let nested_y = dropdown_y + 1 + state.ui.tools_submenu.selected as u16;
                let nested_dropdown = Dropdown::new(
                    &shell_items,
                    state.ui.tools_nested.selected,
                    nested_x,
                    nested_y,
                    theme,
                );
                nested_dropdown.render(frame.buffer_mut());
            }
        }
    }

    // Render Scripts submenu if open
    if state.ui.menu_open
        && state.ui.selected_menu_item == Some(SCRIPTS_MENU_INDEX)
        && state.ui.scripts_submenu.open
    {
        // Load scripts registry
        if let Some(registry) = termide_config::scripts::ScriptsRegistry::load() {
            let menu_x = get_menu_item_x_position(SCRIPTS_MENU_INDEX);
            let dropdown_y = 1_u16; // Below menu bar

            // Render Scripts submenu
            let scripts_items = get_scripts_items(&registry);
            let dropdown = Dropdown::new(
                &scripts_items,
                state.ui.scripts_submenu.selected,
                menu_x,
                dropdown_y,
                theme,
            );
            dropdown.render(frame.buffer_mut());

            // If a group is selected and nested submenu is open
            if state.ui.scripts_nested.open {
                if let Some(group_name) = &state.ui.current_scripts_group {
                    let nested_items = get_scripts_group_items(&registry, group_name);
                    if !nested_items.is_empty() {
                        // Calculate position: to the right of scripts dropdown
                        let nested_x = menu_x + dropdown.width();
                        // Align with selected group item (inside border)
                        let nested_y = dropdown_y + 1 + state.ui.scripts_submenu.selected as u16;

                        let nested_dropdown = Dropdown::new(
                            &nested_items,
                            state.ui.scripts_nested.selected,
                            nested_x,
                            nested_y,
                            theme,
                        );
                        nested_dropdown.render(frame.buffer_mut());
                    }
                }
            }
        }
    }

    // Render Bookmarks submenu if open
    if state.ui.menu_open
        && state.ui.selected_menu_item == Some(BOOKMARKS_MENU_INDEX)
        && state.ui.bookmarks_submenu.open
    {
        let menu_x = get_menu_item_x_position(BOOKMARKS_MENU_INDEX);
        let dropdown_y = 1_u16; // Below menu bar

        // Render Bookmarks submenu
        let bookmarks_items = get_bookmarks_items(&state.bookmarks);
        let dropdown = Dropdown::new(
            &bookmarks_items,
            state.ui.bookmarks_submenu.selected,
            menu_x,
            dropdown_y,
            theme,
        );
        dropdown.render(frame.buffer_mut());

        // If a group is selected and nested submenu is open
        if state.ui.bookmarks_nested.open {
            if let Some(group_name) = &state.ui.current_bookmarks_group {
                let nested_items = get_bookmarks_group_items(&state.bookmarks, group_name);
                if !nested_items.is_empty() {
                    // Calculate position: to the right of bookmarks dropdown
                    let nested_x = menu_x + dropdown.width();
                    // Align with selected group item (inside border)
                    let nested_y = dropdown_y + 1 + state.ui.bookmarks_submenu.selected as u16;

                    let nested_dropdown = Dropdown::new(
                        &nested_items,
                        state.ui.bookmarks_nested.selected,
                        nested_x,
                        nested_y,
                        theme,
                    );
                    nested_dropdown.render(frame.buffer_mut());
                }
            }
        }
    }

    // Render Options submenu if open
    if state.ui.menu_open
        && state.ui.selected_menu_item == Some(OPTIONS_MENU_INDEX)
        && state.ui.options_submenu.open
    {
        // Calculate position of Options menu item
        let menu_x = get_menu_item_x_position(OPTIONS_MENU_INDEX);
        let dropdown_y = 1_u16; // Below menu bar

        // Render Options submenu
        let options_items = get_options_items();
        let dropdown = Dropdown::new(
            &options_items,
            state.ui.options_submenu.selected,
            menu_x,
            dropdown_y,
            theme,
        );
        dropdown.render(frame.buffer_mut());

        // If Themes is selected and nested submenu is open
        if state.ui.nested_submenu.open && state.ui.options_submenu.selected == 0 {
            // Calculate position: to the right of options dropdown
            let nested_x = menu_x + dropdown.width();
            let nested_y = dropdown_y + 1; // Align with "Themes" item (inside border)

            let theme_names = Theme::all_theme_names();
            let theme_dropdown = ThemeDropdown::new(
                &theme_names,
                state.ui.nested_submenu.selected,
                nested_x,
                nested_y,
                theme,
            );
            theme_dropdown.render(frame.buffer_mut());
        }

        // If Language is selected and nested submenu is open
        if state.ui.nested_submenu.open && state.ui.options_submenu.selected == 1 {
            // Calculate position: to the right of options dropdown
            let nested_x = menu_x + dropdown.width();
            let nested_y = dropdown_y + 2; // Align with "Language" item (index 1, inside border)

            let language_dropdown =
                LanguageDropdown::new(state.ui.nested_submenu.selected, nested_x, nested_y, theme);
            language_dropdown.render(frame.buffer_mut());
        }
    }

    // Render active modal window if it's open
    if let Some(modal) = state.get_active_modal_mut() {
        let area = frame.area();
        modal.render(area, frame.buffer_mut(), theme);
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
        net_down_rate: state.system_monitor.net_download_rate(),
        net_up_rate: state.system_monitor.net_upload_rate(),
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

        // Render dividers between groups (ghost line during drag)
        let divider_positions = layout_manager.get_divider_positions();
        render_dividers(
            frame.buffer_mut(),
            &divider_positions,
            state.ui.drag.active_divider,
            state.ui.drag.last_applied_x,
            state.terminal.height,
            state.theme,
        );
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
            (
                None,
                None,
                editor.get_disk_space_info(),
                Some(editor.get_editor_info()),
                None,
            )
        } else if let Some(terminal) = (&mut **panel as &mut dyn Any).downcast_mut::<Terminal>() {
            (None, None, None, None, Some(terminal.get_terminal_info()))
        } else if let Some(git_panel) =
            (&mut **panel as &mut dyn Any).downcast_mut::<GitStatusPanel>()
        {
            (None, None, git_panel.get_disk_space_info(), None, None)
        } else {
            (None, None, None, None, None)
        };

        // Build background operations summary if available
        let background_ops = state.background_operations_summary().map(|summary| {
            termide_ui_render::BackgroundOpsSummary {
                has_operations: summary.has_operations(),
                status_text: summary.status_text(),
                is_paused: summary.any_paused,
            }
        });

        let params = StatusBarParams {
            theme: state.theme,
            status_message: state.ui.status_message.as_ref(),
            terminal_width: state.terminal.width,
            terminal_height: state.terminal.height,
            recommended_layout: state.get_recommended_layout(),
            background_ops,
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
