//! Non-graph diagram rendering: pie, gantt, journey, mindmap, timeline, and
//! quadrant charts. These read clearly as bars, timelines, scored lists,
//! indented trees, and plotted grids without 2-D graph layout.

use crate::canvas::{clip_to_width, label_width, write_cells, Canvas, WIDE_TAIL};
use crate::parser::{Gantt, Journey, Mindmap, Pie, Quadrant, TaskStatus, Timeline};

const BAR_WIDTH: usize = 30;

/// Render a pie chart as a labelled horizontal bar breakdown.
#[must_use]
pub fn render_pie(pie: &Pie) -> Vec<String> {
    if pie.slices.is_empty() {
        return vec!["(empty pie chart)".to_string()];
    }
    let total: f64 = pie.slices.iter().map(|(_, v)| v).sum();
    let total = if total <= 0.0 { 1.0 } else { total };

    // Sort by value descending (Mermaid renders the largest slice first).
    let mut slices = pie.slices.clone();
    slices.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let label_col = slices
        .iter()
        .map(|(l, _)| label_width(l))
        .max()
        .unwrap_or(0);

    let mut out = Vec::new();
    if !pie.title.is_empty() {
        out.push(pie.title.clone());
        out.push(String::new());
    }
    for (label, value) in &slices {
        let frac = value / total;
        let filled = (frac * BAR_WIDTH as f64).round() as usize;
        let bar: String = "█".repeat(filled) + &"░".repeat(BAR_WIDTH - filled.min(BAR_WIDTH));
        let pad = " ".repeat(label_col - label_width(label));
        // Trim trailing zeros from the value for compactness.
        let val = format!("{value}");
        out.push(format!("{label}{pad} │{bar}│ {val} ({:.1}%)", frac * 100.0));
    }
    out
}

