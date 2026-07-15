//! Edge routing + box glyph rendering onto the canvas.

use crate::canvas::Canvas;
use crate::parser::{EdgeLine, NodeShape};

use super::{Fan, Rect};

pub(super) fn corners(shape: NodeShape) -> [char; 4] {
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
pub(super) fn draw_edge(
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
