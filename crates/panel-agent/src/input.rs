//! Input handling: keys, mouse, pastes and the clipboard, the prompt's
//! history, completion of slash commands and `@` mentions, and scrolling.

use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use termide_agent_core::{now_millis, CommandScript, ToolResultMessage, UserMessage};
use termide_core::{KeyChord, Panel, PanelEvent};
use termide_ui::textarea::TextArea;
use termide_ui::{ChoiceAction, CompletionAction, CompletionItem, CompletionList, FieldEdit};

use crate::{
    select, transcript, AgentPanel, BannerHit, FoldMode, Item, NoticeKind, Paste, RunButton,
    CLEAR_COMMAND, COMPACT_COMMAND, CONTINUE_COMMAND, FORK_COMMAND, GOAL_COMMAND, HANDOFF_COMMAND,
    LOOP_COMMAND, MCP_COMMAND, NAME_COMMAND, NEW_COMMAND, NEW_SESSION_ACTION, PAUSE_COMMAND,
    PROMPT_COMMAND, RENAME_ACTION, RENAME_COMMAND, RESUME_ACTION, UNDO_COMMAND, USAGE_COMMAND,
};

/// A paste past either bound is held as a short placeholder rather than
/// inlined, so a big block does not swamp the prompt box.
pub(crate) const PASTE_MAX_CHARS: usize = 2000;
pub(crate) const PASTE_MAX_LINES: usize = 5;

/// The path a tool result saved its full raw log to, if it did.
pub(crate) fn full_log_path(result: &ToolResultMessage) -> Option<std::path::PathBuf> {
    result
        .details
        .as_ref()?
        .get("full_output_path")?
        .as_str()
        .map(std::path::PathBuf::from)
}

/// The span an `@`-file mention occupies on one input line, from the `@`
/// (`start`) to the cursor (`end`), in character columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MentionSpan {
    row: usize,
    start: usize,
    end: usize,
}

/// Files and directories under `root` matching `prefix` (the text after `@`),
/// as completion items: a relative path each, directories ending in `/`,
/// ranked by fuzzy match on the path as the open prompt ranks them. The walk
/// leaves out what git ignores, `.git` and `.termide`, and hidden entries
/// unless `prefix` starts with `.`; it is budgeted, so it stays cheap on
/// every keystroke even in a large tree.
pub(crate) fn file_completions(root: &std::path::Path, prefix: &str) -> Vec<CompletionItem> {
    use termide_ui::fuzzy::{rank, Query};

    const MAX_RESULTS: usize = 50;
    const MAX_VISITED: usize = 4000;

    let mut candidates = termide_walk::project_entries(root, prefix.starts_with('.'), MAX_VISITED);
    // Shortest paths first, so an empty or loose query offers the top of the
    // tree before its depths.
    candidates.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));

    let mut query = Query::fuzzy_path(prefix);
    rank(candidates.iter().map(|path| query.score(path)))
        .into_iter()
        .take(MAX_RESULTS)
        .map(|i| {
            let path = &candidates[i];
            let matched = query.positions(path).unwrap_or_default();
            CompletionItem::new(path.clone())
                .with_label(path.clone())
                .with_matched(matched)
        })
        .collect()
}

impl AgentPanel {
    /// The prompt box's text area. The input bar holds exactly one multi-line
    /// field, so both accessors always resolve.
    pub(crate) fn input_area(&self) -> &TextArea {
        self.input
            .multiline(0)
            .expect("agent input is a multiline field")
    }

    pub(crate) fn input_area_mut(&mut self) -> &mut TextArea {
        self.input
            .multiline_mut(0)
            .expect("agent input is a multiline field")
    }

    /// Clear the prompt box; shell mode, if on, stays.
    pub(crate) fn clear_input(&mut self) {
        self.input.set_field_text(0, "");
        self.pastes.clear();
        self.paste_seq = 0;
    }

    /// Insert pasted `text` at the cursor: a small paste inline, a large one as
    /// a short `[#n pasted …]` placeholder whose full content is spliced back
    /// in on [`AgentPanel::submit`].
    pub(crate) fn paste(&mut self, text: &str) {
        let lines = text.lines().count();
        let large = text.chars().count() > PASTE_MAX_CHARS || lines > PASTE_MAX_LINES;
        if !large {
            self.input_area_mut().insert_str(text);
            return;
        }
        // Pasting the same block again unmasks it: the placeholder the first
        // paste left gives way to the full text, to read or edit in place.
        let input = self.input_text();
        if let Some(last) = self.pastes.last() {
            if last.text == text && input.contains(&last.placeholder) {
                let unmasked = input.replacen(&last.placeholder, text, 1);
                self.pastes.pop();
                self.set_input(&unmasked);
                return;
            }
        }
        self.paste_seq += 1;
        let t = termide_i18n::t();
        let label = if lines > 1 {
            t.agent_paste_lines_fmt(lines)
        } else {
            t.agent_paste_chars_fmt(text.chars().count())
        };
        let placeholder = t.agent_paste_placeholder_fmt(self.paste_seq, &label);
        self.input_area_mut().insert_str(&placeholder);
        self.pastes.push(Paste {
            placeholder,
            text: text.to_string(),
        });
    }

    /// Copy the prompt's selection to the clipboard. Returns whether there was
    /// one to copy, so the caller knows whether the key was the prompt's.
    pub(crate) fn copy_input_selection(&mut self) -> bool {
        match self.input_area().selected_text() {
            Some(text) => {
                self.copy_text(&text);
                true
            }
            None => false,
        }
    }

