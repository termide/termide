//! Line-oriented chart grammars: pie, gantt, journey, mindmap, timeline,
//! quadrant.

// ===========================================================================
// Pie chart
// ===========================================================================

/// A parsed pie chart: a title and labelled values.
#[derive(Debug, Clone, Default)]
pub struct Pie {
    pub title: String,
    pub slices: Vec<(String, f64)>,
}

/// Parse a `pie` chart source (`"Label" : value` lines, optional title).
pub fn parse_pie(src: &str) -> Pie {
    let mut pie = Pie::default();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("pie") {
            // `pie [showData] [title ...]`
            if let Some(t) = rest.find("title ") {
                pie.title = rest[t + "title ".len()..].trim().to_string();
            }
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            pie.title = t.trim().to_string();
            continue;
        }
        // `"Label" : 42`
        if let Some((label, value)) = line.split_once(':') {
            let label = label.trim().trim_matches('"').to_string();
            if let Ok(v) = value.trim().parse::<f64>() {
                if !label.is_empty() {
                    pie.slices.push((label, v));
                }
            }
        }
    }
    pie
}

// ===========================================================================
// Gantt chart
// ===========================================================================

/// Visual status of a Gantt task (from its tags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Plain,
    Active,
    Done,
    Crit,
    Milestone,
}

#[derive(Debug, Clone)]
pub struct GanttTask {
    pub section: String,
    pub name: String,
    /// Start day (relative ordinal) and length in days.
    pub start: i64,
    pub len: i64,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Default)]
pub struct Gantt {
    pub title: String,
    pub tasks: Vec<GanttTask>,
}

fn date_to_days(s: &str) -> Option<i64> {
    let mut it = s.trim().split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() || !(1..=12).contains(&m) {
        return None;
    }
    Some(y * 365 + CUM[(m - 1) as usize] + d)
}

/// Approximate days-per-month prefix sums (ignores leap years) — adequate for
/// relative bar placement and axis labels.
const CUM: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];

/// Inverse of [`date_to_days`]: format a day ordinal as `YYYY-MM-DD`.
pub fn day_to_date(ord: i64) -> String {
    let ord = ord.max(0);
    let mut year = ord / 365;
    let mut rem = ord % 365;
    if rem == 0 {
        // Day 0 belongs to 31 Dec of the previous year.
        year -= 1;
        rem = 365;
    }
    let month = CUM.iter().rposition(|&c| c < rem).unwrap_or(0);
    let day = rem - CUM[month];
    format!("{year:04}-{:02}-{:02}", month + 1, day)
}

fn dur_to_days(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('d') {
        n.trim().parse().ok()
    } else if let Some(n) = s.strip_suffix('w') {
        n.trim().parse::<i64>().ok().map(|w| w * 7)
    } else if let Some(n) = s.strip_suffix('h') {
        n.trim().parse::<i64>().ok().map(|h| (h / 24).max(1))
    } else {
        None
    }
}

