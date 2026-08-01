//! Terminal mouse handling: translating mouse events into the PTY's mouse
//! reporting protocol (when an app enables tracking) and, otherwise, driving
//! local selection/scroll/link interaction. Driven by `Panel::handle_mouse`
//! and `handle_scroll` in the parent module.

use anyhow::Result;

use crossterm::event::KeyModifiers;
use ratatui::layout::Rect;

use termide_core::PanelEvent;
use termide_ui::{extract_hex_color_at_col, ColorPreview};

use crate::input_encoding::{can_send_mouse_event, mouse_modifier_bits, mouse_route, MouseRoute};
use crate::link_detection::{self, LinkType};
use crate::selection::{line_selection, word_selection};
use crate::Terminal;

impl Terminal {
    /// Send mouse event to PTY (if mouse tracking is enabled)
    pub(super) fn send_mouse_to_pty(
        &mut self,
        mouse: &crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Result<()> {
        use crossterm::event::{MouseButton, MouseEventKind};
        use std::io::Write;

        let (mouse_tracking, sgr_mode) = {
            let screen = self.read_screen();
            (screen.mouse_tracking, screen.sgr_mouse_mode)
        };

        if !can_send_mouse_event(mouse.kind, mouse_tracking) {
            return Ok(());
        }

        // Split-mode panels can collapse to a 1- or 2-row strip; the
        // inner area below would have width/height = 0 and the clamp
        // bounds below would invert (`min > max`), which panics.
        if panel_area.width < 3 || panel_area.height < 3 {
            return Ok(());
        }

        let inner_x_min = panel_area.x + 1;
        let inner_x_max = panel_area.x + panel_area.width.saturating_sub(2);
        let inner_y_min = panel_area.y + 1;
        let inner_y_max = panel_area.y + panel_area.height.saturating_sub(2);

        let clamped_col = mouse.column.clamp(inner_x_min, inner_x_max);
        let clamped_row = mouse.row.clamp(inner_y_min, inner_y_max);

        // 1-based coordinates for xterm mouse reporting
        let inner_x = clamped_col.saturating_sub(inner_x_min) + 1;
        let inner_y = clamped_row.saturating_sub(inner_y_min) + 1;

        // Reusable buffer to avoid allocations (max SGR sequence is ~20 bytes)
        let mut buf = [0u8; 32];

        // Determine button code and whether this is release event
        let modifier_bits = mouse_modifier_bits(mouse.modifiers);
        let (btn_code, is_release): (u8, bool) = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => (modifier_bits, false),
            MouseEventKind::Down(MouseButton::Middle) => (1 + modifier_bits, false),
            MouseEventKind::Down(MouseButton::Right) => (2 + modifier_bits, false),
            MouseEventKind::Up(MouseButton::Left)
            | MouseEventKind::Up(MouseButton::Middle)
            | MouseEventKind::Up(MouseButton::Right) => (3 + modifier_bits, true),
            MouseEventKind::Drag(MouseButton::Left) => (32 + modifier_bits, false),
            MouseEventKind::Drag(MouseButton::Middle) => (33 + modifier_bits, false),
            MouseEventKind::Drag(MouseButton::Right) => (34 + modifier_bits, false),
            MouseEventKind::Moved => (35 + modifier_bits, false),
            MouseEventKind::ScrollUp => (64 + modifier_bits, false),
            MouseEventKind::ScrollDown => (65 + modifier_bits, false),
            _ => return Ok(()),
        };

        // Build sequence directly into buffer (zero allocation)
        let len = if sgr_mode {
            // SGR format: ESC [ < btn ; x ; y (M for press, m for release)
            let suffix: u8 = if is_release { b'm' } else { b'M' };
            let mut cursor = std::io::Cursor::new(&mut buf[..]);
            write!(cursor, "\x1b[<{};{};{}", btn_code, inner_x, inner_y).ok();
            let pos = cursor.position() as usize;
            buf[pos] = suffix;
            pos + 1
        } else {
            // X10/Normal format: ESC [ M <btn+32> <x+32> <y+32>
            // Release in non-SGR mode always uses button code 3
            let effective_btn = if is_release { 3 } else { btn_code };
            buf[0] = b'\x1b';
            buf[1] = b'[';
            buf[2] = b'M';
            buf[3] = effective_btn.saturating_add(32);
            buf[4] = (inner_x as u8).saturating_add(32);
            buf[5] = (inner_y as u8).saturating_add(32);
            6
        };

        self.send_input(&buf[..len])?;
        Ok(())
    }

