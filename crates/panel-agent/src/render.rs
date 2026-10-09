//! Rendering: the transcript and the prompt box, the welcome banner, the
//! run controls and state strip, and the status-bar segments.

use crate::runtime::local_minute;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use termide_core::{RenderContext, SegmentKind, StatusSegment, ThemeColors};
use termide_ui::ScrollBar;

use crate::submit::fmt_secs;
use crate::toolset::TOOLSET_ACTION;
use crate::{
    format_tokens, provider_label, shorten_path, single_line, transcript, truncate_title,
    AgentEntry, AgentPanel, BannerHit, Phase, RunButton, AGENT_ACTION, CONNECTION_ACTION,
    CWD_ACTION, GOAL_COMMAND, GOAL_MAX_ITERATIONS, LOOP_COMMAND, LOOP_MAX_ITERATIONS, MODEL_ACTION,
    MODE_ACTION, OPTIONS_ACTION, REASONING_ACTION,
};

/// Rows of the welcome banner's logo.
const WELCOME_LOGO_ROWS: usize = 5;

/// The column the banner's values begin at, counted from where its labels
/// begin. Labels are padded to it, never below one space, so a longer label —
/// `Herramientas`, `エージェント` — pushes only its own value right.
const LABEL_COL: usize = 12;

/// Queued messages the state strip shows before folding the rest into a count.
pub(crate) const STATE_QUEUED_ROWS: usize = 3;

/// The state strip's lines: a dim dashed rule, then a pause row (`pause`:
/// its glyph and text, when a pause is pending or a retry waits) and a row
/// per queued message (its first line, cut to the width), at most
/// [`STATE_QUEUED_ROWS`] of them before a "… N more" row. Empty when there is nothing to show.
pub(crate) fn state_strip<'a>(
    queued: impl ExactSizeIterator<Item = &'a str>,
    pause: Option<(&str, &str)>,
    width: u16,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let t = termide_i18n::t();
    let dim = Style::default().fg(colors.disabled);
    let total = queued.len();
    if total == 0 && pause.is_none() {
        return Vec::new();
    }
    let width = width as usize;
    let cut = |text: &str, room: usize| termide_ui::path_utils::truncate_right(text, room);
    let mut lines = vec![transcript::separator(width as u16, colors)];
    if let Some((glyph, pause)) = pause {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{glyph} "),
                Style::default()
                    .fg(colors.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(cut(pause, width.saturating_sub(2)), dim),
        ]));
    }
    let label = format!(" {}", t.agent_state_queued());
    let label_width = termide_ui::str_display_width(&label);
    for (i, text) in queued.take(STATE_QUEUED_ROWS).enumerate() {
        let first = text.trim().lines().next().unwrap_or("");
        // The first row carries the "queued" label at the right edge, one
        // column short of the scrollbar gutter, like a block's meta.
        let room = width.saturating_sub(3 + if i == 0 { label_width } else { 0 });
        let body = cut(first, room);
        let mut spans = vec![
            Span::styled("› ", Style::default().fg(colors.info)),
            Span::styled(body.clone(), dim),
        ];
        if i == 0 {
            let used = 2 + termide_ui::str_display_width(&body) + label_width;
            spans.push(Span::raw(" ".repeat(width.saturating_sub(used + 1))));
            spans.push(Span::styled(label.clone(), dim));
        }
        lines.push(Line::from(spans));
    }
    if total > STATE_QUEUED_ROWS {
        lines.push(Line::styled(
            format!("  {}", t.agent_state_queued_more(total - STATE_QUEUED_ROWS)),
            dim,
        ));
    }
    lines
}

