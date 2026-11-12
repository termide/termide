//! The session-info rows that break a session's tokens down: by who spent
//! them, by model, what the cache served and what went on reasoning; and the
//! usage window's, for a project's sessions or every project's.

use std::path::{Path, PathBuf};

use termide_agent_core::usage::{UsagePeriod, UsageReport};
use termide_agent_core::{ContextLoad, Spender, Spending, ToolLoad, Usage};

use crate::{format_tokens, token_label};

/// Cells of a share bar.
const BAR: usize = 10;

/// Every token `usage` counts — uncached input, cache writes and reads,
/// output — each as one: what they cost is the user's to weigh, on terms
/// termide does not know.
#[allow(clippy::cast_precision_loss)]
fn weight(usage: &Usage) -> f64 {
    usage.total() as f64
}

fn spender_label(by: Spender) -> &'static str {
    let t = termide_i18n::t();
    match by {
        Spender::Main => t.agent_spender_main(),
        Spender::Subagent => t.agent_spender_subagent(),
        Spender::Reviewer => t.agent_spender_reviewer(),
        Spender::Recall => t.agent_spender_recall(),
        Spender::Compaction => t.agent_spender_compaction(),
        Spender::GoalJudge => t.agent_spender_goal_judge(),
        Spender::Handoff => t.agent_spender_handoff(),
        Spender::Tool => t.agent_spender_tool(),
        Spender::AgentInternal => t.agent_spender_internal(),
    }
}

/// One part of a breakdown: its name, its share of the whole (0 to 1) and
/// its figures.
#[derive(Debug, Clone)]
pub(crate) struct Share {
    name: String,
    share: f64,
    figures: String,
    /// How full its bar is, 0 to 1, when not its share: a day's against
    /// the busiest day's, so a month of days does not draw empty bars.
    bar: Option<f64>,
}

/// A breakdown: its parts, in the order shown.
pub(crate) type Shares = Vec<Share>;

/// `parts` as shares of `of` tokens, the largest first, each with its tokens as
/// the totals show them.
fn usage_shares(parts: Vec<(String, Usage)>, of: f64) -> Shares {
    let mut parts: Vec<(String, f64, String)> = parts
        .into_iter()
        .map(|(name, usage)| {
            let figures = token_label(usage.uncached(), usage.cache_read, usage.output);
            (name, weight(&usage), figures)
        })
        .collect();
    parts.sort_by(|a, b| b.1.total_cmp(&a.1));
    shares(parts, of)
}

/// `parts` in the order given, each weight a share of `of`.
fn shares(parts: Vec<(String, f64, String)>, of: f64) -> Shares {
    parts
        .into_iter()
        .map(|(name, weight, figures)| Share {
            name,
            share: if of > 0.0 { weight / of } else { 0.0 },
            figures,
            bar: None,
        })
        .collect()
}

