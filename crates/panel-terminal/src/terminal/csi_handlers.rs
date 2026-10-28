//! CSI sequence handlers for VT100 parser.
//!
//! This module contains helper functions that handle specific categories
//! of CSI (Control Sequence Introducer) escape sequences, extracted from
//! the main csi_dispatch function for better maintainability.

use ratatui::style::Color;
use vte::Params;

use super::{
    ansi_256_to_color, ansi_to_bright_color, ansi_to_color, CellStyle, MouseTrackingMode,
    TerminalScreen,
};

/// Handle DEC private sequences (sequences starting with '?').
///
/// Returns true if the sequence was handled (caller should return early).
pub fn handle_private_sequence(screen: &mut TerminalScreen, params: &Params, c: char) {
    let mode = params
        .iter()
        .next()
        .and_then(|p| p.first())
        .copied()
        .unwrap_or(0);

    match (mode, c) {
        (1049, 'h') => {
            // Switch to alternate screen and save cursor
            screen.save_cursor();
            screen.switch_to_alt_screen();
        }
        (1049, 'l') => {
            // Return to main screen and restore cursor
            screen.switch_to_main_screen();
            screen.restore_cursor();
            screen.saved_cursor = None;
        }
        (47, 'h') => {
            // Switch to alternate screen (without saving cursor)
            screen.switch_to_alt_screen();
        }
        (47, 'l') => {
            // Return to main screen
            screen.switch_to_main_screen();
        }
        (25, 'h') => {
            // Show cursor
            screen.cursor_visible = true;
        }
        (25, 'l') => {
            // Hide cursor
            screen.cursor_visible = false;
        }
        (1, 'h') => {
            // DECCKM - Application Cursor Keys Mode ON
            screen.application_cursor_keys = true;
        }
        (1, 'l') => {
            // DECCKM - Application Cursor Keys Mode OFF
            screen.application_cursor_keys = false;
        }
        // Mouse tracking modes
        (1000, 'h') => {
            screen.mouse_tracking = MouseTrackingMode::Normal;
        }
        (1000, 'l') => {
            screen.mouse_tracking = MouseTrackingMode::None;
        }
        (1002, 'h') => {
            screen.mouse_tracking = MouseTrackingMode::ButtonEvent;
        }
        (1002, 'l') => {
            screen.mouse_tracking = MouseTrackingMode::None;
        }
        (1003, 'h') => {
            screen.mouse_tracking = MouseTrackingMode::AnyEvent;
        }
        (1003, 'l') => {
            screen.mouse_tracking = MouseTrackingMode::None;
        }
        (1004, 'h') => {
            screen.focus_reporting = true;
        }
        (1004, 'l') => {
            screen.focus_reporting = false;
        }
        (1006, 'h') => {
            screen.sgr_mouse_mode = true;
        }
        (1006, 'l') => {
            screen.sgr_mouse_mode = false;
        }
        (2004, 'h') => {
            screen.bracketed_paste_mode = true;
        }
        (2004, 'l') => {
            screen.bracketed_paste_mode = false;
        }
        (2026, 'h') => {
            // Synchronized output protocol - begin update batch
            screen.sync_output = true;
        }
        (2026, 'l') => {
            // Synchronized output protocol - end update batch
            if screen.sync_output {
                screen.sync_output_ended = true; // Signal cache invalidation needed
            }
            screen.sync_output = false;
        }
        _ => {
            // Unknown private sequence - ignore
        }
    }
    screen.dirty = true;
}