/// Parse a `gantt` chart. Resolves `after <id>` dependencies and durations
/// (`Nd`/`Nw`) or explicit end dates; leap years are ignored.
pub fn parse_gantt(src: &str) -> Gantt {
    let mut g = Gantt::default();
    let mut section = String::new();
    // id -> (start, end) for `after` resolution.
    let mut ends: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();

    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "gantt" {
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            g.title = t.trim().to_string();
            continue;
        }
        if let Some(s) = line.strip_prefix("section ") {
            section = s.trim().to_string();
            continue;
        }
        // Skip directives without a task body.
        if line.starts_with("dateFormat")
            || line.starts_with("axisFormat")
            || line.starts_with("excludes")
            || line.starts_with("todayMarker")
            || line.starts_with("tickInterval")
        {
            continue;
        }
        let Some((name, meta)) = line.split_once(':') else {
            continue;
        };
        let fields: Vec<&str> = meta.split(',').map(|f| f.trim()).collect();
        let status = if fields.contains(&"milestone") {
            TaskStatus::Milestone
        } else if fields.contains(&"done") {
            TaskStatus::Done
        } else if fields.contains(&"active") {
            TaskStatus::Active
        } else if fields.contains(&"crit") {
            TaskStatus::Crit
        } else {
            TaskStatus::Plain
        };
        // The start field is a date or `after <id>`; duration/end follows.
        let start_idx = fields
            .iter()
            .position(|f| date_to_days(f).is_some() || f.starts_with("after "));
        let Some(si) = start_idx else { continue };
        let start = if let Some(days) = date_to_days(fields[si]) {
            days
        } else if let Some(dep) = fields[si].strip_prefix("after ") {
            ends.get(dep.trim()).map(|(_, e)| *e).unwrap_or(0)
        } else {
            0
        };
        let len = match fields.get(si + 1) {
            Some(f) => dur_to_days(f)
                .or_else(|| date_to_days(f).map(|e| (e - start).max(0)))
                .unwrap_or(1),
            None => 1,
        };
        // An explicit id (non-tag field before the start) enables `after` refs.
        let tags = ["done", "active", "crit", "milestone"];
        if let Some(id) = fields[..si].iter().find(|f| !tags.contains(f)) {
            ends.insert(id.to_string(), (start, start + len));
        }
        g.tasks.push(GanttTask {
            section: section.clone(),
            name: name.trim().to_string(),
            start,
            len,
            status,
        });
    }
    g
}

// ===========================================================================
// User journey
// ===========================================================================

#[derive(Debug, Clone)]
pub struct JourneyTask {
    pub section: String,
    pub name: String,
    pub score: u8,
    pub actors: String,
}

#[derive(Debug, Clone, Default)]
pub struct Journey {
    pub title: String,
    pub tasks: Vec<JourneyTask>,
}

/// Parse a `journey` diagram (`Task: score: actors` lines under sections).
pub fn parse_journey(src: &str) -> Journey {
    let mut j = Journey::default();
    let mut section = String::new();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "journey" {
            continue;
        }
        if let Some(t) = line.strip_prefix("title ") {
            j.title = t.trim().to_string();
            continue;
        }
        if let Some(s) = line.strip_prefix("section ") {
            section = s.trim().to_string();
            continue;
        }
        let parts: Vec<&str> = line.splitn(3, ':').map(|p| p.trim()).collect();
        if parts.len() >= 2 {
            if let Ok(score) = parts[1].parse::<u8>() {
                j.tasks.push(JourneyTask {
                    section: section.clone(),
                    name: parts[0].to_string(),
                    score: score.min(5),
                    actors: parts.get(2).copied().unwrap_or("").to_string(),
                });
            }
        }
    }
    j
}

// ===========================================================================
// Mindmap
// ===========================================================================

/// A mindmap node at a nesting depth (derived from indentation).
#[derive(Debug, Clone)]
pub struct MindNode {
    pub depth: usize,
    pub label: String,
}

#[derive(Debug, Clone, Default)]
pub struct Mindmap {
    pub nodes: Vec<MindNode>,
}

/// Parse a `mindmap` into a flat list of `(depth, label)`, depth from indent.
/// Node shape wrappers (`((x))`, `[x]`, `(x)`) are stripped to the label.
pub fn parse_mindmap(src: &str) -> Mindmap {
    let mut m = Mindmap::default();
    // Map raw indentation widths to contiguous depth levels.
    let mut indents: Vec<usize> = Vec::new();
    for raw in src.lines() {
        if raw.trim().is_empty() || raw.trim().starts_with("%%") || raw.trim() == "mindmap" {
            continue;
        }
        let indent = raw.len() - raw.trim_start().len();
        let depth = match indents.iter().position(|&i| i == indent) {
            Some(d) => d,
            None => {
                // Deeper than any seen → new level; shallower handled by find.
                let d = indents.iter().filter(|&&i| i < indent).count();
                indents.truncate(d);
                indents.push(indent);
                d
            }
        };
        m.nodes.push(MindNode {
            depth,
            label: strip_mind_shape(raw.trim()),
        });
    }
    m
}