/// `part` of `whole`, 0 to 1.
#[allow(clippy::cast_precision_loss)]
fn ratio(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// A share as a percentage: a part too small to round to one still shows
/// it is there.
fn percent(share: f64) -> String {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let percent = (share * 100.0).round() as u64;
    if percent == 0 && share > 0.0 {
        "<1%".to_string()
    } else {
        format!("{percent}%")
    }
}

/// `rows` as text, a line per part: its name, a share bar, its share and
/// its figures, each in a column of its own across all the breakdowns, so
/// the bars of one row stand under those of the others.
pub(crate) fn render(rows: Vec<(String, Shares)>) -> Vec<(String, String)> {
    let all = || rows.iter().flat_map(|(_, parts)| parts);
    let name_width = all().map(|p| p.name.chars().count()).max().unwrap_or(0);
    let percent_width = all().map(|p| percent(p.share).len()).max().unwrap_or(0);
    rows.into_iter()
        .map(|(label, parts)| {
            let text = parts
                .iter()
                .map(|part| {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let filled =
                        ((part.bar.unwrap_or(part.share) * BAR as f64).round() as usize).min(BAR);
                    format!(
                        "{:<name_width$}  {}{}  {:>percent_width$}  {}",
                        part.name,
                        "█".repeat(filled),
                        "░".repeat(BAR - filled),
                        percent(part.share),
                        part.figures,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            (label, text)
        })
        .collect()
}

/// The breakdown rows for `spending`, none when nothing was spent. A
/// model's id is shown as `model_name` names it.
pub(crate) fn spending_rows(
    spending: &Spending,
    model_name: impl Fn(&str) -> String,
) -> Vec<(String, Shares)> {
    let t = termide_i18n::t();
    let total = spending.total();
    if total.total() == 0 {
        return Vec::new();
    }
    let of = weight(&total);
    let mut rows = Vec::new();
    let by: Vec<(String, Usage)> = spending
        .by
        .iter()
        .map(|(by, usage)| (spender_label(*by).to_string(), *usage))
        .collect();
    rows.push((t.agent_info_spent_by().into(), usage_shares(by, of)));
    let models: Vec<(String, Usage)> = spending
        .models
        .iter()
        .map(|(model, usage)| {
            let name = if model.is_empty() {
                t.agent_model_unknown().to_string()
            } else {
                model_name(model)
            };
            (name, *usage)
        })
        .collect();
    rows.push((t.agent_info_by_model().into(), usage_shares(models, of)));
    rows
}

/// The rows that sum `spending` up: what the cache served of all the
/// prompt tokens, and what went on reasoning when the provider counts it.
pub(crate) fn spending_figures(spending: &Spending) -> Vec<(String, String)> {
    let t = termide_i18n::t();
    let total = spending.total();
    let mut rows = Vec::new();
    // What the cache served of all the prompt tokens.
    let prompt = total.uncached().saturating_add(total.cache_read);
    if prompt > 0 {
        rows.push((
            t.agent_info_cache_hits().into(),
            percent(ratio(total.cache_read, prompt)),
        ));
    }
    if total.thinking > 0 && total.output > 0 {
        rows.push((
            t.agent_info_thinking().into(),
            format!(
                "↓{} ({})",
                format_tokens(total.thinking),
                percent(ratio(total.thinking, total.output))
            ),
        ));
    }
    rows
}

/// The row of what cleaning took out of `shell` output over the session —
/// its share of the raw output and its estimated tokens — read from the
/// results in the log, a fork's from its own start; none when no `shell`
/// ran. Counted in characters, as tokens are estimated; a result logged
/// before they were counted, in bytes.
pub(crate) fn cleaning_row(session: &termide_agent_core::Session) -> Option<(String, String)> {
    use termide_agent_core::{EntryKind, Message};
    let (mut raw, mut cleaned) = (0_u64, 0_u64);
    for entry in session.own_entries() {
        let EntryKind::Message {
            message: Message::ToolResult(result),
            ..
        } = &entry.kind
        else {
            continue;
        };
        let Some(details) = result.details.as_ref() else {
            continue;
        };
        let count = |unit: &str| {
            let get = |key: String| details.get(key).and_then(serde_json::Value::as_u64);
            Some((get(format!("raw_{unit}"))?, get(format!("cleaned_{unit}"))?))
        };
        if let Some((r, c)) = count("chars").or_else(|| count("bytes")) {
            raw += r;
            cleaned += c;
        }
    }
    if raw == 0 {
        return None;
    }
    let saved = raw.saturating_sub(cleaned);
    Some((
        termide_i18n::t().agent_info_output_cleaned().into(),
        format!(
            "{}  ≈{}",
            percent(ratio(saved, raw)),
            format_tokens(saved / 4)
        ),
    ))
}

/// The tools shown by name in the tool output row; the rest are summed.
const TOOLS_SHOWN: usize = 8;

/// A tool's results in tokens: brought into the context once, read again
/// with every call after.
#[allow(clippy::cast_precision_loss)]
fn tool_weight(load: &ToolLoad) -> f64 {
    (load.brought + load.carried) as f64
}

/// The rows of what the context is made of: its parts now, `system` and
/// `tools` the estimated tokens of termide's system prompt and of the tool
/// definitions it served, when the model has them; with `fill`, how full an
/// external agent reports its context, what it holds beyond those parts as
/// the agent's own — its prompt, its tools, what it adds. And each tool's
/// results, a skill's as `skill:<name>`, by what they brought in and what
/// later calls read again.
pub(crate) fn context_rows(
    load: &ContextLoad,
    system: Option<u64>,
    tools: Option<u64>,
    fill: Option<u64>,
) -> Vec<(String, Shares)> {
    let t = termide_i18n::t();
    let mut rows = Vec::new();
    let now = &load.now;
    let known = system.unwrap_or(0)
        + tools.unwrap_or(0)
        + now.skills
        + now.tool_output
        + now.conversation
        + now.summary;
    let own = fill.map_or(0, |fill| fill.saturating_sub(known));
    let mut parts: Vec<(String, u64)> = [
        (t.agent_context_system(), system.unwrap_or(0)),
        (t.agent_context_tools(), tools.unwrap_or(0)),
        (t.agent_context_skills(), now.skills),
        (t.agent_context_tool_output(), now.tool_output),
        (t.agent_context_conversation(), now.conversation),
        (t.agent_context_summary(), now.summary),
        (t.agent_context_agent(), own),
    ]
    .into_iter()
    .filter(|(_, tokens)| *tokens > 0)
    .map(|(name, tokens)| (name.to_string(), tokens))
    .collect();
    parts.sort_by(|a, b| b.1.cmp(&a.1));
    let sum: u64 = parts.iter().map(|(_, tokens)| tokens).sum();
    if sum > 0 {
        #[allow(clippy::cast_precision_loss)]
        let lines = shares(
            parts
                .into_iter()
                .map(|(name, tokens)| (name, tokens as f64, format!("≈{}", format_tokens(tokens))))
                .collect(),
            sum as f64,
        );
        rows.push((t.agent_info_context_parts().into(), lines));
    }
    let mut tools: Vec<(&String, &ToolLoad)> = load
        .tools
        .iter()
        .filter(|(_, load)| load.brought + load.carried > 0)
        .collect();
    if !tools.is_empty() {
        tools.sort_by(|a, b| tool_weight(b.1).total_cmp(&tool_weight(a.1)));
        let of: f64 = tools.iter().map(|(_, load)| tool_weight(load)).sum();
        let mut shown: Vec<(String, ToolLoad)> = tools
            .iter()
            .take(TOOLS_SHOWN)
            .map(|(name, load)| ((*name).clone(), **load))
            .collect();
        if tools.len() > TOOLS_SHOWN {
            let mut rest = ToolLoad::default();
            for (_, load) in &tools[TOOLS_SHOWN..] {
                rest.brought += load.brought;
                rest.carried += load.carried;
            }
            shown.push((t.agent_tools_others().to_string(), rest));
        }
        let lines = shares(
            shown
                .into_iter()
                .map(|(name, load)| {
                    let figures = format!(
                        "≈{} ↻{}",
                        format_tokens(load.brought),
                        format_tokens(load.carried)
                    );
                    (name, tool_weight(&load), figures)
                })
                .collect(),
            of,
        );
        rows.push((t.agent_info_by_tool().into(), lines));
    }
    rows
}

/// A line of the usage window: its key, its value, and for a session that
/// can be opened here, its log.
pub type UsageReportLine = (String, String, Option<PathBuf>);

/// How long a session's label may be in the usage window.
const SESSION_LABEL: usize = 48;

/// The usage window's lines for `report`, over `period`: its totals, its
/// breakdowns and the sessions that spent the most, those of the project
/// keyed `here` with their logs to open them. With `all`, every project's,
/// with a breakdown by project, each shown by its path, `home` as `~`.
#[must_use]
pub fn usage_report_lines(
    report: &UsageReport,
    period: UsagePeriod,
    all: bool,
    here: &Path,
    home: Option<&Path>,
) -> Vec<UsageReportLine> {
    let t = termide_i18n::t();
    let total = report.spending.total();
    if report.sessions == 0 || total.total() == 0 {
        return vec![(String::new(), t.usage_nothing().to_string(), None)];
    }
    let mut lines: Vec<UsageReportLine> = vec![
        (
            t.agent_info_tokens().into(),
            token_label(total.uncached(), total.cache_read, total.output),
            None,
        ),
        (t.usage_sessions().into(), report.sessions.to_string(), None),
    ];
    lines.extend(
        spending_figures(&report.spending)
            .into_iter()
            .map(|(key, value)| (key, value, None)),
    );
    let of = weight(&total);
    let mut rows = spending_rows(&report.spending, str::to_string);
    let agents: Vec<(String, Usage)> = report
        .agents
        .iter()
        .map(|(agent, usage)| {
            let name = if agent.is_empty() {
                t.usage_agent_unnamed().to_string()
            } else {
                agent.clone()
            };
            (name, *usage)
        })
        .collect();
    rows.push((t.usage_by_agent().into(), usage_shares(agents, of)));
    if all {
        let projects: Vec<(String, Usage)> = report
            .projects
            .iter()
            .map(|(key, usage)| (project_path(key, home), *usage))
            .collect();
        rows.push((t.usage_by_project().into(), usage_shares(projects, of)));
    }
    if !report.buckets.is_empty() {
        let busiest = report
            .buckets
            .iter()
            .map(|(_, usage)| weight(usage))
            .fold(0.0, f64::max);
        let parts = report
            .buckets
            .iter()
            .map(|(label, usage)| {
                let figures = if usage.total() == 0 {
                    String::new()
                } else {
                    token_label(usage.uncached(), usage.cache_read, usage.output)
                };
                (label.clone(), weight(usage), figures)
            })
            .collect();
        let mut parts = shares(parts, of);
        for part in &mut parts {
            part.bar = Some(if busiest > 0.0 {
                part.share * of / busiest
            } else {
                0.0
            });
        }
        let label = if period == UsagePeriod::All {
            t.usage_by_month()
        } else {
            t.usage_by_day()
        };
        rows.push((label.into(), parts));
    }
    // The days come last, after the sessions: a month of them would push
    // the sessions out of sight.
    let mut rendered = render(rows);
    let days = (!report.buckets.is_empty())
        .then(|| rendered.pop())
        .flatten();
    lines.extend(rendered.into_iter().map(|(key, value)| (key, value, None)));
    for (index, session) in report.top.iter().enumerate() {
        let key = if index == 0 {
            t.usage_top_sessions().to_string()
        } else {
            String::new()
        };
        let mut label = session.label.clone();
        if label.chars().count() > SESSION_LABEL {
            label = label.chars().take(SESSION_LABEL - 1).collect::<String>() + "…";
        }
        let usage = &session.usage;
        let mut value = format!(
            "{label:<SESSION_LABEL$}  {}",
            token_label(usage.uncached(), usage.cache_read, usage.output)
        );
        if all {
            value.push_str("  ");
            value.push_str(&project_path(&session.project, home));
        }
        let link = (session.project == here).then(|| session.path.clone());
        lines.push((key, value, link));
    }
    lines.extend(days.map(|(key, value)| (key, value, None)));
    lines
}

/// A project's path from its key, the home directory as `~`.
fn project_path(key: &Path, home: Option<&Path>) -> String {
    let path = Path::new("/").join(key);
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, cache_read: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read,
            ..Default::default()
        }
    }

    #[test]
    fn nothing_spent_shows_no_breakdown() {
        assert!(spending_rows(&Spending::default(), str::to_string).is_empty());
        assert!(spending_figures(&Spending::default()).is_empty());
    }

    /// Shares count every token as one, the largest first, with the
    /// totals' own token figures.
    #[test]
    fn the_breakdown_shares_the_tokens() {
        let mut spending = Spending::default();
        // 1000 + 10 000 + 0 = 11 000 tokens.
        spending.add(Spender::Main, "big", &usage(1_000, 10_000, 0));
        // 4000 tokens of output: no more than they are, whatever they cost.
        spending.add(Spender::Reviewer, "small", &usage(0, 0, 4_000));
        spending.add(Spender::Subagent, "", &usage(0, 0, 0));
        let rows = render(spending_rows(&spending, |id| format!("M-{id}")));
        let by = &rows[0].1;
        let lines: Vec<&str> = by.lines().collect();
        assert_eq!(lines.len(), 2, "{by}");
        assert!(lines[0].contains("███████░░░  73%"), "{by}");
        assert!(lines[1].contains("███░░░░░░░  27%"), "{by}");
        assert!(by.contains("↑1k ↻10k ↓0"), "{by}");
        let models = &rows[1].1;
        assert!(
            models.contains("M-big") && models.contains("M-small"),
            "{models}"
        );
        // 10 000 of 11 000 prompt tokens came from the cache: 90.9%,
        // rounded as the shares are.
        assert_eq!(rows.len(), 2);
        let figures = spending_figures(&spending);
        assert_eq!(figures[0].1, "91%");
        assert_eq!(figures.len(), 1, "no reasoning counted, no row for it");
    }

    /// The context's parts share what it holds; a tool's results count what
    /// later calls read of them too, so a small result read many times
    /// outweighs a big one read once, and the tools past the shown ones are
    /// summed.
    #[test]
    fn the_context_rows_tell_what_it_is_made_of() {
        let mut load = ContextLoad::default();
        load.now.conversation = 300;
        load.now.skills = 100;
        let mut tool = |name: &str, brought, carried| {
            load.tools
                .insert(name.to_string(), ToolLoad { brought, carried });
        };
        // 1000 tokens, read once.
        tool("shell", 1_000, 0);
        // 100 tokens, read again by later calls 200 times over.
        tool("skill:review", 100, 20_000);
        for n in 0..TOOLS_SHOWN {
            tool(&format!("t{n}"), 1, 0);
        }
        let rows = render(context_rows(&load, Some(600), None, None));
        let parts = &rows[0].1;
        let lines: Vec<&str> = parts.lines().collect();
        assert_eq!(lines.len(), 3, "{parts}");
        assert!(lines[0].starts_with("System prompt") && lines[0].contains(" 60%  ≈600"));
        let tools: Vec<&str> = rows[1].1.lines().collect();
        assert!(tools[0].starts_with("skill:review"), "{tools:?}");
        assert!(tools[0].ends_with("≈100 ↻20k"), "{tools:?}");
        assert!(tools[1].starts_with("shell"), "{tools:?}");
        assert_eq!(tools.len(), TOOLS_SHOWN + 1);
        assert!(tools[TOOLS_SHOWN].starts_with("others"), "{tools:?}");
    }

    /// An external agent's reported fill beyond the parts termide knows is
    /// the agent's own; with none reported, there is no such part.
    #[test]
    fn what_an_agent_adds_to_its_context_is_its_own() {
        let mut load = ContextLoad::default();
        load.now.conversation = 3_000;
        let rows = render(context_rows(&load, Some(1_000), None, Some(10_000)));
        let lines: Vec<&str> = rows[0].1.lines().collect();
        assert!(lines[0].starts_with("Agent's own"), "{lines:?}");
        assert!(lines[0].ends_with(" 60%  ≈6k"), "{lines:?}");
        let rows = render(context_rows(&load, Some(1_000), None, None));
        assert_eq!(rows[0].1.lines().count(), 2);
    }

    /// A part too small to round to a percent still shows it is there.
    #[test]
    fn a_tiny_share_is_not_shown_as_none() {
        assert_eq!(percent(0.004), "<1%");
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.996), "100%");
    }

    #[test]
    fn reasoning_shows_as_a_part_of_the_output() {
        let mut spending = Spending::default();
        let mut spent = usage(10, 0, 200);
        spent.thinking = 50;
        spending.add(Spender::Main, "m", &spent);
        let rows = spending_figures(&spending);
        assert_eq!(rows.last().map(|r| r.1.as_str()), Some("↓50 (25%)"));
    }
}