    /// The transcript cell under screen position (`column`, `row`), clamped
    /// into the transcript.
    pub(crate) fn cell_at(&self, column: u16, row: u16) -> select::Cell {
        let area = self.transcript_area;
        let row = row.clamp(area.y, (area.y + area.height).saturating_sub(1));
        let col = column
            .saturating_sub(area.x)
            .min(area.width.saturating_sub(2));
        select::Cell {
            line: self.top + (row - area.y) as usize,
            col: col as usize,
        }
    }

    /// A click on transcript line `line`: focus the chat and select the block
    /// there; a second click on the block already selected folds/unfolds it.
    pub(crate) fn click_line(&mut self, line: usize) -> Vec<PanelEvent> {
        // A pause's ticking line resumes the run.
        if self.paused && self.pause_start.is_some() && self.transcript.is_live_pause_line(line) {
            self.resume();
            return vec![PanelEvent::NeedsRedraw];
        }
        // A click on a run's closing line selects the block above it.
        let Some(index) = self
            .transcript
            .item_at_line(line)
            .and_then(|index| self.transcript.selectable_near(index))
        else {
            return vec![PanelEvent::NeedsRedraw];
        };
        if self.chat_focus && self.selected == index {
            self.transcript.toggle_expanded(index);
        } else {
            self.chat_focus = true;
            self.selected = index;
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// Copy the text selected in the transcript with the mouse. Returns
    /// whether there was any.
    pub(crate) fn copy_text_selection(&mut self) -> bool {
        let Some(selection) = self.text_selection else {
            return false;
        };
        let width = self.transcript_area.width.saturating_sub(1).max(1) as usize;
        let text = selection.text(self.transcript.rendered(), width);
        if text.trim().is_empty() {
            return false;
        }
        self.copy_text(&text);
        true
    }

    /// Copy the prompt's selection and delete it.
    pub(crate) fn cut_input_selection(&mut self) -> bool {
        match self.input_area().selected_text() {
            Some(text) => {
                self.copy_text(&text);
                self.input_area_mut().delete_selection();
                true
            }
            None => false,
        }
    }

    /// Paste the clipboard into the prompt; a large paste is held as a
    /// placeholder rather than flooding the input.
    pub(crate) fn paste_clipboard(&mut self) -> bool {
        match termide_ui::clipboard::paste() {
            Some(text) => {
                self.paste(&text);
                true
            }
            None => false,
        }
    }

    /// Put `text` on the clipboard, telling the user when the clipboard refused.
    pub(crate) fn copy_text(&mut self, text: &str) {
        if let Err(error) = termide_ui::clipboard::copy(text) {
            log::warn!("agent copy failed: {error}");
            self.notice(
                termide_i18n::t().agent_notice_clipboard_failed(),
                NoticeKind::Warn,
            );
        }
    }

    /// Splice every held paste's full content back in place of its placeholder.
    pub(crate) fn expand_pastes(&self, text: &str) -> String {
        let mut out = text.to_string();
        for paste in &self.pastes {
            out = out.replace(&paste.placeholder, &paste.text);
        }
        out
    }

    /// Earlier requests of this session, oldest first, repeats collapsed. In
    /// shell mode these are the commands the user ran by hand, and otherwise
    /// the messages they sent: each mode walks its own history, so recalling
    /// never switches the mode under the arrows.
    pub(crate) fn history(&self) -> Vec<String> {
        let mut history: Vec<String> = Vec::new();
        for item in self.transcript.items() {
            let typed = match item {
                Item::User { text, command, .. } if !self.shell_mode => {
                    command.as_ref().unwrap_or(text).clone()
                }
                Item::Tool { call, .. } if self.shell_mode && transcript::is_user_command(call) => {
                    match call.arguments.get("command").and_then(|v| v.as_str()) {
                        Some(command) => command.to_string(),
                        None => continue,
                    }
                }
                _ => continue,
            };
            if history.last() != Some(&typed) {
                history.push(typed);
            }
        }
        history
    }

    /// Show an earlier (`older`) or later request in the input, the way a
    /// shell recalls its history; past the newest, the draft comes back.
    /// Take the messages still waiting in the queue back into the input, to
    /// edit them before they go: ahead of what is typed, as they were sent
    /// first. Returns whether there were any, so `↑` walks history only once
    /// the queue is empty.
    pub(crate) fn unqueue(&mut self) -> bool {
        if self.history_pos.is_some() || self.queued_texts.is_empty() {
            return false;
        }
        let taken = self.runtime.take_queued();
        self.set_queued(self.runtime.queue_lens());
        self.queued_texts.clear();
        let Some(queued) = UserMessage::merge(taken) else {
            return false;
        };
        let typed = self.input_text();
        let text = if typed.trim().is_empty() {
            queued.typed()
        } else {
            format!("{}\n\n{typed}", queued.typed())
        };
        self.set_input(&text);
        true
    }

    pub(crate) fn recall(&mut self, older: bool) -> bool {
        let history = self.history();
        let next = match (self.history_pos, older) {
            (None, true) if !history.is_empty() => {
                self.draft = self.input_text();
                Some(history.len() - 1)
            }
            (None, _) => return false,
            (Some(pos), true) => Some(pos.saturating_sub(1)),
            (Some(pos), false) if pos + 1 < history.len() => Some(pos + 1),
            (Some(_), false) => None,
        };
        self.history_pos = next;
        let text = match next {
            Some(pos) => history[pos].clone(),
            None => std::mem::take(&mut self.draft),
        };
        self.set_input(&text);
        true
    }

    /// Replace the input with `text`, cursor at its end.
    pub(crate) fn set_input(&mut self, text: &str) {
        self.input.set_field_text(0, text);
        let area = self.input_area_mut();
        while area.move_down() {}
        area.move_end();
    }

    /// Recompute the `/command` popup after the input changed: it shows
    /// while the input is a single `/word` with no space yet.
    /// The plain text of the block under the chat cursor, for `Copy`, trimmed of
    /// the stray leading/trailing blank lines models and tools produce.
    pub(crate) fn selected_block_text(&self) -> Option<String> {
        let text = match self.transcript.items().get(self.selected)? {
            Item::User { text, .. }
            | Item::Assistant { text, .. }
            | Item::Thinking { text, .. }
            | Item::System { text } => text.clone(),
            Item::Notice { text, .. } => text.clone(),
            Item::RunEnd { elapsed_ms, at, .. } => transcript::run_end_text(*elapsed_ms, at),
            Item::Tool {
                result, live, call, ..
            } => result
                .as_ref()
                .map(ToolResultMessage::plain_text)
                .or_else(|| live.clone())
                .unwrap_or_else(|| call.name.clone()),
        };
        Some(text.trim().to_string())
    }

    /// Open the selected block's full output as a read-only panel, for a
    /// bigger view than the inline preview. A tool with a saved raw log opens
    /// that file; anything else is written to a temporary file first. Focus
    /// stays in the chat.
    pub(crate) fn open_selected_in_panel(&mut self) -> Vec<PanelEvent> {
        let Some(item) = self.transcript.items().get(self.selected) else {
            return vec![];
        };
        let (content, name) = match item {
            Item::Tool {
                call, result, live, ..
            } => {
                if let Some(path) = result.as_ref().and_then(full_log_path) {
                    if path.exists() {
                        return vec![PanelEvent::ViewFile(path)];
                    }
                }
                let body = result
                    .as_ref()
                    .map(ToolResultMessage::plain_text)
                    .or_else(|| live.clone())
                    .unwrap_or_default();
                (body, format!("{}-output.txt", call.name))
            }
            Item::Assistant { text, .. } => (text.clone(), "agent-answer.md".to_string()),
            Item::Thinking { text, .. } => (text.clone(), "agent-thinking.md".to_string()),
            Item::System { text } => (text.clone(), "system-prompt.md".to_string()),
            Item::User { text, .. } => (text.clone(), "message.txt".to_string()),
            Item::Notice { .. } | Item::RunEnd { .. } => return vec![PanelEvent::NeedsRedraw],
        };
        if content.trim().is_empty() {
            self.notice(
                termide_i18n::t().agent_notice_nothing_to_open(),
                NoticeKind::Info,
            );
            return vec![PanelEvent::NeedsRedraw];
        }
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        let path = std::env::temp_dir().join(format!("termide-agent-{}-{safe}", now_millis()));
        match std::fs::write(&path, content) {
            Ok(()) => vec![PanelEvent::ViewFile(path), PanelEvent::NeedsRedraw],
            Err(error) => {
                self.notice(
                    termide_i18n::t().agent_notice_cannot_open_block_fmt(&error.to_string()),
                    NoticeKind::Error,
                );
                vec![PanelEvent::NeedsRedraw]
            }
        }
    }

    /// The `@`-file mention under the cursor: the span from the `@` to the
    /// cursor and the text typed after it. `@` counts only at the start of a
    /// word (line start or after whitespace), and the mention ends at the
    /// first space, so it is one path.
    pub(crate) fn mention_at_cursor(&self) -> Option<(MentionSpan, String)> {
        let cursor = self.input_area().cursor();
        let chars: Vec<char> = self.input_area().lines().get(cursor.row)?.chars().collect();
        if cursor.col > chars.len() {
            return None;
        }
        let mut i = cursor.col;
        while i > 0 {
            let c = chars[i - 1];
            if c == '@' {
                let starts_word = i == 1 || chars[i - 2].is_whitespace();
                if !starts_word {
                    return None;
                }
                let prefix: String = chars[i..cursor.col].iter().collect();
                return Some((
                    MentionSpan {
                        row: cursor.row,
                        start: i - 1,
                        end: cursor.col,
                    },
                    prefix,
                ));
            }
            if c.is_whitespace() {
                return None;
            }
            i -= 1;
        }
        None
    }

    pub(crate) fn refresh_completion(&mut self) {
        self.completion_span = None;
        // A shell command is not a slash command or a mention: `/usr/bin` and
        // `@` there are the command's own.
        if self.shell_mode {
            self.completion = None;
            return;
        }
        let text = self.input_area().text();
        let word = text.strip_prefix('/').filter(|rest| {
            self.input_area().line_count() <= 1 && !rest.contains(char::is_whitespace)
        });
        let Some(prefix) = word else {
            return self.refresh_file_completion();
        };
        let mut items: Vec<CompletionItem> = self
            .catalog
            .prompts()
            .into_iter()
            .filter(|template| template.name.starts_with(prefix))
            .map(|template| {
                CompletionItem::new(template.name.clone())
                    .with_label(format!("/{}", template.name))
                    .with_hint(template.argument_hint)
                    .with_description(template.description)
            })
            .collect();
        let taken: Vec<String> = items.iter().map(|i| i.value.clone()).collect();
        let scripts: Vec<CommandScript> = self
            .catalog
            .commands()
            .into_iter()
            .filter(|c| c.name.starts_with(prefix) && !taken.contains(&c.name))
            .collect();
        for script in scripts {
            let description = if script.trusted {
                script.description
            } else if script.description.is_empty() {
                termide_i18n::t().agent_project_command().to_string()
            } else {
                termide_i18n::t().agent_project_command_fmt(&script.description)
            };
            items.push(
                CompletionItem::new(script.name.clone())
                    .with_label(format!("/{}", script.name))
                    .with_hint(script.argument_hint)
                    .with_description(description),
            );
        }
        // A skill whose name something else takes is offered as `skill:<name>`,
        // found by either spelling.
        for (reach, skill) in self.slash_skills() {
            if reach.starts_with(prefix) || skill.name.starts_with(prefix) {
                items.push(
                    CompletionItem::new(reach.clone())
                        .with_label(format!("/{reach}"))
                        .with_hint(skill.argument_hint)
                        .with_description(skill.description),
                );
            }
        }
        if UNDO_COMMAND.starts_with(prefix) && !self.external {
            items.push(
                CompletionItem::new(UNDO_COMMAND)
                    .with_label(format!("/{UNDO_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_undo()),
            );
        }
        if COMPACT_COMMAND.starts_with(prefix) && !self.external {
            items.push(
                CompletionItem::new(COMPACT_COMMAND)
                    .with_label(format!("/{COMPACT_COMMAND}"))
                    .with_hint("[focus]")
                    .with_description(termide_i18n::t().agent_cmd_desc_compact()),
            );
        }
        if self.session_dir.is_some() {
            if NEW_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(NEW_COMMAND)
                        .with_label(format!("/{NEW_COMMAND}"))
                        .with_description(termide_i18n::t().agent_cmd_desc_new()),
                );
            }
            if FORK_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(FORK_COMMAND)
                        .with_label(format!("/{FORK_COMMAND}"))
                        .with_description(termide_i18n::t().agent_cmd_desc_fork()),
                );
            }
            if CLEAR_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(CLEAR_COMMAND)
                        .with_label(format!("/{CLEAR_COMMAND}"))
                        .with_description(termide_i18n::t().agent_cmd_desc_clear()),
                );
            }
            if RENAME_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(RENAME_COMMAND)
                        .with_label(format!("/{RENAME_COMMAND}"))
                        .with_hint("[name]")
                        .with_description(termide_i18n::t().agent_cmd_desc_rename()),
                );
            }
            if NAME_COMMAND.starts_with(prefix) {
                items.push(
                    CompletionItem::new(NAME_COMMAND)
                        .with_label(format!("/{NAME_COMMAND}"))
                        .with_hint("[name]")
                        .with_description(termide_i18n::t().agent_cmd_desc_rename()),
                );
            }
        }
        // Run control is offered only when it applies.
        if self.is_busy() && self.runtime.can_pause() && PAUSE_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(PAUSE_COMMAND)
                    .with_label(format!("/{PAUSE_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_pause()),
            );
        }
        if (self.paused || self.pause_requested) && CONTINUE_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(CONTINUE_COMMAND)
                    .with_label(format!("/{CONTINUE_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_continue()),
            );
        }
        if LOOP_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(LOOP_COMMAND)
                    .with_label(format!("/{LOOP_COMMAND}"))
                    .with_hint(termide_i18n::t().agent_hint_loop())
                    .with_description(termide_i18n::t().agent_cmd_desc_loop()),
            );
        }
        if GOAL_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(GOAL_COMMAND)
                    .with_label(format!("/{GOAL_COMMAND}"))
                    .with_hint(termide_i18n::t().agent_hint_goal())
                    .with_description(termide_i18n::t().agent_cmd_desc_goal()),
            );
        }
        if HANDOFF_COMMAND.starts_with(prefix) && !self.external {
            items.push(
                CompletionItem::new(HANDOFF_COMMAND)
                    .with_label(format!("/{HANDOFF_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_handoff()),
            );
        }
        if USAGE_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(USAGE_COMMAND)
                    .with_label(format!("/{USAGE_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_usage()),
            );
        }
        if PROMPT_COMMAND.starts_with(prefix) {
            items.push(
                CompletionItem::new(PROMPT_COMMAND)
                    .with_label(format!("/{PROMPT_COMMAND}"))
                    .with_description(termide_i18n::t().agent_cmd_desc_prompt()),
            );
        }
        if MCP_COMMAND.starts_with(prefix) && !self.external {
            items.push(
                CompletionItem::new(MCP_COMMAND)
                    .with_label(format!("/{MCP_COMMAND}"))
                    .with_hint(termide_i18n::t().agent_hint_mcp())
                    .with_description(termide_i18n::t().agent_cmd_desc_mcp()),
            );
        }
        if items.is_empty() {
            self.completion = None;
            return;
        }
        // Prompts, command scripts and built-ins are gathered in different
        // groups; show the whole `/` list in one alphabetical order.
        items.sort_by(|a, b| a.value.cmp(&b.value));
        match &mut self.completion {
            Some(list) => list.set_items(items),
            None => self.completion = Some(CompletionList::new(items)),
        }
    }

    /// The `@`-file popup: files and directories under the panel's directory
    /// matching the text after `@`, so a path is a few keystrokes and a
    /// selection. A directory ends with `/` and reopens the popup for its
    /// contents; a file inserts the path and a space. Reuses the same
    /// completion widget as `/`.
    pub(crate) fn refresh_file_completion(&mut self) {
        let Some((span, prefix)) = self.mention_at_cursor() else {
            self.completion = None;
            return;
        };
        let items = file_completions(&self.cwd, &prefix);
        if items.is_empty() {
            self.completion = None;
            return;
        }
        self.completion_span = Some(span);
        match &mut self.completion {
            Some(list) => list.set_items(items),
            None => self.completion = Some(CompletionList::new(items)),
        }
    }

    /// Put the highlighted completion into the input. A `/`-command replaces
    /// the whole input; an `@`-file mention replaces just its span.
    pub(crate) fn accept_completion(&mut self) -> bool {
        let Some(list) = self.completion.take() else {
            return false;
        };
        let Some(item) = list.selected_item().cloned() else {
            return false;
        };
        match self.completion_span.take() {
            None => {
                let text = format!("/{} ", item.value);
                self.set_input(&text);
            }
            Some(span) => {
                let is_dir = item.value.ends_with('/');
                // Delete the `@`+prefix typed so far.
                self.input_area_mut().set_cursor(span.row, span.end);
                for _ in span.start..span.end {
                    self.input_area_mut().backspace();
                }
                if is_dir {
                    // Keep the `@` so the popup reopens for the directory's
                    // contents and the user can drill in.
                    self.input_area_mut().insert('@');
                    self.input_area_mut().insert_str(&item.value);
                    self.refresh_completion();
                } else {
                    // A chosen file becomes a plain path the agent can read.
                    self.input_area_mut().insert_str(&item.value);
                    self.input_area_mut().insert(' ');
                }
            }
        }
        true
    }

    /// The input changed by typing: history browsing ends, the popup follows.
    pub(crate) fn after_edit(&mut self) {
        self.history_pos = None;
        self.refresh_completion();
    }

    pub(crate) fn viewport_height(&self) -> usize {
        self.transcript_area.height as usize
    }

    pub(crate) fn max_top(&self) -> usize {
        self.transcript
            .line_count()
            .saturating_sub(self.viewport_height())
    }

    pub(crate) fn scroll_by(&mut self, delta: i32) {
        let max_top = self.max_top();
        let next = if delta < 0 {
            self.top.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.top.saturating_add(delta as usize)
        };
        self.top = next.min(max_top);
        self.follow = self.top >= max_top;
    }

    /// Unfold every block, or fold them all back when any is unfolded, and
    /// have fresh blocks do the same: unfolded they arrive in full, folded
    /// they follow the configured mode (on finish, when that is `never`).
    pub(crate) fn toggle_all_folded(&mut self) {
        let expand = !self.transcript.any_expanded();
        self.fold = match (expand, self.fold_setting) {
            (true, _) => FoldMode::Never,
            (false, FoldMode::Never) => FoldMode::OnFinish,
            (false, setting) => setting,
        };
        self.transcript.set_fold(self.fold);
        self.transcript.set_all_expanded(expand);
    }

    /// Bring the selected block into view after the selection moves. Scrolling
    /// is otherwise free, so this runs only from block navigation, not on every
    /// frame — a block scrolled off screen stays off until the selection moves.
    /// Uses the geometry of the last render (viewport height, flat-line layout).
    pub(crate) fn scroll_selected_into_view(&mut self) {
        let height = self.viewport_height();
        if height == 0 {
            return;
        }
        let Some(first) = self.transcript.first_line_of(self.selected) else {
            return;
        };
        let mut last = first;
        while self.transcript.item_at_line(last + 1) == Some(self.selected) {
            last += 1;
        }
        if first < self.top {
            // The block starts above the viewport: show it from its start.
            self.top = first;
        } else if last >= self.top + height {
            // It ends below the viewport: reveal its end, or, when it is taller
            // than the viewport, its start so it reads from the top.
            self.top = if last - first < height {
                last + 1 - height
            } else {
                first
            };
        }
        self.top = self.top.min(self.max_top());
        self.follow = self.top >= self.max_top();
    }

    /// The body of [`Panel::handle_key`].
    pub(crate) fn on_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        // A shortcut — a Ctrl/Alt chord, or any key while the chat has focus
        // and nothing is being typed — matches on the canonical form, so it
        // works on a non-Latin layout too (`Ctrl+щ` is `Ctrl+O`). Typing into
        // the input or a pending question keeps the raw key.
        let raw = chord.raw;
        let shortcut = raw
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || (self.chat_focus && self.pending.is_none());
        let key = if shortcut { chord.canonical } else { raw };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let page = (self.viewport_height() as i32 - 1).max(1);

        // A pending question takes the keys first: the arrows, Enter, a
        // digit or Esc answer it; only scrolling passes by.
        if let Some(pending) = &mut self.pending {
            let action = if ctrl || alt {
                ChoiceAction::NotHandled
            } else {
                pending.form_mut().handle_key(key)
            };
            let scroll_key = matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
                || (ctrl
                    && matches!(
                        key.code,
                        KeyCode::Up
                            | KeyCode::Down
                            | KeyCode::Home
                            | KeyCode::End
                            | KeyCode::Char('o')
                    ));
            if self.apply_form_action(action.clone()) {
                return vec![PanelEvent::NeedsRedraw];
            }
            if action == ChoiceAction::NotHandled && !scroll_key {
                return vec![];
            }
        }

        // Ctrl+S saves the chat as Markdown, wherever the focus sits in the
        // panel — the same as the `[≡]` menu's entry.
        if ctrl && !alt && !shift && key.code == KeyCode::Char('s') {
            return self.save_chat();
        }
        // F2 renames the session, wherever the focus sits in the panel — the
        // same prompt as the `[≡]` menu's Rename.
        if key.code == KeyCode::F(2) && !ctrl && !alt && !shift {
            return self.handle_status_action(RENAME_ACTION);
        }
        // F3 shows a summary of the session, F4 offers a checkpoint to roll back
        // to.
        if key.code == KeyCode::F(3) && !ctrl && !alt && !shift {
            return self.session_summary();
        }
        if key.code == KeyCode::F(4) && !ctrl && !alt && !shift {
            return self.ask_rollback();
        }
        // F5 forks the session: the log is copied and the copy opens in a new
        // panel, while this one goes on with its own.
        if key.code == KeyCode::F(5) && !ctrl && !alt && !shift {
            return self.ask_fork_session();
        }
        // F6 switches session (the picker), F7 starts a new one, F8 deletes the
        // current one behind a confirmation card.
        if key.code == KeyCode::F(6) && !ctrl && !alt && !shift {
            return self.handle_status_action(RESUME_ACTION);
        }
        if key.code == KeyCode::F(7) && !ctrl && !alt && !shift {
            return self.handle_status_action(NEW_SESSION_ACTION);
        }
        // In the banner's list of recent sessions F8 deletes the one under the
        // cursor instead: the fresh session itself has nothing to delete.
        if key.code == KeyCode::F(8) && !ctrl && !alt && !shift {
            if self.chat_focus && self.recent_list_shown() {
                return self.ask_delete_recent_session();
            }
            return self.ask_delete_session();
        }

        // The completion list gets the navigation keys while it is open, except
        // the `Shift`-held ones: those extend the prompt's selection.
        let selecting = shift
            && matches!(
                key.code,
                KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Home
                    | KeyCode::End
            );
        let completion_action = match &mut self.completion {
            Some(list) if !ctrl && !alt && !selecting => list.handle_key(key),
            _ => CompletionAction::NotHandled,
        };
        match completion_action {
            CompletionAction::Handled => return vec![PanelEvent::NeedsRedraw],
            CompletionAction::Dismiss => {
                self.completion = None;
                return vec![PanelEvent::NeedsRedraw];
            }
            CompletionAction::Accept => {
                // Enter on the command already typed in full sends it; on a
                // partial one, or on Tab, it completes, like a shell.
                let typed = self.input_area().text();
                let exact = self.completion_span.is_none()
                    && key.code == KeyCode::Enter
                    && self
                        .completion
                        .as_ref()
                        .and_then(CompletionList::selected_item)
                        .is_some_and(|item| format!("/{}", item.value) == typed.trim());
                if exact {
                    return self.submit();
                }
                self.accept_completion();
                return vec![PanelEvent::NeedsRedraw];
            }
            CompletionAction::NotHandled => {}
        }

        // Chat focus: the arrows walk the blocks, Space/Enter fold the one
        // under the cursor and ←/→ fold or unfold it as in the file manager's
        // tree, Tab or Esc hands focus back to the input.
        // On the welcome banner the chat focus walks its recent sessions
        // instead: the arrows, the page keys and Home/End move the cursor,
        // Enter opens the session under it and Delete (like F8) deletes it.
        if self.chat_focus && self.recent_list_shown() {
            let page = self.recent_rows.max(1) as isize;
            match key.code {
                KeyCode::Delete => return self.ask_delete_recent_session(),
                KeyCode::Tab | KeyCode::Esc => self.chat_focus = false,
                KeyCode::Up if !ctrl => self.move_recent_selection(-1),
                KeyCode::Down if !ctrl => self.move_recent_selection(1),
                KeyCode::PageUp => self.move_recent_selection(-page),
                KeyCode::PageDown => self.move_recent_selection(page),
                KeyCode::Home => self.move_recent_selection(isize::MIN),
                KeyCode::End => self.move_recent_selection(isize::MAX),
                KeyCode::Enter => {
                    self.open_recent_session(self.recent_selected);
                }
                _ => return vec![],
            }
            return vec![PanelEvent::NeedsRedraw];
        }
        if self.chat_focus {
            let count = self.transcript.items().len();
            match key.code {
                KeyCode::Tab | KeyCode::Esc => {
                    self.chat_focus = false;
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Up if !ctrl => {
                    if let Some(prev) = (0..self.selected)
                        .rev()
                        .find(|&i| self.transcript.is_selectable(i))
                    {
                        self.selected = prev;
                    }
                    self.follow = false;
                    self.scroll_selected_into_view();
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Down if !ctrl => {
                    if let Some(next) =
                        (self.selected + 1..count).find(|&i| self.transcript.is_selectable(i))
                    {
                        self.selected = next;
                    }
                    self.follow = false;
                    self.scroll_selected_into_view();
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Char(' ') | KeyCode::Enter => {
                    self.transcript.toggle_expanded(self.selected);
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Left | KeyCode::Right if !ctrl && !alt && !shift => {
                    let expand = key.code == KeyCode::Right;
                    if !self.transcript.set_expanded(self.selected, expand) {
                        return vec![];
                    }
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::Char('o') if !ctrl => {
                    return self.open_selected_in_panel();
                }
                KeyCode::Char('o') if ctrl => {
                    self.toggle_all_folded();
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::PageUp => {
                    self.scroll_by(-page);
                    return vec![PanelEvent::NeedsRedraw];
                }
                KeyCode::PageDown => {
                    self.scroll_by(page);
                    return vec![PanelEvent::NeedsRedraw];
                }
                // Everything else is swallowed so it does not type into the
                // (unfocused) input.
                _ => return vec![],
            }
        }
        // From the input, Tab moves focus into the chat when it has a block
        // (annotations alone give the cursor nowhere to stop).
        let last_block = self
            .transcript
            .items()
            .len()
            .checked_sub(1)
            .and_then(|last| self.transcript.selectable_near(last));
        // Under the banner the notices are not blocks to walk.
        let last_block = last_block.filter(|_| !self.banner_shown());
        if let (KeyCode::Tab, Some(last_block)) = (key.code, last_block) {
            self.chat_focus = true;
            self.follow = false;
            self.selected = last_block;
            return vec![PanelEvent::NeedsRedraw];
        }
        // With no conversation yet, Tab moves focus into the banner's list of
        // recent sessions, when it has one.
        if key.code == KeyCode::Tab && self.recent_list_shown() {
            self.chat_focus = true;
            self.scroll_recent_selection_into_view();
            return vec![PanelEvent::NeedsRedraw];
        }

        match key.code {
            KeyCode::Esc => {
                // A command the user ran by hand stops first: it is theirs and
                // in the way, and stopping it should not also abort a run.
                if self.cancel_user_command() {
                    return vec![PanelEvent::NeedsRedraw];
                }
                // Then shell mode ends, with whatever command was being typed;
                // a run the agent is on goes on.
                if self.shell_mode {
                    self.shell_mode = false;
                    self.clear_input();
                    self.after_edit();
                } else if self.is_busy() {
                    self.abort();
                } else if self.goal_task.take().is_some() {
                    self.notice(
                        termide_i18n::t().agent_notice_goal_stopped(),
                        NoticeKind::Info,
                    );
                } else if self.loop_task.take().is_some() {
                    self.notice(
                        termide_i18n::t().agent_notice_loop_stopped(),
                        NoticeKind::Info,
                    );
                } else if !self.input_area().is_empty() {
                    self.clear_input();
                    self.after_edit();
                } else {
                    return vec![];
                }
            }
            KeyCode::Enter if shift || alt => {
                self.input_area_mut().insert_newline();
                self.after_edit();
            }
            KeyCode::Char('j') if ctrl => {
                self.input_area_mut().insert_newline();
                self.after_edit();
            }
            KeyCode::Enter => return self.submit(),
            KeyCode::Char('o') if ctrl => self.toggle_all_folded(),
            KeyCode::BackTab if !self.external || self.runtime.follows_mode() => {
                let next = self.mode.get().next();
                return vec![self.set_mode(next), PanelEvent::NeedsRedraw];
            }
            KeyCode::PageUp => self.scroll_by(-page),
            KeyCode::PageDown => self.scroll_by(page),
            KeyCode::Home if ctrl => {
                self.top = 0;
                self.follow = false;
            }
            KeyCode::End if ctrl => self.follow = true,
            KeyCode::Up if ctrl => self.scroll_by(-1),
            KeyCode::Down if ctrl => self.scroll_by(1),
            KeyCode::Up | KeyCode::Down => {
                // Past the first or last line, `↑` first takes back what is
                // still queued, then the arrows walk through what was asked
                // before, as in a shell; with Shift held the selection takes
                // the arrow and history waits.
                let up = key.code == KeyCode::Up;
                let handled = self.input.edit_field(0, key) != FieldEdit::NotHandled
                    || shift
                    || (up && self.unqueue())
                    || self.recall(up);
                if !handled {
                    return vec![];
                }
            }
            // Prompt clipboard: the panel owns these so a large paste keeps its
            // placeholder handling and a failure can raise a notice.
            KeyCode::Char('c') if ctrl => {
                if !self.copy_input_selection() && !self.copy_text_selection() {
                    return vec![];
                }
            }
            KeyCode::Char('x') if ctrl => {
                if !self.cut_input_selection() {
                    return vec![];
                }
                self.after_edit();
            }
            KeyCode::Char('v') if ctrl => {
                if !self.paste_clipboard() {
                    return vec![];
                }
                self.after_edit();
            }
            // `$` into an empty input switches it to shell commands, and the
            // `$` itself becomes the prompt marker, as it heads a shell call's
            // block; Backspace in the empty input switches back, as Esc does.
            KeyCode::Char('$')
                if !ctrl && !alt && !self.shell_mode && self.input_area().is_empty() =>
            {
                self.shell_mode = true;
                self.after_edit();
            }
            KeyCode::Backspace if !alt && self.shell_mode && self.input_area().is_empty() => {
                self.shell_mode = false;
                self.after_edit();
            }
            // Everything the prompt edits with: typing, deletion, character and
            // word navigation and selection.
            KeyCode::Left
            | KeyCode::Right
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Char(_)
                if !alt =>
            {
                let edit = self.input.edit_field(0, key);
                if edit == FieldEdit::NotHandled {
                    return vec![];
                }
                if edit == FieldEdit::Edited {
                    self.after_edit();
                }
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }

    /// The body of [`Panel::handle_mouse`].
    pub(crate) fn on_mouse(&mut self, event: MouseEvent) -> Vec<PanelEvent> {
        // The prompt box claims its own presses and drags: a press places the
        // cursor, a drag selects the text under it. It is asked first because
        // the bar sits below the transcript, whose rows would otherwise take
        // every click, and because a release must reach the bar to end a drag
        // that started in it — even after the pointer has been dragged up into
        // the transcript. A pending question keeps its clicks to itself.
        // A run control on the prompt's border acts on the press.
        if matches!(event.kind, MouseEventKind::Down(MouseButton::Left)) {
            let button = self
                .input
                .border_button_at(event.column, event.row)
                .and_then(|index| self.run_buttons.get(index).copied());
            if let Some(button) = button {
                match button {
                    RunButton::Pause => {
                        self.request_pause();
                    }
                    RunButton::Continue if self.paused && !self.is_busy() => self.resume(),
                    RunButton::Continue => self.cancel_pause(),
                    RunButton::Stop if self.paused && !self.is_busy() => self.stop_paused(),
                    RunButton::Stop => self.abort(),
                }
                return vec![PanelEvent::NeedsRedraw];
            }
        }
        if self.pending.is_none() && self.press.is_none() && self.input.mouse_hits(event) {
            self.input.handle_mouse(event);
            match event.kind {
                MouseEventKind::Up(_) => return vec![],
                _ => {
                    self.chat_focus = false;
                    return vec![PanelEvent::NeedsRedraw];
                }
            }
        }
        match event.kind {
            MouseEventKind::ScrollDown => self.scroll_by(3),
            MouseEventKind::ScrollUp => self.scroll_by(-3),
            MouseEventKind::Drag(MouseButton::Left) => {
                let Some(anchor) = self.press else {
                    return vec![];
                };
                // Dragged past an edge, the transcript scrolls under it.
                let area = self.transcript_area;
                if event.row < area.y {
                    self.scroll_by(-1);
                } else if event.row >= area.y + area.height {
                    self.scroll_by(1);
                }
                let head = self.cell_at(event.column, event.row);
                self.text_selection = Some(select::TextSelection { anchor, head });
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(press) = self.press.take() else {
                    return vec![];
                };
                if self.text_selection.is_some_and(|sel| !sel.is_empty()) {
                    return vec![PanelEvent::NeedsRedraw];
                }
                self.text_selection = None;
                return self.click_line(press.line);
            }
            MouseEventKind::Down(MouseButton::Left) => {
                if self.pending.is_some() {
                    // A click on a row selects it, and only a second click (a
                    // double click) on the same row confirms — so a misplaced
                    // click cannot answer. A click on the detail folds it.
                    let hit = self
                        .pending
                        .as_ref()
                        .unwrap()
                        .form()
                        .hit(event.column, event.row);
                    if let Some(index) = hit {
                        if self.form_clicks.click(index) >= 2 {
                            self.form_clicks.reset();
                            let action =
                                self.pending.as_mut().unwrap().form_mut().activate_at(index);
                            if self.apply_form_action(action) {
                                return vec![PanelEvent::NeedsRedraw];
                            }
                        } else {
                            self.pending.as_mut().unwrap().form_mut().select(index);
                        }
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    if self
                        .pending
                        .as_mut()
                        .unwrap()
                        .form_mut()
                        .click_select(event.column, event.row)
                    {
                        self.form_clicks.reset();
                        return vec![PanelEvent::NeedsRedraw];
                    }
                }
                if let Some(list) = &mut self.completion {
                    if let Some(index) = list.hit(event.column, event.row) {
                        list.select(index);
                        self.accept_completion();
                        return vec![PanelEvent::NeedsRedraw];
                    }
                }
                // A click on a re-pickable field in the welcome banner opens its
                // picker — the same one its status-bar chip opens — and one on
                // a recent session opens that session.
                let banner_hit = self
                    .banner_hits
                    .iter()
                    .find(|(rect, _)| {
                        event.column >= rect.x
                            && event.column < rect.x + rect.width
                            && event.row == rect.y
                    })
                    .map(|(_, hit)| *hit);
                match banner_hit {
                    Some(BannerHit::Action(action)) => return self.handle_status_action(action),
                    Some(BannerHit::Session(index)) => {
                        self.open_recent_session(index);
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    None => {}
                }
                let area = self.transcript_area;
                let inside = event.column >= area.x
                    && event.column < area.x + area.width
                    && event.row >= area.y
                    && event.row < area.y + area.height;
                // The state strip's pending-pause line withdraws the pause.
                if self.pause_requested && self.pause_row == Some(event.row) {
                    self.cancel_pause();
                    return vec![PanelEvent::NeedsRedraw];
                }
                // Under the banner the notices take no clicks: the banner's
                // own rows did above.
                if inside && self.banner_shown() {
                    return vec![];
                }
                if !inside {
                    // A click below the transcript lands on the input: hand focus
                    // back to it so typing resumes.
                    if self.chat_focus {
                        self.chat_focus = false;
                        return vec![PanelEvent::NeedsRedraw];
                    }
                    return vec![];
                }
                // The press may start a text selection; only a release
                // without a drag clicks the block under it.
                self.press = Some(self.cell_at(event.column, event.row));
                self.text_selection = None;
            }
            _ => return vec![],
        }
        vec![PanelEvent::NeedsRedraw]
    }
}
