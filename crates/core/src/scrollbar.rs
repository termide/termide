//! Scrollbar geometry contract between panels and the mouse dispatcher.
//!
//! Panels draw their own scrollbars and know the scroll units they count in
//! (list rows, virtual wrapped lines, table columns, …). The dispatcher only
//! needs to know where the bar and its thumb ended up on screen, so it can
//! tell a thumb grab apart from a border grab: the thumb belongs to the
//! panel, everything else on the border stays with the layout resize.
//!
//! Panels report geometry from their last render through
//! [`crate::PanelCommand::GetScrollBars`] and accept an absolute position
//! back through [`crate::PanelCommand::SetScrollOffset`], expressed in the
//! same units they reported.

/// Axis a scrollbar runs along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollAxis {
    /// Vertical bar, drawn down a panel's right border.
    Vertical,
    /// Horizontal bar, drawn along a panel's bottom border.
    Horizontal,
}

/// Geometry of one scrollbar as it was last drawn, in absolute terminal
/// coordinates, plus the scroll range it represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollBarGeometry {
    /// Which axis this bar scrolls.
    pub axis: ScrollAxis,
    /// Column of a vertical bar; first column of a horizontal one.
    pub x: u16,
    /// First row of a vertical bar; row of a horizontal one.
    pub y: u16,
    /// Track length — rows for [`ScrollAxis::Vertical`], columns for
    /// [`ScrollAxis::Horizontal`].
    pub len: u16,
    /// Thumb start, measured from the track start along the axis.
    pub thumb_pos: u16,
    /// Thumb length along the axis. Never zero.
    pub thumb_len: u16,
    /// Scroll offset the thumb position was derived from.
    pub offset: usize,
    /// Viewport size in scroll units.
    pub visible: usize,
    /// Content size in scroll units.
    pub total: usize,
}

impl ScrollBarGeometry {
    /// Coordinate of the track start along the bar's own axis.
    fn track_start(&self) -> u16 {
        match self.axis {
            ScrollAxis::Vertical => self.y,
            ScrollAxis::Horizontal => self.x,
        }
    }

    /// Project a screen position onto the bar's axis.
    fn along(&self, x: u16, y: u16) -> u16 {
        match self.axis {
            ScrollAxis::Vertical => y,
            ScrollAxis::Horizontal => x,
        }
    }

    /// Whether a screen position sits on the bar at all (thumb or track).
    pub fn hits_bar(&self, x: u16, y: u16) -> bool {
        let on_line = match self.axis {
            ScrollAxis::Vertical => x == self.x,
            ScrollAxis::Horizontal => y == self.y,
        };
        let pos = self.along(x, y);
        on_line && pos >= self.track_start() && pos < self.track_start() + self.len
    }

    /// Whether a screen position sits on the thumb — the only part of the
    /// border the panel takes over from the layout resize handles.
    pub fn hits_thumb(&self, x: u16, y: u16) -> bool {
        if !self.hits_bar(x, y) {
            return false;
        }
        let rel = self.along(x, y) - self.track_start();
        rel >= self.thumb_pos && rel < self.thumb_pos + self.thumb_len
    }

    /// Offset of the cursor inside the thumb, so a drag can keep the grab
    /// point under the cursor instead of snapping the thumb's top to it.
    /// `None` when the position is not on the thumb.
    pub fn grab_offset(&self, x: u16, y: u16) -> Option<u16> {
        if !self.hits_thumb(x, y) {
            return None;
        }
        Some(self.along(x, y) - self.track_start() - self.thumb_pos)
    }

    /// Largest scroll offset that still shows content, i.e. the offset the
    /// thumb reaches at the end of the track.
    pub fn max_offset(&self) -> usize {
        self.total.saturating_sub(self.visible)
    }

    /// Scroll offset for a thumb placed `thumb_start` cells from the track
    /// start. Inverse of the thumb placement in `ScrollBar::geometry`, so a
    /// drag maps back to exactly the position the user sees.
    pub fn offset_for_thumb_start(&self, thumb_start: u16) -> usize {
        let span = self.len.saturating_sub(self.thumb_len);
        let max_offset = self.max_offset();
        if span == 0 || max_offset == 0 {
            return max_offset;
        }
        let clamped = thumb_start.min(span);
        // Round to the nearest unit: dragging half a cell past a boundary
        // should land on the next offset, not stay behind.
        let scaled = clamped as usize * max_offset;
        let half = span as usize / 2;
        ((scaled + half) / span as usize).min(max_offset)
    }

    /// Scroll offset for a cursor at a screen position, honouring the point
    /// inside the thumb the drag started from.
    pub fn offset_for_drag(&self, x: u16, y: u16, grab_offset: u16) -> usize {
        let pos = self.along(x, y);
        let start = self.track_start().saturating_add(grab_offset);
        let thumb_start = pos.saturating_sub(start);
        self.offset_for_thumb_start(thumb_start)
    }
}

