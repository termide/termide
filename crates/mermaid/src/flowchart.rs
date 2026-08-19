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

mod layout;
mod ranking;
mod render;

use crate::canvas::{label_width, Canvas};
use crate::parser::{EdgeLine, Flowchart};

use layout::layout;
use ranking::assign_ranks;
use render::{corners, draw_edge};

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
/// Within-rank spacing.
const COL_GAP: usize = 3;

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
    let mut bw = box_w;
    let mut bh = box_h;
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