fn strip_mind_shape(s: &str) -> String {
    let s = s.trim();
    for (open, close) in [("((", "))"), ("(", ")"), ("[", "]"), ("{{", "}}")] {
        if let Some(inner) = s.strip_prefix(open).and_then(|x| x.strip_suffix(close)) {
            return inner.trim().to_string();
        }
    }
    s.to_string()
}

// ===========================================================================
// Timeline
// ===========================================================================

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub section: String,
    pub period: String,
    pub events: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub title: String,
    pub entries: Vec<TimelineEntry>,
}

/// Parse a `timeline` (`period : event : event` rows, optional sections).
pub fn parse_timeline(src: &str) -> Timeline {
    let mut t = Timeline::default();
    let mut section = String::new();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "timeline" {
            continue;
        }
        if let Some(s) = line.strip_prefix("title ") {
            t.title = s.trim().to_string();
            continue;
        }
        if let Some(s) = line.strip_prefix("section ") {
            section = s.trim().to_string();
            continue;
        }
        let mut parts = line.split(':').map(|p| p.trim().to_string());
        let Some(period) = parts.next() else { continue };
        let events: Vec<String> = parts.filter(|e| !e.is_empty()).collect();
        t.entries.push(TimelineEntry {
            section: section.clone(),
            period,
            events,
        });
    }
    t
}

// ===========================================================================
// Quadrant chart
// ===========================================================================

#[derive(Debug, Clone)]
pub struct QuadPoint {
    pub name: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Quadrant {
    pub title: String,
    pub x_axis: String,
    pub y_axis: String,
    /// Quadrant labels (1..=4 in Mermaid's numbering: TR, TL, BL, BR).
    pub quads: [String; 4],
    pub points: Vec<QuadPoint>,
}

/// Parse a `quadrantChart` (axes, quadrant labels, and `Name: [x, y]` points).
pub fn parse_quadrant(src: &str) -> Quadrant {
    let mut q = Quadrant::default();
    for raw in src.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("%%") || line == "quadrantChart" {
            continue;
        }
        if let Some(s) = line.strip_prefix("title ") {
            q.title = s.trim().to_string();
        } else if let Some(s) = line.strip_prefix("x-axis ") {
            q.x_axis = s.trim().to_string();
        } else if let Some(s) = line.strip_prefix("y-axis ") {
            q.y_axis = s.trim().to_string();
        } else if let Some(s) = line.strip_prefix("quadrant-") {
            if let Some((n, label)) = s.split_once(' ') {
                if let Ok(i) = n.trim().parse::<usize>() {
                    if (1..=4).contains(&i) {
                        q.quads[i - 1] = label.trim().to_string();
                    }
                }
            }
        } else if let Some((name, coords)) = line.split_once(':') {
            // `Name: [x, y]`
            let coords = coords.trim().trim_start_matches('[').trim_end_matches(']');
            let mut it = coords.split(',').map(|c| c.trim().parse::<f64>());
            if let (Some(Ok(x)), Some(Ok(y))) = (it.next(), it.next()) {
                q.points.push(QuadPoint {
                    name: name.trim().to_string(),
                    x: x.clamp(0.0, 1.0),
                    y: y.clamp(0.0, 1.0),
                });
            }
        }
    }
    q
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pie_parses_title_and_slices() {
        let pie = parse_pie("pie title Share\n\"Rust\" : 55\n\"Other\" : 45");
        assert_eq!(pie.title, "Share");
        assert_eq!(pie.slices.len(), 2);
        assert_eq!(pie.slices[0], ("Rust".to_string(), 55.0));
    }
}
