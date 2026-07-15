//! Panel rectangle geometry and drag-and-drop hit-testing.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::layout_manager::MIN_GROUP_WIDTH;
use crate::PanelGroup;

/// Compute per-panel rectangles inside `main_area` using the same Layout
/// constraints as the renderer. Returns
/// `Vec<(group_idx, panel_idx, rect, is_expanded)>` — the authoritative
/// geometry used by mouse hit-testing and drag-overlay rendering.
pub fn calculate_panel_rects(
    panel_groups: &[PanelGroup],
    main_area: Rect,
) -> Vec<(usize, usize, Rect, bool)> {
    let mut result = Vec::new();
    if panel_groups.is_empty() {
        return result;
    }

    let group_constraints: Vec<Constraint> = panel_groups
        .iter()
        .map(|g| {
            let width = g.width.unwrap_or(main_area.width);
            Constraint::Length(width.max(MIN_GROUP_WIDTH))
        })
        .collect();

    let group_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(group_constraints)
        .split(main_area);

    for (group_idx, group) in panel_groups.iter().enumerate() {
        if group.is_empty() || group_chunks[group_idx].height == 0 {
            continue;
        }
        let group_area = group_chunks[group_idx];
        let expanded_idx = group.expanded_index();

        let vertical_constraints = compute_vertical_constraints(group, group_area.height);

        let vertical_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vertical_constraints)
            .split(group_area);

        for panel_idx in 0..group.len() {
            // In Accordion mode `is_expanded` carries both meanings (expanded
            // and focused). In Split mode every panel is "expanded" in the
            // visual sense, so the flag means "focused" — rendering uses it
            // to choose the active border colour, which is the right
            // behaviour in both modes.
            let is_expanded = panel_idx == expanded_idx;
            result.push((
                group_idx,
                panel_idx,
                vertical_chunks[panel_idx],
                is_expanded,
            ));
        }
    }

    result
}

/// Compute vertical layout constraints for the panels of `group` given the
/// available `area_height`. Shared by [`calculate_panel_rects`] and the
/// main rendering loop in `src/ui.rs` so geometry stays in lockstep.
///
/// Heights come from the group's cached `split_heights`, falling back to
/// equal distribution. The fullscreen-current-panel preset is just a
/// particular shape of those heights (`[1, …, area_height − (n − 1), …,
/// 1]`) — there is no separate code path.
pub fn compute_vertical_constraints(group: &PanelGroup, area_height: u16) -> Vec<Constraint> {
    let heights = group.effective_split_heights(area_height);
    heights
        .into_iter()
        .map(|h| Constraint::Length(h.max(crate::MIN_PANEL_HEIGHT)))
        .collect()
}

/// Collapse panel rects into group spans `(group_idx, left_edge, right_edge)`,
/// sorted by `group_idx`. Shared between hit-testing and overlay rendering.
pub fn group_spans_from_rects(rects: &[(usize, usize, Rect, bool)]) -> Vec<(usize, u16, u16)> {
    let mut spans: Vec<(usize, u16, u16)> = Vec::new();
    for (gi, _, rect, _) in rects {
        if let Some(entry) = spans.iter_mut().find(|(g, _, _)| *g == *gi) {
            entry.1 = entry.1.min(rect.x);
            entry.2 = entry.2.max(rect.x + rect.width);
        } else {
            spans.push((*gi, rect.x, rect.x + rect.width));
        }
    }
    spans.sort_by_key(|(gi, _, _)| *gi);
    spans
}

/// Determine the drop target under the cursor given pre-calculated panel
/// rects. Shared by the mouse handler and the drag overlay renderer.
///
/// Returns `None` if the cursor is outside the panel area (e.g. in the
/// menu/status bar or over an empty main area).
pub fn compute_drop_target(
    rects: &[(usize, usize, Rect, bool)],
    x: u16,
    y: u16,
) -> Option<PanelDropTarget> {
    if rects.is_empty() {
        return None;
    }

    let group_spans = group_spans_from_rects(rects);

    const GUTTER: u16 = 2;
    for i in 0..group_spans.len().saturating_sub(1) {
        let right_edge = group_spans[i].2;
        let next_left = group_spans[i + 1].1;
        let zone_start = right_edge.saturating_sub(GUTTER);
        let zone_end = next_left.saturating_add(GUTTER);
        if x >= zone_start && x < zone_end {
            return Some(PanelDropTarget::NewGroup {
                insert_at: group_spans[i + 1].0,
            });
        }
    }
    if let Some((last_gi, _, right_edge)) = group_spans.last() {
        if x >= *right_edge {
            return Some(PanelDropTarget::NewGroup {
                insert_at: *last_gi + 1,
            });
        }
    }

    for (gi, pi, rect, _) in rects {
        if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
            let at_position = if y == rect.y { *pi } else { *pi + 1 };
            return Some(PanelDropTarget::IntoGroup {
                group_idx: *gi,
                at_position,
            });
        }
    }

    None
}

/// Where a dragged panel should be dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelDropTarget {
    /// Insert into an existing group at the given position (expanding to it).
    IntoGroup {
        group_idx: usize,
        at_position: usize,
    },
    /// Create a new group at the given index.
    NewGroup { insert_at: usize },
}