/// Render a Gantt chart as labelled timeline bars, grouped by section.
#[must_use]
pub fn render_gantt(g: &Gantt) -> Vec<String> {
    if g.tasks.is_empty() {
        return vec!["(empty gantt chart)".to_string()];
    }
    let origin = g.tasks.iter().map(|t| t.start).min().unwrap_or(0);
    let end = g.tasks.iter().map(|t| t.start + t.len).max().unwrap_or(1);
    let span = (end - origin).max(1) as f64;
    let chart = 40usize;
    let scale = chart as f64 / span;
    let name_col = g
        .tasks
        .iter()
        .map(|t| label_width(&t.name))
        .max()
        .unwrap_or(0);

    // Column geometry. The chart's left edge is the first tick's dotted
    // gridline (aligned under the first date), matching every other tick; bars
    // start at `bar0`, one column past it. Gridlines run the full height —
    // becoming `┬`/`┴`/`┼` where they cross the top/bottom rulers and the
    // section divider lines — so each axis reads as one continuous line.
    let left = name_col + 1;
    let bar0 = name_col + 2;
    let step = 13;
    let end_col = bar0 + chart;
    let row_width = end_col + 1;
    // (column, day) for the origin tick and each interior tick.
    let ticks: Vec<(usize, i64)> = std::iter::once((left, origin))
        .chain((step..=chart).step_by(step).map(|col| {
            (
                bar0 + col,
                origin + (col as f64 * span / chart as f64).round() as i64,
            )
        }))
        .collect();

    // A chart row: the name/section prefix plus a dotted gridline at each tick
    // (filled only into otherwise-empty cells, so a wide section header or a
    // bar is never overwritten).
    let make_row = |prefix: &str| -> Vec<char> {
        let mut row = vec![' '; row_width];
        write_cells(&mut row, 0, prefix);
        for &(x, _) in &ticks {
            if row[x] == ' ' {
                row[x] = '┆';
            }
        }
        row
    };
    let finish = |row: Vec<char>| -> String {
        let s: String = row.into_iter().filter(|&ch| ch != WIDE_TAIL).collect();
        s.trim_end().to_string()
    };

    // A full-width horizontal line (top/bottom ruler or section divider),
    // spanning to the left edge with the given junction where a vertical crosses.
    let hline = |junction: char| -> String {
        let mut row = vec!['─'; row_width];
        for &(c, _) in &ticks {
            row[c] = junction;
        }
        row.into_iter().collect()
    };
    let date_row = || -> String {
        let mut row = vec![' '; row_width + 12];
        for &(c, day) in &ticks {
            for (k, ch) in crate::parser::day_to_date(day).chars().enumerate() {
                if c + k < row.len() {
                    row[c + k] = ch;
                }
            }
        }
        finish(row)
    };

    let mut out = Vec::new();
    if !g.title.is_empty() {
        out.push(g.title.clone());
        out.push(String::new());
    }

    // Top axis (duplicated): dates then ruler.
    out.push(date_row());
    out.push(hline('┬'));

    let mut section = None;
    for t in &g.tasks {
        if section.as_deref() != Some(t.section.as_str()) && !t.section.is_empty() {
            // A divider line between sections (not before the first).
            if section.is_some() {
                out.push(hline('┼'));
            }
            out.push(finish(make_row(&format!("▌ {}", t.section))));
            section = Some(t.section.clone());
        }
        let offset = (((t.start - origin) as f64) * scale).round() as usize;
        let (fillc, mark) = match t.status {
            TaskStatus::Done => ('█', None),
            TaskStatus::Active => ('▓', None),
            TaskStatus::Crit => ('▒', Some('!')),
            TaskStatus::Milestone => (' ', Some('◆')),
            TaskStatus::Plain => ('▒', None),
        };
        let pad = " ".repeat(name_col - label_width(&t.name));
        let bar: Vec<char> = if let Some(m) = mark {
            if t.status == TaskStatus::Milestone {
                vec![m]
            } else {
                let w = ((t.len as f64 * scale).round() as usize).max(1);
                std::iter::once(m)
                    .chain(std::iter::repeat_n(fillc, w.saturating_sub(1)))
                    .collect()
            }
        } else {
            let w = ((t.len as f64 * scale).round() as usize).max(1);
            std::iter::repeat_n(fillc, w).collect()
        };
        let mut row = make_row(&format!("{}{}", t.name, pad));
        for (k, ch) in bar.into_iter().enumerate() {
            let x = bar0 + offset + k;
            if x < row.len() {
                row[x] = ch;
            }
        }
        out.push(finish(row));
    }

    // Bottom axis: ruler then dates.
    out.push(hline('┴'));
    out.push(date_row());
    out
}

/// Render a user journey as scored rows (`★` of 5), grouped by section.
#[must_use]
pub fn render_journey(j: &Journey) -> Vec<String> {
    if j.tasks.is_empty() {
        return vec!["(empty journey)".to_string()];
    }
    let name_col = j
        .tasks
        .iter()
        .map(|t| label_width(&t.name))
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    if !j.title.is_empty() {
        out.push(j.title.clone());
        out.push(String::new());
    }
    let mut section = None;
    for t in &j.tasks {
        if section.as_deref() != Some(t.section.as_str()) && !t.section.is_empty() {
            out.push(format!("▌ {}", t.section));
            section = Some(t.section.clone());
        }
        let stars: String = "★".repeat(t.score as usize) + &"☆".repeat(5 - t.score as usize);
        let pad = " ".repeat(name_col - label_width(&t.name));
        let actors = if t.actors.is_empty() {
            String::new()
        } else {
            format!("  {}", t.actors)
        };
        out.push(format!(
            "{}{}  {} ({}){}",
            t.name, pad, stars, t.score, actors
        ));
    }
    out
}

/// Render a mindmap as an indented tree.
#[must_use]
pub fn render_mindmap(m: &Mindmap) -> Vec<String> {
    if m.nodes.is_empty() {
        return vec!["(empty mindmap)".to_string()];
    }
    m.nodes
        .iter()
        .map(|n| {
            if n.depth == 0 {
                n.label.clone()
            } else {
                format!("{}└─ {}", "   ".repeat(n.depth - 1), n.label)
            }
        })
        .collect()
}

