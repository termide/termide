//! Flowchart layout + rendering.
//!
//! A layered (Sugiyama-style) layout: nodes are assigned ranks by longest path
//! (cycles broken via DFS back-edge detection), positioned within ranks by an
//! iterative barycenter pass (so edges run as straight as possible), and
//! connected with orthogonal elbow edges that attach to box borders with
//! T-junctions. A binary branch leaves the two opposite sides of its source;
//! when boxes overlap on the cross axis the edge is a straight run (the boxes
//! sit centred under it). Vertical (`TD`/`BT`) and horizontal (`LR`/`RL`)
//! orientations are supported.

use std::collections::HashSet;

use crate::canvas::{label_width, Canvas};
use crate::parser::{Direction, EdgeLine, Flowchart, NodeShape};

/// Placed node box (top-left `(x, y)`, size `w`×`h`).
#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
}

impl Rect {
    fn top(&self) -> usize {
        self.y
    }
    fn bottom(&self) -> usize {
        self.y + self.h - 1
    }
    fn left(&self) -> usize {
        self.x
    }
    fn right(&self) -> usize {
        self.x + self.w - 1
    }
    fn cx(&self) -> usize {
        self.x + self.w / 2
    }
    fn cy(&self) -> usize {
        self.y + self.h / 2
    }
    /// Cross-axis coordinate of the `k`-th of `n` fan-out/-in points.
    fn fan_x(&self, k: usize, n: usize) -> usize {
        self.x + (k + 1) * self.w / (n + 1)
    }
    fn fan_y(&self, k: usize, n: usize) -> usize {
        self.y + (k + 1) * self.h / (n + 1)
    }
}

const BOX_H: usize = 3;
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
/// Within-rank spacing.
const COL_GAP: usize = 3;
const ROW_GAP: usize = 1;