#[cfg(test)]
mod usage_tests {
    use std::path::{Path, PathBuf};

    use termide_agent_core::usage::{SessionUsage, UsagePeriod, UsageReport};
    use termide_agent_core::{Spender, Usage};

    use super::usage_report_lines;

    fn spent(input: u64) -> Usage {
        Usage {
            input,
            ..Default::default()
        }
    }

    fn report() -> UsageReport {
        let mut report = UsageReport::default();
        report.spending.add(Spender::Main, "big", &spent(900));
        report.spending.add(Spender::Reviewer, "small", &spent(100));
        report.sessions = 2;
        report.agents.insert("default".into(), spent(1_000));
        report
            .projects
            .insert(PathBuf::from("home/me/a"), spent(600));
        report.projects.insert(PathBuf::from("srv/b"), spent(400));
        report.buckets = vec![
            ("2026-11-10".into(), spent(0)),
            ("2026-11-11".into(), spent(1_000)),
        ];
        for (name, project, input) in [("mine", "home/me/a", 600), ("theirs", "srv/b", 400)] {
            report.top.push(SessionUsage {
                path: PathBuf::from(format!("/s/{name}.jsonl")),
                project: PathBuf::from(project),
                label: name.into(),
                agent: None,
                modified: 0,
                usage: spent(input),
            });
        }
        report
    }