/// Render a timeline: each period with its events, grouped by section.
#[must_use]
pub fn render_timeline(t: &Timeline) -> Vec<String> {
    if t.entries.is_empty() {
        return vec!["(empty timeline)".to_string()];
    }
    let col = t
        .entries
        .iter()
        .map(|e| label_width(&e.period))
        .max()
        .unwrap_or(0);
    let mut out = Vec::new();
    if !t.title.is_empty() {
        out.push(t.title.clone());
        out.push(String::new());
    }
    let mut section = None;
    for e in &t.entries {
        if section.as_deref() != Some(e.section.as_str()) && !e.section.is_empty() {
            out.push(format!("▌ {}", e.section));
            section = Some(e.section.clone());
        }
        let pad = " ".repeat(col - label_width(&e.period));
        let first = e.events.first().map(|s| s.as_str()).unwrap_or("");
        out.push(format!("{}{} ─ {}", e.period, pad, first));
        for ev in e.events.iter().skip(1) {
            out.push(format!("{}   {}", " ".repeat(col), ev));
        }
    }
    out
}

/// Render a quadrant chart as a plotted grid with axes and quadrant labels.
#[must_use]
pub fn render_quadrant(q: &Quadrant) -> Vec<String> {
    let (w, h) = (44usize, 16usize);
    let mut c = Canvas::new();

    // Border + centre crosshair.
    c.hline(0, w, 0, '─');
    c.hline(0, w, h, '─');
    c.vline(0, 0, h, '│');
    c.vline(w, 0, h, '│');
    c.put(0, 0, '┌');
    c.put(w, 0, '┐');
    c.put(0, h, '└');
    c.put(w, h, '┘');
    c.vline(w / 2, 1, h - 1, '┊');
    c.hline(1, w - 1, h / 2, '┄');

    // Quadrant labels: Mermaid numbers 1=TR, 2=TL, 3=BL, 4=BR.
    let put_clip = |c: &mut Canvas, x: usize, y: usize, s: &str| {
        let max = if x < w / 2 { w / 2 - 1 } else { w - 1 };
        let avail = max.saturating_sub(x);
        c.text(x, y, clip_to_width(s, avail));
    };
    put_clip(&mut c, w / 2 + 2, h / 4, &q.quads[0]);
    put_clip(&mut c, 2, h / 4, &q.quads[1]);
    put_clip(&mut c, 2, 3 * h / 4, &q.quads[2]);
    put_clip(&mut c, w / 2 + 2, 3 * h / 4, &q.quads[3]);

    // Points (y grows upward, so invert for rows).
    for p in &q.points {
        let px = 1 + (p.x * (w - 2) as f64).round() as usize;
        let py = 1 + ((1.0 - p.y) * (h - 2) as f64).round() as usize;
        c.put(px.min(w - 1), py.min(h - 1), '●');
        put_clip(&mut c, (px + 1).min(w - 1), py.min(h - 1), &p.name);
    }

    let mut out = Vec::new();
    if !q.title.is_empty() {
        out.push(q.title.clone());
        out.push(String::new());
    }
    out.extend(c.into_lines());
    if !q.x_axis.is_empty() {
        out.push(format!("x: {}", q.x_axis));
    }
    if !q.y_axis.is_empty() {
        out.push(format!("y: {}", q.y_axis));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{
        parse_gantt, parse_journey, parse_mindmap, parse_pie, parse_quadrant, parse_timeline,
    };

    #[test]
    fn renders_bars_and_percentages() {
        let out = render_pie(&parse_pie("pie title T\n\"A\" : 75\n\"B\" : 25")).join("\n");
        assert!(out.contains("T"), "no title: {out}");
        assert!(out.contains('█'), "no bar fill: {out}");
        assert!(out.contains("75"), "no value: {out}");
        assert!(out.contains("75.0%"), "no percent: {out}");
    }

    #[test]
    fn empty_handled() {
        assert_eq!(render_pie(&parse_pie("pie")), vec!["(empty pie chart)"]);
    }

    #[test]
    fn sorted_descending() {
        let out = render_pie(&parse_pie("pie\n\"small\" : 10\n\"big\" : 90"));
        // The larger slice is listed first.
        assert!(out[0].starts_with("big"), "{out:?}");
    }

    #[test]
    fn gantt_bars_and_sections() {
        let src = "gantt\ntitle Plan\nsection Design\nSpec :done, a, 2026-01-01, 4d\nBuild :active, b, after a, 6d";
        let out = render_gantt(&parse_gantt(src)).join("\n");
        assert!(out.contains("Plan"), "title: {out}");
        assert!(out.contains("Design"), "section: {out}");
        assert!(out.contains('█') && out.contains('▓'), "bars: {out}");
        // `Build` starts after `Spec`, so its bar is indented further.
        let spec = out.lines().find(|l| l.contains("Spec")).unwrap();
        let build = out.lines().find(|l| l.contains("Build")).unwrap();
        let lead = |l: &str| l.find(['█', '▓']).unwrap_or(0);
        assert!(lead(build) > lead(spec), "after-dep not offset:\n{out}");
    }

    #[test]
    fn gantt_has_dual_axis_and_gridlines() {
        let src = "gantt\ntitle Plan\nsection Design\nSpec :done, a, 2026-01-01, 4d\nsection Build\nBack :active, b, 2026-02-01, 6d";
        let lines = render_gantt(&parse_gantt(src));
        let out = lines.join("\n");
        // Axis duplicated top (`┬`) and bottom (`┴`), spanning to the left edge.
        assert!(
            out.contains('┬') && out.contains('┴'),
            "no dual ruler:\n{out}"
        );
        assert!(
            lines.iter().any(|l| l.starts_with('─') && l.contains('┬')),
            "top ruler not full width:\n{out}"
        );
        // Vertical gridlines drop through the rows.
        assert!(out.contains('┆'), "no gridlines:\n{out}");
        // Sections are split by a divider line, not a blank row.
        assert!(
            lines.iter().any(|l| l.starts_with('─') && l.contains('┼')),
            "no section divider line:\n{out}"
        );
    }

    #[test]
    fn journey_scores() {
        let out = render_journey(&parse_journey(
            "journey\nsection Day\nWake: 5: Me\nCommute: 2: Me",
        ))
        .join("\n");
        assert!(out.contains("Day"), "section: {out}");
        assert!(out.contains("★★★★★"), "full score: {out}");
        assert!(out.contains("★★☆☆☆"), "partial score: {out}");
    }

    #[test]
    fn mindmap_indents() {
        let out = render_mindmap(&parse_mindmap("mindmap\n  root\n    A\n    B"));
        assert_eq!(out[0], "root");
        assert!(out[1].contains("└─ A"), "{out:?}");
        assert!(out[2].contains("└─ B"), "{out:?}");
    }

    #[test]
    fn timeline_periods_and_events() {
        let out = render_timeline(&parse_timeline(
            "timeline\ntitle Hist\nsection Old\n2002 : LinkedIn\n2004 : Facebook : Google",
        ))
        .join("\n");
        assert!(out.contains("Hist"), "title: {out}");
        assert!(out.contains("Old"), "section: {out}");
        assert!(out.contains("2002 ─ LinkedIn"), "period+event: {out}");
        assert!(out.contains("Google"), "second event: {out}");
    }

    #[test]
    fn quadrant_plots_points() {
        let out = render_quadrant(&parse_quadrant(
            "quadrantChart\ntitle Map\nx-axis Low --> High\nquadrant-1 Expand\nA: [0.3, 0.6]",
        ))
        .join("\n");
        assert!(out.contains("Map"), "title: {out}");
        assert!(out.contains('●'), "no plotted point: {out}");
        assert!(out.contains("Expand"), "quadrant label: {out}");
        assert!(out.contains("x: Low --> High"), "axis: {out}");
    }
}
