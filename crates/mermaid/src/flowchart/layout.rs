//! Coordinate geometry: primary-axis ranks + barycentric cross-axis solve.

use crate::parser::Direction;

use super::{Rect, BOX_H};

/// Rows between stacked ranks (vertical) when a labelled elbow crosses the gap:
/// the extra row keeps the label clear of the row a jog routes along.
const V_CHANNEL: usize = 3;
/// Rows for a gap that needs one extra row — an unlabelled elbow (room for the
/// jog) or a labelled straight drop (room for the label beside the line).
const V_CHANNEL_MID: usize = 2;
/// Rows for a plain straight drop with no label or crossing: just the
/// arrowhead, since the box `┬`/`┴` junctions already anchor the connection.
const V_CHANNEL_TIGHT: usize = 1;
/// Columns between stacked ranks (horizontal) — room for arrows + labels.
const H_CHANNEL: usize = 8;
const ROW_GAP: usize = 1;

/// Place every node. The primary axis comes from the rank; the cross axis is
/// solved by [`assign_cross`] to straighten edges.
#[allow(clippy::too_many_arguments)]
pub(super) fn layout(
    direction: Direction,
    groups: &[Vec<usize>],
    box_w: &[usize],
    box_h: &[usize],
    preds: &[Vec<usize>],
    succs: &[Vec<usize>],
    max_rank: usize,
    vertical: bool,
    col_gap: usize,
    seg_ends: &[(usize, usize, bool, bool)],
    is_dummy: &[bool],
) -> Vec<Rect> {
    let n = box_w.len();

    // Cross-axis size per node: box width (vertical) or box height (horizontal).
    let cross_size: Vec<usize> = if vertical {
        box_w.to_vec()
    } else {
        box_h.to_vec()
    };
    let cross_gap = if vertical { col_gap } else { ROW_GAP };
    // A node is a dummy (thin long-edge waypoint) if no rank lists it as a real
    // member with a box — detected by the caller and passed via `is_dummy`.
    let center = assign_cross(groups, &cross_size, cross_gap, preds, succs, is_dummy);

    // Per-gap primary channel (vertical only; horizontal keeps a fixed one):
    //   - label AND elbow   → V_CHANNEL  (label sits clear of the jog row)
    //   - label XOR elbow    → V_CHANNEL_MID (one row for the label or the jog)
    //   - neither (plain drop) → V_CHANNEL_TIGHT (just the arrowhead; the box
    //     `┬`/`┴` junctions anchor the line, so no `│` row is needed).
    let mut node_rank = vec![0usize; n];
    for (r, g) in groups.iter().enumerate() {
        for &i in g {
            node_rank[i] = r;
        }
    }
    let mut channel = vec![if vertical { V_CHANNEL_TIGHT } else { H_CHANNEL }; groups.len()];
    if vertical {
        let mut has_label = vec![false; groups.len()];
        let mut has_elbow = vec![false; groups.len()];
        // A non-solid (dotted/thick) edge needs a line row so its glyph shows,
        // even when it is an otherwise-tight straight drop.
        let mut needs_line = vec![false; groups.len()];
        for &(from, to, lbl, solid) in seg_ends {
            let gap = node_rank[from].min(node_rank[to]);
            has_label[gap] |= lbl;
            let dist = center[from].abs_diff(center[to]);
            let elbow = dist >= (cross_size[from] + cross_size[to]) / 2;
            has_elbow[gap] |= elbow;
            needs_line[gap] |= !solid;
        }
        for g in 0..groups.len() {
            channel[g] = if has_label[g] && has_elbow[g] {
                V_CHANNEL
            } else if has_label[g] || has_elbow[g] || needs_line[g] {
                V_CHANNEL_MID
            } else {
                V_CHANNEL_TIGHT
            };
        }
    }

    // Primary-axis start of each rank (cumulative; a rank is as tall/wide as
    // its biggest node, so variable-height boxes stack without overlap).
    let rank_size: Vec<usize> = groups
        .iter()
        .map(|g| {
            g.iter()
                .map(|&i| if vertical { box_h[i] } else { box_w[i] })
                .max()
                .unwrap_or(1)
        })
        .collect();
    let mut rank_start = vec![0usize; groups.len()];
    let mut acc = 0;
    for r in 0..groups.len() {
        rank_start[r] = acc;
        acc += rank_size[r] + channel[r];
    }

    let mut rects = vec![
        Rect {
            x: 0,
            y: 0,
            w: 1,
            h: BOX_H
        };
        n
    ];
    for (r, g) in groups.iter().enumerate() {
        let prim = if (vertical && direction == Direction::Up)
            || (!vertical && direction == Direction::Left)
        {
            rank_start[max_rank - r]
        } else {
            rank_start[r]
        };
        for &i in g {
            rects[i] = if vertical {
                Rect {
                    x: center[i] - box_w[i] / 2,
                    y: prim,
                    w: box_w[i],
                    h: box_h[i],
                }
            } else {
                Rect {
                    x: prim,
                    y: center[i] - box_h[i] / 2,
                    w: box_w[i],
                    h: box_h[i],
                }
            };
        }
    }
    rects
}