/// The state strip's subagent rows, one per running `task` call
/// (`(agent, prompt, progress, elapsed ms)`), at most [`STATE_QUEUED_ROWS`]
/// before a `+N` row: `& agent: prompt` on the left and, at the right edge,
/// what the subagent did last (or its wait for a slot), a spinner and its
/// time. A block scrolled out of view stays in sight here while it runs.
pub(crate) fn task_strip(
    tasks: &[(String, String, String, u32)],
    width: u16,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(colors.disabled);
    let accent = Style::default().fg(colors.info);
    let width = width as usize;
    let cut = |text: &str, room: usize| termide_ui::path_utils::truncate_right(text, room);
    let mut lines = Vec::new();
    for (agent, prompt, progress, ms) in tasks.iter().take(STATE_QUEUED_ROWS) {
        let frames = transcript::RUN_FRAMES;
        let frame = (*ms / 120) as usize % frames.len();
        let clock = format!(" {} {}", frames[frame], transcript::fmt_dur(*ms));
        // What it does now takes at most a third of the row, so the task
        // itself stays readable.
        let progress = cut(progress, width / 3);
        let right = if progress.is_empty() {
            clock.clone()
        } else {
            format!(" {progress}{clock}")
        };
        let right_width = termide_ui::str_display_width(&right);
        let head = format!("{agent}: {prompt}");
        let room = width.saturating_sub(3 + right_width);
        let body = cut(&head, room);
        let used = 2 + termide_ui::str_display_width(&body) + right_width;
        let mut spans = vec![
            Span::styled("& ", accent),
            Span::styled(body, Style::default().fg(colors.fg)),
            Span::raw(" ".repeat(width.saturating_sub(used + 1))),
        ];
        if !progress.is_empty() {
            spans.push(Span::styled(format!(" {progress}"), dim));
        }
        spans.push(Span::styled(
            format!(" {}", frames[frame]),
            accent.add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {}", transcript::fmt_dur(*ms)), dim));
        lines.push(Line::from(spans));
    }
    if tasks.len() > STATE_QUEUED_ROWS {
        lines.push(Line::styled(
            format!("& +{}", tasks.len() - STATE_QUEUED_ROWS),
            dim,
        ));
    }
    lines
}

/// The state strip's rows for what goes on by itself between requests (an
/// active `/goal` or `/loop`): `glyph command` on the left and, at the right
/// edge, its state, dim. The right side takes at most half the row, so the
/// command stays readable.
pub(crate) fn autorun_strip(
    rows: &[(&'static str, String, String)],
    width: u16,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(colors.disabled);
    let accent = Style::default()
        .fg(colors.info)
        .add_modifier(Modifier::BOLD);
    let width = width as usize;
    let cut = |text: &str, room: usize| termide_ui::path_utils::truncate_right(text, room);
    rows.iter()
        .map(|(glyph, command, state)| {
            let state = if state.is_empty() {
                String::new()
            } else {
                format!(" {}", cut(state, width / 2))
            };
            let state_width = termide_ui::str_display_width(&state);
            let body = cut(command, width.saturating_sub(3 + state_width));
            let used = 2 + termide_ui::str_display_width(&body) + state_width;
            Line::from(vec![
                Span::styled(format!("{glyph} "), accent),
                Span::styled(body, Style::default().fg(colors.fg)),
                Span::raw(" ".repeat(width.saturating_sub(used + 1))),
                Span::styled(state, dim),
            ])
        })
        .collect()
}

/// The glyph of an active `/goal` in the state strip.
pub(crate) const GOAL_GLYPH: &str = "◎";
/// The glyph of an active `/loop` in the state strip.
pub(crate) const LOOP_GLYPH: &str = "↺";

/// An eight-cell fill bar for a 0–100 percentage, e.g. `▰▰▱▱▱▱▱▱` at 25%.
/// Cells round to the nearest, so the bar never runs more than half a cell
/// ahead of or behind the figure it stands beside.
pub(crate) fn context_bar(percent: u64) -> String {
    const CELLS: u64 = 8;
    let filled = ((percent * CELLS + 50) / 100).min(CELLS);
    let mut bar = String::with_capacity(CELLS as usize * 3);
    for i in 0..CELLS {
        bar.push(if i < filled { '▰' } else { '▱' });
    }
    bar
}

impl AgentPanel {
    /// The state strip above the input: what holds right now rather than what
    /// happened — a pause asked for but not reached yet, and the queued
    /// messages. Empty when there is nothing to show. A pause that took
    /// effect is the transcript's `‖` line, not a line here.
    pub(crate) fn state_lines(&self, width: u16) -> Vec<Line<'static>> {
        let wait = self.retry_wait_text();
        let pause = match &wait {
            Some(wait) => Some((crate::failure::WAIT_GLYPH, wait.as_str())),
            None => self.pause_requested.then(|| {
                (
                    transcript::PAUSED_GLYPH,
                    termide_i18n::t().agent_notice_will_pause(),
                )
            }),
        };
        state_strip(
            self.queued_texts.iter().map(String::as_str),
            pause,
            width,
            &self.colors,
        )
    }

    /// The active goal as a sentence (`Goal: … — turn 3 of 50`), for a bare
    /// `/goal`; `None` without one.
    pub(crate) fn goal_status(&self) -> Option<String> {
        let task = self.goal_task.as_ref()?;
        Some(termide_i18n::t().agent_unfinished_goal_fmt(
            &single_line(&task.goal),
            task.iterations,
            GOAL_MAX_ITERATIONS,
        ))
    }

    /// The active loop as a sentence (`Loop (5m): … — run 3 of 100, next
    /// run in 4m`), for a bare `/loop`; `None` without one.
    pub(crate) fn loop_status(&self) -> Option<String> {
        let task = self.loop_task.as_ref()?;
        let t = termide_i18n::t();
        let interval = match task.interval {
            Some(d) => fmt_secs(d.as_secs()),
            None => t.agent_unfinished_back_to_back().to_string(),
        };
        let mut line = t.agent_unfinished_loop_fmt(
            &interval,
            &single_line(&task.prompt),
            task.iterations,
            LOOP_MAX_ITERATIONS,
        );
        if let Some(wait) = self.loop_wait() {
            line.push_str(", ");
            line.push_str(&t.agent_unfinished_loop_due_fmt(&wait));
        }
        Some(line)
    }

    /// What is left of the active loop's wait for its next run, rounded up
    /// to the second; `None` while a run is in flight or due.
    fn loop_wait(&self) -> Option<String> {
        let at = self.loop_task.as_ref()?.next_at?;
        let left = at.saturating_duration_since(Instant::now());
        (!left.is_zero()).then(|| fmt_secs(left.as_secs() + u64::from(left.subsec_nanos() > 0)))
    }

    /// The state strip's rows for an active goal and loop (see
    /// [`autorun_strip`]): the command as typed, and its turn or run count —
    /// with the judge's check or the wait for the next run before it.
    pub(crate) fn autorun_rows(&self) -> Vec<(&'static str, String, String)> {
        let t = termide_i18n::t();
        let mut rows = Vec::new();
        if let Some(task) = &self.goal_task {
            let count = format!("{}/{GOAL_MAX_ITERATIONS}", task.iterations);
            let state = if task.judging {
                format!("{} · {count}", t.agent_notice_goal_checking())
            } else {
                count
            };
            rows.push((
                GOAL_GLYPH,
                format!("/{GOAL_COMMAND} {}", single_line(&task.goal)),
                state,
            ));
        }
        if let Some(task) = &self.loop_task {
            let command = match task.interval {
                Some(d) => format!(
                    "/{LOOP_COMMAND} {} {}",
                    fmt_secs(d.as_secs()),
                    single_line(&task.prompt)
                ),
                None => format!("/{LOOP_COMMAND} {}", single_line(&task.prompt)),
            };
            let count = format!("{}/{LOOP_MAX_ITERATIONS}", task.iterations);
            let state = match self.loop_wait() {
                Some(wait) => format!("{} · {count}", t.agent_unfinished_loop_due_fmt(&wait)),
                None => count,
            };
            rows.push((LOOP_GLYPH, command, state));
        }
        rows
    }

    /// The running subagents for the state strip: each one's block index and
    /// `(agent, prompt's first line, its latest progress line, elapsed ms)`.
    /// The clock of each starts when it is first seen, and is dropped once
    /// the call has returned.
    pub(crate) fn running_task_rows(&mut self) -> Vec<(usize, (String, String, String, u32))> {
        let now = Instant::now();
        let mut seen = Vec::new();
        let mut rows = Vec::new();
        for (index, call, live) in self.transcript.running_tasks() {
            let start = *self.task_clocks.entry(call.id.clone()).or_insert(now);
            seen.push(call.id.clone());
            let arg = |key: &str| call.arguments[key].as_str().unwrap_or("").to_string();
            let prompt = arg("prompt");
            let prompt = prompt
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string();
            let progress = live
                .unwrap_or("")
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string();
            let ms = now.duration_since(start).as_millis() as u32;
            rows.push((index, (arg("agent"), prompt, progress, ms)));
        }
        self.task_clocks.retain(|id, _| seen.contains(id));
        rows
    }

    /// The streaming block's live meta line: the same right-aligned zone a
    /// finished block shows, but with the block glyph, the ticking elapsed time
    /// (and a live token estimate while generating) and, in place of the status
    /// check, an animated spinner. Sits after the last block, animating on the
    /// panel's ~10 fps redraw while busy; `None` when idle.
    pub(crate) fn live_footer_lines(&self, width: u16) -> Vec<Line<'static>> {
        let Some(activity) = self.activity.as_ref() else {
            return Vec::new();
        };
        let dim = Style::default().fg(self.colors.disabled);
        let mut lines: Vec<Line<'static>> = Vec::new();
        // Until the first token, a prefill line in the shape of a finished
        // block's `⏫` meta: the time the model has been reading and what it
        // reads. A server that reports its progress gets a bar, the tokens
        // read of the total and the speed over those not served from its
        // cache; any other gets the loop's estimate of the prompt (the exact
        // count arrives with the turn's usage). A long context re-read after
        // reopening a session otherwise shows nothing but the run clock.
        // An external agent's message starts with its first text, so it has
        // no prefill to show.
        let reading = match activity.phase {
            Phase::Prefill => true,
            Phase::Compact => activity.first_token.is_none(),
            Phase::Generating | Phase::Tool => false,
        };
        // A request waiting for a free slot of its connection is not being
        // read yet: the line says it waits, and how many wait before it.
        if let (Some(ahead), true) = (activity.queued, reading) {
            let t = termide_i18n::t();
            let waiting = if ahead == 0 {
                t.agent_queued().to_string()
            } else {
                t.agent_queued_ahead_fmt(ahead)
            };
            let dur = transcript::fmt_dur(activity.msg_start.elapsed().as_millis() as u32);
            lines.push(transcript::right_meta(
                width,
                vec![Span::styled(format!("⏳ {dur} {waiting}"), dim)],
            ));
        } else if reading && !self.external {
            let prefill_ms = activity.msg_start.elapsed().as_millis() as u32;
            let dur = transcript::fmt_dur(prefill_ms);
            let text = match (activity.prefill, activity.prompt_tokens) {
                (Some((processed, total, cached)), _) => Some(format!(
                    "⏫ {dur} {} (↑{}/{}, {})",
                    context_bar(processed * 100 / total.max(1)),
                    format_tokens(processed),
                    format_tokens(total),
                    transcript::fmt_speed(processed.saturating_sub(cached), prefill_ms)
                )),
                (None, Some(prompt)) => Some(format!("⏫ {dur} (↑~{})", format_tokens(prompt))),
                (None, None) => None,
            };
            if let Some(text) = text {
                lines.push(transcript::right_meta(width, vec![Span::styled(text, dim)]));
            }
        }
        // While tokens stream (an answer or reasoning), a generation line in the
        // same shape as a finished block's `✍️` meta, with the live estimate.
        // Only while tokens stream: once a tool runs the message's first token
        // is still known (the cost needs it), but nothing is being generated.
        // An external agent sends its text in bursts and reports no tokens, so
        // there is no generation to time: none is shown for it.
        if let (Phase::Generating | Phase::Compact, Some(first_token), false) =
            (activity.phase, activity.first_token, self.external)
        {
            let gen_ms = first_token.elapsed().as_millis() as u32;
            let tokens = activity.est_tokens();
            lines.push(transcript::right_meta(
                width,
                vec![Span::styled(
                    format!(
                        "✍\u{fe0f} {} (↓{}, {})",
                        transcript::fmt_dur(gen_ms),
                        format_tokens(tokens),
                        transcript::fmt_speed(tokens, gen_ms)
                    ),
                    dim,
                )],
            ));
        }
        // The run clock: an animated glyph and the time since the request.
        // Its own glyph keeps it apart from a block's `🕒`, and when the run
        // ends it freezes on the answer (or on the run's closing line).
        let elapsed = self
            .run_start
            .map_or_else(|| activity.msg_start.elapsed(), |start| start.elapsed());
        let frames = transcript::RUN_FRAMES;
        let frame = (elapsed.as_millis() / 120) as usize % frames.len();
        lines.push(transcript::right_meta(
            width,
            vec![
                Span::styled(
                    format!("{} ", frames[frame]),
                    Style::default()
                        .fg(self.colors.info)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(transcript::fmt_dur(elapsed.as_millis() as u32), dim),
            ],
        ));
        lines
    }

    /// The session's token totals: `↑` the prompt tokens billed in full
    /// (uncached input and cache writes), `↻` those the cache served (when
    /// any), `↓` the output.
    pub(crate) fn token_totals(&self) -> String {
        crate::token_label(self.session_input, self.session_cached, self.session_output)
    }

    /// The model as the banner and the chip show it: a spinner while an
    /// external agent is still starting, `auto` while the model is left to
    /// the provider and not known yet, else the name an external agent gives
    /// it (its id when it gives none).
    pub(crate) fn model_display(&self) -> String {
        if self.external && self.runtime.is_starting() {
            termide_config::constants::spinner_frame().to_string()
        } else if self.model.id.is_empty() {
            "auto".to_string()
        } else {
            self.model_name(&self.model.id)
        }
    }

    /// The human name of model `id`: the one an external agent advertised
    /// for it, else the id itself.
    pub(crate) fn model_name(&self, id: &str) -> String {
        if !self.external {
            return id.to_string();
        }
        self.runtime
            .available_models()
            .into_iter()
            .find(|model| model.id == id)
            .map_or_else(|| id.to_string(), |model| model.name)
    }

    /// The connection as the status-bar chip shows it: its name alone, the
    /// protocol only when it has no name.
    pub(crate) fn connection_chip(&self) -> String {
        if self.connection.is_empty() {
            provider_label(&self.provider_kind).to_string()
        } else {
            self.connection.clone()
        }
    }

    /// The connection as the banner shows it: its name beside the protocol.
    pub(crate) fn connection_display(&self) -> String {
        let kind = provider_label(&self.provider_kind);
        if self.connection.is_empty() {
            kind.to_string()
        } else {
            format!("{} · {kind}", self.connection)
        }
    }

    pub(crate) fn input_rows(&self, available: u16, width: u16) -> u16 {
        // Size by wrapped (visual) rows so a long prompt grows the box instead
        // of being clipped; the `› ` prompt takes two columns. It grows up to
        // half the panel, leaving the other half to the conversation, and
        // scrolls beyond that.
        let text_width = width.saturating_sub(2).max(1) as usize;
        let rows = termide_ui::input_bar::wrapped_row_count(&self.input_area().text(), text_width)
            .max(1) as u16;
        rows.min((available / 2).max(1))
            .min(available.saturating_sub(2).max(1))
    }

    /// The current agent's catalog entry, looked up again only when the
    /// agent changed; an agent the catalog does not list has an empty one.
    pub(crate) fn agent_entry(&mut self) -> &AgentEntry {
        if self
            .agent_entry
            .as_ref()
            .is_none_or(|entry| entry.name != self.agent)
        {
            let entry = self
                .catalog
                .list()
                .into_iter()
                .find(|entry| entry.name == self.agent)
                .unwrap_or_else(|| AgentEntry {
                    name: self.agent.clone(),
                    description: String::new(),
                    icon: None,
                });
            self.agent_entry = Some(entry);
        }
        self.agent_entry.as_ref().expect("filled above")
    }

    /// The agent's description from its definition, empty when it has
    /// none; the banner shows it under the agent's name.
    pub(crate) fn agent_description(&mut self) -> String {
        self.agent_entry().description.clone()
    }

    /// Rows the banner's fields take, before its list of sessions: the
    /// agent's name as the title, its description (when it has one), a
    /// blank, cwd, connection, model and tools (when they are termide's).
    pub(crate) fn banner_field_rows(&mut self) -> usize {
        5 + usize::from(!self.agent_description().is_empty()) + usize::from(self.has_toolset())
    }

    /// The welcome banner shown while the session is empty, in place of the
    /// transcript: a logo on the left and what the agent is set up with
    /// (agent, directory, connection, model) on the right, at the top, then
    /// what the panel reported before the first request (`notices`, past a
    /// dashed rule), then the recent sessions. The three scroll as one
    /// document, so on a short panel the fields make way for the list. On a
    /// narrow panel the logo is dropped and only the details show. The
    /// list's keyboard cursor is drawn only while `panel_focused`, so a
    /// background panel does not draw the eye. Returns the document's rows,
    /// for the scrollbar.
    pub(crate) fn render_welcome(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        colors: &ThemeColors,
        panel_focused: bool,
        notices: &[Line<'static>],
    ) -> usize {
        const LOGO: [&str; WELCOME_LOGO_ROWS] = [
            "╭───────╮",
            "│ ▀█ █▀ │",
            "│ ▄▀▀▀▄ │",
            "│  ▀▀▀  │",
            "╰TERMIDE╯",
        ];
        self.banner_hits.clear();
        if area.width < 14 || area.height == 0 {
            return 0;
        }
        let logo_w = LOGO
            .iter()
            .map(|l| termide_ui::str_display_width(l))
            .max()
            .unwrap_or(0) as u16;
        let gap = 3u16;
        let show_logo = area.width >= logo_w + gap + 22;
        let info_x = area.x + 2 + if show_logo { logo_w + gap } else { 0 };
        let info_w = (area.x + area.width).saturating_sub(info_x + 1);
        // The list of sessions starts below the logo, so it takes the logo's
        // column and the width that comes with it.
        let list_x = area.x + 2;
        let list_w = (area.x + area.width).saturating_sub(list_x + 1);
        // The notices keep the transcript's full width.
        let text_w = area.width.saturating_sub(1).max(1);

        let accent = Style::default()
            .fg(colors.info)
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(colors.disabled);
        let fg = Style::default().fg(colors.fg);
        // A re-pickable value (cwd, connection, model, tools) is drawn bold in
        // the accent colour, so it reads as clickable; a fixed one is plain.
        // The click itself is wired through `banner_hits` below.
        let link = Style::default()
            .fg(colors.info)
            .add_modifier(Modifier::BOLD);
        let field = |name: &str, value: String, clickable: bool| -> Line<'static> {
            // Padded to the column the values share, in cells rather than by
            // `{:width$}` so a label of another script and length is measured
            // honestly; at least one space always divides it from its value.
            let pad = LABEL_COL
                .saturating_sub(termide_ui::str_display_width(name))
                .max(1);
            Line::from(vec![
                Span::styled(format!("{name}{}", " ".repeat(pad)), dim),
                Span::styled(value, if clickable { link } else { fg }),
            ])
        };
        let cwd = shorten_path(&self.cwd, (info_w as usize).saturating_sub(LABEL_COL));
        // Each entry is a line and, when clicking it does something (re-pick a
        // choice, open a session), what that click does.
        let t = termide_i18n::t();
        // The title is the agent's name, re-pickable with a click, and its
        // description, when it has one, goes under it.
        let mut fields: Vec<(Line<'static>, Option<BannerHit>)> = vec![(
            Line::styled(self.agent.clone(), accent),
            Some(BannerHit::Action(AGENT_ACTION)),
        )];
        let description = self.agent_description();
        if !description.is_empty() {
            fields.push((Line::styled(description, fg), None));
        }
        fields.extend([
            (Line::from(""), None),
            // The directory moves only while the panel is idle.
            (
                field(t.agent_banner_cwd(), cwd, !self.is_busy()),
                (!self.is_busy()).then_some(BannerHit::Action(CWD_ACTION)),
            ),
            (
                field(
                    t.agent_banner_connection(),
                    self.connection_display(),
                    self.connections.is_some(),
                ),
                self.connections
                    .is_some()
                    .then_some(BannerHit::Action(CONNECTION_ACTION)),
            ),
            (
                field(t.agent_banner_model(), self.model_display(), true),
                Some(BannerHit::Action(MODEL_ACTION)),
            ),
        ]);
        // What the session may use, re-pickable before the first request,
        // when switching it off keeps it out of the context altogether.
        if self.has_toolset() {
            let (on, all) = self.toolset_counts();
            fields.push((
                field(t.agent_banner_tools(), format!("{on}/{all}"), true),
                Some(BannerHit::Action(TOOLSET_ACTION)),
            ));
        }
        debug_assert_eq!(fields.len(), self.banner_field_rows());

        // The document, row by row: what each row shows, at which column and
        // width, and what a click on it does.
        let mut doc: Vec<(Line<'static>, u16, u16, Option<BannerHit>)> = Vec::new();
        for (line, hit) in fields {
            doc.push((line, info_x, info_w, hit));
        }
        if !notices.is_empty() {
            doc.push((
                transcript::separator(text_w + 1, colors),
                area.x,
                text_w,
                None,
            ));
            for line in notices {
                doc.push((line.clone(), area.x, text_w, None));
            }
        }
        // This directory's other sessions, newest first, one click (or
        // Tab, the arrows and Enter) away. The sessions go under the heading
        // rather than beside it, so their titles get the full width.
        if !self.recent_sessions.is_empty() {
            doc.push((Line::from(""), list_x, list_w, None));
            doc.push((
                Line::styled(t.agent_banner_sessions().to_string(), dim),
                list_x,
                list_w,
                None,
            ));
        }
        let list_start = doc.len();
        for (index, summary) in self.recent_sessions.iter().enumerate() {
            // Only the title is in the accent colour; the date before it is
            // dim.
            let line = Line::from(vec![
                Span::styled(format!("{}  ", local_minute(summary.modified)), dim),
                Span::styled(truncate_title(&summary.display_label()), link),
            ]);
            doc.push((line, list_x, list_w, Some(BannerHit::Session(index))));
        }
        // A blank row above the banner only when the whole document fits
        // anyway, so a short panel spends none of its rows on it.
        let height = area.height as usize;
        let margin = usize::from(height > doc.len());
        let total = doc.len() + margin;
        let list_start = list_start + margin;
        // The logo starts on the title's row.
        let logo_row = margin;

        // A notice arriving above the list while it is scrolled moves the
        // view along with it, so the session under the pointer stays put.
        if self.banner_top > 0 && list_start != self.banner_list_start {
            self.banner_top = (self.banner_top + list_start).saturating_sub(self.banner_list_start);
        }
        self.banner_list_start = list_start;
        self.banner_rows = total;
        self.banner_top = self.banner_top.min(total.saturating_sub(height));

        let cursor_shown = self.chat_focus && panel_focused;
        let top = self.banner_top;
        for screen_row in 0..height.min(total - top) {
            let row = top + screen_row;
            let y = area.y + screen_row as u16;
            if show_logo && (logo_row..logo_row + WELCOME_LOGO_ROWS).contains(&row) {
                buf.set_stringn(
                    area.x + 2,
                    y,
                    LOGO[row - logo_row],
                    logo_w as usize,
                    Style::default().fg(colors.info),
                );
            }
            let Some((line, x, w, hit)) = row.checked_sub(margin).and_then(|i| doc.get(i)) else {
                continue;
            };
            buf.set_line(*x, y, line, *w);
            // The whole field row is the click target, so the label is as good
            // as the value; an external agent still routes the click, and its
            // action answers with the "unsupported" notice. A row scrolled out
            // of view takes no clicks.
            if let Some(hit) = hit {
                self.banner_hits.push((
                    Rect {
                        x: *x,
                        y,
                        width: *w,
                        height: 1,
                    },
                    *hit,
                ));
            }
            // The session under the keyboard cursor is shown inverted, like
            // a selected chat block.
            if cursor_shown && *hit == Some(BannerHit::Session(self.recent_selected)) {
                for x in *x..*x + *w {
                    buf[(x, y)].set_style(Style::default().fg(colors.bg).bg(colors.fg));
                }
            }
        }
        total
    }

    /// The run controls the current state offers: pause and stop while the
    /// agent works, continue in place of pause once a pause is asked for or
    /// has taken effect, stop alone while a stop is under way, none while
    /// idle.
    pub(crate) fn run_buttons(&self) -> Vec<RunButton> {
        let paused = (self.paused || self.retry_wait.is_some()) && !self.is_busy();
        if self.is_busy() && self.stop_requested {
            vec![RunButton::Stop]
        } else if paused || (self.is_busy() && self.pause_requested) {
            vec![RunButton::Continue, RunButton::Stop]
        } else if self.is_busy() && self.runtime.can_pause() {
            vec![RunButton::Pause, RunButton::Stop]
        } else if self.is_busy() {
            // An external agent runs its own loop: it can be stopped, not paused.
            vec![RunButton::Stop]
        } else {
            Vec::new()
        }
    }

    /// A run control's color: the panel border's accent at rest, so a
    /// control reads as part of the frame rather than engaged just because
    /// a run is on; continue is green and stop red only while the pause or
    /// stop they stand for is under way. A panel in the background dims
    /// them all to the inactive border color, as it does its frame.
    pub(crate) fn run_button_color(&self, button: RunButton, focused: bool) -> Color {
        if !focused {
            return self.colors.border;
        }
        match button {
            RunButton::Pause => self.colors.border_focused,
            RunButton::Continue => self.colors.success,
            RunButton::Stop if self.stop_requested => self.colors.error,
            RunButton::Stop => self.colors.border_focused,
        }
    }

    /// `panel_focused` colors the run controls; `focused` (the input's own
    /// focus) brightens the bar's border.
    pub(crate) fn render_input(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        focused: bool,
        panel_focused: bool,
    ) {
        let colors = self.colors;
        // The run controls sit at the right end of the top border, always in
        // view whatever the transcript's scroll.
        self.run_buttons = self.run_buttons();
        let buttons = self
            .run_buttons
            .iter()
            .map(|&button| {
                let label = match button {
                    RunButton::Pause => "[‖]",
                    RunButton::Continue => "[▶]",
                    RunButton::Stop => "[■]",
                };
                (
                    label.to_string(),
                    Style::default().fg(self.run_button_color(button, panel_focused)),
                )
            })
            .collect();
        self.input.set_border_buttons(buttons);
        // Shell mode shows in the prompt marker and the placeholder, so what
        // Enter will do is visible before anything is typed.
        let t = termide_i18n::t();
        let (label, prompt_style, placeholder) = if self.shell_mode {
            (
                "$ ",
                Some(
                    Style::default()
                        .fg(colors.info)
                        .add_modifier(Modifier::BOLD),
                ),
                t.agent_input_placeholder_shell(),
            )
        } else {
            ("", None, t.agent_input_placeholder())
        };
        self.input.set_label(0, label);
        self.input.set_prompt_style(prompt_style);
        self.input.set_placeholder(Some(placeholder.to_string()));
        // The bar's top border is the divider from the content above and
        // brightens while the input is focused; the agent's name lives in the
        // panel title, not here.
        self.input.render(area, buf, &colors, focused);
    }

    /// The body of [`Panel::render`].
    pub(crate) fn render_panel(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        buf.set_style(area, Style::default().fg(self.colors.fg).bg(self.colors.bg));
        // Shown focused, whatever waited is now in front of the user.
        if ctx.is_focused {
            self.attention = false;
            // Seen only if the window is in front too; until then the bell
            // already rung stands.
            if self.host_focused {
                self.rung = false;
            }
        }

        let input_rows = self.input_rows(area.height, area.width);
        // The input bar carries its own titled top border, which divides it
        // from the content above, so the box is one row taller than its text.
        let bar_rows = input_rows + 1;
        // The agent's question sits above the input; when a card is present a
        // plain separator divides it from the transcript (the bar's own border
        // divides the card from the input). When the panel is too short for the
        // card the keys still answer.
        let form_rows = self
            .pending
            .as_ref()
            .map_or(0, |pending| pending.form().height(area.width))
            .min(area.height.saturating_sub(bar_rows + 1));
        let has_separator = form_rows > 0 && area.height > bar_rows + form_rows;
        // The state strip (a pending pause, queued messages) sits between the
        // transcript and the card, leaving the transcript at least one row.
        let text_width = area.width.saturating_sub(1).max(1);
        let mut state = self.state_lines(text_width);
        // An active goal or loop stays in sight while it goes on, between
        // its runs as well as during them.
        let autorun = autorun_strip(&self.autorun_rows(), text_width, &self.colors);
        if !autorun.is_empty() {
            if state.is_empty() {
                state.push(transcript::separator(text_width, &self.colors));
            }
            state.extend(autorun);
        }
        // The subagents still running, closest to the input, so a block
        // scrolled out of view does not hide that one works.
        let task_items = self.running_task_rows();
        let task_lines = task_strip(
            &task_items
                .iter()
                .map(|(_, row)| row.clone())
                .collect::<Vec<_>>(),
            text_width,
            &self.colors,
        );
        let tasks_from = if task_lines.is_empty() {
            None
        } else {
            if state.is_empty() {
                state.push(transcript::separator(text_width, &self.colors));
            }
            let from = state.len();
            state.extend(task_lines);
            Some(from)
        };
        let room = area
            .height
            .saturating_sub(bar_rows + form_rows + u16::from(has_separator) + 1);
        state.truncate(room as usize);
        let state_rows = state.len() as u16;
        let transcript_height = area
            .height
            .saturating_sub(bar_rows + form_rows + state_rows)
            .saturating_sub(u16::from(has_separator));
        self.transcript_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: transcript_height,
        };
        self.input_area = Rect {
            x: area.x,
            y: area.y + area.height - bar_rows,
            width: area.width,
            height: bar_rows,
        };
        let form_area = Rect {
            x: area.x,
            y: self.input_area.y - form_rows,
            width: area.width,
            height: form_rows,
        };

        // The rightmost column is the scrollbar gutter (`text_width` above), so
        // wrapped text never sits under the bar.
        let colors = self.colors;
        let is_light = self.is_light;
        // The streaming block's live meta (ticking time + spinner) sits after
        // the last block while the agent works, animating on the ~10 fps redraw.
        let footer = self.live_footer_lines(text_width);
        self.transcript.set_live_footer(footer);
        let total = self.transcript.lines(text_width, &colors, is_light).len();
        let max_top = total.saturating_sub(transcript_height as usize);
        if self.follow {
            self.top = max_top;
        } else {
            self.top = self.top.min(max_top);
        }
        // Keep the chat selection valid, on screen, and note the flat-line
        // range to tint — computed now, before `lines` borrows the transcript.
        let item_count = self.transcript.items().len();
        let banner = self.banner_shown();
        let mut selected_range: Option<(usize, usize)> = None;
        // Under the banner the keyboard is in its list of sessions, not on
        // the notices.
        if self.chat_focus && item_count > 0 && !banner {
            self.selected = self
                .transcript
                .selectable_near(self.selected)
                .unwrap_or(item_count - 1);
            // Scrolling stays free while a block is selected: the view is
            // brought to a block only when the selection moves (see
            // `scroll_selected_into_view`), not on every frame. Here we only
            // note the block's flat-line range to tint.
            // The rule or gap around a block stays out of the highlight, and
            // a background panel shows none, so it does not draw the eye.
            if ctx.is_focused {
                selected_range = self.transcript.content_lines_of(self.selected);
            }
        }
        let lines = self.transcript.lines(text_width, &colors, is_light);
        // The block under the chat cursor is shown inverted (text and
        // background swapped), so the selection reads as one solid block.
        let selected_style = Style::default().fg(colors.bg).bg(colors.fg);
        // Rows and first row on screen of what the scrollbar tracks: the
        // banner's document or the transcript.
        let mut scroll = (self.top, total);
        if banner {
            // A fresh session shows a welcome banner in place of the
            // transcript, with what the panel reports before the first
            // request inside it.
            let notices: Vec<Line<'static>> = lines.to_vec();
            let welcome = Rect {
                height: transcript_height,
                ..area
            };
            let rows = self.render_welcome(welcome, buf, &colors, ctx.is_focused, &notices);
            scroll = (self.banner_top, rows);
        } else {
            // No banner while the session has content, so its click targets go.
            self.banner_hits.clear();
            for row in 0..transcript_height as usize {
                let Some(line) = lines.get(self.top + row) else {
                    break;
                };
                buf.set_line(area.x, area.y + row as u16, line, text_width);
                if selected_range.is_some_and(|(f, l)| self.top + row >= f && self.top + row <= l) {
                    for dx in 0..text_width {
                        let cell = &mut buf[(area.x + dx, area.y + row as u16)];
                        // Success and error keep their hue under the
                        // selection, inverted like the rest: an edit's diff,
                        // a status glyph or a failure still reads as one.
                        let style = if cell.fg == colors.success || cell.fg == colors.error {
                            Style::default().fg(colors.bg).bg(cell.fg)
                        } else {
                            selected_style
                        };
                        cell.set_style(style);
                    }
                }
                // The link under the pointer while Ctrl is held, lit whole.
                for &(line, start, end) in self.hovered_link.iter().flatten() {
                    if line != self.top + row {
                        continue;
                    }
                    for dx in start.min(text_width as usize)..end.min(text_width as usize) {
                        buf[(area.x + dx as u16, area.y + row as u16)].set_style(
                            Style::default()
                                .fg(colors.info)
                                .add_modifier(Modifier::UNDERLINED),
                        );
                    }
                }
                // A mouse selection over the text, as a terminal shows one.
                if let Some((start, end)) = self
                    .text_selection
                    .and_then(|sel| sel.columns_on(self.top + row, text_width as usize))
                {
                    for dx in start..end {
                        buf[(area.x + dx as u16, area.y + row as u16)].set_style(
                            Style::default()
                                .fg(colors.selection_fg)
                                .bg(colors.selection_bg),
                        );
                    }
                }
            }
        }
        self.scrollbars.vertical = ScrollBar::render_tracked(
            buf,
            ctx.border_right_x.unwrap_or(area.x + area.width - 1),
            area.y,
            transcript_height,
            scroll.0,
            transcript_height as usize,
            scroll.1,
            &self.colors,
            ctx.is_focused,
        );

        let state_y = area.y + transcript_height;
        for (row, line) in state.iter().enumerate() {
            buf.set_line(area.x, state_y + row as u16, line, text_width);
        }
        // Each subagent row brings its block into view on a click.
        self.task_rows = tasks_from
            .map(|from| {
                task_items
                    .iter()
                    .take(STATE_QUEUED_ROWS)
                    .enumerate()
                    .filter(|(i, _)| from + i < state.len())
                    .map(|(i, (index, _))| (state_y + (from + i) as u16, *index))
                    .collect()
            })
            .unwrap_or_default();
        // The strip's pending-pause line (after its rule) withdraws the pause
        // on a click.
        self.pause_row = (self.pause_requested && self.retry_wait.is_none() && state.len() > 1)
            .then_some(state_y + 1);
        if has_separator {
            let y = form_area.y - 1;
            let style = Style::default().fg(if ctx.is_focused {
                self.colors.border_focused
            } else {
                self.colors.disabled
            });
            for dx in 0..area.width {
                buf[(area.x + dx, y)].set_symbol("─").set_style(style);
            }
        }
        let input_area = self.input_area;
        self.render_input(
            input_area,
            buf,
            ctx.is_focused && !self.chat_focus,
            ctx.is_focused,
        );
        if form_rows >= 3 {
            if let Some(pending) = &mut self.pending {
                pending
                    .form_mut()
                    .render(form_area, buf, &colors, ctx.is_focused);
            }
        }
        if ctx.is_focused && transcript_height > 0 {
            // The completion list overlays the bottom of the transcript,
            // right above the input bar.
            let above = Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: transcript_height,
            };
            if let Some(list) = &mut self.completion {
                list.render(above, buf, &colors);
            }
            if let Some(picker) = &mut self.rewind_picker {
                picker.list.render(above, buf, &colors);
            }
        }
    }

    /// The body of [`Panel::status_segments`].
    pub(crate) fn segments(&self) -> Vec<StatusSegment> {
        // Separators are the panel's job: the status bar concatenates the
        // segments as given. The knobs sit on the left, the figures flush
        // right; a narrow bar cuts the knobs, never the figures. The live
        // phase is not repeated here: each chat block carries its own byline.
        let t = termide_i18n::t();
        let sep = || StatusSegment::new(" │ ", SegmentKind::Label);
        let mut segments = vec![
            StatusSegment::new(" ", SegmentKind::Label),
            StatusSegment::clickable(t.agent_chip_agent(), SegmentKind::Label, AGENT_ACTION),
            StatusSegment::clickable(self.agent.clone(), SegmentKind::Active, AGENT_ACTION),
        ];
        if self.external {
            // An external agent has its own model and permission model, unless
            // termide judges its calls or maps its modes.
            segments.push(StatusSegment::new(" (acp)", SegmentKind::Label));
            if self.runtime.follows_mode() {
                segments.extend([
                    sep(),
                    StatusSegment::clickable(t.agent_chip_mode(), SegmentKind::Label, MODE_ACTION),
                    StatusSegment::clickable(
                        self.mode.get().label(),
                        SegmentKind::Active,
                        MODE_ACTION,
                    ),
                ]);
            }
            // The agent's own reasoning setting and the rest of its settings,
            // when it offers them.
            if let Some(option) = self.acp_thought_option() {
                segments.extend([
                    sep(),
                    StatusSegment::clickable(
                        t.agent_chip_reasoning(),
                        SegmentKind::Label,
                        REASONING_ACTION,
                    ),
                    StatusSegment::clickable(
                        option.current_name().to_string(),
                        SegmentKind::Active,
                        REASONING_ACTION,
                    ),
                ]);
            }
            let extra = self.acp_extra_options();
            if !extra.is_empty() {
                let values: Vec<&str> = extra.iter().map(|option| option.current_name()).collect();
                segments.extend([
                    sep(),
                    StatusSegment::clickable(
                        t.agent_chip_options(),
                        SegmentKind::Label,
                        OPTIONS_ACTION,
                    ),
                    StatusSegment::clickable(
                        values.join(" · "),
                        SegmentKind::Active,
                        OPTIONS_ACTION,
                    ),
                ]);
            }
            if self.has_toolset() {
                let (on, all) = self.toolset_counts();
                segments.extend([
                    sep(),
                    StatusSegment::clickable(
                        t.agent_chip_tools(),
                        SegmentKind::Label,
                        TOOLSET_ACTION,
                    ),
                    StatusSegment::clickable(
                        format!("{on}/{all}"),
                        SegmentKind::Active,
                        TOOLSET_ACTION,
                    ),
                ]);
            }
            // A CLI agent is a connection too: the way back is here.
            if self.connections.is_some() {
                segments.extend([
                    sep(),
                    StatusSegment::clickable(
                        t.agent_chip_connection(),
                        SegmentKind::Label,
                        CONNECTION_ACTION,
                    ),
                    StatusSegment::clickable(
                        self.connection_chip(),
                        SegmentKind::Active,
                        CONNECTION_ACTION,
                    ),
                ]);
            }
        } else {
            segments.extend([
                sep(),
                StatusSegment::clickable(t.agent_chip_mode(), SegmentKind::Label, MODE_ACTION),
                StatusSegment::clickable(self.mode.get().label(), SegmentKind::Active, MODE_ACTION),
            ]);
            // A model that cannot be asked to reason has no level to show.
            let levels = self.thinking_levels();
            if !levels.is_empty() {
                let level = self.effective_thinking(&levels);
                segments.extend([
                    sep(),
                    StatusSegment::clickable(
                        t.agent_chip_reasoning(),
                        SegmentKind::Label,
                        REASONING_ACTION,
                    ),
                    StatusSegment::clickable(
                        Self::thinking_label(level, &levels),
                        SegmentKind::Active,
                        REASONING_ACTION,
                    ),
                ]);
            }
            segments.extend([
                sep(),
                StatusSegment::clickable(t.agent_chip_tools(), SegmentKind::Label, TOOLSET_ACTION),
                StatusSegment::clickable(
                    {
                        let (on, all) = self.toolset_counts();
                        format!("{on}/{all}")
                    },
                    SegmentKind::Active,
                    TOOLSET_ACTION,
                ),
                sep(),
                StatusSegment::clickable(
                    t.agent_chip_connection(),
                    SegmentKind::Label,
                    CONNECTION_ACTION,
                ),
                StatusSegment::clickable(
                    self.connection_chip(),
                    SegmentKind::Active,
                    CONNECTION_ACTION,
                ),
            ]);
        }
        // The agent's model over ACP, when it advertised any: clickable to
        // switch, like the built-in loop's Model chip. While the agent starts
        // the chip holds a spinner, its models not known yet.
        if !self.external || self.acp_has_models || self.runtime.is_starting() {
            segments.extend([
                sep(),
                StatusSegment::clickable(t.agent_chip_model(), SegmentKind::Label, MODEL_ACTION),
                StatusSegment::clickable(self.model_display(), SegmentKind::Active, MODEL_ACTION),
            ]);
        }
        segments.push(StatusSegment::spacer());
        let queued = self.queued.0 + self.queued.1;
        if queued > 0 {
            segments.push(StatusSegment::new(
                format!("{} ", t.agent_queued_fmt(queued)),
                SegmentKind::Inactive,
            ));
        }
        // Session token totals, for an external agent too when it reports
        // them.
        if self.session_input > 0 || self.session_cached > 0 || self.session_output > 0 {
            segments.push(StatusSegment::new(
                format!("{} ", self.token_totals()),
                SegmentKind::Value,
            ));
        }
        // An external agent's window is known once it reports it; until then
        // the configured fallback would mislead.
        let window_known = !self.external || self.runtime.context_usage().is_some();
        if window_known && self.model.context_window > 0 {
            let percent = ((self.context_tokens * 100) / self.model.context_window).min(100);
            let kind = if percent >= 80 {
                SegmentKind::Warn
            } else {
                SegmentKind::Value
            };
            segments.push(StatusSegment::new(
                format!(
                    "{}/{} {} ",
                    format_tokens(self.context_tokens),
                    format_tokens(self.model.context_window),
                    context_bar(percent)
                ),
                kind,
            ));
        }
        segments
    }
}
