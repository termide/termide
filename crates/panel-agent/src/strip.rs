//! The state strip above the input: what holds right now rather than what
//! happened — a pause asked for or a retry waited for, the queued messages,
//! an active `/goal` or `/loop`, the running subagents and the commands the
//! agent moved to the background. Every row reads the same way: its glyph
//! and what it is, in the accent; what it works on, dim; at the right edge
//! how long it has gone on and, where it can be called off, `✕`. No row
//! spins: the transcript's live line does, once, for the run.

use std::time::Instant;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use termide_core::ThemeColors;

use crate::render::{capitalized, Autorun, GOAL_GLYPH, LOOP_GLYPH, STATE_QUEUED_ROWS};
use crate::{single_line, transcript, AgentPanel, PanelEvent, LOOP_MAX_ITERATIONS};

/// The stop button at the right end of a row.
pub(crate) const STRIP_STOP: &str = "✕";

/// What a row stands for, for a click on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StripTarget {
    Pause,
    RetryWait,
    Queued,
    Autorun(Autorun),
    /// A running subagent, by its block's index.
    Task(usize),
    /// A command the agent moved to the background, by its block's index.
    Background(usize),
    /// The count of rows left out.
    More,
}

/// One row of the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StripRow {
    pub glyph: String,
    /// What it is (`Heading for the goal`), in the accent after the glyph;
    /// empty for a count of rows left out.
    pub label: String,
    /// Short facts about its state, kept whole: dim, after the label.
    pub notes: Vec<String>,
    /// What it works on, dim; cut first when the row is short.
    pub body: String,
    /// What it does right now, dim, before the time (a subagent's last
    /// step); cut to a third of the row.
    pub status: String,
    /// How long it has gone on, in ms.
    pub time: Option<u32>,
    /// Whether `✕` calls it off.
    pub stop: bool,
    pub target: StripTarget,
}

impl StripRow {
    fn new(glyph: &str, label: String, body: String, target: StripTarget) -> Self {
        Self {
            glyph: glyph.to_string(),
            label,
            notes: Vec::new(),
            body,
            status: String::new(),
            time: None,
            stop: false,
            target,
        }
    }

    fn since(mut self, at: Instant) -> Self {
        self.time = Some(u32::try_from(at.elapsed().as_millis()).unwrap_or(u32::MAX));
        self
    }

    fn stoppable(mut self) -> Self {
        self.stop = true;
        self
    }

    /// A count of rows left out, under the glyph of the rows it stands for.
    fn more(glyph: &str, text: String) -> Self {
        Self::new(glyph, String::new(), text, StripTarget::More)
    }
}

/// The columns a row's body keeps before its notes are cut.
const MIN_BODY: usize = 10;

/// The column of a row's `✕`, from the strip's left edge, for a strip
/// `width` cells wide: the row's last cell is left blank.
pub(crate) fn strip_stop_column(width: u16) -> u16 {
    width.saturating_sub(2)
}

/// The rows drawn `width` cells wide. The glyph, label, notes, time and
/// `✕` stay whole; the body gives way, and a row with no `✕` keeps its
/// place blank so the times line up.
pub(crate) fn strip_lines(
    rows: &[StripRow],
    width: u16,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(colors.disabled);
    let accent = Style::default()
        .fg(colors.info)
        .add_modifier(Modifier::BOLD);
    let width = width as usize;
    let cut = |text: &str, room: usize| termide_ui::path_utils::truncate_right(text, room);
    let w = termide_ui::str_display_width;
    rows.iter()
        .map(|row| {
            let head = if row.label.is_empty() {
                format!("{} ", row.glyph)
            } else {
                format!("{} {}", row.glyph, row.label)
            };
            let status = if row.status.is_empty() {
                String::new()
            } else {
                format!(" {}", cut(&row.status, width / 3))
            };
            let time = row
                .time
                .map(|ms| format!(" {}", transcript::fmt_dur(ms)))
                .unwrap_or_default();
            let stop = format!(" {}", if row.stop { STRIP_STOP } else { " " });
            let gap = if row.label.is_empty() || row.body.is_empty() {
                ""
            } else {
                "  "
            };
            // The notes give way only where the body would have no room.
            let notes = if row.notes.is_empty() {
                String::new()
            } else {
                let room = width.saturating_sub(
                    w(&head) + w(gap) + w(&status) + w(&time) + w(&stop) + 1 + MIN_BODY,
                );
                cut(&format!(" · {}", row.notes.join(" · ")), room)
            };
            let fixed = w(&head) + w(&notes) + w(gap) + w(&status) + w(&time) + w(&stop) + 1;
            let body = cut(&row.body, width.saturating_sub(fixed));
            let used = fixed - 1 + w(&body);
            let mut spans = vec![
                Span::styled(head, accent),
                Span::styled(notes, dim),
                Span::styled(format!("{gap}{body}"), dim),
                Span::raw(" ".repeat(width.saturating_sub(used + 1))),
                Span::styled(status, dim),
                Span::styled(time, dim),
            ];
            spans.push(if row.stop {
                Span::styled(stop, accent)
            } else {
                Span::raw(stop)
            });
            Line::from(spans)
        })
        .collect()
}