/// Solve cross-axis centers: start packed, then a few barycenter sweeps pull
/// each node toward the average of its neighbors while preserving order and
/// minimum spacing. Returns a center coordinate per node (shifted to start 0).
fn assign_cross(
    groups: &[Vec<usize>],
    size: &[usize],
    gap: usize,
    preds: &[Vec<usize>],
    succs: &[Vec<usize>],
    is_dummy: &[bool],
) -> Vec<usize> {
    let n = size.len();
    let mut center = vec![0i64; n];

    // Initial packing per rank.
    for g in groups {
        let mut edge = 0i64;
        for &i in g {
            center[i] = edge + (size[i] / 2) as i64;
            edge += (size[i] + gap) as i64;
        }
    }

    let sweeps = 6;
    for s in 0..sweeps {
        let down = s % 2 == 0;
        let order: Vec<usize> = if down {
            (0..groups.len()).collect()
        } else {
            (0..groups.len()).rev().collect()
        };
        for &r in &order {
            let g = &groups[r];
            // Desired center = mean of neighbor centers in the adjacent rank.
            let mut desired = vec![0i64; g.len()];
            for (idx, &i) in g.iter().enumerate() {
                let neigh = if down { &preds[i] } else { &succs[i] };
                desired[idx] = if neigh.is_empty() {
                    center[i]
                } else {
                    neigh.iter().map(|&p| center[p]).sum::<i64>() / neigh.len() as i64
                };
            }
            // Place left-to-right honoring desired + minimum spacing.
            let mut prev_right = i64::MIN / 4;
            for (idx, &i) in g.iter().enumerate() {
                let half = (size[i] / 2) as i64;
                let min_center = prev_right + gap as i64 + half;
                let c = desired[idx].max(min_center);
                center[i] = c;
                prev_right = c + half;
            }
            // Re-center the rank. The pass above can only push a node right of
            // its desired position (never left), so a rank whose nodes crowd the
            // same target — e.g. the many children of a hub entity that all point
            // back at one parent — drifts rightward as a block, stranding the
            // first nodes far to the left with long connecting elbows. Shifting
            // the whole rank by its mean displacement restores symmetry (spacing
            // is preserved since every node moves equally) so a hub's children
            // straddle it instead of fanning out to one side.
            if !g.is_empty() {
                let drift: i64 = g
                    .iter()
                    .enumerate()
                    .map(|(idx, &i)| center[i] - desired[idx])
                    .sum::<i64>()
                    / g.len() as i64;
                if drift > 0 {
                    for &i in g {
                        center[i] -= drift;
                    }
                }
            }
        }
    }

    // Straighten dummy chains: pull each dummy toward its predecessor so a long
    // edge descends straight from its source and jogs only on its final
    // segment, instead of drifting toward the target across every rank. Real
    // nodes keep their barycenter positions; only dummies move, clamped to
    // their rank's left-to-right order and spacing.
    for g in groups {
        let mut prev_right = i64::MIN / 4;
        for &i in g {
            let half = (size[i] / 2) as i64;
            let min_center = prev_right + gap as i64 + half;
            let desired = if is_dummy[i] && !preds[i].is_empty() {
                preds[i].iter().map(|&p| center[p]).sum::<i64>() / preds[i].len() as i64
            } else {
                center[i]
            };
            center[i] = desired.max(min_center);
            prev_right = center[i] + half;
        }
    }

    // Shift so the minimum left edge is 0.
    let min_left = (0..n)
        .map(|i| center[i] - (size[i] / 2) as i64)
        .min()
        .unwrap_or(0);
    (0..n).map(|i| (center[i] - min_left) as usize).collect()
}