/// Render a flowchart into canvas lines.
#[must_use]
pub fn render_flowchart(fc: &Flowchart) -> Vec<String> {
    let n = fc.nodes.len();
    if n == 0 {
        return vec!["(empty flowchart)".to_string()];
    }

    let rank = assign_ranks(fc);
    let max_rank = *rank.iter().max().unwrap_or(&0);

    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    for (i, &r) in rank.iter().enumerate() {
        groups[r].push(i);
    }

    // Box width: at least the node label, but also wide enough to seat the
    // widest incident edge label — so a short title with several labelled
    // edges still leaves the labels room (the text stays centred).
    let mut edge_label = vec![0usize; n];
    for e in &fc.edges {
        let w = label_width(&e.label);
        edge_label[e.from] = edge_label[e.from].max(w);
        edge_label[e.to] = edge_label[e.to].max(w);
    }
    let box_w: Vec<usize> = (0..n)
        .map(|i| {
            let body = fc.nodes[i]
                .body
                .iter()
                .map(|l| label_width(l))
                .max()
                .unwrap_or(0);
            label_width(&fc.nodes[i].label)
                .max(1)
                .max(edge_label[i])
                .max(body)
                + 2
        })
        .collect();

    // Box height: 3 for a plain node; taller when it has a body compartment
    // (title row + separator + one row per body line).
    let box_h: Vec<usize> = fc
        .nodes
        .iter()
        .map(|node| {
            if node.body.is_empty() {
                BOX_H
            } else {
                BOX_H + 1 + node.body.len()
            }
        })
        .collect();

    // Tallest box per rank, used to size the dummy nodes that carry long edges.
    let mut rank_h = vec![1usize; max_rank + 1];
    for i in 0..n {
        rank_h[rank[i]] = rank_h[rank[i]].max(box_h[i]);
    }

    // Expand the graph: an edge spanning more than one rank is split into a
    // chain of segments through thin dummy nodes (one per intermediate rank).
    // The layout then routes the chain around the boxes in between instead of
    // crashing straight through them (classic Sugiyama virtual nodes).
    let mut rank_ext = rank.clone();
    let mut bw = box_w.clone();
    let mut bh = box_h.clone();
    let mut segs: Vec<Seg> = Vec::new();
    let mut dummies: Vec<usize> = Vec::new();
    for e in &fc.edges {
        if e.from == e.to {
            continue; // self-loops not drawn yet
        }
        let (ru, rv) = (rank[e.from], rank[e.to]);
        if rv > ru + 1 {
            let mut prev = e.from;
            for (r, &rh) in (ru + 1..rv).zip(&rank_h[ru + 1..rv]) {
                let d = rank_ext.len();
                rank_ext.push(r);
                bw.push(1);
                bh.push(rh);
                dummies.push(d);
                segs.push(Seg {
                    from: prev,
                    to: d,
                    line: e.line,
                    arrow: false,
                    label: if prev == e.from {
                        e.label.clone()
                    } else {
                        String::new()
                    },
                });
                prev = d;
            }
            segs.push(Seg {
                from: prev,
                to: e.to,
                line: e.line,
                arrow: e.arrow,
                label: String::new(),
            });
        } else {
            segs.push(Seg {
                from: e.from,
                to: e.to,
                line: e.line,
                arrow: e.arrow,
                label: e.label.clone(),
            });
        }
    }
    let ext = rank_ext.len();

    let mut groups_ext: Vec<Vec<usize>> = vec![Vec::new(); max_rank + 1];
    for (i, &r) in rank_ext.iter().enumerate() {
        groups_ext[r].push(i);
    }
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); ext];
    let mut succs: Vec<Vec<usize>> = vec![Vec::new(); ext];
    for s in &segs {
        succs[s.from].push(s.to);
        preds[s.to].push(s.from);
    }

    // Widen the within-rank gap so sibling branches sit far enough apart for
    // their edge labels to fit on the connecting run.
    let max_label = edge_label.iter().copied().max().unwrap_or(0);
    let col_gap = COL_GAP.max(max_label + 1);

    let vertical = fc.direction.vertical();
    // (from, to, has_label, is_solid) per segment. A gap carrying both a label
    // and an elbow needs the tallest channel so the label clears the jog row; a
    // non-solid edge needs at least a line row so its dotted/thick glyph shows.
    let seg_ends: Vec<(usize, usize, bool, bool)> = segs
        .iter()
        .map(|s| {
            (
                s.from,
                s.to,
                !s.label.is_empty(),
                matches!(s.line, EdgeLine::Solid),
            )
        })
        .collect();
    let mut is_dummy = vec![false; ext];
    for &d in &dummies {
        is_dummy[d] = true;
    }
    let rects = layout(
        fc.direction,
        &groups_ext,
        &bw,
        &bh,
        &preds,
        &succs,
        max_rank,
        vertical,
        col_gap,
        &seg_ends,
        &is_dummy,
    );

    // Per-side fan assignment. Edges meeting the same box side (top/bottom for
    // vertical, left/right for horizontal) share one ordered set of attachment
    // points — combining the node's exits and entries on that side — so a
    // back-edge entry never lands on a forward-edge exit column. A node with
    // exactly two out-edges is a fork: its edges leave the cross sides instead,
    // so they don't claim a primary-side slot.
    let mut out_degree = vec![0usize; ext];
    for s in &segs {
        out_degree[s.from] += 1;
    }
    let cross = |i: usize| {
        if vertical {
            rects[i].cx()
        } else {
            rects[i].cy()
        }
    };
    let prim = |i: usize| {
        if vertical {
            rects[i].top()
        } else {
            rects[i].left()
        }
    };
    // sides[node] = [near-side attachments, far-side attachments]; each entry is
    // (seg index, is_source, the other endpoint's cross coordinate).
    let mut sides: Vec<[Vec<(usize, bool, usize)>; 2]> =
        (0..ext).map(|_| [Vec::new(), Vec::new()]).collect();
    for (k, s) in segs.iter().enumerate() {
        let forward = prim(s.to) > prim(s.from);
        // Source exits the far side when the target is further along the axis.
        if out_degree[s.from] != 2 {
            sides[s.from][usize::from(forward)].push((k, true, cross(s.to)));
        }
        // Target is entered from its near side in that same case.
        sides[s.to][usize::from(!forward)].push((k, false, cross(s.from)));
    }
    let mut out_pos = vec![0usize; segs.len()];
    let mut out_cnt = vec![1usize; segs.len()];
    let mut in_pos = vec![0usize; segs.len()];
    let mut in_cnt = vec![1usize; segs.len()];
    for node in sides.iter() {
        for side in node.iter() {
            let mut list = side.clone();
            list.sort_by_key(|&(_, _, oc)| oc);
            let cnt = list.len();
            for (slot, &(k, is_source, _)) in list.iter().enumerate() {
                if is_source {
                    out_pos[k] = slot;
                    out_cnt[k] = cnt;
                } else {
                    in_pos[k] = slot;
                    in_cnt[k] = cnt;
                }
            }
        }
    }

    let mut c = Canvas::new();
    let mut ticks: Vec<(usize, usize, char)> = Vec::new();
    let mut heads: Vec<(usize, usize, char)> = Vec::new();
    let mut labels: Vec<(usize, usize, String)> = Vec::new();

    for (k, s) in segs.iter().enumerate() {
        draw_edge(
            &mut c,
            &mut ticks,
            &mut heads,
            &mut labels,
            &rects[s.from],
            &rects[s.to],
            s.line,
            s.arrow,
            &s.label,
            vertical,
            Fan {
                op: out_pos[k],
                ok: out_cnt[k],
                ip: in_pos[k],
                ik: in_cnt[k],
                fork: out_degree[s.from] == 2,
            },
        );
    }

    for (i, node) in fc.nodes.iter().enumerate() {
        let r = rects[i];
        c.draw_panel(
            r.x,
            r.y,
            r.w - 2,
            &node.label,
            &node.body,
            corners(node.shape),
        );
    }

    // Attach edges to box borders with T-junctions (over the box outline).
    for (x, y, g) in ticks {
        c.put(x, y, g);
    }

    // Dummy pass-throughs: a continuous line across each dummy's rank band,
    // drawn last so it overwrites any junction stubs left on it.
    for &d in &dummies {
        let r = rects[d];
        if vertical {
            c.vline(r.cx(), r.top(), r.bottom(), '│');
        } else {
            c.hline(r.left(), r.right(), r.cy(), '─');
        }
    }

    // Arrowheads after lines (including dummy pass-throughs) so a sibling edge
    // sharing the target column can't overwrite a head with its own line.
    for (x, y, g) in heads {
        c.put(x, y, g);
    }

    // Edge labels last, so a crossing line never overwrites the text.
    for (x, y, text) in labels {
        c.text(x, y, &text);
    }

    c.into_lines()
}