impl AgentPanel {
    /// The strip's rows, in order: a retry waited for or a pause asked for,
    /// the queued messages, the goal and the loop, the running subagents and
    /// the background commands. Empty when nothing holds.
    pub(crate) fn strip_rows(&mut self, width: u16) -> Vec<StripRow> {
        let t = termide_i18n::t();
        let mut rows = Vec::new();
        if let Some(wait) = &self.retry_wait {
            rows.push(
                StripRow::new(
                    crate::failure::WAIT_GLYPH,
                    crate::failure::title(wait.failure.kind),
                    t.agent_strip_retry_at_fmt(
                        &crate::failure::clock(wait.wall),
                        &crate::failure::left_until(wait.wall),
                    ),
                    StripTarget::RetryWait,
                )
                .since(wait.since)
                .stoppable(),
            );
        } else if self.pause_requested {
            rows.push(
                StripRow::new(
                    transcript::PAUSED_GLYPH,
                    t.agent_strip_pause().to_string(),
                    t.agent_strip_after_step().to_string(),
                    StripTarget::Pause,
                )
                .since(self.pause_requested_at)
                .stoppable(),
            );
        }
        let queued_label = capitalized(t.agent_state_queued());
        for (text, since) in self.queued_texts.iter().take(STATE_QUEUED_ROWS) {
            let first = text.trim().lines().next().unwrap_or("").to_string();
            rows.push(
                StripRow::new("›", queued_label.clone(), first, StripTarget::Queued).since(*since),
            );
        }
        if self.queued_texts.len() > STATE_QUEUED_ROWS {
            rows.push(StripRow::more(
                "›",
                t.agent_state_queued_more(self.queued_texts.len() - STATE_QUEUED_ROWS)
                    .trim_start_matches(['…', ' '])
                    .to_string(),
            ));
        }
        if let Some(task) = &self.goal_task {
            let mut row = StripRow::new(
                GOAL_GLYPH,
                capitalized(t.agent_notice_goal_working()),
                single_line(&task.goal),
                StripTarget::Autorun(Autorun::Goal),
            )
            .since(task.started)
            .stoppable();
            if task.judging {
                row.notes.push(t.agent_strip_checking().to_string());
            }
            rows.push(row);
        }
        if let Some(task) = &self.loop_task {
            let label = match task.interval {
                Some(d) => t.agent_autorun_loop_every_fmt(&termide_i18n::duration(d)),
                None => t.agent_autorun_loop().to_string(),
            };
            let mut row = StripRow::new(
                LOOP_GLYPH,
                capitalized(&label),
                single_line(&task.prompt),
                StripTarget::Autorun(Autorun::Loop),
            )
            .since(task.started)
            .stoppable();
            row.notes
                .push(format!("{}/{LOOP_MAX_ITERATIONS}", task.iterations));
            if let Some(wait) = self.loop_wait() {
                row.notes.push(t.agent_strip_next_fmt(&wait));
            }
            rows.push(row);
        }
        let tasks = self.running_tasks();
        let more = tasks.len().saturating_sub(STATE_QUEUED_ROWS);
        rows.extend(tasks.into_iter().take(STATE_QUEUED_ROWS));
        if more > 0 {
            rows.push(StripRow::more("&", format!("+{more}")));
        }
        rows.extend(self.background_rows(width));
        rows
    }