/// The scrollbars a panel drew on its last render.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScrollBars {
    /// Vertical bar on the right border, when the content overflows.
    pub vertical: Option<ScrollBarGeometry>,
    /// Horizontal bar on the bottom border, when the content overflows.
    pub horizontal: Option<ScrollBarGeometry>,
}

impl ScrollBars {
    /// The bar whose thumb sits under a screen position, if any.
    pub fn hit_thumb(&self, x: u16, y: u16) -> Option<ScrollBarGeometry> {
        self.vertical
            .filter(|bar| bar.hits_thumb(x, y))
            .or_else(|| self.horizontal.filter(|bar| bar.hits_thumb(x, y)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertical(len: u16, thumb_pos: u16, thumb_len: u16, total: usize) -> ScrollBarGeometry {
        ScrollBarGeometry {
            axis: ScrollAxis::Vertical,
            x: 40,
            y: 5,
            len,
            thumb_pos,
            thumb_len,
            offset: 0,
            visible: len as usize,
            total,
        }
    }

    #[test]
    fn thumb_hit_test_is_limited_to_the_thumb_rows() {
        let bar = vertical(10, 3, 2, 50);
        assert!(!bar.hits_thumb(40, 5), "track above the thumb");
        assert!(bar.hits_thumb(40, 8), "first thumb row");
        assert!(bar.hits_thumb(40, 9), "last thumb row");
        assert!(!bar.hits_thumb(40, 10), "track below the thumb");
        assert!(!bar.hits_thumb(39, 8), "column left of the bar");
        assert!(!bar.hits_thumb(40, 4), "row above the track");
        assert!(!bar.hits_thumb(40, 15), "row below the track");
    }

    #[test]
    fn grab_offset_tracks_the_point_inside_the_thumb() {
        let bar = vertical(10, 3, 3, 50);
        assert_eq!(bar.grab_offset(40, 8), Some(0));
        assert_eq!(bar.grab_offset(40, 10), Some(2));
        assert_eq!(bar.grab_offset(40, 11), None);
    }

    #[test]
    fn thumb_start_maps_to_the_scroll_range_ends() {
        // 10-row track, 2-row thumb → 8 cells of travel for 40 offsets.
        let bar = vertical(10, 0, 2, 50);
        assert_eq!(bar.offset_for_thumb_start(0), 0);
        assert_eq!(bar.offset_for_thumb_start(8), 40);
        assert_eq!(bar.offset_for_thumb_start(20), 40, "clamped past the end");
        assert_eq!(bar.offset_for_thumb_start(4), 20, "midpoint");
    }

    #[test]
    fn drag_keeps_the_grab_point_under_the_cursor() {
        let bar = vertical(10, 0, 2, 50);
        // Grabbed the thumb's second row: the thumb top stays one row above
        // the cursor, so a cursor at the track's 5th row means thumb_start 4.
        assert_eq!(bar.offset_for_drag(40, 5 + 5, 1), 20);
        // Dragging above the track start pins to the top instead of wrapping.
        assert_eq!(bar.offset_for_drag(40, 0, 1), 0);
    }

    #[test]
    fn unscrollable_bar_reports_a_single_offset() {
        let bar = vertical(10, 0, 10, 10);
        assert_eq!(bar.max_offset(), 0);
        assert_eq!(bar.offset_for_thumb_start(4), 0);
    }

    #[test]
    fn horizontal_bar_hit_tests_along_columns() {
        let bar = ScrollBarGeometry {
            axis: ScrollAxis::Horizontal,
            x: 10,
            y: 20,
            len: 10,
            thumb_pos: 2,
            thumb_len: 3,
            offset: 0,
            visible: 10,
            total: 30,
        };
        assert!(bar.hits_thumb(12, 20));
        assert!(bar.hits_thumb(14, 20));
        assert!(!bar.hits_thumb(15, 20));
        assert!(!bar.hits_thumb(12, 21));
        assert_eq!(bar.grab_offset(13, 20), Some(1));
    }

    #[test]
    fn scroll_bars_pick_the_axis_under_the_cursor() {
        let v = vertical(10, 0, 2, 50);
        let h = ScrollBarGeometry {
            axis: ScrollAxis::Horizontal,
            x: 10,
            y: 20,
            len: 10,
            thumb_pos: 0,
            thumb_len: 2,
            offset: 0,
            visible: 10,
            total: 30,
        };
        let bars = ScrollBars {
            vertical: Some(v),
            horizontal: Some(h),
        };
        assert_eq!(
            bars.hit_thumb(40, 5).map(|b| b.axis),
            Some(ScrollAxis::Vertical)
        );
        assert_eq!(
            bars.hit_thumb(10, 20).map(|b| b.axis),
            Some(ScrollAxis::Horizontal)
        );
        assert!(bars.hit_thumb(1, 1).is_none());
    }
}