/// A routed segment of an edge (an edge spanning >1 rank becomes several).
struct Seg {
    from: usize,
    to: usize,
    line: EdgeLine,
    arrow: bool,
    label: String,
}

/// Per-edge fan position among same-source / same-target siblings.
struct Fan {
    /// Source-side slot and count (its exit point among that box side's edges).
    op: usize,
    ok: usize,
    /// Target-side slot and count (its entry point among that box side's edges).
    ip: usize,
    ik: usize,
    /// The source has exactly two out-edges → they leave the cross sides.
    fork: bool,
}

/// Longest-path rank assignment, ignoring back edges (cycle support).
fn assign_ranks(fc: &Flowchart) -> Vec<usize> {
    let n = fc.nodes.len();
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for e in &fc.edges {
        if e.from != e.to {
            succ[e.from].push(e.to);
        }
    }

    let mut visited = vec![false; n];
    let mut onstack = vec![false; n];
    let mut order = Vec::new();
    let mut back: HashSet<(usize, usize)> = HashSet::new();
    for u in 0..n {
        if !visited[u] {
            dfs(u, &succ, &mut visited, &mut onstack, &mut order, &mut back);
        }
    }
    order.reverse(); // reverse postorder ≈ topological order

    let mut rank = vec![0usize; n];
    for &u in &order {
        for &v in &succ[u] {
            if !back.contains(&(u, v)) {
                rank[v] = rank[v].max(rank[u] + 1);
            }
        }
    }
    rank
}

