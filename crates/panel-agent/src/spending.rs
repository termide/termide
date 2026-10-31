//! The session-info rows that break a session's tokens down: by who spent
//! them, by model, what the cache served and what went on reasoning.

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
                    let filled = ((part.share * BAR as f64).round() as usize).min(BAR);
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

/// The row of what cleaning took out of `bash` output over the session —
/// its share of the raw output and its estimated tokens — read from the
/// results in the log, a fork's from its own start; none when no `bash`
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
        tool("bash", 1_000, 0);
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
        assert!(tools[1].starts_with("bash"), "{tools:?}");
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