/// What a header drag should do when the user releases the mouse.
///
/// Disambiguation is purely spatial:
/// - Cursor over the source panel itself → [`Self::ResizeAbove`].
/// - Cursor over a different panel of the source group, or in another
///   column, or in a between-groups gutter → [`Self::Move`] with the
///   target produced by [`compute_drop_target`].
/// - Cursor outside any panel area (e.g. the menu/status row) →
///   [`Self::Cancel`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelDragIntent {
    /// Resize the divider above the source panel. The renderer should
    /// preview a thick horizontal line at `divider_y`; the drag-end
    /// handler converts that into a delta against the current panel
    /// boundary.
    ResizeAbove { divider_y: u16 },
    /// Apply the existing move semantics (insert into a group or
    /// create a new one). `drop_y` is the cursor's row at release;
    /// for `IntoGroup` drops the drag-end handler uses it to split
    /// the target panel — the upper part keeps the rows above
    /// `drop_y`, the dragged panel takes the rest.
    Move {
        target: PanelDropTarget,
        drop_y: u16,
    },
    /// No valid drop position.
    Cancel,
}

/// Classify a header drag by cursor position.
///
/// The resize zone covers the **bodies** of the source panel and the
/// panel immediately above it — the divider being dragged sits between
/// them, and the cursor naturally roams across both interiors as the
/// divider slides up or down. The **top row** of either panel (its
/// titled header) is intentionally excluded so that dropping the
/// dragged header onto another panel's header reorders panels (a
/// `Move`), not resizes the divider.
///
/// Anything outside that body zone (a non-adjacent panel of the source
/// group, another column, a between-groups gutter, or any panel's
/// header row) falls through to the move semantics produced by
/// [`compute_drop_target`].
///
/// Top-of-group panels (source `panel_idx == 0`) have no divider above
/// them, so the resize zone is empty and any drop becomes a move.
pub fn classify_panel_drag(
    rects: &[(usize, usize, Rect, bool)],
    src_group_idx: usize,
    src_panel_idx: usize,
    cursor_x: u16,
    cursor_y: u16,
) -> PanelDragIntent {
    if src_panel_idx > 0 {
        let in_resize_zone = rects.iter().any(|(gi, pi, rect, _)| {
            *gi == src_group_idx
                && (*pi == src_panel_idx || *pi + 1 == src_panel_idx)
                && cursor_x >= rect.x
                && cursor_x < rect.x + rect.width
                // Strict `>` excludes the header row from the resize
                // zone — drops there reorder via `Move` instead.
                && cursor_y > rect.y
                && cursor_y < rect.y + rect.height
        });
        if in_resize_zone {
            return PanelDragIntent::ResizeAbove {
                divider_y: cursor_y,
            };
        }
    }
    match compute_drop_target(rects, cursor_x, cursor_y) {
        Some(target) => PanelDragIntent::Move {
            target,
            drop_y: cursor_y,
        },
        None => PanelDragIntent::Cancel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drag_rects() -> Vec<(usize, usize, Rect, bool)> {
        // Two panels stacked vertically in one group. Header rows
        // sit at y=0 (panel 0) and y=10 (panel 1).
        vec![
            (0, 0, Rect::new(0, 0, 80, 10), true),
            (0, 1, Rect::new(0, 10, 80, 10), false),
        ]
    }

    #[test]
    fn classify_drop_on_neighbour_header_is_move() {
        // Dragging panel 1's header onto panel 0's header (y=0)
        // must reorder, not resize.
        let rects = drag_rects();
        let intent = classify_panel_drag(&rects, 0, 1, 40, 0);
        assert!(
            matches!(intent, PanelDragIntent::Move { .. }),
            "expected Move when dropping on the upper neighbour's \
             header, got {intent:?}"
        );
    }

    #[test]
    fn classify_drop_in_neighbour_body_is_resize() {
        // Cursor inside panel 0's body (y=5) — divider drag.
        let rects = drag_rects();
        let intent = classify_panel_drag(&rects, 0, 1, 40, 5);
        assert!(
            matches!(intent, PanelDragIntent::ResizeAbove { .. }),
            "expected ResizeAbove for body drop, got {intent:?}"
        );
    }

    #[test]
    fn classify_drop_in_source_body_is_resize() {
        // Cursor inside source's own body (y=15, panel 1 spans 10..20).
        let rects = drag_rects();
        let intent = classify_panel_drag(&rects, 0, 1, 40, 15);
        assert!(
            matches!(intent, PanelDragIntent::ResizeAbove { .. }),
            "expected ResizeAbove for source body drop, got {intent:?}"
        );
    }

    #[test]
    fn classify_drop_on_source_header_is_not_resize() {
        // Cursor on source's own header (y=10) — should not be a
        // resize, and falls through to compute_drop_target / Move.
        let rects = drag_rects();
        let intent = classify_panel_drag(&rects, 0, 1, 40, 10);
        assert!(
            !matches!(intent, PanelDragIntent::ResizeAbove { .. }),
            "drop on source header should not resize, got {intent:?}"
        );
    }
}
