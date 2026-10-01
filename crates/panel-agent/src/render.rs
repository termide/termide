//! Rendering: the transcript and the prompt box, the welcome banner, the
//! run controls and state strip, and the status-bar segments.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use termide_agent_core::civil_date;
use termide_core::{RenderContext, SegmentKind, StatusSegment, ThemeColors};
use termide_ui::ScrollBar;

use crate::toolset::TOOLSET_ACTION;
use crate::{
    format_tokens, provider_label, shorten_path, transcript, truncate_title, AgentPanel, BannerHit,
    Phase, RunButton, AGENT_ACTION, CONNECTION_ACTION, MODEL_ACTION, MODE_ACTION, REASONING_ACTION,
};

/// Rows of the welcome banner's logo.
const WELCOME_LOGO_ROWS: usize = 5;

/// The column the banner's values begin at, counted from where its labels
/// begin. Labels are padded to it, never below one space, so a longer label —
/// `Herramientas`, `エージェント` — pushes only its own value right.
const LABEL_COL: usize = 12;

/// Queued messages the state strip shows before folding the rest into a count.
pub(crate) const STATE_QUEUED_ROWS: usize = 3;

/// The state strip's lines: a dim dashed rule, then a pause row (`pause`,
/// when one is pending or active) and a row per queued message (its first
/// line, cut to the width), at most [`STATE_QUEUED_ROWS`] of them before a
/// "… N more" row. Empty when there is nothing to show.
pub(crate) fn state_strip<'a>(
    queued: impl ExactSizeIterator<Item = &'a str>,
    pause: Option<&str>,
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
    if let Some(pause) = pause {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", transcript::PAUSED_GLYPH),
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

/// An eight-cell fill bar for a 0–100 percentage, e.g. `▰▰▱▱▱▱▱▱` at 20%.
pub(crate) fn context_bar(percent: u64) -> String {
    const CELLS: u64 = 8;
    let filled = (percent * CELLS).div_ceil(100).min(CELLS);
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
        let pause = self
            .pause_requested
            .then(|| termide_i18n::t().agent_notice_will_pause());
        state_strip(
            self.queued_texts.iter().map(String::as_str),
            pause,
            width,
            &self.colors,
        )
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
        if activity.phase == Phase::Prefill && !self.external {
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
        if let (Phase::Generating, Some(first_token), false) =
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
        let cached = if self.session_cached > 0 {
            format!(" ↻{}", format_tokens(self.session_cached))
        } else {
            String::new()
        };
        format!(
            "↑{}{cached} ↓{}",
            format_tokens(self.session_input),
            format_tokens(self.session_output)
        )
    }

    /// The model as the banner and the chip show it: `auto` while it is left
    /// to the provider and not known yet.
    pub(crate) fn model_display(&self) -> String {
        if self.model.id.is_empty() {
            "auto".to_string()
        } else {
            self.model.id.clone()
        }
    }

    /// The connection as the banner and the chip show it: its name beside
    /// the protocol.
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
        let rows =
            termide_ui::input_bar::wrapped_row_count(&self.input_text(), text_width).max(1) as u16;
        rows.min((available / 2).max(1))
            .min(available.saturating_sub(2).max(1))
    }

    /// Rows the banner's fields take, before its list of sessions: the name,
    /// the subtitle, a blank, connection, model, agent, tools (for our own
    /// agent) and cwd.
    pub(crate) fn banner_field_rows(&self) -> usize {
        7 + usize::from(!self.external)
    }

    /// The welcome banner shown while the session is empty: a logo on the left
    /// and what the agent is set up with (provider, model, agent, directory) on
    /// the right, at the top of the transcript area, with the recent sessions
    /// filling the rows below. On a narrow panel the logo is dropped and only
    /// the details show.
    pub(crate) fn render_welcome(&mut self, area: Rect, buf: &mut Buffer, colors: &ThemeColors) {
        const LOGO: [&str; WELCOME_LOGO_ROWS] = [
            "╭───────╮",
            "│       │",
            "│  ›_   │",
            "│       │",
            "╰───────╯",
        ];
        self.banner_hits.clear();
        if area.width < 14 || area.height == 0 {
            return;
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

        let accent = Style::default()
            .fg(colors.info)
            .add_modifier(Modifier::BOLD);
        let dim = Style::default().fg(colors.disabled);
        let fg = Style::default().fg(colors.fg);
        // A re-pickable value (model, agent, tools) is drawn bold in the
        // accent colour, so it reads as clickable; a fixed one (cwd)
        // is plain. The click itself is wired through `banner_hits` below.
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
        let info: Vec<(Line<'static>, Option<BannerHit>)> = vec![
            (Line::styled("termide", accent), None),
            (Line::styled(t.agent_banner_subtitle(), dim), None),
            (Line::from(""), None),
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
            (
                field(t.agent_banner_agent(), self.agent.clone(), true),
                Some(BannerHit::Action(AGENT_ACTION)),
            ),
        ];
        // What the session may use, re-pickable before the first request,
        // when switching it off keeps it out of the context altogether.
        let mut info = info;
        if !self.external {
            let (on, all) = self.toolset_counts();
            info.push((
                field(t.agent_banner_tools(), format!("{on}/{all}"), true),
                Some(BannerHit::Action(TOOLSET_ACTION)),
            ));
        }
        info.push((field(t.agent_banner_cwd(), cwd, false), None));
        debug_assert_eq!(info.len(), self.banner_field_rows());
        // This directory's other sessions, newest first, one click (or
        // Tab, the arrows and Enter) away: as many rows as the panel's height
        // leaves, scrolling through the rest.
        // The banner sits at the top, so the list gets every row under the
        // fields; a blank row above it only when the whole list fits anyway.
        let header_len = info.len();
        let total = self.recent_sessions.len();
        let list_need = if total > 0 { total + 1 } else { 0 };
        let margin = usize::from(area.height as usize > header_len + list_need);
        let rows = total.min((area.height as usize).saturating_sub(margin + header_len + 1));
        self.recent_rows = rows;
        let list_start = header_len + 1;
        if rows > 0 {
            self.recent_top = self.recent_top.min(total - rows);
            if self.chat_focus {
                self.scroll_recent_selection_into_view();
            }
            info.push((Line::from(""), None));
            let first = self.recent_top;
            for (index, summary) in self.recent_sessions[first..first + rows]
                .iter()
                .enumerate()
                .map(|(row, summary)| (first + row, summary))
            {
                let label = if index == first {
                    t.agent_banner_sessions()
                } else {
                    ""
                };
                let value = format!(
                    "{} · {}",
                    civil_date(summary.modified),
                    truncate_title(&summary.label())
                );
                info.push((field(label, value, true), Some(BannerHit::Session(index))));
            }
        }

        // The logo is centred against the fields, not the list below them.
        let header_h = header_len.max(LOGO.len()) as u16;
        let bottom = area.y + area.height;
        let top = area.y + margin as u16;
        if show_logo {
            let logo_top = top + (header_h - LOGO.len() as u16) / 2;
            for (i, line) in LOGO.iter().enumerate() {
                let y = logo_top + i as u16;
                if y >= bottom {
                    break;
                }
                buf.set_stringn(
                    area.x + 2,
                    y,
                    line,
                    logo_w as usize,
                    Style::default().fg(colors.info),
                );
            }
        }
        let info_top = top + (header_h - header_len as u16) / 2;
        for (i, (line, hit)) in info.iter().enumerate() {
            let y = info_top + i as u16;
            if y >= bottom {
                break;
            }
            buf.set_line(info_x, y, line, info_w);
            // The whole field row is the click target, so the label is as good
            // as the value; an external agent still routes the click, and its
            // action answers with the "unsupported" notice.
            if let Some(hit) = hit {
                self.banner_hits.push((
                    Rect {
                        x: info_x,
                        y,
                        width: info_w,
                        height: 1,
                    },
                    *hit,
                ));
            }
            // The session under the keyboard cursor is shown inverted, like
            // a selected chat block; its label column stays plain.
            if self.chat_focus && *hit == Some(BannerHit::Session(self.recent_selected)) {
                for x in info_x + (LABEL_COL as u16).min(info_w)..info_x + info_w {
                    buf[(x, y)].set_style(Style::default().fg(colors.bg).bg(colors.fg));
                }
            }
        }
        // A list longer than its rows gets a scrollbar in the gutter beside it.
        if rows > 0 {
            let list_y = info_top + list_start as u16;
            ScrollBar::render(
                buf,
                area.x + area.width - 1,
                list_y,
                (rows as u16).min(bottom.saturating_sub(list_y)),
                self.recent_top,
                rows,
                total,
                colors,
                self.chat_focus,
            );
        }
    }

    /// The run controls the current state offers: pause and stop while the
    /// agent works, continue in place of pause once a pause is asked for or
    /// has taken effect, stop alone while a stop is under way, none while
    /// idle.
    pub(crate) fn run_buttons(&self) -> Vec<RunButton> {
        let paused = self.paused && !self.is_busy();
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
    /// stop they stand for is under way.
    pub(crate) fn run_button_color(&self, button: RunButton) -> Color {
        match button {
            RunButton::Pause => self.colors.border_focused,
            RunButton::Continue => self.colors.success,
            RunButton::Stop if self.stop_requested => self.colors.error,
            RunButton::Stop => self.colors.border_focused,
        }
    }

    pub(crate) fn render_input(&mut self, area: Rect, buf: &mut Buffer, focused: bool) {
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
                    Style::default().fg(self.run_button_color(button)),
                )
            })
            .collect();
        self.input.set_border_buttons(buttons);
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
        // The banner's top margin and its fields, which the notices under it
        // leave room for.
        let needed = 1 + self.banner_field_rows().max(WELCOME_LOGO_ROWS);
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
            // The rule or gap around a block stays out of the highlight.
            selected_range = self.transcript.content_lines_of(self.selected);
        }
        let lines = self.transcript.lines(text_width, &colors, is_light);
        // The block under the chat cursor is shown inverted (text and
        // background swapped), so the selection reads as one solid block.
        let selected_style = Style::default().fg(colors.bg).bg(colors.fg);
        if banner {
            // A fresh session shows a welcome banner in place of the
            // transcript: the logo and what the agent is set up with. What
            // the panel reports before the first request goes under it, past
            // a dashed rule, the latest kept in view; the banner's list of
            // sessions gives up its rows first, its fields never.
            let room = (transcript_height as usize).saturating_sub(needed + 1);
            let shown = lines.len().min(room);
            let notices: Vec<Line<'static>> = lines[lines.len() - shown..].to_vec();
            let rule_rows = u16::from(shown > 0);
            let welcome = Rect {
                height: transcript_height - shown as u16 - rule_rows,
                ..area
            };
            self.render_welcome(welcome, buf, &colors);
            if shown > 0 {
                let rule_y = area.y + welcome.height;
                let rule = transcript::separator(text_width + 1, &colors);
                buf.set_line(area.x, rule_y, &rule, text_width);
                for (row, line) in notices.iter().enumerate() {
                    buf.set_line(area.x, rule_y + 1 + row as u16, line, text_width);
                }
            }
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
        // The banner's list draws its own bar; the notices under it keep to
        // their latest rows and scroll nowhere.
        self.scrollbars.vertical = if banner {
            None
        } else {
            ScrollBar::render_tracked(
                buf,
                ctx.border_right_x.unwrap_or(area.x + area.width - 1),
                area.y,
                transcript_height,
                self.top,
                transcript_height as usize,
                total,
                &self.colors,
                ctx.is_focused,
            )
        };

        let state_y = area.y + transcript_height;
        for (row, line) in state.iter().enumerate() {
            buf.set_line(area.x, state_y + row as u16, line, text_width);
        }
        // The strip's pending-pause line (after its rule) withdraws the pause
        // on a click.
        self.pause_row = (self.pause_requested && state.len() > 1).then_some(state_y + 1);
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
        self.render_input(input_area, buf, ctx.is_focused && !self.chat_focus);
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
                        self.connection_display(),
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
                    self.connection_display(),
                    SegmentKind::Active,
                    CONNECTION_ACTION,
                ),
            ]);
        }
        // The agent's model over ACP, when it advertised any: clickable to
        // switch, like the built-in loop's Model chip.
        if !self.external || self.acp_has_models {
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