/// Handle SGR (Select Graphic Rendition) sequences for text styling.
///
/// Uses iterator-based parsing to avoid heap allocation (previously allocated
/// a Vec on every SGR call - hundreds of times per htop frame update).
pub fn handle_sgr(screen: &mut TerminalScreen, params: &Params) {
    // Iterate directly without collecting to Vec - eliminates heap allocation
    let mut iter = params.iter().flat_map(|p| p.iter().copied());

    while let Some(p) = iter.next() {
        match p {
            0 => screen.current_style = CellStyle::default(),
            1 => screen.current_style.bold = true,
            3 => screen.current_style.italic = true,
            4 => screen.current_style.underline = true,
            7 => screen.current_style.reverse = true,
            22 => screen.current_style.bold = false,
            23 => screen.current_style.italic = false,
            24 => screen.current_style.underline = false,
            27 => screen.current_style.reverse = false,
            // Standard foreground colors
            30..=37 => {
                screen.current_style.fg = ansi_to_color(p - 30);
            }
            38 => {
                // 256-color or RGB foreground
                match iter.next() {
                    Some(5) => {
                        // 38;5;N - 256-color
                        if let Some(color_idx) = iter.next() {
                            screen.current_style.fg = ansi_256_to_color(color_idx);
                        }
                    }
                    Some(2) => {
                        // 38;2;R;G;B - True Color (24-bit)
                        if let (Some(r), Some(g), Some(b)) = (iter.next(), iter.next(), iter.next())
                        {
                            screen.current_style.fg = Color::Rgb(r as u8, g as u8, b as u8);
                        }
                    }
                    _ => {}
                }
            }
            39 => {
                // Reset foreground to default
                screen.current_style.fg = Color::Reset;
            }
            // Standard background colors
            40..=47 => {
                screen.current_style.bg = ansi_to_color(p - 40);
            }
            48 => {
                // 256-color or RGB background
                match iter.next() {
                    Some(5) => {
                        // 48;5;N - 256-color
                        if let Some(color_idx) = iter.next() {
                            screen.current_style.bg = ansi_256_to_color(color_idx);
                        }
                    }
                    Some(2) => {
                        // 48;2;R;G;B - True Color (24-bit)
                        if let (Some(r), Some(g), Some(b)) = (iter.next(), iter.next(), iter.next())
                        {
                            screen.current_style.bg = Color::Rgb(r as u8, g as u8, b as u8);
                        }
                    }
                    _ => {}
                }
            }
            49 => {
                // Reset background to default
                screen.current_style.bg = Color::Reset;
            }
            // Bright foreground colors
            90..=97 => {
                screen.current_style.fg = ansi_to_bright_color(p - 90);
            }
            // Bright background colors
            100..=107 => {
                screen.current_style.bg = ansi_to_bright_color(p - 100);
            }
            _ => {}
        }
    }
}

/// Handle cursor movement sequences (CUU, CUD, CUF, CUB, CNL, CPL, CHA, VPA, CUP).
///
/// Returns true if the sequence was handled.
pub fn handle_cursor_movement(screen: &mut TerminalScreen, params: &Params, c: char) -> bool {
    let param1 = params
        .iter()
        .next()
        .and_then(|p| p.first())
        .copied()
        .unwrap_or(1) as usize;

    match c {
        'H' | 'f' => {
            // CUP/HVP - Cursor Position
            let row = param1;
            let col = params
                .iter()
                .nth(1)
                .and_then(|p| p.first())
                .copied()
                .unwrap_or(1) as usize;
            screen.move_cursor(row.saturating_sub(1), col.saturating_sub(1));
        }
        'A' => {
            // CUU - Cursor Up
            let n = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.0 = screen.cursor.0.saturating_sub(n);
        }
        'B' => {
            // CUD - Cursor Down
            let n = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.0 = (screen.cursor.0 + n).min(screen.rows.saturating_sub(1));
        }
        'C' => {
            // CUF - Cursor Forward (Right)
            let n = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.1 = (screen.cursor.1 + n).min(screen.cols.saturating_sub(1));
        }
        'D' => {
            // CUB - Cursor Back (Left)
            let n = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.1 = screen.cursor.1.saturating_sub(n);
        }
        'E' => {
            // CNL - Cursor Next Line
            let n = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.0 = (screen.cursor.0 + n).min(screen.rows.saturating_sub(1));
            screen.cursor.1 = 0;
        }
        'F' => {
            // CPL - Cursor Previous Line
            let n = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.0 = screen.cursor.0.saturating_sub(n);
            screen.cursor.1 = 0;
        }
        'G' => {
            // CHA - Cursor Horizontal Absolute
            let col = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.1 = col.saturating_sub(1).min(screen.cols.saturating_sub(1));
        }
        'd' => {
            // VPA - Vertical Position Absolute
            let row = param1.max(1);
            screen.wrap_pending = false;
            screen.cursor.0 = row.saturating_sub(1).min(screen.rows.saturating_sub(1));
        }
        _ => return false,
    }
    true
}
