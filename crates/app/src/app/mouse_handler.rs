//! Mouse event handling for the application.

use anyhow::Result;
use crossterm::event::{MouseButton, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use unicode_width::UnicodeWidthStr;

use super::App;
use crate::PanelExt;
use termide_i18n as i18n;

use crate::state::{ActiveModal, PendingAction};
use termide_modal as modal;

use termide_theme::Theme;
use termide_ui_render::{
    get_bookmarks_group_items, get_bookmarks_items, get_menu_item_x_position, get_options_items,
    get_resource_indicator_ranges, get_scripts_group_items, get_scripts_items, get_sessions_items,
    get_shell_items, get_tools_items, MenuRenderParams, BOOKMARKS_MENU_INDEX, OPTIONS_MENU_INDEX,
    SCRIPTS_MENU_INDEX, SESSIONS_MENU_INDEX, WINDOWS_MENU_INDEX,
};

/// Hit-test a dropdown menu and return the clicked item index (if any).
///
/// `menu_x` is the left edge of the dropdown, `dropdown_y` is the top row.
/// Returns `Some(index)` if the click is on a valid item, `None` otherwise.
fn hit_dropdown_item(
    x: u16,
    y: u16,
    menu_x: u16,
    dropdown_y: u16,
    items: &[termide_ui_render::DropdownItem],
) -> Option<usize> {
    let width = items.iter().map(|i| i.label.width()).max().unwrap_or(10) as u16 + 4;
    let height = items.len() as u16 + 2; // +2 for borders
    if x >= menu_x && x < menu_x + width && y >= dropdown_y && y < dropdown_y + height {
        let item_index = y.saturating_sub(dropdown_y + 1) as usize;
        if item_index < items.len() {
            return Some(item_index);
        }
    }
    None
}

impl App {
    /// Handle mouse event
    pub(super) fn handle_mouse_event(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        log::trace!(
            "Mouse event: kind={:?}, col={}, row={}, modifiers={:?}",
            mouse.kind,
            mouse.column,
            mouse.row,
            mouse.modifiers
        );

        // Handle divider drag first (highest priority for smooth resize)
        if self.state.ui.drag.is_dragging() {
            match mouse.kind {
                MouseEventKind::Drag(MouseButton::Left) => {
                    self.handle_divider_drag(mouse.column)?;
                    return Ok(());
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.handle_divider_drag_end()?;
                    return Ok(());
                }
                _ => {}
            }
        }

        // Handle modal mouse events first if a modal is open
        if self.state.active_modal.is_some() {
            let modal_area = Rect {
                x: 0,
                y: 0,
                width: self.state.terminal.width,
                height: self.state.terminal.height,
            };
            self.handle_modal_mouse(mouse, modal_area)?;
            return Ok(());
        }

        // Scroll events should reach panels when no modal is active
        // This allows scrolling terminal history, editor, etc.
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            // Track scroll timing for throttling heavy operations in Event::Tick
            self.state.last_mouse_scroll = Some(std::time::Instant::now());
            self.state.pending_scroll_render = true;
            self.forward_scroll_to_panel_at_cursor(mouse)?;
            return Ok(());
        }

        // Click on menu bar (row 0)
        if mouse.row == 0 && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            self.handle_menu_click(mouse.column)?;
            return Ok(());
        }

        // Click on status bar (bottom row)
        let status_bar_row = self.state.terminal.height.saturating_sub(1);
        if mouse.row == status_bar_row
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.handle_status_bar_click(mouse.column)?;
            return Ok(());
        }

        // Handle Sessions submenu clicks when it's open
        if self.state.ui.sessions_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_sessions_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Preferences submenu clicks when submenu is open
        if self.state.ui.options_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Tools submenu clicks when it's open
        if self.state.ui.tools_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_tools_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Scripts submenu clicks when it's open
        if self.state.ui.scripts_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_scripts_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Handle Bookmarks submenu clicks when it's open
        if self.state.ui.bookmarks_submenu.open
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_bookmarks_submenu_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // If menu is open, close it on click outside menu
        if self.state.ui.menu_open && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        {
            self.state.close_menu();
            return Ok(());
        }

        // Check click on divider for resize (before panel click handling)
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && self.handle_divider_click(mouse.column, mouse.row)?
        {
            return Ok(());
        }

        // Check click on panel [X] button
        if matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left)) {
            if self.handle_panel_close_click(mouse.column, mouse.row)? {
                return Ok(());
            }

            // Check click on panel to switch focus
            self.handle_panel_focus_click(mouse.column, mouse.row)?;
        }

        // Scroll events handled at the top of this function (before modal check)

        // Other mouse events - to active panel
        self.forward_mouse_to_panel(mouse)?;

        Ok(())
    }

    /// Forward mouse event to active panel
    fn forward_mouse_to_panel(&mut self, mouse: crossterm::event::MouseEvent) -> Result<()> {
        use crate::panel_ext::PanelExt;

        // Determine active panel area
        let panel_area = self.get_active_panel_area();

        // Handle mouse event and collect results
        let (mut events, modal_request) =
            if let Some(panel) = self.layout_manager.active_panel_mut() {
                let events = panel.handle_mouse(mouse, panel_area);
                let modal_request = panel.take_modal_request();
                (events, modal_request)
            } else {
                (vec![], None)
            };

        // Handle editor-specific LSP requests (Ctrl+click go-to-definition)
        if let Some(panel) = self.layout_manager.active_panel_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                // Handle go-to-definition request (Ctrl+click)
                if let Some((line, col)) = editor.take_definition_request() {
                    if let Some(ref lsp_manager) = self.state.lsp_manager {
                        editor.request_definition(line, col, lsp_manager);
                    }
                }

                // Poll for definition response (returns PanelEvent::OpenFileAt)
                if let Some(event) = editor.poll_definition() {
                    events.push(event);
                }
            }
        }

        // Process panel events (new event-based architecture)
        self.process_panel_events(events)?;

        // Handle modal window request from panel (legacy, still used)
        if let Some((action, modal)) = modal_request {
            self.handle_modal_request(action, modal)?;
        }

        Ok(())
    }

    /// Forward scroll to panel under mouse cursor
    fn forward_scroll_to_panel_at_cursor(
        &mut self,
        mouse: crossterm::event::MouseEvent,
    ) -> Result<()> {
        if let Some((group_idx, rect)) = self.find_expanded_panel_group_at(mouse.column, mouse.row)
        {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                if let Some(panel) = group.expanded_panel_mut() {
                    let events = panel.handle_mouse(mouse, rect);
                    self.process_panel_events(events)?;
                }
            }
        }

        Ok(())
    }

    /// Get active panel area
    fn get_active_panel_area(&self) -> Rect {
        let focused_group_idx = self.layout_manager.focus;

        // Find expanded panel rect in the focused group
        for (group_idx, _panel_idx, rect, is_expanded) in self.calculate_panel_rects() {
            if group_idx == focused_group_idx && is_expanded {
                return rect;
            }
        }

        // Fallback: return full main area if active panel not found
        let width = self.state.terminal.width;
        let height = self.state.terminal.height;
        Rect {
            x: 0,
            y: 1,
            width,
            height: height.saturating_sub(2),
        }
    }

    /// Handle click on panel [X] button, [▶]/[▼] expand/collapse button, or title area.
    /// Returns true if a button or title was clicked.
    fn handle_panel_close_click(&mut self, click_x: u16, click_y: u16) -> Result<bool> {
        let panel_rects = self.calculate_panel_rects();

        for (group_idx, panel_idx, rect, is_expanded) in panel_rects {
            // Check if click is on this panel's top line
            if click_y != rect.y {
                continue;
            }

            // Check if click is within the panel's horizontal bounds
            if click_x < rect.x || click_x >= rect.x + rect.width {
                continue;
            }

            let relative_x = click_x - rect.x;

            // Button format: ─[X][▶] Title ─── (collapsed)
            //          or:   ┌[X][▼] Title ──┐ (expanded)
            // [X] button: offsets 1-3
            // [▶]/[▼] button: offsets 4-6

            if (1..=3).contains(&relative_x) {
                // Click on [X] button - close panel with confirmation if needed
                log::debug!("Panel close button [X] clicked");
                // First, activate the clicked panel
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    group.set_expanded(panel_idx);
                }
                self.layout_manager.focus = group_idx;

                // Now use the same close logic as keyboard shortcut (with confirmation)
                self.handle_close_panel_request()?;
                return Ok(true);
            } else if (4..=6).contains(&relative_x) {
                // Click on [▶]/[▼] button - expand/collapse panel
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    if is_expanded && group.len() > 1 {
                        // Currently expanded - collapse by expanding next panel
                        let next_idx = (panel_idx + 1) % group.len();
                        group.set_expanded(next_idx);
                    } else {
                        // Currently collapsed - expand this panel
                        group.set_expanded(panel_idx);
                        // Also make this group active
                        self.layout_manager.focus = group_idx;
                    }
                }
                return Ok(true);
            }

            // Calculate title zone boundaries
            // Format: ─[X][▶] Title ─── (group_size > 1) or ─[X] Title ─── (group_size == 1)
            let group_size = self
                .layout_manager
                .panel_groups
                .get(group_idx)
                .map(|g| g.len())
                .unwrap_or(1);

            // Check for title click (only if buttons didn't handle it)
            let title_start = if group_size > 1 { 7 } else { 4 };

            if relative_x >= title_start {
                // Check if this panel was already active before click
                let was_active = group_idx == self.layout_manager.focus
                    && self
                        .layout_manager
                        .panel_groups
                        .get(group_idx)
                        .map(|g| g.expanded_index() == panel_idx)
                        .unwrap_or(false);

                // Always activate the clicked panel
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    group.set_expanded(panel_idx);
                }
                self.layout_manager.focus = group_idx;

                if was_active {
                    // Panel was already active — check for double-click
                    if self
                        .title_click_tracker
                        .is_double_click(&(click_x, click_y))
                    {
                        self.title_click_tracker.reset();
                        // Double-click on active FileManager title → open directory picker
                        if let Some(panel) = self.layout_manager.active_panel_mut() {
                            if let Some(fm) = panel.as_file_manager_mut() {
                                let current_path = fm.current_path().to_path_buf();
                                let t = i18n::t();
                                let modal = modal::DirectoryPickerModal::new(
                                    current_path,
                                    t.directory_switcher_title().to_string(),
                                    t.directory_picker_move().to_string(),
                                );
                                self.state.set_pending_action(
                                    PendingAction::SwitchDirectory,
                                    ActiveModal::DirectoryPicker(Box::new(modal)),
                                );
                                return Ok(true);
                            }
                        }
                    }
                }

                // Record this click for double-click detection
                self.title_click_tracker.record((click_x, click_y));
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Handle click on panel to switch focus
    fn handle_panel_focus_click(&mut self, click_x: u16, click_y: u16) -> Result<()> {
        let panel_rects = self.calculate_panel_rects();

        for (group_idx, panel_idx, rect, _is_expanded) in panel_rects {
            // Check if click is within this panel's bounds
            if click_x >= rect.x
                && click_x < rect.x + rect.width
                && click_y >= rect.y
                && click_y < rect.y + rect.height
            {
                // Close completion popup only when focus is actually changing to different group
                let focus_changing = group_idx != self.layout_manager.focus;
                if focus_changing {
                    if let Some(panel) = self.layout_manager.active_panel_mut() {
                        if let Some(editor) = panel.as_editor_mut() {
                            editor.cancel_completion();
                        }
                    }
                }

                // Click on a panel group - make it active
                self.layout_manager.focus = group_idx;
                if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                    group.set_expanded(panel_idx);
                }
                return Ok(());
            }
        }

        Ok(())
    }

    /// Handle click on menu bar
    fn handle_menu_click(&mut self, x: u16) -> Result<()> {
        let mut current_x = 1_u16;

        // Get menu items with translations
        let menu_items = termide_ui_render::menu::get_menu_items();

        for (i, item) in menu_items.iter().enumerate() {
            let item_width = item.width() as u16;
            if x >= current_x && x < current_x + item_width {
                // Toggle: if this menu item is already open, close it
                if self.state.ui.menu_open && self.state.ui.selected_menu_item == Some(i) {
                    // Restore theme if nested submenu was open
                    if let Some(original_name) = self.state.ui.theme_preview_original.take() {
                        self.state.theme = Theme::get_by_name(&original_name);
                    }
                    self.state.close_menu();
                } else {
                    // Open menu with the selected item
                    self.state.open_menu(Some(i));
                    self.execute_menu_action()?;
                }
                return Ok(());
            }
            current_x += item_width + 2; // +2 for spaces
        }

        // Check CPU/RAM/clock indicator clicks (right side of menu bar)
        let (cpu_range, ram_range, clock_range) = self.get_indicator_ranges();

        if cpu_range.contains(&x) {
            self.open_cpu_modal();
            return Ok(());
        }
        if ram_range.contains(&x) {
            self.open_ram_modal();
            return Ok(());
        }
        if clock_range.contains(&x) {
            self.open_calendar_modal();
            return Ok(());
        }

        Ok(())
    }

    /// Open calendar modal.
    fn open_calendar_modal(&mut self) {
        let modal = modal::CalendarModal::new();
        self.state.active_modal = Some(ActiveModal::Calendar(Box::new(modal)));
        self.state.needs_redraw = true;
    }

    /// Compute CPU, RAM and clock indicator x-ranges in the menu bar.
    fn get_indicator_ranges(
        &self,
    ) -> (
        std::ops::Range<u16>,
        std::ops::Range<u16>,
        std::ops::Range<u16>,
    ) {
        let (ram_value, ram_unit) = self.state.system_monitor.format_ram();
        let params = MenuRenderParams {
            theme: self.state.theme,
            selected_menu_item: self.state.ui.selected_menu_item,
            menu_open: self.state.ui.menu_open,
            cpu_usage: self.state.system_monitor.cpu_usage(),
            ram_percent: self.state.system_monitor.ram_usage_percent(),
            ram_value,
            ram_unit,
            net_down_rate: self.state.system_monitor.net_download_rate(),
            net_up_rate: self.state.system_monitor.net_upload_rate(),
        };
        get_resource_indicator_ranges(self.state.terminal.width, &params)
    }

    /// Handle click on status bar (bottom row)
    fn handle_status_bar_click(&mut self, x: u16) -> Result<()> {
        // Check if disk indicator is present (right-aligned in status bar)
        // Disk indicator text is formatted as " DEVICE: used/totalGB (percent%) "
        // and is right-aligned, so it occupies the last N columns
        let disk_info = self.get_active_panel_disk_space();
        if let Some(disk) = disk_info {
            use termide_system_monitor::DiskSpaceInfoExt;
            let disk_text = format!(" {} ", disk.format_space());
            let disk_start = self
                .state
                .terminal
                .width
                .saturating_sub(disk_text.len() as u16);
            if x >= disk_start {
                self.open_disk_modal();
                return Ok(());
            }
        }
        Ok(())
    }

    /// Get disk space info from the active panel (if available).
    fn get_active_panel_disk_space(&self) -> Option<termide_system_monitor::DiskSpaceInfo> {
        use std::any::Any;
        let panel = self.layout_manager.active_panel()?;
        let panel_any = &**panel as &dyn Any;
        if let Some(fm) = panel_any.downcast_ref::<termide_panel_file_manager::FileManager>() {
            return fm.get_disk_space_info();
        }
        if let Some(editor) = panel_any.downcast_ref::<termide_panel_editor::Editor>() {
            return editor.get_disk_space_info();
        }
        if let Some(git) = panel_any.downcast_ref::<termide_panel_git_status::GitStatusPanel>() {
            return git.get_disk_space_info();
        }
        if let Some(terminal) = panel_any.downcast_ref::<termide_panel_terminal::Terminal>() {
            return terminal.get_terminal_info().disk_space;
        }
        None
    }

    /// Open CPU processes modal.
    fn open_cpu_modal(&mut self) {
        use crate::state::ResourceModalKind;

        let t = i18n::t();
        let lines = self.build_process_lines(ResourceModalKind::Cpu);
        let modal =
            modal::InfoModal::new_rich(t.resource_cpu_top_title(), lines).with_min_width(57);
        self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
        self.state.resource_modal_kind = Some(ResourceModalKind::Cpu);
        self.state.last_resource_modal_refresh = Some(std::time::Instant::now());
        self.state.needs_redraw = true;
    }

    /// Open RAM processes modal.
    fn open_ram_modal(&mut self) {
        use crate::state::ResourceModalKind;

        let t = i18n::t();
        let lines = self.build_process_lines(ResourceModalKind::Ram);
        let modal =
            modal::InfoModal::new_rich(t.resource_ram_top_title(), lines).with_min_width(57);
        self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
        self.state.resource_modal_kind = Some(ResourceModalKind::Ram);
        self.state.last_resource_modal_refresh = Some(std::time::Instant::now());
        self.state.needs_redraw = true;
    }

    /// Build process lines for CPU or RAM modal (header + data rows).
    ///
    /// Column order is always: Application | CPU | RAM (for both modals).
    pub(super) fn build_process_lines(
        &self,
        kind: crate::state::ResourceModalKind,
    ) -> Vec<(String, termide_modal::info::ModalValue)> {
        use crate::state::ResourceModalKind;
        use termide_modal::info::{ModalValue, SegmentStyle, StyledSegment};
        use termide_system_monitor::format_bytes;
        use termide_ui_render::resource_color;
        use unicode_width::UnicodeWidthChar;

        // Fixed name column width so CPU/RAM columns never shift
        const NAME_COL: usize = 24;

        /// Pad or truncate `s` to exactly `width` display columns.
        fn fit_name(s: &str, width: usize) -> String {
            let w = s.width();
            if w <= width {
                // Pad with spaces
                let mut out = s.to_string();
                for _ in 0..(width - w) {
                    out.push(' ');
                }
                out
            } else {
                // Truncate and add "…"
                let mut out = String::new();
                let mut cur = 0;
                for ch in s.chars() {
                    let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
                    if cur + cw > width - 1 {
                        break;
                    }
                    out.push(ch);
                    cur += cw;
                }
                out.push('…');
                cur += 1;
                for _ in 0..(width - cur) {
                    out.push(' ');
                }
                out
            }
        }

        let t = i18n::t();
        let processes = match kind {
            ResourceModalKind::Cpu => self.state.system_monitor.top_cpu_processes(10),
            ResourceModalKind::Ram => self.state.system_monitor.top_memory_processes(10),
        };
        let total_mem = self.state.system_monitor.stats().memory_total;

        // Header row — empty key, all columns in segments
        // Columns: count(6) + CPU(7) + RAM(10) = 23 chars in segments
        let mut lines: Vec<(String, ModalValue)> = vec![(
            fit_name("", NAME_COL),
            ModalValue::Segments(vec![
                StyledSegment {
                    text: format!("{:>6}", t.resource_count()),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>7}", "CPU"),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("  {:>8}", "RAM"),
                    style: SegmentStyle::Default,
                },
            ]),
        )];

        // Data rows
        for p in &processes {
            // CPU color based on per-process percentage
            let cpu_pct = p.cpu_percent.round() as u8;
            let cpu_color = match resource_color(cpu_pct, self.state.theme) {
                c if c == self.state.theme.error => SegmentStyle::Error,
                c if c == self.state.theme.warning => SegmentStyle::Warning,
                _ => SegmentStyle::Success,
            };

            // RAM color based on share of total memory
            let mem_pct = if total_mem > 0 {
                ((p.memory_bytes as f64 / total_mem as f64) * 100.0) as u8
            } else {
                0
            };
            let ram_color = match resource_color(mem_pct, self.state.theme) {
                c if c == self.state.theme.error => SegmentStyle::Error,
                c if c == self.state.theme.warning => SegmentStyle::Warning,
                _ => SegmentStyle::Success,
            };

            let count_text = format!("{:>6}", p.count);

            let segments = vec![
                StyledSegment {
                    text: count_text,
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!(" {:>5.1}%", p.cpu_percent),
                    style: cpu_color,
                },
                StyledSegment {
                    text: format!("  {:>8}", format_bytes(p.memory_bytes)),
                    style: ram_color,
                },
            ];
            lines.push((fit_name(&p.name, NAME_COL), ModalValue::Segments(segments)));
        }

        lines
    }

    /// Open disk space modal.
    fn open_disk_modal(&mut self) {
        use termide_modal::info::{ModalValue, SegmentStyle, StyledSegment};
        use termide_system_monitor::{format_bytes, get_all_disk_space_info};
        use termide_ui_render::resource_color;

        let t = i18n::t();
        let disks = get_all_disk_space_info();

        // Header row
        let mut lines: Vec<(String, ModalValue)> = vec![(
            String::new(),
            ModalValue::Segments(vec![
                StyledSegment {
                    text: "     ".to_string(),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("  {:>8}", t.resource_disk_free()),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("  {:>8}", t.resource_disk_total()),
                    style: SegmentStyle::Default,
                },
            ]),
        )];

        // Data rows
        for d in &disks {
            let name = d.device_name().unwrap_or_else(|| "???".to_string());
            let avail_pct = 100_u8.saturating_sub(d.usage_percent());
            let usage = d.usage_percent();
            let color = match resource_color(usage, self.state.theme) {
                c if c == self.state.theme.error => SegmentStyle::Error,
                c if c == self.state.theme.warning => SegmentStyle::Warning,
                _ => SegmentStyle::Success,
            };
            let segments = vec![
                StyledSegment {
                    text: format!("{:>4}%", avail_pct),
                    style: color,
                },
                StyledSegment {
                    text: format!("  {:>8}", format_bytes(d.available)),
                    style: color,
                },
                StyledSegment {
                    text: format!("  {:>8}", format_bytes(d.total)),
                    style: SegmentStyle::Default,
                },
            ];
            lines.push((name, ModalValue::Segments(segments)));
        }

        let modal = modal::InfoModal::new_rich(t.resource_disk_title(), lines);
        self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
        self.state.needs_redraw = true;
    }

    /// Handle click on Options submenu dropdown
    /// Returns true if click was handled
    fn handle_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        // Get Options dropdown position
        let menu_x = get_menu_item_x_position(OPTIONS_MENU_INDEX);
        let dropdown_y = 1_u16;

        // Calculate Options dropdown dimensions
        let options_items = get_options_items();
        let options_width = options_items
            .iter()
            .map(|i| i.label.width())
            .max()
            .unwrap_or(10) as u16
            + 4;
        let options_height = options_items.len() as u16 + 2; // +2 for borders

        // Check if nested submenu (Themes) is open
        if self.state.ui.nested_submenu.open && self.state.ui.options_submenu.selected == 0 {
            // Theme dropdown is to the right of Options dropdown
            let nested_x = menu_x + options_width;
            let nested_y = dropdown_y + 1;

            let theme_names = Theme::all_theme_names();
            let nested_width = theme_names.iter().map(|n| n.width()).max().unwrap_or(10) as u16 + 6;
            // Must match ThemeDropdown::max_visible
            let max_visible = 25;
            let nested_height = theme_names.len().min(max_visible) as u16 + 2;

            // Check click on theme dropdown
            if x >= nested_x
                && x < nested_x + nested_width
                && y >= nested_y
                && y < nested_y + nested_height
            {
                // Calculate scroll offset same as ThemeDropdown
                let scroll_offset = if self.state.ui.nested_submenu.selected >= max_visible {
                    self.state.ui.nested_submenu.selected - max_visible + 1
                } else {
                    0
                };
                let item_y = y.saturating_sub(nested_y + 1); // -1 for top border
                let item_index = scroll_offset + item_y as usize;
                if item_index < theme_names.len() {
                    // Clear preview state - theme is confirmed
                    self.state.ui.theme_preview_original = None;
                    // Apply selected theme
                    if let Some(name) = theme_names.get(item_index) {
                        self.apply_theme(name)?;
                    }
                    self.state.close_menu();
                    return Ok(true);
                }
            }
        }

        // Check if nested submenu (Language) is open
        if self.state.ui.nested_submenu.open && self.state.ui.options_submenu.selected == 1 {
            // Language dropdown is to the right of Options dropdown
            let nested_x = menu_x + options_width;
            let nested_y = dropdown_y + 2; // Language is at index 1

            let languages = i18n::get_language_list();
            let nested_width = languages
                .iter()
                .map(|(_, name)| name.width())
                .max()
                .unwrap_or(10) as u16
                + 4;
            // Must match LanguageDropdown::max_visible
            let max_visible = 15;
            let nested_height = languages.len().min(max_visible) as u16 + 2;

            // Check click on language dropdown
            if x >= nested_x
                && x < nested_x + nested_width
                && y >= nested_y
                && y < nested_y + nested_height
            {
                // Calculate scroll offset same as LanguageDropdown
                let scroll_offset = if self.state.ui.nested_submenu.selected >= max_visible {
                    self.state.ui.nested_submenu.selected - max_visible + 1
                } else {
                    0
                };
                let item_y = y.saturating_sub(nested_y + 1); // -1 for top border
                let item_index = scroll_offset + item_y as usize;
                if item_index < languages.len() {
                    // Clear preview state - language is confirmed
                    self.state.ui.language_preview_original = None;
                    // Apply selected language
                    if let Some((code, name)) = languages.get(item_index) {
                        self.apply_language(code, name)?;
                    }
                    self.state.close_menu();
                    return Ok(true);
                }
            }
        }

        // Check click on Options dropdown
        if x >= menu_x
            && x < menu_x + options_width
            && y >= dropdown_y
            && y < dropdown_y + options_height
        {
            let item_y = y.saturating_sub(dropdown_y + 1); // -1 for top border
            let item_index = item_y as usize;
            if item_index < options_items.len() {
                self.state.ui.options_submenu.selected = item_index;
                match item_index {
                    0 => {
                        // Themes - toggle nested submenu
                        if self.state.ui.nested_submenu.open
                            && self.state.ui.options_submenu.selected == 0
                        {
                            // Already open - close it and restore theme
                            if let Some(original_name) = self.state.ui.theme_preview_original.take()
                            {
                                self.state.theme = Theme::get_by_name(&original_name);
                            }
                            self.state.close_nested_submenu();
                        } else {
                            // Open nested submenu with live preview
                            let theme_names = Theme::all_theme_names();
                            let current_idx = theme_names
                                .iter()
                                .position(|n| n == self.state.theme.name)
                                .unwrap_or(0);
                            // Save current theme for restoration on cancel
                            self.state.ui.theme_preview_original =
                                Some(self.state.theme.name.to_string());
                            self.state.open_nested_submenu(current_idx);
                        }
                    }
                    1 => {
                        // Language - toggle nested submenu
                        use termide_i18n as i18n;
                        use termide_ui_render::find_current_language_index;
                        if self.state.ui.nested_submenu.open
                            && self.state.ui.options_submenu.selected == 1
                        {
                            // Already open - close it and restore language
                            if let Some(original_lang) =
                                self.state.ui.language_preview_original.take()
                            {
                                let _ = i18n::set_language(&original_lang);
                            }
                            self.state.close_nested_submenu();
                        } else {
                            // Open nested submenu with live preview
                            let current_idx = find_current_language_index();
                            // Save current language for restoration on cancel
                            self.state.ui.language_preview_original =
                                Some(i18n::current_language());
                            self.state.open_nested_submenu(current_idx);
                        }
                    }
                    2 => {
                        // Manage scripts
                        self.state.close_menu();
                        self.handle_manage_scripts()?;
                    }
                    3 => {
                        // Manage bookmarks
                        self.state.close_menu();
                        self.handle_manage_bookmarks()?;
                    }
                    4 => {
                        // Edit preferences
                        self.state.close_menu();
                        self.open_config_in_editor()?;
                    }
                    5 => {
                        // Help
                        self.state.close_menu();
                        self.handle_new_help()?;
                    }
                    6 => {
                        // Quit
                        self.state.close_menu();
                        if self.has_panels_requiring_confirmation() {
                            use crate::state::{ActiveModal, PendingAction};
                            use termide_i18n as i18n;
                            let t = i18n::t();
                            let modal = termide_modal::ConfirmModal::new(
                                t.app_quit_title(),
                                t.app_quit_confirm(),
                            );
                            self.state.set_pending_action(
                                PendingAction::QuitApplication,
                                ActiveModal::Confirm(Box::new(modal)),
                            );
                        } else {
                            self.state.quit();
                        }
                    }
                    _ => {}
                }
                return Ok(true);
            }
        }

        // Click outside dropdowns - close all menus
        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on Sessions submenu dropdown
    /// Returns true if click was handled
    fn handle_sessions_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let menu_x = get_menu_item_x_position(SESSIONS_MENU_INDEX);
        let items = get_sessions_items();
        if let Some(index) = hit_dropdown_item(x, y, menu_x, 1, &items) {
            self.state.ui.sessions_submenu.selected = index;
            self.execute_sessions_submenu_action()?;
            return Ok(true);
        }
        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on Tools submenu dropdown
    /// Returns true if click was handled
    fn handle_tools_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let menu_x = get_menu_item_x_position(WINDOWS_MENU_INDEX);
        let items = get_tools_items();

        // If shell picker nested submenu is open, check clicks on it first
        if self.state.ui.tools_nested.open {
            let shell_items = get_shell_items(
                &self.state.cached_shells,
                self.state.config.terminal.default_shell.as_deref(),
            );
            if !shell_items.is_empty() {
                // Calculate nested dropdown position (same formula as in ui.rs rendering)
                let dropdown_y = 1_u16;
                let parent_width =
                    items.iter().map(|i| i.label.width()).max().unwrap_or(10) as u16 + 4;
                let nested_x = menu_x + parent_width;
                let nested_y = dropdown_y + 1 + self.state.ui.tools_submenu.selected as u16;
                if let Some(index) = hit_dropdown_item(x, y, nested_x, nested_y, &shell_items) {
                    if let Some(shell) = self.state.cached_shells.get(index) {
                        let shell_path = shell.path.clone();
                        self.state.config.terminal.default_shell = Some(shell_path.clone());
                        if let Err(e) = self.save_shell_preference(&shell_path) {
                            log::warn!("Failed to save shell preference: {}", e);
                        }
                        self.state.close_menu();
                        self.handle_new_terminal_with_shell(Some(&shell_path))?;
                        return Ok(true);
                    }
                }
            }
        }

        // Check click on Tools main dropdown
        if let Some(index) = hit_dropdown_item(x, y, menu_x, 1, &items) {
            self.state.ui.tools_submenu.selected = index;
            self.execute_tools_submenu_action()?;
            return Ok(true);
        }
        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on Scripts submenu dropdown
    /// Returns true if click was handled
    fn handle_scripts_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let registry = match termide_config::scripts::ScriptsRegistry::load() {
            Some(r) => r,
            None => {
                self.state.close_menu();
                return Ok(true);
            }
        };

        // If nested submenu is open, handle clicks on it first
        if self.state.ui.scripts_nested.open {
            if let Some(group_name) = self.state.ui.current_scripts_group.as_ref() {
                let nested_items = get_scripts_group_items(&registry, group_name);
                if !nested_items.is_empty() {
                    let menu_x = get_menu_item_x_position(SCRIPTS_MENU_INDEX);
                    let parent_items = get_scripts_items(&registry);
                    let parent_width = parent_items
                        .iter()
                        .map(|i| i.label.width())
                        .max()
                        .unwrap_or(10) as u16
                        + 4;
                    let nested_x = menu_x + parent_width;
                    let nested_y = 2 + self.state.ui.scripts_submenu.selected as u16;
                    if let Some(index) = hit_dropdown_item(x, y, nested_x, nested_y, &nested_items)
                    {
                        self.state.ui.scripts_nested.selected = index;
                        self.execute_scripts_nested_action()?;
                        return Ok(true);
                    }
                }
            }
        }

        // Check click on Scripts main dropdown
        let menu_x = get_menu_item_x_position(SCRIPTS_MENU_INDEX);
        let scripts_items = get_scripts_items(&registry);
        if let Some(index) = hit_dropdown_item(x, y, menu_x, 1, &scripts_items) {
            self.state.ui.scripts_submenu.selected = index;
            self.execute_scripts_submenu_action()?;
            return Ok(true);
        }

        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on Bookmarks submenu dropdown
    /// Returns true if click was handled
    fn handle_bookmarks_submenu_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let bookmarks_items = get_bookmarks_items(&self.state.bookmarks);

        // If nested submenu is open, handle clicks on it first
        if self.state.ui.bookmarks_nested.open {
            if let Some(group_name) = self.state.ui.current_bookmarks_group.as_ref() {
                let nested_items = get_bookmarks_group_items(&self.state.bookmarks, group_name);
                if !nested_items.is_empty() {
                    let menu_x = get_menu_item_x_position(BOOKMARKS_MENU_INDEX);
                    let parent_width = bookmarks_items
                        .iter()
                        .map(|i| i.label.width())
                        .max()
                        .unwrap_or(10) as u16
                        + 4;
                    let nested_x = menu_x + parent_width;
                    let nested_y = 2 + self.state.ui.bookmarks_submenu.selected as u16;
                    if let Some(index) = hit_dropdown_item(x, y, nested_x, nested_y, &nested_items)
                    {
                        self.state.ui.bookmarks_nested.selected = index;
                        self.execute_bookmarks_nested_action()?;
                        return Ok(true);
                    }
                }
            }
        }

        // Check click on Bookmarks main dropdown
        let menu_x = get_menu_item_x_position(BOOKMARKS_MENU_INDEX);
        if let Some(index) = hit_dropdown_item(x, y, menu_x, 1, &bookmarks_items) {
            self.state.ui.bookmarks_submenu.selected = index;
            self.execute_bookmarks_submenu_action()?;
            return Ok(true);
        }

        self.state.close_menu();
        Ok(true)
    }

    /// Handle click on divider to start resize drag
    /// Returns true if click was on a divider
    fn handle_divider_click(&mut self, x: u16, y: u16) -> Result<bool> {
        let terminal_height = self.state.terminal.height;

        if let Some(divider_idx) =
            self.layout_manager
                .find_divider_at_position(x, y, terminal_height)
        {
            // Get current widths of adjacent groups
            let left_width = self
                .layout_manager
                .panel_groups
                .get(divider_idx)
                .and_then(|g| g.width)
                .unwrap_or(0);
            let right_width = self
                .layout_manager
                .panel_groups
                .get(divider_idx + 1)
                .and_then(|g| g.width)
                .unwrap_or(0);

            // Start drag
            self.state
                .ui
                .drag
                .start(divider_idx, x, left_width, right_width);
            self.state.needs_redraw = true;

            return Ok(true);
        }

        Ok(false)
    }

    /// Handle divider drag — track cursor position and draw ghost line.
    /// Only the ghost divider line is rendered (lightweight), while actual
    /// panel resize is deferred to mouse release.
    fn handle_divider_drag(&mut self, current_x: u16) -> Result<()> {
        if self.state.ui.drag.last_applied_x == Some(current_x) {
            return Ok(());
        }
        self.state.ui.drag.last_applied_x = Some(current_x);
        // Trigger redraw for the ghost line — this is lightweight because
        // only ~2 columns change (ratatui diff rendering sends minimal data).
        self.state.needs_redraw = true;
        Ok(())
    }

    /// Handle divider drag end — apply final widths and redraw once.
    fn handle_divider_drag_end(&mut self) -> Result<()> {
        // Apply the accumulated drag position as a single resize
        if let (Some(divider_idx), Some(final_x)) = (
            self.state.ui.drag.active_divider,
            self.state.ui.drag.last_applied_x,
        ) {
            let delta = final_x as i32 - self.state.ui.drag.start_x as i32;
            let (start_left, start_right) = self.state.ui.drag.start_widths;

            let min_width = self.state.config.general.min_panel_width;
            let total_width = start_left + start_right;

            let new_left = (start_left as i32 + delta)
                .max(min_width as i32)
                .min((total_width - min_width) as i32) as u16;
            let new_right = total_width - new_left;

            self.layout_manager
                .resize_groups(divider_idx, new_left, new_right);
        }

        self.state.ui.drag.end();
        self.state.needs_redraw = true;

        // Save session with new widths
        self.auto_save_session();

        Ok(())
    }

    /// Find the expanded panel group at the given screen coordinates.
    /// Returns `(group_idx, rect)` if an expanded panel contains the point.
    fn find_expanded_panel_group_at(&self, x: u16, y: u16) -> Option<(usize, Rect)> {
        for (group_idx, _panel_idx, rect, is_expanded) in self.calculate_panel_rects() {
            if !is_expanded {
                continue;
            }
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return Some((group_idx, rect));
            }
        }
        None
    }

    /// Calculate panel rectangles for mouse hit testing
    /// Returns Vec<(group_idx, panel_idx, rect, is_expanded)>
    fn calculate_panel_rects(&self) -> Vec<(usize, usize, Rect, bool)> {
        let mut result = Vec::new();

        let width = self.state.terminal.width;
        let height = self.state.terminal.height;

        // Main area: from row 1 to height-2 (excluding menu and status bar)
        let main_area = Rect {
            x: 0,
            y: 1,
            width,
            height: height.saturating_sub(2),
        };

        // Calculate group areas
        if !self.layout_manager.panel_groups.is_empty() {
            let groups_area = main_area;

            // Calculate horizontal constraints for groups (using widths)
            // Группы могут иметь фиксированную ширину или auto-width
            let group_constraints: Vec<Constraint> = self
                .layout_manager
                .panel_groups
                .iter()
                .map(|g| {
                    let width = g.width.unwrap_or(groups_area.width);
                    Constraint::Length(width.max(20))
                })
                .collect();

            let group_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(group_constraints)
                .split(groups_area);

            // Process each group
            for (group_idx, group) in self.layout_manager.panel_groups.iter().enumerate() {
                if group.is_empty() || group_chunks[group_idx].height == 0 {
                    continue;
                }

                let group_area = group_chunks[group_idx];
                let expanded_idx = group.expanded_index();

                // Build vertical constraints for panels in group
                let vertical_constraints: Vec<Constraint> = (0..group.len())
                    .map(|i| {
                        if i == expanded_idx {
                            Constraint::Min(0) // Expanded panel
                        } else {
                            Constraint::Length(1) // Collapsed panel (1 line)
                        }
                    })
                    .collect();

                let vertical_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(vertical_constraints)
                    .split(group_area);

                // Add each panel's rect to results
                for panel_idx in 0..group.len() {
                    let is_expanded = panel_idx == expanded_idx;
                    result.push((
                        group_idx,
                        panel_idx,
                        vertical_chunks[panel_idx],
                        is_expanded,
                    ));
                }
            }
        }

        result
    }

    /// Handle coalesced scroll events (batched for performance).
    ///
    /// This method processes multiple scroll events that have been coalesced
    /// into a single event with a combined delta value.
    pub(super) fn handle_coalesced_scroll(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        delta: i32,
    ) -> Result<()> {
        log::trace!(
            "Scroll event: delta={}, col={}, row={}",
            delta,
            mouse.column,
            mouse.row
        );

        // Track scroll timing for throttling heavy operations in Event::Tick
        self.state.last_mouse_scroll = Some(std::time::Instant::now());

        // Skip scroll when modal is active
        if self.state.active_modal.is_some() {
            return Ok(());
        }

        self.forward_coalesced_scroll_to_panel(mouse, delta)
    }

    /// Forward coalesced scroll to the panel under the mouse cursor.
    fn forward_coalesced_scroll_to_panel(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        delta: i32,
    ) -> Result<()> {
        if let Some((group_idx, rect)) = self.find_expanded_panel_group_at(mouse.column, mouse.row)
        {
            if let Some(group) = self.layout_manager.panel_groups.get_mut(group_idx) {
                if let Some(panel) = group.expanded_panel_mut() {
                    let events = panel.handle_scroll(delta, rect);
                    self.process_panel_events(events)?;
                }
            }
        }

        Ok(())
    }
}