    /// The running subagents' rows: `& Delegating to · agent  prompt`, its
    /// last step before the time. The clock of each starts when it is first
    /// seen, and is dropped once the call has returned.
    fn running_tasks(&mut self) -> Vec<StripRow> {
        let t = termide_i18n::t();
        let now = Instant::now();
        let mut seen = Vec::new();
        let mut rows = Vec::new();
        for (index, call, live) in self.transcript.running_tasks() {
            let start = *self.task_clocks.entry(call.id.clone()).or_insert(now);
            seen.push(call.id.clone());
            let arg = |key: &str| call.arguments[key].as_str().unwrap_or("").to_string();
            let last_line = |text: &str, rev: bool| {
                let mut lines: Box<dyn Iterator<Item = &str>> = if rev {
                    Box::new(text.lines().rev())
                } else {
                    Box::new(text.lines())
                };
                lines
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            let mut row = StripRow::new(
                "&",
                t.agent_tool_task().to_string(),
                last_line(&arg("prompt"), false),
                StripTarget::Task(index),
            )
            .since(start);
            row.notes.push(arg("agent"));
            row.status = last_line(live.unwrap_or(""), true);
            rows.push(row);
        }
        self.task_clocks.retain(|id, _| seen.contains(id));
        rows
    }

    /// The rows of the commands the agent moved to the background that
    /// termide still runs, each worded as its block's headline.
    fn background_rows(&self, width: u16) -> Vec<StripRow> {
        self.runtime
            .background_calls()
            .into_iter()
            .filter_map(|background| {
                let index = self.transcript.index_of_call(&background.call.id)?;
                let head = transcript::call_headline(&background.call, width, &self.colors);
                let text = |span: Option<&Span>| {
                    span.map(|s| s.content.trim().to_string())
                        .unwrap_or_default()
                };
                let body: String = head.iter().skip(2).map(|s| s.content.as_ref()).collect();
                let mut row = StripRow::new(
                    &text(head.first()),
                    text(head.get(1)),
                    body.trim().to_string(),
                    StripTarget::Background(index),
                );
                row.time = Some(u32::try_from(background.elapsed.as_millis()).unwrap_or(u32::MAX));
                Some(row)
            })
            .take(STATE_QUEUED_ROWS)
            .collect()
    }

    /// The strip's lines `width` cells wide: a dim dashed rule, then its
    /// rows. Empty when nothing holds.
    #[cfg(test)]
    pub(crate) fn state_lines(&mut self, width: u16) -> Vec<Line<'static>> {
        let rows = self.strip_rows(width);
        self.strip_with_rule(&rows, width)
    }

    pub(crate) fn strip_with_rule(&self, rows: &[StripRow], width: u16) -> Vec<Line<'static>> {
        if rows.is_empty() {
            return Vec::new();
        }
        let mut lines = vec![transcript::separator(width, &self.colors)];
        lines.extend(strip_lines(rows, width, &self.colors));
        lines
    }

    /// A click on the strip's row `target`, on its `✕` when `on_stop`.
    pub(crate) fn click_strip(&mut self, target: StripTarget, on_stop: bool) -> Vec<PanelEvent> {
        let select = |panel: &mut Self, index: usize| {
            panel.chat_focus = true;
            panel.follow = false;
            panel.selected = index;
            panel.scroll_selected_into_view();
        };
        match target {
            // The whole row withdraws a pause asked for, as it always has.
            StripTarget::Pause => self.cancel_pause(),
            StripTarget::RetryWait if on_stop => self.cancel_retry_wait(),
            // `✕` stops a goal or loop, as `/goal stop` and `/loop stop` do;
            // elsewhere on the row its latest run comes into view.
            StripTarget::Autorun(kind) if on_stop => {
                match kind {
                    Autorun::Goal => self.stop_goal(),
                    Autorun::Loop => self.stop_loop(),
                }
                self.settle_restored();
            }
            StripTarget::Autorun(kind) => {
                if let Some(index) = self.autorun_block(kind) {
                    select(self, index);
                }
            }
            // A subagent's block; a task running in a panel of its own
            // brings that panel forward.
            StripTarget::Task(index) => {
                select(self, index);
                let mut events = vec![PanelEvent::NeedsRedraw];
                if let Some(transcript::Item::Tool { call, .. }) =
                    self.transcript.items().get(index)
                {
                    events.push(PanelEvent::FocusDelegate {
                        panel: self.panel_id,
                        call: call.id.clone(),
                    });
                }
                return events;
            }
            StripTarget::Background(index) => select(self, index),
            StripTarget::RetryWait | StripTarget::Queued | StripTarget::More => {}
        }
        vec![PanelEvent::NeedsRedraw]
    }
}
