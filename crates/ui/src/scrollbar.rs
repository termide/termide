//! Unified scrollbar component for termide.
//!
//! Provides a consistent scrollbar visualization across all panels,
//! dropdowns, and text areas.

use ratatui::{buffer::Buffer, style::Style};
use termide_core::ThemeColors;

/// Unified scrollbar component.
///
/// Renders a vertical scrollbar on the right border of a panel.
/// Uses `▌` (left half-block) for the thumb and `│` (border line) for the track.
/// The thumb visually "thickens" the border line to the left, creating a seamless look.
///
/// # Example
///
/// ```ignore
/// ScrollBar::render(
///     buf,
///     x,           // X position (right border)
///     y_start,     // Start Y position
///     height,      // Height of scrollable area
///     offset,      // Current scroll offset
///     visible,     // Number of visible items
///     total,       // Total number of items
///     theme,       // Theme colors
///     is_focused,  // Whether the parent component is focused
/// );
/// ```
pub struct ScrollBar;

impl ScrollBar {
    /// Render a vertical scrollbar.
    ///
    /// # Arguments
    ///
    /// * `buf` - Buffer to render into
    /// * `x` - X position (typically the right border column)
    /// * `y_start` - Starting Y position
    /// * `height` - Height of the scrollbar area
    /// * `offset` - Current scroll offset (first visible item index)
    /// * `visible` - Number of visible items in the viewport
    /// * `total` - Total number of items
    /// * `theme` - Theme colors for styling
    /// * `is_focused` - Whether the parent component is focused (affects thumb color)
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        buf: &mut Buffer,
        x: u16,
        y_start: u16,
        height: u16,
        offset: usize,
        visible: usize,
        total: usize,
        theme: &ThemeColors,
        is_focused: bool,
    ) {
        // No scrollbar needed if all content fits
        if total <= visible || height == 0 {
            return;
        }

        let border_color = if is_focused {
            theme.border_focused
        } else {
            theme.disabled
        };
        let track_style = Style::default().fg(border_color);
        let thumb_style = Style::default().fg(border_color);

        // Calculate thumb size and position
        let visible_ratio = height as f32 / total as f32;
        let thumb_height = (height as f32 * visible_ratio).max(1.0) as u16;
        let max_scroll = total.saturating_sub(visible);
        let scroll_ratio = if max_scroll > 0 {
            offset as f32 / max_scroll as f32
        } else {
            0.0
        };
        let thumb_pos = ((height.saturating_sub(thumb_height)) as f32 * scroll_ratio) as u16;

        // Render scrollbar
        for i in 0..height {
            let y = y_start + i;
            if i >= thumb_pos && i < thumb_pos + thumb_height {
                // Thumb - left half-block (connects with │ border line)
                buf[(x, y)].set_symbol("▌").set_style(thumb_style);
            } else {
                // Track - border line
                buf[(x, y)].set_symbol("│").set_style(track_style);
            }
        }
    }

    /// Render a horizontal scrollbar on a single row (e.g. a panel's bottom
    /// border). Mirrors [`ScrollBar::render`] but along the X axis: `─` track,
    /// `━` thumb. No-op when all content fits (`total <= visible`).
    ///
    /// * `x_start` / `y` — top-left of the bar row.
    /// * `width` — number of columns the bar spans.
    /// * `offset` — first visible column index.
    /// * `visible` — number of visible columns in the viewport.
    /// * `total` — total content width in columns.
    #[allow(clippy::too_many_arguments)]
    pub fn render_horizontal(
        buf: &mut Buffer,
        x_start: u16,
        y: u16,
        width: u16,
        offset: usize,
        visible: usize,
        total: usize,
        theme: &ThemeColors,
        is_focused: bool,
    ) {
        if total <= visible || width == 0 {
            return;
        }
        let color = if is_focused {
            theme.border_focused
        } else {
            theme.disabled
        };
        let track = Style::default().fg(color);
        let visible_ratio = width as f32 / total as f32;
        let thumb_width = (width as f32 * visible_ratio).max(1.0) as u16;
        let max_scroll = total.saturating_sub(visible);
        let scroll_ratio = if max_scroll > 0 {
            offset.min(max_scroll) as f32 / max_scroll as f32
        } else {
            0.0
        };
        let thumb_pos = ((width.saturating_sub(thumb_width)) as f32 * scroll_ratio) as u16;
        for i in 0..width {
            let sym = if i >= thumb_pos && i < thumb_pos + thumb_width {
                "━"
            } else {
                "─"
            };
            buf[(x_start + i, y)].set_symbol(sym).set_style(track);
        }
    }

    /// Check if a scrollbar is needed for the given content.
    ///
    /// Returns `true` if `total > visible`.
    #[inline]
    pub fn needs_scrollbar(visible: usize, total: usize) -> bool {
        total > visible
    }

    /// Calculate the width that should be reserved for the scrollbar.
    ///
    /// Returns 1 if scrollbar is needed, 0 otherwise.
    #[inline]
    pub fn reserved_width(visible: usize, total: usize) -> u16 {
        if Self::needs_scrollbar(visible, total) {
            1
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_scrollbar() {
        assert!(!ScrollBar::needs_scrollbar(10, 5));
        assert!(!ScrollBar::needs_scrollbar(10, 10));
        assert!(ScrollBar::needs_scrollbar(10, 15));
        assert!(ScrollBar::needs_scrollbar(10, 100));
    }

    #[test]
    fn test_reserved_width() {
        assert_eq!(ScrollBar::reserved_width(10, 5), 0);
        assert_eq!(ScrollBar::reserved_width(10, 10), 0);
        assert_eq!(ScrollBar::reserved_width(10, 15), 1);
    }
}