fn dfs(
    u: usize,
    succ: &[Vec<usize>],
    visited: &mut [bool],
    onstack: &mut [bool],
    order: &mut Vec<usize>,
    back: &mut HashSet<(usize, usize)>,
) {
    visited[u] = true;
    onstack[u] = true;
    for &v in &succ[u] {
        if onstack[v] {
            back.insert((u, v));
        } else if !visited[v] {
            dfs(v, succ, visited, onstack, order, back);
        }
    }
    onstack[u] = false;
    order.push(u);
}

/// Place every node. The primary axis comes from the rank; the cross axis is
/// solved by [`assign_cross`] to straighten edges.
#[allow(clippy::too_many_arguments)]
fn layout(
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

fn corners(shape: NodeShape) -> [char; 4] {
    match shape {
        // Rounded outline for round/stadium/circle; everything else is a clean
        // rectangle. Diagonal `╱╲` glyphs read poorly in a character grid, so
        // diamonds/hexagons use square corners too.
        NodeShape::Round | NodeShape::Stadium | NodeShape::Circle => ['╭', '╮', '╰', '╯'],
        _ => ['┌', '┐', '└', '┘'],
    }
}

type Ticks = Vec<(usize, usize, char)>;

fn line_glyphs(line: EdgeLine) -> (char, char) {
    match line {
        EdgeLine::Solid => ('│', '─'),
        EdgeLine::Dotted => ('┊', '┄'),
        EdgeLine::Thick => ('┃', '━'),
    }
}

/// Corner glyph connecting a vertical arm (`up`) and a horizontal arm (`right`).
fn corner(up: bool, right: bool) -> char {
    match (up, right) {
        (true, true) => '└',
        (true, false) => '┘',
        (false, true) => '┌',
        (false, false) => '┐',
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_edge(
    c: &mut Canvas,
    ticks: &mut Ticks,
    heads: &mut Ticks,
    labels: &mut Vec<(usize, usize, String)>,
    from: &Rect,
    to: &Rect,
    line: EdgeLine,
    arrow: bool,
    label: &str,
    vertical: bool,
    fan: Fan,
) {
    let (vch, hch) = line_glyphs(line);

    if vertical {
        if from.y == to.y {
            return side_edge(c, ticks, heads, labels, from, to, line, arrow, label, true);
        }
        let down = to.y > from.y;
        let head = if down { '▼' } else { '▲' };
        let src_b = if down { from.bottom() } else { from.top() };
        let sy = if down {
            from.bottom() + 1
        } else {
            from.top() - 1
        };
        let tgt_b = if down { to.top() } else { to.bottom() };
        let ty = if down { to.top() - 1 } else { to.bottom() + 1 };
        let s_tick = if down { '┬' } else { '┴' };
        let t_tick = if down { '┴' } else { '┬' };
        let entry_x = to.fan_x(fan.ip, fan.ik);

        // Binary fork: the two edges leave the left/right sides of the source.
        if fan.fork {
            let left = to.cx() < from.cx();
            let (outer, border, stick) = if left {
                (from.left() - 1, from.left(), '┤')
            } else {
                (from.right() + 1, from.right(), '├')
            };
            let ay = from.cy();
            c.hline(outer.min(entry_x), outer.max(entry_x), ay, hch);
            c.vline(entry_x, ay.min(ty), ay.max(ty), vch);
            c.put(entry_x, ay, corner(ty < ay, outer > entry_x));
            if arrow {
                heads.push((entry_x, ty, head));
            }
            ticks.push((border, ay, stick));
            ticks.push((entry_x, tgt_b, t_tick));
            // Label on the run, anchored just outside the source box (so the
            // box never clobbers it) and extending toward the child.
            let len = label.chars().count();
            let lx = if left {
                outer.saturating_sub(len)
            } else {
                outer
            };
            label_at(labels, label, lx, ay);
            return;
        }

        // Boxes overlapping on x → one straight drop; the boxes sit centred
        // under it even when their widths differ.
        let lo = from.left().max(to.left()) + 1;
        let hi = from.right().min(to.right()).saturating_sub(1);
        if lo <= hi {
            let col = from.cx().clamp(lo, hi);
            c.vline(col, sy.min(ty), sy.max(ty), vch);
            if arrow {
                heads.push((col, ty, head));
            }
            ticks.push((col, src_b, s_tick));
            ticks.push((col, tgt_b, t_tick));
            // Label on the row just past the source, above where crossing edges
            // route (their horizontal jog sits at the channel midpoint).
            label_at(labels, label, col + 1, sy);
            return;
        }

        // General elbow, fanning out from the source's bottom/top edge.
        let sx = from.fan_x(fan.op, fan.ok);
        // Source exit aligned with the target entry (e.g. a straightened dummy
        // chain) → a plain drop with no jog or corners.
        if sx == entry_x {
            c.vline(sx, sy.min(ty), sy.max(ty), vch);
            if arrow {
                heads.push((sx, ty, head));
            }
            ticks.push((sx, src_b, s_tick));
            ticks.push((entry_x, tgt_b, t_tick));
            label_at(labels, label, sx + 1, sy);
            return;
        }
        let mid = (sy + ty) / 2;
        c.vline(sx, sy.min(mid), sy.max(mid), vch);
        c.hline(sx.min(entry_x), sx.max(entry_x), mid, hch);
        c.vline(entry_x, mid.min(ty), mid.max(ty), vch);
        // Corners last (after every segment) so a later vline/hline can't
        // overwrite the bend glyph with `│`/`─`. Direction-based corners are
        // robust when the channel is 1 row tall (`mid` coincides with a port).
        c.put(sx, mid, corner(down, entry_x > sx));
        c.put(entry_x, mid, corner(!down, sx > entry_x));
        if arrow {
            heads.push((entry_x, ty, head));
        }
        ticks.push((sx, src_b, s_tick));
        ticks.push((entry_x, tgt_b, t_tick));
        // Label beside the source's vertical drop (top of the channel), not on
        // the shared jog row. Align it away from the drop in the edge's own
        // direction so sibling labels (e.g. a left and a right branch leaving
        // the same box) spread apart instead of colliding.
        let lx = if entry_x < sx {
            sx.saturating_sub(label.chars().count())
        } else {
            sx + 1
        };
        label_at(labels, label, lx, sy);
    } else {
        if from.x == to.x {
            return side_edge(c, ticks, heads, labels, from, to, line, arrow, label, false);
        }
        let right = to.x > from.x;
        let head = if right { '▶' } else { '◀' };
        let src_b = if right { from.right() } else { from.left() };
        let sx = if right {
            from.right() + 1
        } else {
            from.left() - 1
        };
        let tgt_b = if right { to.left() } else { to.right() };
        let tx = if right { to.left() - 1 } else { to.right() + 1 };
        let s_tick = if right { '├' } else { '┤' };
        let t_tick = if right { '┤' } else { '├' };
        let entry_y = to.fan_y(fan.ip, fan.ik);

        // Binary fork: edges leave the top/bottom sides of the source.
        if fan.fork {
            let up = to.cy() < from.cy();
            let (outer, border, stick) = if up {
                (from.top() - 1, from.top(), '┴')
            } else {
                (from.bottom() + 1, from.bottom(), '┬')
            };
            let ax = from.cx();
            c.vline(ax, outer.min(entry_y), outer.max(entry_y), vch);
            c.hline(ax.min(tx), ax.max(tx), entry_y, hch);
            c.put(ax, entry_y, corner(outer < entry_y, tx > ax));
            if arrow {
                heads.push((tx, entry_y, head));
            }
            ticks.push((ax, border, stick));
            ticks.push((tgt_b, entry_y, t_tick));
            let lx = (ax + tx) / 2;
            label_at(
                labels,
                label,
                lx.saturating_sub(label.chars().count() / 2),
                entry_y,
            );
            return;
        }

        // Boxes overlapping on y → one straight horizontal run.
        let lo = from.top().max(to.top()) + 1;
        let hi = from.bottom().min(to.bottom()).saturating_sub(1);
        if lo <= hi {
            let row = from.cy().clamp(lo, hi);
            c.hline(sx.min(tx), sx.max(tx), row, hch);
            if arrow {
                heads.push((tx, row, head));
            }
            ticks.push((src_b, row, s_tick));
            ticks.push((tgt_b, row, t_tick));
            label_at(labels, label, sx.min(tx) + 1, row.saturating_sub(1));
            return;
        }

        // General elbow, fanning out from the source's right/left edge.
        let sy = from.fan_y(fan.op, fan.ok);
        // Source exit aligned with the target entry → a plain horizontal run.
        if sy == entry_y {
            c.hline(sx.min(tx), sx.max(tx), sy, hch);
            if arrow {
                heads.push((tx, sy, head));
            }
            ticks.push((src_b, sy, s_tick));
            ticks.push((tgt_b, sy, t_tick));
            label_at(labels, label, sx.min(tx) + 1, sy.saturating_sub(1));
            return;
        }
        let mid = (sx + tx) / 2;
        c.hline(sx.min(mid), sx.max(mid), sy, hch);
        c.vline(mid, sy.min(entry_y), sy.max(entry_y), vch);
        c.hline(mid.min(tx), mid.max(tx), entry_y, hch);
        // Corners last so the second hline can't overwrite the bend glyph.
        c.put(mid, sy, corner(entry_y < sy, sx > mid));
        c.put(mid, entry_y, corner(sy < entry_y, tx > mid));
        if arrow {
            heads.push((tx, entry_y, head));
        }
        ticks.push((src_b, sy, s_tick));
        ticks.push((tgt_b, entry_y, t_tick));
        label_at(labels, label, mid + 1, sy.min(entry_y).saturating_sub(1));
    }
}

/// Queue a non-empty edge label at `(x, y)`. Labels are stamped after all
/// lines/boxes so a crossing line can never clobber the text.
fn label_at(labels: &mut Vec<(usize, usize, String)>, label: &str, x: usize, y: usize) {
    if !label.is_empty() {
        labels.push((x, y, label.to_string()));
    }
}

/// A same-rank edge: a short straight arrow between facing box sides, attached
/// with T-junctions.
#[allow(clippy::too_many_arguments)]
fn side_edge(
    c: &mut Canvas,
    ticks: &mut Ticks,
    heads: &mut Ticks,
    labels: &mut Vec<(usize, usize, String)>,
    from: &Rect,
    to: &Rect,
    line: EdgeLine,
    arrow: bool,
    label: &str,
    vertical: bool,
) {
    let (vch, hch) = line_glyphs(line);
    if vertical {
        let y = from.cy();
        let right = to.x > from.x;
        let (x0, x1, head, sb, tb, st, tt) = if right {
            (
                from.right() + 1,
                to.left() - 1,
                '▶',
                from.right(),
                to.left(),
                '├',
                '┤',
            )
        } else {
            (
                to.right() + 1,
                from.left() - 1,
                '◀',
                from.left(),
                to.right(),
                '┤',
                '├',
            )
        };
        if x0 <= x1 {
            c.hline(x0, x1, y, hch);
            label_at(labels, label, x0, y.saturating_sub(1));
            if arrow {
                heads.push((if right { x1 } else { x0 }, y, head));
            }
            ticks.push((sb, y, st));
            ticks.push((tb, y, tt));
        }
    } else {
        let x = from.cx();
        let down = to.y > from.y;
        let (y0, y1, head, sb, tb, st, tt) = if down {
            (
                from.bottom() + 1,
                to.top() - 1,
                '▼',
                from.bottom(),
                to.top(),
                '┬',
                '┴',
            )
        } else {
            (
                to.bottom() + 1,
                from.top() - 1,
                '▲',
                from.top(),
                to.bottom(),
                '┴',
                '┬',
            )
        };
        if y0 <= y1 {
            c.vline(x, y0, y1, vch);
            if arrow {
                heads.push((x, if down { y1 } else { y0 }, head));
            }
            ticks.push((x, sb, st));
            ticks.push((x, tb, tt));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_flowchart;

    fn render(src: &str) -> String {
        render_flowchart(&parse_flowchart(src)).join("\n")
    }

    #[test]
    fn renders_nodes_and_edge() {
        let out = render("flowchart TD\nA[Start] --> B[End]");
        assert!(out.contains("Start") && out.contains("End"), "{out}");
        assert!(out.contains('┌'), "no box: {out}");
        assert!(out.contains('▼'), "no down arrowhead: {out}");
    }

    #[test]
    fn decision_branches_with_labels() {
        let out = render("flowchart TD\nA{ok?} -->|yes| B[Go]\nA -->|no| C[Stop]");
        assert!(out.contains("yes"), "no edge label: {out}");
        assert!(out.contains("no"), "no edge label: {out}");
        assert!(out.contains("ok?"), "no decision label: {out}");
        // Edges attach to boxes with T-junctions rather than detached lines.
        assert!(out.contains('┬') || out.contains('┴') || out.contains('├') || out.contains('┤'));
    }

    #[test]
    fn horizontal_uses_side_arrows() {
        let out = render("flowchart LR\nA --> B --> C");
        assert!(out.contains('▶'), "no right arrowhead: {out}");
    }

    #[test]
    fn dotted_and_thick_lines() {
        let out = render("flowchart TD\nA -.-> B\nB ==> C");
        assert!(out.contains('┊') || out.contains('┄'), "no dotted: {out}");
        assert!(out.contains('┃') || out.contains('━'), "no thick: {out}");
    }

    #[test]
    fn straight_edge_is_compact() {
        // A plain unlabelled drop uses the tightest channel: just the arrowhead
        // row, with no `│`-only rows (the box `┬`/`┴` junctions anchor it).
        let lines = render_flowchart(&parse_flowchart("flowchart TD\nA[Start] --> B[End]"));
        let bars = lines
            .iter()
            .filter(|l| !l.is_empty() && l.chars().all(|c| c == '│' || c == ' '))
            .count();
        assert_eq!(bars, 0, "expected no bar-only channel rows:\n{lines:#?}");
        assert!(
            lines.iter().any(|l| l.contains('▼')),
            "no arrowhead:\n{lines:#?}"
        );
    }

    #[test]
    fn long_edge_runs_straight() {
        // A -> C skips a rank (A,B,C stacked), so A->C routes through a dummy.
        // The dummy is straightened under the source, so the long edge has a
        // straight vertical run rather than drifting across each rank.
        let out = render("flowchart TD\nA --> B\nA --> C\nB --> C");
        // Some row carries two separate vertical runs (the A->C bypass beside
        // the A->B->C spine), confirming the bypass stays vertical.
        let two_bars = out
            .lines()
            .any(|l| l.chars().filter(|&c| c == '│').count() >= 2);
        assert!(
            two_bars,
            "long edge did not run as a straight bypass:\n{out}"
        );
    }

    #[test]
    fn empty_handled() {
        assert_eq!(
            render_flowchart(&parse_flowchart("flowchart TD")),
            vec!["(empty flowchart)"]
        );
    }

    #[test]
    fn cycle_does_not_hang() {
        // A -> B -> A: the back edge must be ignored during ranking.
        let out = render("flowchart TD\nA --> B\nB --> A");
        assert!(out.contains('A') && out.contains('B'), "{out}");
    }
}