    pub(super) fn on_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        panel_area: Rect,
    ) -> Vec<PanelEvent> {
        use crossterm::event::{MouseButton, MouseEventKind};

        // If process exited, don't handle mouse
        if !self.is_alive() {
            return vec![];
        }

        // Split-mode panels can collapse to a 1- or 2-row strip; the
        // inner area would have zero width/height and the clamp bounds
        // below would invert (`min > max`), which panics.
        if panel_area.width < 3 || panel_area.height < 3 {
            return vec![];
        }

        // When the find bar is docked at the top, the grid starts below it
        // (bar rows + separator). A click in that region is routed to the bar
        // (toggles / Prev / Next); grid rows below are offset so they map to
        // the correct PTY cell.
        let bar_offset = self.find_bar.as_ref().map(|b| b.height() + 1).unwrap_or(0);
        if bar_offset > 0 {
            let bar_top = panel_area.y + 1;
            let bar_bottom = bar_top + bar_offset; // exclusive (includes separator)
            if mouse.row >= bar_top && mouse.row < bar_bottom {
                if let Some(mut bar) = self.find_bar.take() {
                    let action = bar.handle_mouse(mouse);
                    self.find_bar = Some(bar);
                    return self.apply_find_bar_action(action);
                }
                return vec![];
            }
        }

        // Calculate inner area (without border); the grid starts below the bar.
        let inner_x_min = panel_area.x + 1;
        let inner_x_max = panel_area.x + panel_area.width.saturating_sub(2);
        let inner_y_min = panel_area.y + 1 + bar_offset;
        let inner_y_max = panel_area.y + panel_area.height.saturating_sub(2);
        // A bar on a very short panel can leave no grid rows; avoid an inverted
        // clamp range below.
        if inner_y_min > inner_y_max {
            return vec![];
        }

        // Calculate coordinates relative to terminal inner area (0-based for selection)
        // Clamped to panel boundaries
        let clamped_col = mouse.column.clamp(inner_x_min, inner_x_max);
        let clamped_row = mouse.row.clamp(inner_y_min, inner_y_max);
        let inner_col = clamped_col.saturating_sub(inner_x_min) as usize;
        let inner_row = clamped_row.saturating_sub(inner_y_min) as usize;

        // Check if click is inside terminal area
        let is_inside = mouse.column >= inner_x_min
            && mouse.column <= inner_x_max
            && mouse.row >= inner_y_min
            && mouse.row <= inner_y_max;

        // Save panel bounds and mouse position for auto-scroll in tick()
        self.panel_bounds = Some(panel_area);
        self.last_mouse_position = Some((mouse.column, mouse.row));

        // Track Ctrl key state for URL highlighting
        let ctrl_pressed = mouse.modifiers.contains(KeyModifiers::CONTROL);
        let alt_pressed = mouse.modifiers.contains(KeyModifiers::ALT);
        self.ctrl_pressed = ctrl_pressed;
        let mut needs_redraw = false;

        // Detect link (URL or path) under cursor when Ctrl is pressed
        if ctrl_pressed && is_inside {
            let screen = self.read_screen();
            let abs_row = screen.visual_to_absolute(inner_row);
            let cols = screen.cols;

            if let Some((link_type, link_start_row, link_start_col, display_len)) =
                link_detection::detect_link_at_position(
                    &screen,
                    abs_row,
                    inner_col,
                    &self.initial_cwd,
                )
            {
                // Link found - check if it's new
                let is_new_link = self
                    .hovered_link
                    .as_ref()
                    .map(|(l, _)| l != &link_type)
                    .unwrap_or(true);

                // Build segments for multi-line highlighting
                let segments = link_detection::build_link_segments(
                    display_len,
                    link_start_row,
                    link_start_col,
                    cols,
                );
                drop(screen);

                if is_new_link {
                    // Copy link text to clipboard
                    let _ = termide_ui::clipboard::copy(&link_detection::link_text(&link_type));
                }
                self.hovered_link = Some((link_type, segments));
                self.cached_lines = None; // Force redraw
                needs_redraw = true;
            } else {
                // No link under cursor
                drop(screen);
                if self.hovered_link.is_some() {
                    self.hovered_link = None;
                    self.cached_lines = None; // Force redraw
                    needs_redraw = true;
                }
            }
        } else if !ctrl_pressed && self.hovered_link.is_some() {
            // Ctrl not pressed - clear link highlight
            self.hovered_link = None;
            self.cached_lines = None; // Force redraw
            needs_redraw = true;
        }

        // `selection_active` gates whether an in-progress local selection drag
        // keeps capturing drag/up events even over a mouse-tracking app. It
        // must reflect a drag IN PROGRESS, not a completed selection that still
        // shows a highlight: otherwise, once a tracking app (e.g. an agent)
        // turns mouse reporting on, every click's button-up would be treated as
        // a selection drag and extend the stale selection to the click point —
        // which then can't be cleared.
        let selection_active = self.selection_drag_active;
        let mouse_tracking = self.read_screen().mouse_tracking;

        // If mouse is outside and selection is not active - ignore other events
        if !is_inside && !selection_active {
            return if needs_redraw {
                vec![PanelEvent::NeedsRedraw]
            } else {
                vec![]
            };
        }

        let route = mouse_route(
            mouse.kind,
            is_inside,
            selection_active,
            mouse_tracking,
            alt_pressed,
        );

        // Ctrl+Click local actions override PTY passthrough
        if ctrl_pressed
            && matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
            && is_inside
        {
            let line_text = {
                let screen = self.read_screen();
                let abs_row = screen.visual_to_absolute(inner_row);
                screen
                    .get_line_by_absolute(abs_row)
                    .map(|cells| cells.iter().map(|c| c.ch).collect::<String>())
                    .unwrap_or_default()
            };
            if let Some((r, g, b, hex)) = extract_hex_color_at_col(&line_text, inner_col) {
                self.color_preview = Some(ColorPreview {
                    r,
                    g,
                    b,
                    hex,
                    screen_row: mouse.row,
                    screen_col: mouse.column,
                });
                return vec![PanelEvent::NeedsRedraw];
            }

            if let Some((ref link_type, _)) = self.hovered_link {
                match link_type {
                    LinkType::Url(url) => {
                        let _ = open::that(url);
                        return if needs_redraw {
                            vec![PanelEvent::NeedsRedraw]
                        } else {
                            vec![]
                        };
                    }
                    LinkType::FilePath(path) => {
                        let (dir, file) = if path.is_dir() {
                            (path.clone(), None)
                        } else {
                            (
                                path.parent()
                                    .map(|p| p.to_path_buf())
                                    .unwrap_or_else(|| path.clone()),
                                path.file_name().map(|n| n.to_os_string()),
                            )
                        };
                        return vec![PanelEvent::OpenPath {
                            path: dir,
                            select_file: file,
                        }];
                    }
                }
            }
        }

        match route {
            MouseRoute::LocalScrollback => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.write_screen().scroll_view_up(3);
                    needs_redraw = true;
                }
                MouseEventKind::ScrollDown => {
                    self.write_screen().scroll_view_down(3);
                    needs_redraw = true;
                }
                _ => {}
            },
            MouseRoute::LocalSelection => match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    // Start selection only inside panel
                    if !is_inside {
                        return if needs_redraw {
                            vec![PanelEvent::NeedsRedraw]
                        } else {
                            vec![]
                        };
                    }

                    let abs_row = self.write_screen().visual_to_absolute(inner_row);
                    // 1 = single (start drag select), 2 = word, 3 = line.
                    let clicks = self.click_tracker.click((abs_row, inner_col));
                    let mut screen = self.write_screen();
                    let range = match clicks {
                        2 => word_selection(&screen, abs_row, inner_col),
                        3 => line_selection(&screen, abs_row),
                        _ => None,
                    };
                    if let Some((start, end)) = range {
                        screen.selection_start = Some(start);
                        screen.selection_end = Some(end);
                    } else {
                        screen.selection_start = Some((abs_row, inner_col));
                        screen.selection_end = Some((abs_row, inner_col));
                    }
                    drop(screen);

                    // Only single-click begins a drag selection; word/line
                    // clicks set a fixed selection.
                    self.selection_drag_active = clicks == 1;
                    needs_redraw = true;
                }
                MouseEventKind::Drag(MouseButton::Left) => {
                    let mut screen = self.write_screen();
                    if screen.selection_start.is_some() {
                        // Auto-scroll if mouse is above or below content area
                        let max_scroll = screen.scrollback.len();
                        if mouse.row < inner_y_min && screen.scroll_offset < max_scroll {
                            // Mouse above panel - scroll up into history
                            screen.scroll_view_up(1);
                        } else if mouse.row > inner_y_max && screen.scroll_offset > 0 {
                            // Mouse below panel - scroll down towards current
                            screen.scroll_view_down(1);
                        }

                        // Update selection end with absolute coordinates (using clamped row)
                        let abs_row = screen.visual_to_absolute(inner_row);
                        screen.selection_end = Some((abs_row, inner_col));
                        needs_redraw = true;
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.color_preview = None;

                    self.selection_drag_active = false;
                    self.last_mouse_position = None;

                    let is_single_click = {
                        let mut screen = self.write_screen();
                        let abs_row = screen.visual_to_absolute(inner_row);
                        if let Some(start) = screen.selection_start {
                            screen.selection_end = Some((abs_row, inner_col));
                            start == (abs_row, inner_col)
                        } else {
                            false
                        }
                    };

                    if is_single_click {
                        let mut screen = self.write_screen();
                        screen.clear_selection();
                    }
                    needs_redraw = true;
                }
                _ => {}
            },
            MouseRoute::Pty => {
                self.color_preview = None;
                self.selection_drag_active = false;
                // A click into a mouse-tracking app supersedes any lingering
                // local selection highlight — drop it so it clears instead of
                // staying stuck on screen.
                if matches!(mouse.kind, MouseEventKind::Down(_)) {
                    let mut screen = self.write_screen();
                    if screen.selection_start.is_some() {
                        screen.clear_selection();
                        needs_redraw = true;
                    }
                }
                let _ = self.send_mouse_to_pty(&mouse, panel_area);
            }
            MouseRoute::Ignore => {
                if matches!(mouse.kind, MouseEventKind::Up(MouseButton::Left)) {
                    self.color_preview = None;
                    needs_redraw = true;
                }
            }
        }

        if needs_redraw {
            vec![PanelEvent::NeedsRedraw]
        } else {
            vec![]
        }
    }
}