    /// The window lists the totals, the breakdowns with the busiest day's
    /// bar full, and the costliest sessions, only this project's to open.
    #[test]
    fn the_usage_lines_break_the_report_down() {
        let lines = usage_report_lines(
            &report(),
            UsagePeriod::Week,
            true,
            Path::new("home/me/a"),
            Some(Path::new("/home/me")),
        );
        let value = |key: &str| {
            lines
                .iter()
                .find(|(k, _, _)| k == key)
                .map(|(_, v, _)| v.clone())
                .unwrap_or_else(|| panic!("no {key} in {lines:?}"))
        };
        assert_eq!(value("Tokens"), "↑1k ↓0");
        assert_eq!(value("Sessions"), "2");
        let projects = value("By project");
        assert!(
            projects.contains("~/a") && projects.contains("/srv/b"),
            "{projects}"
        );
        let days = value("By day");
        let day = |date: &str| {
            days.lines()
                .find(|l| l.starts_with(date))
                .unwrap()
                .to_string()
        };
        assert!(day("2026-11-11").contains("██████████  100%"), "{days}");
        assert!(day("2026-11-10").contains("░░░░░░░░░░    0%"), "{days}");
        let sessions: Vec<&(String, String, Option<PathBuf>)> = lines
            .iter()
            .skip_while(|(k, _, _)| k != "Top sessions")
            .take_while(|(k, _, _)| k != "By day")
            .collect();
        assert_eq!(lines.last().map(|(k, _, _)| k.as_str()), Some("By day"));
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].2, Some(PathBuf::from("/s/mine.jsonl")));
        assert!(sessions[0].1.ends_with("~/a"), "{:?}", sessions[0]);
        assert_eq!(sessions[1].2, None);
        // One project's report has no breakdown by project.
        let here = usage_report_lines(&report(), UsagePeriod::Week, false, Path::new("x"), None);
        assert!(here.iter().all(|(k, _, _)| k != "By project"));
        // Nothing spent says so.
        let none = usage_report_lines(
            &UsageReport::default(),
            UsagePeriod::Today,
            false,
            Path::new("x"),
            None,
        );
        assert_eq!(none.len(), 1);
    }
}
