//! Unified scrollbar component for termide.
//!
//! Provides a consistent scrollbar visualization across all panels,
//! dropdowns, and text areas.

use ratatui::{buffer::Buffer, style::Style};
use termide_core::{ScrollAxis, ScrollBarGeometry, ThemeColors};

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
        Self::render_tracked(
            buf, x, y_start, height, offset, visible, total, theme, is_focused,
        );
    }

    /// Render a vertical scrollbar and return where it landed.
    ///
    /// Same drawing as [`ScrollBar::render`]; the returned geometry lets a
    /// panel answer `PanelCommand::GetScrollBars` so mouse grabs on the
    /// thumb can be routed back to it. `None` when no bar was drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn render_tracked(
        buf: &mut Buffer,
        x: u16,
        y_start: u16,
        height: u16,
        offset: usize,
        visible: usize,
        total: usize,
        theme: &ThemeColors,
        is_focused: bool,
    ) -> Option<ScrollBarGeometry> {
        let geometry = Self::geometry(
            ScrollAxis::Vertical,
            x,
            y_start,
            height,
            offset,
            visible,
            total,
        )?;

        let border_color = if is_focused {
            theme.border_focused
        } else {
            theme.disabled
        };
        let track_style = Style::default().fg(border_color);
        let thumb_style = Style::default().fg(border_color);

        for i in 0..height {
            let y = y_start + i;
            if i >= geometry.thumb_pos && i < geometry.thumb_pos + geometry.thumb_len {
                // Thumb - left half-block (connects with │ border line)
                buf[(x, y)].set_symbol("▌").set_style(thumb_style);
            } else {
                // Track - border line
                buf[(x, y)].set_symbol("│").set_style(track_style);
            }
        }

        Some(geometry)
    }

    /// Thumb placement for a bar, or `None` when the content fits and no bar
    /// is drawn.
    ///
    /// Single source of truth for the thumb: both the drawing above and the
    /// mouse hit-testing in [`ScrollBarGeometry`] derive from it, so a grab
    /// always lands where the user sees the thumb.
    #[allow(clippy::too_many_arguments)]
    pub fn geometry(
        axis: ScrollAxis,
        x: u16,
        y: u16,
        len: u16,
        offset: usize,
        visible: usize,
        total: usize,
    ) -> Option<ScrollBarGeometry> {
        if total <= visible || len == 0 {
            return None;
        }
        let visible_ratio = len as f32 / total as f32;
        let thumb_len = (len as f32 * visible_ratio).max(1.0) as u16;
        let max_scroll = total.saturating_sub(visible);
        let scroll_ratio = if max_scroll > 0 {
            offset.min(max_scroll) as f32 / max_scroll as f32
        } else {
            0.0
        };
        let thumb_pos = ((len.saturating_sub(thumb_len)) as f32 * scroll_ratio) as u16;
        Some(ScrollBarGeometry {
            axis,
            x,
            y,
            len,
            thumb_pos,
            thumb_len,
            offset,
            visible,
            total,
        })
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
        Self::render_horizontal_tracked(
            buf, x_start, y, width, offset, visible, total, theme, is_focused,
        );
    }

    /// Render a horizontal scrollbar and return where it landed — the
    /// horizontal counterpart of [`ScrollBar::render_tracked`].
    #[allow(clippy::too_many_arguments)]
    pub fn render_horizontal_tracked(
        buf: &mut Buffer,
        x_start: u16,
        y: u16,
        width: u16,
        offset: usize,
        visible: usize,
        total: usize,
        theme: &ThemeColors,
        is_focused: bool,
    ) -> Option<ScrollBarGeometry> {
        let geometry = Self::geometry(
            ScrollAxis::Horizontal,
            x_start,
            y,
            width,
            offset,
            visible,
            total,
        )?;
        let color = if is_focused {
            theme.border_focused
        } else {
            theme.disabled
        };
        let track = Style::default().fg(color);
        for i in 0..width {
            let sym = if i >= geometry.thumb_pos && i < geometry.thumb_pos + geometry.thumb_len {
                "━"
            } else {
                "─"
            };
            buf[(x_start + i, y)].set_symbol(sym).set_style(track);
        }
        Some(geometry)
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

    /// The mouse hit-test trusts `geometry`; if the drawing ever diverged
    /// from it, a grab would miss the thumb the user sees.
    #[test]
    fn tracked_geometry_matches_the_drawn_thumb() {
        use ratatui::layout::Rect;

        let theme = ThemeColors::default();
        for offset in [0usize, 7, 23, 42] {
            let mut buf = Buffer::empty(Rect::new(0, 0, 4, 12));
            let bar = ScrollBar::render_tracked(&mut buf, 3, 1, 10, offset, 10, 50, &theme, true)
                .expect("content overflows, so a bar is drawn");

            let drawn: Vec<u16> = (1..11).filter(|y| buf[(3, *y)].symbol() == "▌").collect();
            let expected: Vec<u16> = (bar.thumb_pos..bar.thumb_pos + bar.thumb_len)
                .map(|i| 1 + i)
                .collect();
            assert_eq!(drawn, expected, "offset {offset}");
            for y in &drawn {
                assert!(
                    bar.hits_thumb(3, *y),
                    "row {y} draws the thumb but misses the hit-test"
                );
            }
        }
    }

    #[test]
    fn horizontal_tracked_geometry_matches_the_drawn_thumb() {
        use ratatui::layout::Rect;

        let theme = ThemeColors::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 2));
        let bar =
            ScrollBar::render_horizontal_tracked(&mut buf, 1, 1, 10, 12, 10, 40, &theme, false)
                .expect("content overflows, so a bar is drawn");

        let drawn: Vec<u16> = (1..11).filter(|x| buf[(*x, 1)].symbol() == "━").collect();
        let expected: Vec<u16> = (bar.thumb_pos..bar.thumb_pos + bar.thumb_len)
            .map(|i| 1 + i)
            .collect();
        assert_eq!(drawn, expected);
        assert!(drawn.iter().all(|x| bar.hits_thumb(*x, 1)));
    }

    #[test]
    fn no_geometry_when_content_fits() {
        assert!(ScrollBar::geometry(ScrollAxis::Vertical, 0, 0, 10, 0, 10, 10).is_none());
        assert!(ScrollBar::geometry(ScrollAxis::Vertical, 0, 0, 0, 0, 1, 100).is_none());
    }

    #[test]
    fn test_reserved_width() {
        assert_eq!(ScrollBar::reserved_width(10, 5), 0);
        assert_eq!(ScrollBar::reserved_width(10, 10), 0);
        assert_eq!(ScrollBar::reserved_width(10, 15), 1);
    }
}
