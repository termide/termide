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
use termide_panel_terminal::Terminal;
use termide_theme::Theme;
use termide_ui_render::{
    get_bookmarks_group_items, get_bookmarks_items, get_commands_group_items, get_commands_items,
    get_menu_item_x_position, get_options_items, get_sessions_items, get_shell_items,
    get_tools_items, render_collapsed_panel, render_dividers, render_expanded_panel, render_menu,
    render_v_divider_ghost, Dropdown, ExpandedPanelParams, LanguageDropdown, MenuRenderParams,
    ThemeDropdown, BOOKMARKS_MENU_INDEX, COMMANDS_MENU_INDEX, OPTIONS_MENU_INDEX,
    SESSIONS_MENU_INDEX, WINDOWS_MENU_INDEX,
};

use termide_ui_render::{StatusBar, StatusBarParams};

/// Render dropdown submenus and modal windows
fn render_dropdowns_and_modals(
    frame: &mut Frame,
    state: &mut AppState,
    layout_manager: &LayoutManager,
) {
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
        if state.ui.tools_nested.open && state.ui.tools_submenu.selected == 0 {
            let shell_items = get_shell_items(
                &state.cache.shells,
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

    // Render Commands submenu if open
    if state.ui.menu_open
        && state.ui.selected_menu_item == Some(COMMANDS_MENU_INDEX)
        && state.ui.commands_submenu.open
    {
        // Load commands registry (use cache if available)
        let registry = if let Some(ref cached) = state.cache.commands_registry {
            Some(cached.clone())
        } else {
            let loaded =
                termide_config::commands::CommandsRegistry::load_merged(Some(&state.project_root));
            state.cache.commands_registry = loaded.clone();
            loaded
        };
        if let Some(registry) = registry {
            let menu_x = get_menu_item_x_position(COMMANDS_MENU_INDEX);
            let dropdown_y = 1_u16; // Below menu bar

            // Render Commands submenu
            let commands_items = get_commands_items(&registry);
            let dropdown = Dropdown::new(
                &commands_items,
                state.ui.commands_submenu.selected,
                menu_x,
                dropdown_y,
                theme,
            );
            dropdown.render(frame.buffer_mut());

            // If a group is selected and nested submenu is open
            if state.ui.commands_nested.open {
                if let Some(group_name) = &state.ui.current_commands_group {
                    let nested_items = get_commands_group_items(&registry, group_name);
                    if !nested_items.is_empty() {
                        // Calculate position: to the right of commands dropdown
                        let nested_x = menu_x + dropdown.width();
                        // Align with selected group item (inside border)
                        let nested_y = dropdown_y + 1 + state.ui.commands_submenu.selected as u16;

                        let nested_dropdown = Dropdown::new(
                            &nested_items,
                            state.ui.commands_nested.selected,
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
        let bookmarks_items =
            get_bookmarks_items(&state.bookmarks, state.project_bookmarks.as_ref());
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
                let nested_items = get_bookmarks_group_items(
                    &state.bookmarks,
                    state.project_bookmarks.as_ref(),
                    group_name,
                    state.ui.current_bookmarks_group_is_project,
                );
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

    // Stash dropdown (anchored to button in git status panel)
    if state.ui.stash_submenu.open {
        if let Some(btn_area) = state.ui.stash_button_area {
            let items =
                termide_ui_render::get_stash_items(&state.stash.entries, state.stash.has_changes);
            let dropdown = termide_ui_render::Dropdown::new(
                &items,
                state.ui.stash_submenu.selected,
                btn_area.x,
                btn_area.bottom(),
                theme,
            );
            dropdown.render(frame.buffer_mut());
        }
    }

    // Panel action context menu (anchored to [≡] button on panel header)
    if state.ui.panel_action_menu.open {
        let group_count = layout_manager.panel_groups.len();
        let group_idx = state.ui.panel_action_menu.group_idx;
        let panel_idx = state.ui.panel_action_menu.panel_idx;
        let current_group_len = layout_manager
            .panel_groups
            .get(group_idx)
            .map(|g| g.len())
            .unwrap_or(0);

        // Panel-specific items (e.g. "View as diagram") prepended above
        // generic move/close items — same order as panel_action_menu_items().
        let mut items: Vec<termide_ui_render::DropdownItem> = layout_manager
            .panel_groups
            .get(group_idx)
            .and_then(|g| g.panels().get(panel_idx))
            .map(|p| {
                p.context_menu_items()
                    .into_iter()
                    .map(|(label, action)| termide_ui_render::DropdownItem::new(&label, action))
                    .collect()
            })
            .unwrap_or_default();
        items.extend(termide_ui_render::get_panel_action_menu_items(
            group_count,
            current_group_len,
        ));

        if !items.is_empty() {
            let (x, y) = termide_ui_render::panel_action_dropdown_position(
                &items,
                state.ui.panel_action_menu.anchor_x,
                state.ui.panel_action_menu.anchor_y,
                state.terminal.width,
                state.terminal.height,
            );
            let dropdown = Dropdown::new(&items, state.ui.panel_action_menu.selected, x, y, theme);
            dropdown.render(frame.buffer_mut());
        }
    }

    // Per-operation popup menu (Pause/Resume/Cancel on an Operations card)
    if state.ui.operation_action_menu.open {
        let items = termide_ui_render::get_operation_action_menu_items(
            state.ui.operation_action_menu.is_paused,
            state.ui.operation_action_menu.is_command,
        );
        if !items.is_empty() {
            let (x, y) = termide_ui_render::operation_action_dropdown_position(
                &items,
                state.ui.operation_action_menu.anchor_x,
                state.ui.operation_action_menu.anchor_y,
                state.terminal.width,
                state.terminal.height,
            );
            let dropdown =
                Dropdown::new(&items, state.ui.operation_action_menu.selected, x, y, theme);
            dropdown.render(frame.buffer_mut());
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
        battery: state.system_monitor.battery_cached(),
    };
    render_menu(frame, main_chunks[0], &menu_params);

    // Render main area with accordion support
    render_main_area_with_accordion(frame, main_chunks[1], state, layout_manager);

    // Render status bar for active panel
    render_status_bar_for_active(frame, main_chunks[2], state, layout_manager);

    // Render drag overlay (ghost + drop-zone highlight) on top of panels
    render_drag_overlay(frame, state, layout_manager, main_chunks[1]);

    // Render dropdowns and modals
    render_dropdowns_and_modals(frame, state, layout_manager);
}

/// Render the panel drag overlay: a bright highlight for the drop zone and
/// a ghost indicator under the cursor. Only runs when a drag is active.
fn render_drag_overlay(
    frame: &mut Frame,
    state: &AppState,
    layout_manager: &LayoutManager,
    main_area: Rect,
) {
    if !state.ui.panel_drag.active {
        return;
    }
    let Some(source) = state.ui.panel_drag.source else {
        return;
    };

    let cursor_x = state.ui.panel_drag.cursor_x;
    let cursor_y = state.ui.panel_drag.cursor_y;

    let rects = termide_layout::calculate_panel_rects(&layout_manager.panel_groups, main_area);
    let intent = termide_layout::classify_panel_drag(
        &rects,
        source.group_idx,
        source.panel_idx,
        cursor_x,
        cursor_y,
    );

    let theme = state.theme;

    let highlight_style = Style::default()
        .fg(theme.accented_fg)
        .bg(theme.bg)
        .add_modifier(ratatui::style::Modifier::BOLD);

    let buf = frame.buffer_mut();

    match intent {
        termide_layout::PanelDragIntent::ResizeAbove { divider_y } => {
            // Thick `━` line spanning the source group's column at the
            // cursor row — previews where the divider above the source
            // panel will land on release.
            let group_spans = termide_layout::group_spans_from_rects(&rects);
            if let Some(&(_, left, right)) = group_spans
                .iter()
                .find(|(gi, _, _)| *gi == source.group_idx)
            {
                for col in left..right {
                    if col >= buf.area.width {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut((col, divider_y)) {
                        cell.set_symbol("━").set_style(highlight_style);
                    }
                }
            }
        }
        termide_layout::PanelDragIntent::Move {
            target: termide_layout::PanelDropTarget::IntoGroup { group_idx, .. },
            drop_y,
        } => {
            // Thick `━` line at the cursor's row, clamped to the target
            // panel's body — previews where the dropped panel's title
            // row will land. The drag-end handler splits the target at
            // exactly this row.
            if let Some((target_rect, span)) = rects
                .iter()
                .find(|(gi, _, r, _)| *gi == group_idx && drop_y >= r.y && drop_y < r.y + r.height)
                .map(|(_, _, r, _)| *r)
                .zip(
                    termide_layout::group_spans_from_rects(&rects)
                        .into_iter()
                        .find(|(gi, _, _)| *gi == group_idx),
                )
            {
                let _ = target_rect;
                let (_, left, right) = span;
                for col in left..right {
                    if col >= buf.area.width {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut((col, drop_y)) {
                        cell.set_symbol("━").set_style(highlight_style);
                    }
                }
            }
        }
        termide_layout::PanelDragIntent::Move {
            target: termide_layout::PanelDropTarget::NewGroup { insert_at },
            ..
        } => {
            // Find x for the new group boundary.
            let group_spans = termide_layout::group_spans_from_rects(&rects);

            let line_x = if insert_at == 0 {
                group_spans.first().map(|(_, left, _)| *left)
            } else if insert_at >= group_spans.len() {
                group_spans
                    .last()
                    .map(|(_, _, right)| right.saturating_sub(1))
            } else {
                // Between spans[insert_at - 1] and spans[insert_at]:
                // pick the midpoint cell.
                let left = group_spans[insert_at - 1].2;
                let right = group_spans[insert_at].1;
                Some(((left + right) / 2).saturating_sub(1))
            };

            if let Some(x) = line_x {
                for row in main_area.y..main_area.y + main_area.height {
                    if row >= buf.area.height {
                        break;
                    }
                    if let Some(cell) = buf.cell_mut((x, row)) {
                        cell.set_symbol("┃").set_style(highlight_style);
                    }
                }
            }
        }
        termide_layout::PanelDragIntent::Cancel => {}
    }
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

        // Vertical-divider drag (in-group panel resize) ghost line.
        if let (Some((group_idx, _upper_idx)), Some(ghost_y)) =
            (state.ui.vdrag.active, state.ui.vdrag.last_applied_y)
        {
            // Span the dragged group horizontally so the line covers
            // exactly that column.
            let mut start_x = area.x;
            let mut end_x = area.x + area.width;
            for (gi, _, rect, _) in
                termide_layout::calculate_panel_rects(&layout_manager.panel_groups, area)
            {
                if gi == group_idx {
                    start_x = rect.x;
                    end_x = rect.x + rect.width;
                    break;
                }
            }
            render_v_divider_ghost(frame.buffer_mut(), ghost_y, start_x, end_x, state.theme);
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

    // Constraints come from the layout crate so accordion / split geometry
    // stays in sync with `calculate_panel_rects` (used for hit-testing).
    let vertical_constraints = termide_layout::compute_vertical_constraints(group, area.height);

    let vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vertical_constraints)
        .split(area);

    // Get group size for conditional icon rendering
    let group_size = group.len();

    // Render each panel in the group.
    //
    // Border policy:
    // - `panel_area.height >= 2` panels draw their own bottom border
    //   (`Block::borders(ALL)`) — every panel becomes a self-contained
    //   box. Two adjacent panels show two consecutive border rows
    //   (bottom of upper + top of lower), which lets the focused
    //   panel be highlighted as a complete coloured rectangle on all
    //   four sides.
    // - `panel_area.height == 1` (accordion / fullscreen-preset
    //   collapsed) keeps top-only borders — `Block::ALL` cannot fit
    //   in a single row.
    //
    // Top-row corners (`left_corner`, `right_corner`) are chosen so
    // the group looks coherent regardless of how each panel is sized:
    // 1. First panel in the group → plain `┌┐`.
    // 2. Last panel in the group AND it's collapsed (h==1) → `└┘`,
    //    so the group closes off visually even when the bottom panel
    //    is just a header row.
    // 3. Both current panel and previous panel are accordions (h==1)
    //    → `├┤` T-junctions, continuing the accordion run.
    // 4. Otherwise → `┌┐`. An expanded panel always starts a fresh
    //    box (no T-junction with the accordion run above), and after
    //    an expanded panel the next box starts cleanly too.
    let mut prev_was_accordion = false;
    for (panel_idx, panel) in group.panels_mut().iter_mut().enumerate() {
        let panel_area = vertical_chunks[panel_idx];
        let is_focused_panel = panel_idx == expanded_idx;
        let is_focused = is_active_group && is_focused_panel;

        // Calculate global panel index for rendering
        // (не используется сейчас, но может понадобиться для совместимости)
        let global_panel_index = group_idx * 100 + panel_idx;

        let is_last = panel_idx + 1 == group_size;
        let is_accordion = panel_area.height < 2;
        let collapsed_last = is_last && is_accordion;

        let (left_corner, right_corner) = if collapsed_last {
            ("└", "┘")
        } else if panel_idx > 0 && prev_was_accordion && is_accordion {
            ("├", "┤")
        } else {
            ("┌", "┐")
        };

        if !is_accordion {
            let params = ExpandedPanelParams {
                tab_size: state.config.editor.tab_size,
                word_wrap: state.config.editor.word_wrap,
                terminal_width: state.terminal.width,
                terminal_height: state.terminal.height,
                // Bottom border is only skipped when there's no room
                // for it; expanded panels always render `Block::ALL`.
                omit_bottom_border: false,
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

            // Patch the top corners — ratatui drew `┌`/`┐` and we
            // override them only when the chosen corner differs.
            if panel_area.width >= 2 && (left_corner != "┌" || right_corner != "┐") {
                let buf = frame.buffer_mut();
                let right_x = panel_area.x + panel_area.width - 1;
                if let Some(cell) = buf.cell_mut((panel_area.x, panel_area.y)) {
                    if cell.symbol() == "┌" {
                        cell.set_symbol(left_corner);
                    }
                }
                if let Some(cell) = buf.cell_mut((right_x, panel_area.y)) {
                    if cell.symbol() == "┐" {
                        cell.set_symbol(right_corner);
                    }
                }
            }
        } else {
            // Render collapsed panel (only title bar).
            render_collapsed_panel(
                &**panel,
                panel_area,
                frame.buffer_mut(),
                is_focused,
                state.theme,
                group_size,
            );

            // The collapsed renderer draws plain `─` end-caps; replace
            // them with the corner pair chosen above so the title row
            // ties into the neighbouring panels correctly.
            if panel_area.width >= 2 {
                let buf = frame.buffer_mut();
                let right_x = panel_area.x + panel_area.width - 1;
                if let Some(cell) = buf.cell_mut((panel_area.x, panel_area.y)) {
                    if cell.symbol() == "─" {
                        cell.set_symbol(left_corner);
                    }
                }
                if let Some(cell) = buf.cell_mut((right_x, panel_area.y)) {
                    if cell.symbol() == "─" {
                        cell.set_symbol(right_corner);
                    }
                }
            }
        }

        prev_was_accordion = is_accordion;
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
        let (selected_count, file_info, editor_info, terminal_info) = if let Some(fm) =
            (&mut **panel as &mut dyn Any).downcast_mut::<FileManager>()
        {
            (
                Some(fm.get_selected_count()),
                fm.get_current_file_info(),
                None,
                None,
            )
        } else if let Some(editor) = (&mut **panel as &mut dyn Any).downcast_mut::<Editor>() {
            (None, None, Some(editor.get_editor_info()), None)
        } else if let Some(terminal) = (&mut **panel as &mut dyn Any).downcast_mut::<Terminal>() {
            (None, None, None, Some(terminal.get_terminal_info()))
        } else {
            (None, None, None, None)
        };
        // Segments a panel contributes for the generic status-bar path
        // (e.g. binary viewer offset + Hex/Text toggle). Empty for panels on
        // the typed editor/FM/terminal path.
        let segments = panel.status_segments();

        // Disk space is read from the tick-updated cache instead of calling statvfs per render.
        let disk_space = state.cache.disk_space.as_ref();

        // Build background operations summary if available
        let background_ops = state.background_operations_summary().map(|summary| {
            termide_ui_render::BackgroundOpsSummary {
                has_operations: summary.has_operations(),
                status_text: summary.status_text(),
                is_paused: summary.any_paused,
            }
        });

        let disk_selected = state.ui.menu_open
            && state.ui.selected_menu_item == Some(termide_ui_render::INDICATOR_DISK_INDEX);
        let params = StatusBarParams {
            theme: state.theme,
            status_message: state.ui.status_message.as_ref(),
            terminal_width: state.terminal.width,
            terminal_height: state.terminal.height,
            recommended_layout: state.get_recommended_layout(),
            background_ops,
            disk_selected,
        };
        StatusBar::render(
            frame.buffer_mut(),
            area,
            &params,
            &panel.title(),
            selected_count,
            file_info.as_ref(),
            disk_space,
            editor_info.as_ref(),
            terminal_info.as_ref(),
            Some(segments.as_slice()),
        );
    }
}
