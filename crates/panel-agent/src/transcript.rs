//! The visible conversation: items mirrored from agent events and their
//! rendered lines, cached per item so a streaming token re-renders one
//! message rather than the whole history.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use termide_agent_core::{
    DecidedBy, Lasting, PermissionNote, ToolCall, ToolResultMessage, UserMessage,
};
use termide_core::ThemeColors;
use termide_panel_markdown::render_markdown;
use termide_richtext::{Builder, RowCopy};

/// Lines of tool output shown when a call is expanded.
const EXPANDED_OUTPUT_LINES: usize = 60;
/// A block whose foldable content is this many lines or fewer is shown in full,
/// without a fold marker or fold logic — there is nothing to save by hiding it.
const FOLD_THRESHOLD: usize = 5;
/// A collapsed content block shows this many lines from the top, then the
/// "… N more lines" marker, then [`FOLD_TAIL_LINES`] from the bottom — the
/// ellipsis sits between the first line and the last few, as a tool call shows.
const FOLD_HEAD_LINES: usize = 1;
/// Lines shown from the bottom of a collapsed content block; with
/// [`FOLD_HEAD_LINES`] they sum to [`FOLD_THRESHOLD`], so a foldable block
/// (more than that many lines) always hides at least one.
const FOLD_TAIL_LINES: usize = FOLD_THRESHOLD - FOLD_HEAD_LINES;
/// Lines of a collapsed user message shown before it is cut off.
const USER_PREVIEW_LINES: usize = FOLD_THRESHOLD;

/// The collapsed head/tail split every foldable content block shares: the
/// index where the shown tail begins, and how many lines are hidden between
/// the head and that tail. Only meaningful when the block is foldable
/// (`total > FOLD_THRESHOLD`), where `hidden >= 1`.
fn fold_split(total: usize) -> (usize, usize) {
    let tail_start = total.saturating_sub(FOLD_TAIL_LINES);
    let hidden = tail_start.saturating_sub(FOLD_HEAD_LINES);
    (hidden, tail_start)
}

/// When reasoning and tool calls fold to their one-line headline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FoldMode {
    /// Folded from the start, while they are still running.
    #[default]
    Immediately,
    /// In full while they run, folded once they finish.
    OnFinish,
    /// Never folded; every block shows in full.
    Never,
}

/// Whether `item` is still being produced: reasoning that is streaming, or a
/// tool call that has not returned. Unless blocks fold immediately, a live
/// block shows in full whatever its fold flag says; the flag takes effect
/// once it finishes.
fn is_live(item: &Item) -> bool {
    match item {
        Item::Thinking { streaming, .. } | Item::Assistant { streaming, .. } => *streaming,
        // A restored call that never got a result carries its time, so it is
        // not mistaken for a running one.
        Item::Tool { result, at, .. } => result.is_none() && at.is_empty(),
        _ => false,
    }
}

/// Whether `item` has detail worth folding away. A live block never folds. A
/// finished tool call or reasoning folds to its one-line headline, so any
/// output, a multi-line command or any reasoning is worth hiding; other blocks
/// fold only past
/// [`FOLD_THRESHOLD`] lines. A block with nothing to hide shows in full and
/// ignores the collapse flag, so it carries no fold marker.
fn is_foldable(item: &Item, fold: FoldMode) -> bool {
    if is_live(item)
        && !(fold == FoldMode::Immediately
            && matches!(item, Item::Thinking { .. } | Item::Tool { .. }))
    {
        return false;
    }
    match item {
        // What a command expanded to folds under the command whatever its size.
        Item::User {
            text,
            command: Some(_),
            ..
        } => !text.trim().is_empty(),
        Item::User { text, .. } | Item::System { text } => {
            text.trim().lines().count() > FOLD_THRESHOLD
        }
        // Finished reasoning folds to its first line, so any of it has detail
        // worth hiding: the rest of the text and the full cost lines.
        Item::Thinking { text, .. } => !text.trim().is_empty(),
        Item::Tool {
            call, result, live, ..
        } => {
            // A running call's output so far counts too: it folds away when
            // blocks fold immediately.
            let output = match result {
                Some(result) => !result.plain_text().trim().is_empty(),
                None => live.as_ref().is_some_and(|live| !live.trim().is_empty()),
            };
            let command = shell_command(call).map_or(0, |c| c.lines().count());
            output || command > 1
        }
        // The answer is always shown in full, so it never folds.
        Item::Assistant { .. } | Item::Notice { .. } | Item::RunEnd { .. } => false,
    }
}

/// Whether `item` is an annotation — a notice or a run's closing line — rather
/// than a block: it marks a moment in the flow, never folds, and the chat
/// cursor passes over it.
fn is_annotation(item: &Item) -> bool {
    matches!(item, Item::Notice { .. } | Item::RunEnd { .. })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Warn,
    Error,
}

/// The prefill/generation cost of a finished model turn, split into the two
/// phases for the `⏫` (prefill) and `✍️` (generation) meta lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    pub prefill_ms: u32,
    pub gen_ms: u32,
    /// Input tokens (the prompt processed during prefill).
    pub input: u64,
    /// Output tokens the model generated.
    pub output: u64,
}

/// The call id of a command the user ran by hand. Such a command shows as a
/// `bash` call block, though the agent never made the call; the id is what
/// tells the two apart, and no model-issued id takes this form.
pub(crate) const USER_COMMAND_ID: &str = "user-command";

/// Whether `call` is a command the user ran by hand rather than the agent's.
pub(crate) fn is_user_command(call: &ToolCall) -> bool {
    call.id == USER_COMMAND_ID
}

/// The block of a command the user ran by hand: a `bash` call with what it
/// printed, as the agent's own shell calls show. `None` for any message that
/// is not one.
pub(crate) fn user_command_item(
    user: &UserMessage,
    at: String,
    duration_ms: Option<u32>,
) -> Option<Item> {
    let command = user.ran.as_ref()?;
    let call = ToolCall {
        id: USER_COMMAND_ID.into(),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": command }),
        extra_content: None,
    };
    let output = user.plain_text();
    let result = if user.ran_failed {
        ToolResultMessage::error(&call, output)
    } else {
        ToolResultMessage::text(&call, output)
    };
    Some(Item::Tool {
        call,
        result: Some(result),
        live: None,
        at,
        duration_ms,
        waited_ms: None,
        waiting: false,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    User {
        text: String,
        /// Local wall-clock time the message was sent, e.g. `21:03:14`.
        at: String,
        /// The `/name args` typed when a template, script or skill produced
        /// `text`: the block's headline, with `text` folded under it.
        command: Option<String>,
    },
    /// The system prompt in effect, shown (folded by default) at the start of a
    /// session and whenever it changes before a new message. Marked with `#`.
    System {
        text: String,
    },
    /// The model's reasoning, its own block above the answer, marked with `@`.
    Thinking {
        text: String,
        streaming: bool,
        /// Local wall-clock time the reasoning finished (its meta's "when").
        at: String,
        /// The turn's cost; the reasoning block shows the prefill part (`⏫`).
        cost: Option<Cost>,
    },
    Assistant {
        text: String,
        streaming: bool,
        error: Option<String>,
        /// Local wall-clock time the turn finished (the meta line's "when").
        at: String,
        /// The turn's prefill/generation cost, once it has finished.
        cost: Option<Cost>,
        /// The whole run's time from the request, when this answer closed a
        /// run that finished cleanly (`✻`); otherwise a closing annotation
        /// carries it.
        run_ms: Option<u32>,
    },
    Tool {
        call: ToolCall,
        result: Option<ToolResultMessage>,
        /// Accumulated output while the tool is still running.
        live: Option<String>,
        /// Local wall-clock time the call finished.
        at: String,
        /// How long the call took, in ms, once it has finished (`🕒`).
        duration_ms: Option<u32>,
        /// How long the call waited on a permission answer, in ms, kept apart
        /// from its own duration (`‖`).
        waited_ms: Option<u32>,
        /// The permission question is still up: the wait ticks and stands out.
        waiting: bool,
    },
    Notice {
        text: String,
        kind: NoticeKind,
    },
    /// The closing line of a run that did not end on a clean answer: how long
    /// it took from the request, or — for a pause — how long the pause has
    /// lasted (ticking while it does).
    RunEnd {
        /// How long the run took, or the pause has lasted, in ms.
        elapsed_ms: u32,
        /// Local wall-clock time the run finished.
        at: String,
        /// Whether the run ended without an error or an abort.
        ok: bool,
        /// The run stopped at a `/pause`, resumable with `/continue`.
        paused: bool,
        /// The pause is still on: its line ticks and its `‖` stands out, as
        /// the live run clock does; once it ends both rest, dimmed.
        live: bool,
    },
}

struct Cached {
    width: u16,
    is_light: bool,
    /// Whether the item folds at this width (see [`render_item`]).
    foldable: bool,
    lines: Vec<Line<'static>>,
    /// The links the item marks up, on its own lines.
    links: Vec<RowLink>,
    /// How each of `lines` reads when copied.
    copy: Vec<RowCopy>,
}

/// A lit stretch of a link: `(line, start, end)`, a `[start, end)` column
/// range on a flattened line.
pub(crate) type LinkRegion = (usize, usize, usize);

/// A link a block marks up rather than writes out — a Markdown
/// `[text](href)` in an answer, a tool call's path or URL in its wrapped
/// headline — as a `[start, end)` column range on one of the rendered lines.
/// A link wrapped over several lines is several of these sharing an `id`.
/// A web address or path written out in plain text is found on its row
/// instead (`select::link_at`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowLink {
    pub line: usize,
    pub start: usize,
    pub end: usize,
    /// The target as the block gives it: a URL, or a path that may be
    /// relative to the working directory.
    pub href: String,
    /// Which link of its block the region belongs to.
    pub id: usize,
}

#[derive(Default)]
pub struct Transcript {
    items: Vec<Item>,
    /// When reasoning and tool calls fold; `Never` shows everything
    /// expanded.
    fold: FoldMode,
    /// Whether each item hides its detail (thinking / full output / the rest
    /// of a long message). Parallel to `items`. The assistant's answer always
    /// shows; collapsing only folds its thinking away.
    collapsed: Vec<bool>,
    cache: Vec<Option<Cached>>,
    /// Flattened lines of every item, rebuilt when any cache entry changed.
    flat: Vec<Line<'static>>,
    /// Item index per flattened line, for click-to-expand.
    line_item: Vec<usize>,
    /// How each flattened line reads when copied.
    flat_copy: Vec<RowCopy>,
    /// The marked-up links on the flattened lines, with their item.
    flat_links: Vec<(usize, RowLink)>,
    flat_dirty: bool,
    /// Live footer lines appended after the last item while the agent works
    /// (the ticking generation and clock meta with the spinner); empty when
    /// idle.
    live_footer: Vec<Line<'static>>,
}

impl Transcript {
    /// Set when blocks fold from now on. Blocks already pushed keep their
    /// fold flag, but whether a live one can fold follows the mode, so each
    /// is laid out again.
    pub fn set_fold(&mut self, fold: FoldMode) {
        if self.fold == fold {
            return;
        }
        self.fold = fold;
        for index in 0..self.cache.len() {
            self.invalidate(index);
        }
    }

    /// The `task` calls still running: each one's index, call and the
    /// subagent's progress so far.
    pub(crate) fn running_tasks(&self) -> Vec<(usize, &ToolCall, Option<&str>)> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match item {
                Item::Tool { call, live, .. } if call.name == "task" && is_live(item) => {
                    Some((index, call, live.as_deref()))
                }
                _ => None,
            })
            .collect()
    }

    #[must_use]
    pub fn items(&self) -> &[Item] {
        &self.items
    }

    pub fn push(&mut self, item: Item) {
        // Everything folds by default except an annotation (a notice, a run's
        // closing line);
        // an item's primary content still shows, only its detail is hidden.
        // Set never to fold, nothing does.
        let collapsed = self.fold != FoldMode::Never && !is_annotation(&item);
        self.items.push(item);
        self.collapsed.push(collapsed);
        self.cache.push(None);
        self.flat_dirty = true;
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.collapsed.clear();
        self.cache.clear();
        self.flat.clear();
        self.line_item.clear();
        self.flat_copy.clear();
        self.flat_links.clear();
        self.flat_dirty = true;
    }

    fn invalidate(&mut self, index: usize) {
        if let Some(slot) = self.cache.get_mut(index) {
            *slot = None;
        }
        self.flat_dirty = true;
    }

    /// Set the live footer shown after the last block while the agent works
    /// (the generation and clock meta with the spinner). An empty vector
    /// removes it.
    pub fn set_live_footer(&mut self, footer: Vec<Line<'static>>) {
        // The footer ticks every frame, so a rebuild of the flat line list is
        // needed whenever it is present or is being cleared.
        if !footer.is_empty() || !self.live_footer.is_empty() {
            self.flat_dirty = true;
        }
        self.live_footer = footer;
    }

    /// Append reasoning to the streaming thinking block, starting one if the
    /// last item is not already a streaming thinking block.
    pub fn stream_thinking(&mut self, delta: &str) {
        if let Some(index) = self.items.len().checked_sub(1) {
            if let Item::Thinking {
                text,
                streaming: true,
                ..
            } = &mut self.items[index]
            {
                text.push_str(delta);
                self.invalidate(index);
                return;
            }
        }
        self.push(Item::Thinking {
            text: delta.to_string(),
            streaming: true,
            at: String::new(),
            cost: None,
        });
    }

    /// Append answer text to the streaming assistant block, starting one if the
    /// last item is not already a streaming assistant block.
    pub fn stream_answer(&mut self, delta: &str) {
        if let Some(index) = self.items.len().checked_sub(1) {
            if let Item::Assistant {
                text,
                streaming: true,
                ..
            } = &mut self.items[index]
            {
                text.push_str(delta);
                self.invalidate(index);
                return;
            }
        }
        self.push(Item::Assistant {
            text: delta.to_string(),
            streaming: true,
            error: None,
            at: String::new(),
            cost: None,
            run_ms: None,
        });
    }

    fn last_streaming(&self, is_thinking: bool) -> Option<usize> {
        self.items.iter().rposition(|item| match item {
            Item::Thinking { streaming, .. } => is_thinking && *streaming,
            Item::Assistant { streaming, .. } => !is_thinking && *streaming,
            _ => false,
        })
    }

    /// Close the streaming reasoning block, if one is open, giving it its
    /// completion time and cost. Returns whether a reasoning block was closed,
    /// so the caller can decide where the prefill indicator belongs.
    pub fn finish_thinking(&mut self, at: &str, cost: Option<Cost>) -> bool {
        let Some(index) = self.last_streaming(true) else {
            return false;
        };
        if let Item::Thinking {
            streaming,
            at: current_at,
            cost: current_cost,
            ..
        } = &mut self.items[index]
        {
            *streaming = false;
            *current_at = at.to_string();
            *current_cost = cost;
        }
        self.invalidate(index);
        true
    }

    /// Finish the turn: complete the streaming answer with the authoritative
    /// message. When `drop_if_empty` is set (a reasoning block above already
    /// carries the turn's meta) and there is no answer text and no error, no
    /// answer block is created — the reasoning block stands for the turn. A whole
    /// message that arrives without a `MessageStart` (an external agent's
    /// failure, for one) is appended as it is. Close the reasoning block first
    /// with [`Transcript::finish_thinking`].
    pub fn finish_assistant(
        &mut self,
        text: String,
        error: Option<String>,
        cost: Option<Cost>,
        at: String,
        drop_if_empty: bool,
    ) {
        if let Some(index) = self.last_streaming(false) {
            // A streaming answer that turns out empty (only whitespace deltas,
            // as before a tool call) leaves no block behind.
            if text.trim().is_empty() && error.is_none() {
                self.items.remove(index);
                self.collapsed.remove(index);
                self.cache.remove(index);
                self.flat_dirty = true;
                return;
            }
            if let Item::Assistant {
                text: current,
                streaming,
                error: current_error,
                cost: current_cost,
                at: current_at,
                ..
            } = &mut self.items[index]
            {
                *current = text;
                *streaming = false;
                *current_error = error;
                *current_cost = cost;
                *current_at = at;
                self.invalidate(index);
                return;
            }
        }
        if drop_if_empty && text.trim().is_empty() && error.is_none() {
            return;
        }
        self.push(Item::Assistant {
            text,
            streaming: false,
            error,
            at,
            cost,
            run_ms: None,
        });
    }

    /// Rewrite the notice at `index` in place; `false` when there is none
    /// there (the transcript was cleared since).
    pub fn replace_notice(&mut self, index: usize, text: String, kind: NoticeKind) -> bool {
        let Some(item @ Item::Notice { .. }) = self.items.get_mut(index) else {
            return false;
        };
        *item = Item::Notice { text, kind };
        self.invalidate(index);
        true
    }

    pub fn with_tool(&mut self, call_id: &str, f: impl FnOnce(&mut Item)) -> bool {
        let found = self
            .items
            .iter()
            .rposition(|item| matches!(item, Item::Tool { call, .. } if call.id == call_id));
        let Some(index) = found else {
            return false;
        };
        f(&mut self.items[index]);
        self.invalidate(index);
        true
    }

    /// Set how long the running call has waited on a permission answer,
    /// `waiting` while the question is still up. Returns whether the shown
    /// line changed (whole seconds, so a redraw a second is enough).
    pub fn set_tool_wait(&mut self, ms: u32, still: bool) -> bool {
        let Some(index) = self
            .items
            .iter()
            .rposition(|item| matches!(item, Item::Tool { result: None, .. }))
        else {
            return false;
        };
        let Item::Tool {
            waited_ms, waiting, ..
        } = &mut self.items[index]
        else {
            return false;
        };
        let changed = *waiting != still || waited_ms.is_none_or(|shown| shown / 1000 != ms / 1000);
        *waited_ms = Some(ms);
        *waiting = still;
        if changed {
            self.invalidate(index);
        }
        changed
    }

    /// How long the call `call_id` waited on a permission answer.
    #[must_use]
    pub fn tool_wait(&self, call_id: &str) -> Option<u32> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Tool {
                call, waited_ms, ..
            } if call.id == call_id => *waited_ms,
            _ => None,
        })
    }

    /// How long the call `call_id` took, once it has finished.
    #[must_use]
    pub fn tool_duration(&self, call_id: &str) -> Option<u32> {
        self.items.iter().rev().find_map(|item| match item {
            Item::Tool {
                call, duration_ms, ..
            } if call.id == call_id => *duration_ms,
            _ => None,
        })
    }

    /// Close a run. One that ended on an answer whose own status says how the
    /// run went — a clean answer after a clean run, a failed one after a
    /// failed run — takes its total on that answer's meta, beside the time
    /// and the status it already shows. Any other (paused, ended on a tool
    /// call, aborted after an answer) gets a closing line that says how it
    /// ended.
    pub fn end_run(&mut self, elapsed_ms: u32, at: &str, ok: bool, paused: bool) {
        if !paused {
            if let Some(index) = self.items.len().checked_sub(1) {
                if let Item::Assistant {
                    streaming: false,
                    error,
                    run_ms,
                    ..
                } = &mut self.items[index]
                {
                    if error.is_none() == ok {
                        *run_ms = Some(elapsed_ms);
                        self.invalidate(index);
                        return;
                    }
                }
            }
        }
        self.push(Item::RunEnd {
            elapsed_ms,
            at: at.to_string(),
            ok,
            paused,
            live: paused,
        });
    }

    /// Set the length of the pause the transcript ends on, while it lasts.
    /// Returns whether the shown line changed (it shows whole seconds, so a
    /// redraw a second is enough).
    pub fn set_pause_length(&mut self, ms: u32) -> bool {
        let Some(index) = self.items.len().checked_sub(1) else {
            return false;
        };
        let Item::RunEnd {
            elapsed_ms,
            live: true,
            ..
        } = &mut self.items[index]
        else {
            return false;
        };
        if *elapsed_ms / 1000 == ms / 1000 {
            *elapsed_ms = ms;
            return false;
        }
        *elapsed_ms = ms;
        self.invalidate(index);
        true
    }

    /// End the pause the transcript ends on at `ms` long: its line keeps that
    /// length and rests, dimmed.
    pub fn finish_pause(&mut self, ms: u32) {
        let Some(index) = self.items.len().checked_sub(1) else {
            return;
        };
        if let Item::RunEnd {
            elapsed_ms,
            live: live @ true,
            ..
        } = &mut self.items[index]
        {
            *elapsed_ms = ms;
            *live = false;
            self.invalidate(index);
        }
    }

    /// Whether flattened line `line` shows the pause the transcript ends on.
    #[must_use]
    pub fn is_live_pause_line(&self, line: usize) -> bool {
        let last = self.items.len().checked_sub(1);
        last.is_some_and(|last| {
            self.item_at_line(line) == Some(last)
                && matches!(self.items[last], Item::RunEnd { live: true, .. })
        })
    }

    /// Flip whether item `index` shows its detail. A small block has no detail
    /// to hide, so it does not fold.
    pub fn toggle_expanded(&mut self, index: usize) -> bool {
        if !self.foldable(index) {
            return false;
        }
        let Some(slot) = self.collapsed.get_mut(index) else {
            return false;
        };
        *slot = !*slot;
        self.invalidate(index);
        true
    }

    /// Show (`expanded` true) or hide the detail of item `index`; false when
    /// it does not fold or already is that way.
    pub fn set_expanded(&mut self, index: usize, expanded: bool) -> bool {
        if !self.foldable(index) {
            return false;
        }
        match self.collapsed.get_mut(index) {
            Some(slot) if *slot == expanded => {
                *slot = !expanded;
                self.invalidate(index);
                true
            }
            _ => false,
        }
    }

    /// Expand (`value` true) or collapse every foldable item at once.
    pub fn set_all_expanded(&mut self, value: bool) {
        for index in 0..self.items.len() {
            if is_annotation(&self.items[index]) {
                continue;
            }
            if self.collapsed[index] == value {
                self.collapsed[index] = !value;
                self.invalidate(index);
            }
        }
    }

    /// Whether item `index` is foldable and shows its detail.
    #[cfg(test)]
    pub(crate) fn is_expanded(&self, index: usize) -> bool {
        self.foldable(index) && !self.collapsed.get(index).copied().unwrap_or(true)
    }

    /// Whether any foldable item is currently expanded.
    #[must_use]
    pub fn any_expanded(&self) -> bool {
        self.items
            .iter()
            .enumerate()
            .any(|(index, _)| self.foldable(index) && !self.collapsed[index])
    }

    /// Whether item `index` folds: as last laid out, since a block whose
    /// unfolded form is a single row at that width has nothing to fold.
    fn foldable(&self, index: usize) -> bool {
        match self.cache.get(index) {
            Some(Some(cached)) => cached.foldable,
            _ => self
                .items
                .get(index)
                .is_some_and(|item| is_foldable(item, self.fold)),
        }
    }

    /// Whether the chat cursor can stop on item `index`. A notice (an error,
    /// say) can be selected and copied; a run's closing line only marks the
    /// end, so selection passes over it.
    #[must_use]
    pub fn is_selectable(&self, index: usize) -> bool {
        self.items
            .get(index)
            .is_some_and(|item| !matches!(item, Item::RunEnd { .. }))
    }

    /// The nearest selectable item at or before `index`, else after it.
    #[must_use]
    pub fn selectable_near(&self, index: usize) -> Option<usize> {
        let index = index.min(self.items.len().checked_sub(1)?);
        (0..=index)
            .rev()
            .chain(index + 1..self.items.len())
            .find(|&i| self.is_selectable(i))
    }

    /// Item index shown on flattened line `line`.
    #[must_use]
    pub fn item_at_line(&self, line: usize) -> Option<usize> {
        self.line_item.get(line).copied()
    }

    /// The flattened lines of item `index` that hold its content, without the
    /// rule or blank gap it opens with and the gap it ends with, so a
    /// highlight covers the block and not its surroundings.
    #[must_use]
    pub fn content_lines_of(&self, index: usize) -> Option<(usize, usize)> {
        let first = self.first_line_of(index)?;
        let mut last = first;
        while self.item_at_line(last + 1) == Some(index) {
            last += 1;
        }
        let (lead, trail) = match self.items.get(index)? {
            Item::User { .. } => (1, 1),
            Item::System { .. } => (1, 0),
            Item::Assistant { .. } => (1, usize::from(self.gap_after(index))),
            Item::Thinking { .. }
            | Item::Tool { .. }
            | Item::Notice { .. }
            | Item::RunEnd { .. } => (0, 0),
        };
        let (first, last) = (first + lead, last.saturating_sub(trail));
        (first <= last).then_some((first, last))
    }

    /// The first flattened line of item `index`, for scrolling it into view.
    #[must_use]
    pub fn first_line_of(&self, index: usize) -> Option<usize> {
        self.line_item.iter().position(|&i| i == index)
    }

    /// Lay out every item at `width` (re-rendering only what changed) and
    /// return the flattened lines.
    pub fn lines(&mut self, width: u16, colors: &ThemeColors, is_light: bool) -> &[Line<'static>] {
        let width = width.max(1);
        for index in 0..self.items.len() {
            let stale = match &self.cache[index] {
                Some(cached) => cached.width != width || cached.is_light != is_light,
                None => true,
            };
            if stale {
                let (lines, foldable, links, copy) = render_item(
                    &self.items[index],
                    self.collapsed[index],
                    self.fold,
                    width,
                    colors,
                    is_light,
                );
                self.cache[index] = Some(Cached {
                    width,
                    is_light,
                    foldable,
                    lines,
                    links,
                    copy,
                });
                self.flat_dirty = true;
            }
        }
        if self.flat_dirty {
            self.flat.clear();
            self.line_item.clear();
            self.flat_copy.clear();
            self.flat_links.clear();
            for (index, cached) in self.cache.iter().enumerate() {
                if let Some(cached) = cached {
                    let base = self.flat.len();
                    self.flat_links.extend(cached.links.iter().map(|link| {
                        let line = base + link.line;
                        (
                            index,
                            RowLink {
                                line,
                                ..link.clone()
                            },
                        )
                    }));
                    self.flat.extend(cached.lines.iter().cloned());
                    self.flat_copy.extend(cached.copy.iter().copied());
                    self.line_item
                        .extend(std::iter::repeat_n(index, cached.lines.len()));
                    if !cached.lines.is_empty() && self.gap_after(index) {
                        self.flat.push(Line::default());
                        self.flat_copy.push(RowCopy::default());
                        self.line_item.push(index);
                    }
                }
            }
            for line in &self.live_footer {
                self.flat.push(line.clone());
                self.flat_copy.push(RowCopy::default());
                self.line_item.push(self.items.len().saturating_sub(1));
            }
            self.flat_dirty = false;
        }
        &self.flat
    }

    /// Whether a blank line follows item `index`: an answer is set apart
    /// from the step after it, unless that step opens with a gap or rule of
    /// its own, or is the closing line of the run the answer ended.
    fn gap_after(&self, index: usize) -> bool {
        matches!(self.items[index], Item::Assistant { .. })
            && matches!(
                self.items.get(index + 1),
                Some(Item::Thinking { .. } | Item::Tool { .. } | Item::Notice { .. })
            )
    }

    /// The marked-up link under display column `col` of flattened line
    /// `line`, with every region of it — a link wrapped over several lines
    /// is lit whole.
    #[must_use]
    pub(crate) fn link_at(&self, line: usize, col: usize) -> Option<(String, Vec<LinkRegion>)> {
        let (item, hit) = self
            .flat_links
            .iter()
            .find(|(_, link)| link.line == line && (link.start..link.end).contains(&col))?;
        let regions = self
            .flat_links
            .iter()
            .filter(|(i, link)| i == item && link.id == hit.id)
            .map(|(_, link)| (link.line, link.start, link.end))
            .collect();
        Some((hit.href.clone(), regions))
    }

    /// The flattened lines as last laid out by [`Transcript::lines`].
    #[must_use]
    pub fn rendered(&self) -> &[Line<'static>] {
        &self.flat
    }

    /// How each of [`Transcript::rendered`]'s lines reads when copied.
    #[must_use]
    pub(crate) fn rendered_copy(&self) -> &[RowCopy] {
        &self.flat_copy
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.flat.len()
    }
}

/// Display width of `s`, honouring wide and emoji cells.
fn width_of(s: &str) -> usize {
    termide_ui::str_display_width(s)
}

/// A right-aligned meta line: `spans` pushed to the right edge (one column
/// short of the scrollbar gutter), the rest padded with spaces.
pub(crate) fn right_meta(width: u16, spans: Vec<Span<'static>>) -> Line<'static> {
    let content: usize = spans.iter().map(|s| width_of(&s.content)).sum();
    let pad = (width as usize).saturating_sub(content + 1);
    let mut out = Vec::with_capacity(spans.len() + 1);
    out.push(Span::raw(" ".repeat(pad)));
    out.extend(spans);
    Line::from(out)
}

/// A dim dashed rule drawn above a block to set it apart from the one before.
pub(crate) fn separator(width: u16, colors: &ThemeColors) -> Line<'static> {
    Line::styled(
        "╌".repeat(width.saturating_sub(1) as usize),
        Style::default().fg(colors.disabled),
    )
}

/// The prefill (`⏫`) and generation (`✍️`) indicator lines: each phase's
/// duration, tokens and average speed. Shown on whichever block owns the turn's
/// cost — the reasoning block, or the answer when a turn does not reason.
fn cost_lines(width: u16, cost: &Cost, colors: &ThemeColors) -> Vec<Line<'static>> {
    let dim = Style::default().fg(colors.disabled);
    vec![
        right_meta(
            width,
            vec![Span::styled(
                format!(
                    "⏫ {} (↑{}, {})",
                    fmt_dur(cost.prefill_ms),
                    crate::format_tokens(cost.input),
                    fmt_speed(cost.input, cost.prefill_ms)
                ),
                dim,
            )],
        ),
        right_meta(
            width,
            vec![Span::styled(
                format!(
                    "✍\u{fe0f} {} (↓{}, {})",
                    fmt_dur(cost.gen_ms),
                    crate::format_tokens(cost.output),
                    fmt_speed(cost.output, cost.gen_ms)
                ),
                dim,
            )],
        ),
    ]
}

/// The wall-clock time + status meta, shown on the user's message and the
/// final answer (the two `›` message blocks): at the right end of the text's
/// last row when it fits there, else on a row of its own.
fn push_time_meta(
    lines: &mut Vec<Line<'static>>,
    width: u16,
    at: &str,
    ok: bool,
    color: ratatui::style::Color,
    colors: &ThemeColors,
) {
    let meta = vec![
        Span::styled(format!("{at} "), Style::default().fg(color)),
        status_span(ok, colors),
    ];
    let meta_w: usize = meta.iter().map(|s| width_of(&s.content)).sum();
    if let Some(last) = lines.last_mut() {
        let used: usize = last.spans.iter().map(|s| width_of(&s.content)).sum();
        // A space before the meta and the scrollbar gutter after it.
        if used > 0 && used + 1 + meta_w < width as usize {
            let pad = width as usize - used - meta_w - 1;
            last.spans.push(Span::raw(" ".repeat(pad)));
            last.spans.extend(meta);
            return;
        }
    }
    lines.push(right_meta(width, meta));
}

/// A duration in whole seconds with localized units, as its two largest
/// parts: `2s` under a minute, `1m13s` under an hour, `2h5m` under a day,
/// `3d4h` beyond. Tenths add no useful information here.
/// What a subagent spent, as the session's totals show tokens.
fn spent_label(spent: &termide_agent_core::Usage) -> String {
    crate::token_label(spent.uncached(), spent.cache_read, spent.output)
}

pub(crate) fn fmt_dur(ms: u32) -> String {
    let t = termide_i18n::t();
    let total = (u64::from(ms) + 500) / 1000;
    let (mins, secs) = (total / 60, total % 60);
    let (hours, mins_left) = (mins / 60, mins % 60);
    let (days, hours_left) = (hours / 24, hours % 24);
    if mins == 0 {
        format!("{secs}{}", t.agent_unit_secs())
    } else if hours == 0 {
        format!("{mins}{}{secs}{}", t.agent_unit_mins(), t.agent_unit_secs())
    } else if days == 0 {
        format!(
            "{hours}{}{mins_left}{}",
            t.agent_unit_hours(),
            t.agent_unit_mins()
        )
    } else {
        format!(
            "{days}{}{hours_left}{}",
            t.agent_unit_days(),
            t.agent_unit_hours()
        )
    }
}

/// Average token throughput for a phase, localized (e.g. `88 tok/s`).
pub(crate) fn fmt_speed(tokens: u64, ms: u32) -> String {
    let t = termide_i18n::t();
    let secs = (ms as f32 / 1000.0).max(0.001);
    let rate = (tokens as f32 / secs).round() as u64;
    format!(
        "{} {}",
        crate::format_tokens(rate),
        t.agent_unit_tok_per_sec()
    )
}

/// Wrap plain text to `width` with a single style, using the rich-text builder
/// so long lines fold instead of being clipped at draw time. Each line of
/// `text` starts a row of its own and a blank line stays blank, so the text
/// keeps the shape it was written in rather than reflowing into one paragraph.
fn wrap_plain(
    text: &str,
    width: u16,
    style: Style,
    colors: &ThemeColors,
    is_light: bool,
) -> Vec<Line<'static>> {
    let mut builder = Builder::new(width, colors, is_light);
    builder.push_style(style);
    for line in text.lines() {
        if line.trim().is_empty() {
            builder.blank();
        } else {
            builder.text(line);
            builder.hard_break();
        }
    }
    builder.pop_style();
    builder.end_paragraph();
    builder.finish().lines
}

/// The user's own text after `lead` (the `›` mark, or an indent), keeping its
/// line breaks: [`wrap_plain`] two columns narrower, with continuation rows
/// indented under the text. Blank rows stay empty.
fn prompt_rows(
    text: &str,
    lead: Span<'static>,
    width: u16,
    style: Style,
    colors: &ThemeColors,
    is_light: bool,
) -> Vec<Line<'static>> {
    let mut rows = wrap_plain(text, width.saturating_sub(2), style, colors, is_light);
    for (i, row) in rows.iter_mut().enumerate() {
        if i == 0 {
            row.spans.insert(0, lead.clone());
        } else if !row.spans.is_empty() {
            row.spans.insert(0, Span::raw("  "));
        }
    }
    rows
}

/// The `✓`/`✗` status glyph for a finished block.
fn status_span(ok: bool, colors: &ThemeColors) -> Span<'static> {
    if ok {
        Span::styled("✓", Style::default().fg(colors.success))
    } else {
        Span::styled("✗", Style::default().fg(colors.error))
    }
}

/// Fill each line's full width with `bg` (used for the user message's faint
/// background), so the tint covers the whole row, not just the text.
fn fill_bg(lines: &mut [Line<'static>], width: u16, bg: ratatui::style::Color) {
    for line in lines.iter_mut() {
        let w: usize = line.spans.iter().map(|s| width_of(&s.content)).sum();
        for span in &mut line.spans {
            span.style = span.style.bg(bg);
        }
        let pad = (width as usize).saturating_sub(w);
        if pad > 0 {
            line.spans
                .push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
        }
    }
}

/// The command of a shell call, whose headline wraps and folds instead of
/// being a single clipped line. A `suggest_command` offer counts: `[Run]`
/// sends it through the same shell, its output coming back as the call's.
fn shell_command(call: &ToolCall) -> Option<String> {
    matches!(call.name.as_str(), "bash" | "shell" | "suggest_command").then(|| {
        call.arguments
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .replace('\t', "    ")
    })
}

/// The color of a tool call's type glyph and action: the accent while it
/// runs, then success or error by how it ended. Only the prefix carries it;
/// the subject keeps its own style.
fn tool_state_color(running: bool, ok: bool, colors: &ThemeColors) -> ratatui::style::Color {
    if running {
        colors.info
    } else if ok {
        colors.success
    } else {
        colors.error
    }
}

/// A shell call's type glyph and its localized action, in the call's state
/// color: `$ Running `, or `$ Suggesting ` for a command offered for the user
/// to run.
fn shell_prefix(call: &ToolCall, state: ratatui::style::Color) -> Vec<Span<'static>> {
    let t = termide_i18n::t();
    let verb = if call.name == "suggest_command" {
        t.agent_tool_suggest()
    } else {
        t.agent_tool_bash()
    };
    let accent = Style::default().fg(state);
    vec![
        Span::styled("$ ", accent),
        Span::styled(format!("{verb} "), accent),
    ]
}

/// The `$ <command>` headline of an unfolded shell call, the fold `marker`
/// (if any) after the `$`, each command line wrapped under the command rather
/// than clipped.
fn command_lines(
    call: &ToolCall,
    command: &str,
    marker: Option<Span<'static>>,
    width: u16,
    state: ratatui::style::Color,
    colors: &ThemeColors,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(colors.disabled);
    let mut prefix = shell_prefix(call, state);
    prefix.extend(marker);
    let indent: usize = prefix.iter().map(|s| width_of(&s.content)).sum();
    let avail = (width as usize).saturating_sub(indent);
    let all: Vec<&str> = command.lines().collect();
    let mut prefix = Some(prefix);
    let mut lines = Vec::new();
    let mut push_rows = |line: &str, lines: &mut Vec<Line<'static>>| {
        for row in wrap_row(line, avail) {
            let mut spans = prefix
                .take()
                .unwrap_or_else(|| vec![Span::raw(" ".repeat(indent))]);
            spans.push(Span::styled(row, dim));
            lines.push(Line::from(spans));
        }
    };
    if all.is_empty() {
        push_rows("", &mut lines);
    }
    for line in &all {
        push_rows(line, &mut lines);
    }
    lines
}

/// `spans` cut to at most `max` display columns, the last kept one ending in
/// `…` when anything was dropped.
fn truncate_spans(spans: Vec<Span<'static>>, max: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|s| width_of(&s.content)).sum();
    if total <= max {
        return spans;
    }
    let mut room = max.saturating_sub(1);
    let mut out = Vec::new();
    for span in spans {
        let w = width_of(&span.content);
        if w <= room {
            room -= w;
            out.push(span);
            continue;
        }
        let cut = termide_ui::path_utils::truncate_to_width_str(&span.content, room).to_string();
        out.push(Span::styled(format!("{cut}…"), span.style));
        break;
    }
    out
}

/// A headline row with a right-aligned `meta` at its end. When both do not fit
/// the width, `clip` cuts the headline to make room, as a folded tool call's
/// single line does; otherwise the meta drops to a row of its own. A row too
/// narrow to leave a useful headline beside the meta also gives it its own.
fn row_with_meta(
    head: Vec<Span<'static>>,
    meta: Vec<Span<'static>>,
    clip: bool,
    width: u16,
) -> Vec<Line<'static>> {
    /// Columns of headline worth keeping beside the meta when clipping.
    const MIN_HEAD: usize = 12;
    let width = width as usize;
    let head_w: usize = head.iter().map(|s| width_of(&s.content)).sum();
    let meta_w: usize = meta.iter().map(|s| width_of(&s.content)).sum();
    // One space between the two and the scrollbar gutter at the edge.
    let room = width.saturating_sub(meta_w + 2);
    if meta.is_empty() {
        let head = if clip {
            truncate_spans(head, width.saturating_sub(1))
        } else {
            head
        };
        return vec![Line::from(head)];
    }
    if head_w <= room || (clip && room >= MIN_HEAD) {
        let mut spans = truncate_spans(head, room);
        let used: usize = spans.iter().map(|s| width_of(&s.content)).sum();
        spans.push(Span::raw(
            " ".repeat(width.saturating_sub(used + meta_w + 1)),
        ));
        spans.extend(meta);
        return vec![Line::from(spans)];
    }
    let head = if clip {
        truncate_spans(head, width.saturating_sub(1))
    } else {
        head
    };
    vec![Line::from(head), right_meta(width as u16, meta)]
}

/// A tool headline whose subject (its last span) wraps under the prefix — the
/// glyph, the action and the fold marker — instead of being clipped, with
/// `meta` at the last row's end when it fits there, else on a row of its own.
/// A row too narrow to leave a useful subject beside the prefix clips it.
/// Each subject row's `(row, start, end)` columns go to `subject_rows`.
fn wrapped_row_with_meta(
    mut head: Vec<Span<'static>>,
    meta: Vec<Span<'static>>,
    width: u16,
    subject_rows: &mut Vec<(usize, usize, usize)>,
) -> Vec<Line<'static>> {
    /// Columns of subject worth wrapping into; narrower, the headline clips.
    const MIN_SUBJECT: usize = 12;
    let Some(subject) = head.pop() else {
        return row_with_meta(head, meta, true, width);
    };
    let indent: usize = head.iter().map(|s| width_of(&s.content)).sum();
    // The scrollbar gutter at the edge.
    let avail = (width as usize).saturating_sub(indent + 1);
    if avail < MIN_SUBJECT {
        let end = (indent + width_of(&subject.content)).min(width as usize);
        subject_rows.push((0, indent, end));
        head.push(subject);
        return row_with_meta(head, meta, true, width);
    }
    let mut prefix = Some(head);
    let mut lines: Vec<Line<'static>> = wrap_row(&subject.content, avail)
        .into_iter()
        .enumerate()
        .map(|(i, row)| {
            let mut spans = prefix
                .take()
                .unwrap_or_else(|| vec![Span::raw(" ".repeat(indent))]);
            let row = row.trim_end().to_string();
            subject_rows.push((i, indent, indent + width_of(&row)));
            spans.push(Span::styled(row, subject.style));
            Line::from(spans)
        })
        .collect();
    if !meta.is_empty() {
        let last = lines.pop().unwrap_or_default();
        lines.extend(row_with_meta(last.spans, meta, false, width));
    }
    lines
}

/// What a tool call's headline subject links to: the file a read, write or
/// edit touched, the page a fetch loaded.
fn headline_href(call: &ToolCall) -> Option<String> {
    let key = match call.name.as_str() {
        "read" | "write" | "edit" => "path",
        "fetch" => "url",
        _ => return None,
    };
    let href = call.arguments.get(key)?.as_str()?.trim();
    (!href.is_empty()).then(|| href.to_string())
}

/// The links of a tool headline: its subject's rows, when the subject is a
/// path or a URL.
fn headline_links(call: &ToolCall, subject_rows: Vec<(usize, usize, usize)>) -> Vec<RowLink> {
    let Some(href) = headline_href(call) else {
        return Vec::new();
    };
    subject_rows
        .into_iter()
        .filter(|(_, start, end)| start < end)
        .map(|(line, start, end)| RowLink {
            line,
            start,
            end,
            href: href.clone(),
            id: 0,
        })
        .collect()
}

/// Split `line` into rows no wider than `width` columns, breaking after the
/// last space in reach and mid-word only when a row has none. Spaces are kept,
/// so a command reads exactly as it was run.
fn wrap_row(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_w = 0;
    // Byte offset just past the row's last space, where a break is clean.
    let mut last_space: Option<usize> = None;
    for ch in line.chars() {
        let cw = width_of(ch.encode_utf8(&mut [0; 4]));
        if row_w + cw > width && !row.is_empty() {
            let rest = last_space.map_or_else(String::new, |cut| row.split_off(cut));
            rows.push(std::mem::replace(&mut row, rest));
            row_w = width_of(&row);
            last_space = None;
        }
        row.push(ch);
        row_w += cw;
        if ch == ' ' {
            last_space = Some(row.len());
        }
    }
    rows.push(row);
    rows
}

/// How a line of an edit's result is painted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffKind {
    /// The summary sentence before the diff.
    Summary,
    /// A `---`/`+++` file header.
    File,
    /// A `@@` hunk header or a `\ No newline` note.
    Hunk,
    Added,
    Removed,
    Context,
}

/// Classify every line of an edit's result. A `---`/`+++` line is a file
/// header only before the first hunk; inside one it is a removed or added line
/// whose text happens to start with dashes or pluses.
fn diff_kinds(lines: &[&str]) -> Vec<DiffKind> {
    let mut in_diff = false;
    let mut in_hunk = false;
    lines
        .iter()
        .map(|line| {
            if line.starts_with("@@") {
                in_diff = true;
                in_hunk = true;
                return DiffKind::Hunk;
            }
            if !in_hunk && (line.starts_with("--- ") || line.starts_with("+++ ")) {
                in_diff = true;
                return DiffKind::File;
            }
            if !in_hunk {
                return if in_diff {
                    DiffKind::File
                } else {
                    DiffKind::Summary
                };
            }
            match line.as_bytes().first() {
                Some(b'+') => DiffKind::Added,
                Some(b'-') => DiffKind::Removed,
                Some(b'\\') => DiffKind::Hunk,
                _ => DiffKind::Context,
            }
        })
        .collect()
}

/// What an edit's block shows in place of the tool's text, drawn from the
/// result's details (for the UI, never the model): the hunks of the full diff
/// — the headline already names the file, so the summary and the `---`/`+++`
/// header are left to Copy and Open — and the added/removed line counts for
/// the headline. `None` without details (a failed edit), so the text shows.
struct EditView {
    hunks: String,
    added: usize,
    removed: usize,
    /// The match was not exact (whitespace differed), worth a note.
    loose: bool,
}

fn edit_view(result: &ToolResultMessage) -> Option<EditView> {
    let details = result.details.as_ref()?;
    let diff = details.get("diff")?.as_str()?;
    let start = diff
        .find("\n@@")
        .map(|i| i + 1)
        .or_else(|| diff.starts_with("@@").then_some(0))?;
    let hunks = diff[start..].trim_end().to_string();
    let (mut added, mut removed) = (0, 0);
    for line in hunks.lines() {
        match line.as_bytes().first() {
            Some(b'+') => added += 1,
            Some(b'-') => removed += 1,
            _ => {}
        }
    }
    let loose = details
        .get("strategy")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    Some(EditView {
        hunks,
        added,
        removed,
        loose,
    })
}

/// One indented line of tool output: dim, or — for an edit's diff — colored
/// the way the git diff panel colors it, an added or removed line tinted
/// across the whole row.
fn output_line(
    line: &str,
    kind: Option<DiffKind>,
    width: u16,
    colors: &ThemeColors,
) -> Line<'static> {
    let style = match kind {
        None | Some(DiffKind::Summary | DiffKind::Hunk) => Style::default().fg(colors.disabled),
        Some(DiffKind::File) => Style::default().fg(colors.info),
        Some(DiffKind::Context) => Style::default().fg(colors.fg),
        Some(DiffKind::Added) => Style::default()
            .fg(colors.success)
            .bg(termide_ui::diff_line_bg(colors.success, colors.bg)),
        Some(DiffKind::Removed) => Style::default()
            .fg(colors.error)
            .bg(termide_ui::diff_line_bg(colors.error, colors.bg)),
    };
    let mut spans = vec![Span::raw("  "), Span::styled(line.to_string(), style)];
    if style.bg.is_some() {
        let pad = (width as usize).saturating_sub(2 + width_of(line));
        spans.push(Span::styled(" ".repeat(pad), style));
    }
    Line::from(spans)
}

/// One line of prose tool output, dim and indented like [`output_line`] but
/// wrapped to `width`; a `- ` list item continues under its text.
fn prose_output_lines(line: &str, width: u16, colors: &ThemeColors) -> Vec<Line<'static>> {
    let style = Style::default().fg(colors.disabled);
    let hang = if line.starts_with("- ") { 4 } else { 2 };
    let first = (width as usize).saturating_sub(2);
    let rest = (width as usize).saturating_sub(hang);
    let mut rows = wrap_row(line, first).into_iter();
    let mut out = Vec::new();
    if let Some(row) = rows.next() {
        out.push(Line::from(vec![Span::raw("  "), Span::styled(row, style)]));
    }
    // Continuations re-wrap to the narrower room left by the hanging indent.
    let tail: String = rows.collect();
    if !tail.is_empty() {
        for row in wrap_row(&tail, rest) {
            out.push(Line::from(vec![
                Span::raw(" ".repeat(hang)),
                Span::styled(row, style),
            ]));
        }
    }
    out
}

/// The line that says who decided a call, or `None` when the rules allowed
/// it without asking anyone — the common case, left unsaid.
fn permission_text(note: &PermissionNote) -> Option<String> {
    let t = termide_i18n::t();
    let text = match (note.by, note.allowed) {
        (DecidedBy::Rules, true) => return None,
        (DecidedBy::Rules, false) => t.agent_perm_note_rules_denied().to_string(),
        (DecidedBy::Plan, _) => t.agent_perm_note_plan().to_string(),
        (DecidedBy::Hook, true) => t.agent_perm_note_hook_allowed().to_string(),
        (DecidedBy::Hook, false) => t.agent_perm_note_hook_denied().to_string(),
        (DecidedBy::Reviewer, true) => t.agent_perm_note_reviewer_allowed_fmt(&note.reason),
        (DecidedBy::Reviewer, false) => t.agent_perm_note_reviewer_blocked_fmt(&note.reason),
        (DecidedBy::Unattended, _) => t.agent_perm_note_unattended().to_string(),
        (DecidedBy::User, true) => match note.lasting {
            Some(Lasting::Session) => t.agent_perm_note_user_session().to_string(),
            Some(Lasting::Project) => t.agent_perm_note_user_project().to_string(),
            Some(Lasting::Global) => t.agent_perm_note_user_global().to_string(),
            _ => t.agent_perm_note_user_once().to_string(),
        },
        (DecidedBy::User, false) if !note.reason.is_empty() => {
            t.agent_perm_note_user_denied_reason_fmt(&note.reason)
        }
        (DecidedBy::User, false) => match note.lasting {
            Some(Lasting::Session) => t.agent_perm_note_user_denied_session().to_string(),
            _ => t.agent_perm_note_user_denied().to_string(),
        },
    };
    let glyph = if note.allowed { "✓" } else { "✗" };
    Some(format!("{glyph} {text}"))
}

/// The first line of a non-shell tool call (a shell's is [`command_lines`]):
/// a type glyph, a localized action and its subject for the file, web, skill,
/// task and MCP tools, else the tool name and a summary. The fold `marker`, if
/// any, follows the action or the name.
fn tool_headline(
    call: &ToolCall,
    marker: Option<Span<'static>>,
    width: u16,
    state: ratatui::style::Color,
    colors: &ThemeColors,
) -> Vec<Span<'static>> {
    let t = termide_i18n::t();
    let fg = Style::default().fg(colors.fg);
    let arg = |key: &str| {
        call.arguments
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    // A tool opens with its type glyph like every block — `<` read and `>`
    // write, as a shell redirects, `±` for an edit's diff, `↓` for a page
    // fetched, `?` for a search, `/` for a skill as it is typed by hand, `&`
    // for a subagent, as a shell backgrounds a job, `*` for an MCP tool, `¿`
    // for a question to the user — then its localized action in the same
    // state color and its subject.
    let accent = Style::default().fg(state);
    let styled_action = |glyph: &str, verb: &str, subject: String, style: Style| {
        let mut spans = vec![
            Span::styled(format!("{glyph} "), accent),
            Span::styled(format!("{verb} "), accent),
        ];
        spans.extend(marker.clone());
        spans.push(Span::styled(subject, style));
        spans
    };
    let action = |glyph: &str, verb: &str, subject: String| styled_action(glyph, verb, subject, fg);
    let with_args = |head: String, args: &str| {
        if args.is_empty() {
            head
        } else {
            format!("{head} {args}")
        }
    };
    match call.name.as_str() {
        "read" => action("<", t.agent_tool_read(), arg("path")),
        "write" => action(">", t.agent_tool_write(), arg("path")),
        "edit" => action("±", t.agent_tool_edit(), arg("path")),
        // A fetched URL is dim, as a link reads beside its text; Ctrl+click
        // opens it in the browser.
        "fetch" => styled_action(
            "↓",
            t.agent_tool_fetch(),
            arg("url"),
            Style::default().fg(colors.disabled),
        ),
        "web_search" => action("?", t.agent_tool_web_search(), arg("query")),
        "recall" => {
            // A reference to open, or the first query and how many more.
            let subject = match call.arguments["open"].as_str() {
                Some(reference) if !reference.trim().is_empty() => reference.trim().to_string(),
                _ => {
                    let queries = call.arguments["queries"].as_array();
                    let first = queries
                        .and_then(|list| list.first())
                        .and_then(|q| q.as_str())
                        .unwrap_or("")
                        .replace('\n', " ");
                    let more = queries.map_or(0, |list| list.len().saturating_sub(1));
                    if more > 0 {
                        format!("{} (+{more})", first.trim())
                    } else {
                        first.trim().to_string()
                    }
                }
            };
            action("↺", t.agent_tool_recall(), subject)
        }
        "skill" => action(
            "/",
            t.agent_tool_skill(),
            with_args(arg("name"), arg("args").trim()),
        ),
        "task" => {
            let prompt = arg("prompt");
            let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
            action(
                "&",
                t.agent_tool_task(),
                format!("{}: {}", arg("agent"), first.trim()),
            )
        }
        "question" => {
            // The first question, and how many more follow it.
            let questions = call.arguments["questions"].as_array();
            let first = questions
                .and_then(|list| list.first())
                .and_then(|q| q["question"].as_str())
                .unwrap_or("")
                .replace('\n', " ");
            let more = questions.map_or(0, |list| list.len().saturating_sub(1));
            let subject = if more > 0 {
                format!("{} (+{more})", first.trim())
            } else {
                first.trim().to_string()
            };
            action("¿", t.agent_tool_question(), subject)
        }
        name if mcp_parts(name).is_some() => {
            let (server, tool) = mcp_parts(name).unwrap_or_default();
            action(
                "*",
                t.agent_tool_mcp(),
                with_args(format!("{server}: {tool}"), &inline_args(&call.arguments)),
            )
        }
        _ => {
            let mut spans = vec![
                Span::styled(
                    call.name.clone(),
                    Style::default().fg(state).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ];
            spans.extend(marker.clone());
            spans.push(Span::styled(
                summarize_call(call, width.saturating_sub(4) as usize),
                fg,
            ));
            spans
        }
    }
}

/// The server and tool of an MCP call: `<server>__<tool>`, as `agent-mcp`
/// names the tools it brings in, or `mcp__<server>__<tool>`, as an external
/// agent reports one. No built-in tool has `__` in its name; a server whose
/// own name does is split at its first `__`.
fn mcp_parts(name: &str) -> Option<(&str, &str)> {
    let name = name.strip_prefix("mcp__").unwrap_or(name);
    let (server, tool) = name.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
}

/// A call's arguments as `key=value` pairs on one line, a string bare and
/// anything else as JSON, so a headline reads as a command rather than an
/// object.
fn inline_args(arguments: &Value) -> String {
    let Value::Object(map) = arguments else {
        return String::new();
    };
    map.iter()
        .map(|(key, value)| match value {
            Value::String(text) => format!("{key}={}", text.replace('\n', " ")),
            other => format!("{key}={other}"),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The lines of `item` at `width`, folded when `collapsed` and the item folds,
/// and whether it does. A block whose unfolded form is a single row has
/// nothing to fold, so it shows that row with no fold marker.
fn render_item(
    item: &Item,
    collapsed: bool,
    fold: FoldMode,
    width: u16,
    colors: &ThemeColors,
    is_light: bool,
) -> (Vec<Line<'static>>, bool, Vec<RowLink>, Vec<RowCopy>) {
    let mut foldable = is_foldable(item, fold);
    // Only reasoning can be foldable yet fit one row unfolded: a foldable tool
    // call has output or a second command line below its headline.
    if foldable && matches!(item, Item::Thinking { .. }) {
        foldable = render_body(
            item,
            false,
            false,
            width,
            colors,
            is_light,
            &mut Marks::default(),
        )
        .len()
            > 1;
    }
    let mut marks = Marks::default();
    let mut lines = render_body(
        item,
        collapsed && foldable,
        foldable,
        width,
        colors,
        is_light,
        &mut marks,
    );
    let Marks {
        mut links,
        copy: copy_marks,
    } = marks;
    let mut copy = vec![RowCopy::default(); lines.len()];
    for (line, row) in copy_marks {
        if let Some(slot) = copy.get_mut(line) {
            *slot = row;
        }
    }
    // The answer draws no dividing rule; a blank line above sets it apart
    // from the steps before it. Reasoning and tool calls stack with no gap.
    if matches!(item, Item::Assistant { .. }) && !lines.is_empty() {
        lines.insert(0, Line::default());
        copy.insert(0, RowCopy::default());
        for link in &mut links {
            link.line += 1;
        }
    }
    (lines, foldable, links, copy)
}

/// What [`render_body`] marks on the lines it returns: the links, and how
/// rows read when copied (a row not listed is a line of its own).
#[derive(Default)]
struct Marks {
    links: Vec<RowLink>,
    copy: Vec<(usize, RowCopy)>,
}

/// The lines of `item` itself, without the gap [`render_item`] puts above it;
/// `foldable` decides whether it carries a fold marker. What it marks up goes
/// to `marks`.
fn render_body(
    item: &Item,
    collapsed: bool,
    foldable: bool,
    width: u16,
    colors: &ThemeColors,
    is_light: bool,
    marks: &mut Marks,
) -> Vec<Line<'static>> {
    let t = termide_i18n::t();
    let dim = Style::default().fg(colors.disabled);
    match item {
        Item::User { text, at, command } => {
            let trimmed = text.trim();
            let all: Vec<&str> = trimmed.lines().collect();
            let mark = Style::default()
                .fg(colors.info)
                .add_modifier(Modifier::BOLD);
            let bold = Style::default().add_modifier(Modifier::BOLD);
            // The plate is a blank line, the text, the time, and a blank line —
            // all on the faint background, with an even margin around them.
            let mut plate = vec![Line::default()];
            if let Some(command) = command {
                // Headed by the command as typed; what it expanded to — what
                // the model got — folds to a line count under it.
                let mut head = Builder::new(width, colors, is_light);
                head.styled("› ", mark);
                head.push_style(bold);
                head.text(command.trim());
                head.pop_style();
                head.end_paragraph();
                plate.extend(head.finish().lines);
                if collapsed {
                    plate.push(Line::styled(
                        format!("  {}", t.agent_more_lines(all.len())),
                        Style::default().fg(colors.fg),
                    ));
                } else {
                    // Indented under the command, keeping the text's own lines.
                    let body = wrap_plain(
                        trimmed,
                        width.saturating_sub(2),
                        Style::default().fg(colors.fg),
                        colors,
                        is_light,
                    );
                    plate.extend(body.into_iter().map(|mut line| {
                        line.spans.insert(0, Span::raw("  "));
                        line
                    }));
                }
            } else if collapsed && all.len() > USER_PREVIEW_LINES {
                // A long paste folds to its first line, a "… N more lines"
                // marker, then the last few — the ellipsis between first and
                // last, like every other block. The model still gets the whole
                // message; this is only the transcript.
                let (hidden, tail_start) = fold_split(all.len());
                plate.extend(prompt_rows(
                    &all[..FOLD_HEAD_LINES].join("\n"),
                    Span::styled("› ", mark),
                    width,
                    bold,
                    colors,
                    is_light,
                ));
                plate.push(Line::styled(
                    format!("  {}", t.agent_more_lines(hidden)),
                    Style::default().fg(colors.fg),
                ));
                plate.extend(prompt_rows(
                    &all[tail_start..].join("\n"),
                    Span::raw("  "),
                    width,
                    bold,
                    colors,
                    is_light,
                ));
            } else {
                plate.extend(prompt_rows(
                    trimmed,
                    Span::styled("› ", mark),
                    width,
                    bold,
                    colors,
                    is_light,
                ));
            }
            if !at.is_empty() {
                push_time_meta(&mut plate, width, at, true, colors.fg, colors);
            }
            plate.push(Line::default());
            fill_bg(&mut plate, width, colors.disabled);
            // A plain gap above the plate keeps it off the block before it (the
            // last answer's time), since the user block has no leading rule.
            // A plain gap below it sets the steps that follow apart too.
            let mut content = vec![Line::default()];
            content.append(&mut plate);
            content.push(Line::default());
            content
        }
        Item::System { text } => {
            // The system prompt, marked with `#`, dim throughout — it is
            // reference, not a step. It folds like a tool: collapsed shows the
            // first few lines and a skipped-line note, behind a `▸`; expanded
            // shows a `▾` and wraps the whole thing.
            let prompt = text.trim();
            let all: Vec<&str> = prompt.lines().collect();
            // The first line carries the `#` and the fold marker (when foldable);
            // continuation lines indent to match.
            let head = |first: bool| -> Vec<Span<'static>> {
                if !first {
                    return vec![Span::raw("  ")];
                }
                let mut spans = vec![Span::styled("# ", dim)];
                if foldable {
                    spans.push(Span::styled(if collapsed { "▸ " } else { "▾ " }, dim));
                }
                spans
            };
            let mut lines: Vec<Line<'static>> = Vec::new();
            if foldable && collapsed {
                let (hidden, tail_start) = fold_split(all.len());
                for (i, line) in all[..FOLD_HEAD_LINES].iter().enumerate() {
                    let mut spans = head(i == 0);
                    spans.push(Span::styled((*line).to_string(), dim));
                    lines.push(Line::from(spans));
                }
                lines.push(Line::styled(
                    format!("  {}", t.agent_more_lines(hidden)),
                    dim,
                ));
                for line in &all[tail_start..] {
                    let mut spans = head(false);
                    spans.push(Span::styled((*line).to_string(), dim));
                    lines.push(Line::from(spans));
                }
            } else {
                let mut body = wrap_plain(prompt, width.saturating_sub(2), dim, colors, is_light);
                for (i, line) in body.iter_mut().enumerate() {
                    for span in head(i == 0).into_iter().rev() {
                        line.spans.insert(0, span);
                    }
                }
                lines.append(&mut body);
            }
            let mut framed = vec![separator(width, colors)];
            framed.append(&mut lines);
            framed
        }
        Item::Thinking {
            text,
            cost,
            streaming,
            ..
        } => {
            // Reasoning is its own dim block, marked with `@`: the accent while
            // it streams, like a running tool, then dim like the reasoning.
            // Folded, a finished block is its first line alone with the turn's
            // cost at the row's end; unfolded (and always while it streams)
            // the whole text wraps under the marker.
            let accent = Style::default().fg(if *streaming {
                colors.info
            } else {
                colors.disabled
            });
            let reasoning = text.trim();
            if reasoning.is_empty() {
                return Vec::new();
            }
            // The `@` glyph and the block's localized action, like a tool's.
            let mut head = vec![
                Span::styled("@ ", accent),
                Span::styled(format!("{} ", t.agent_thinking()), accent),
            ];
            if foldable {
                head.push(Span::styled(if collapsed { "▸ " } else { "▾ " }, dim));
            }
            if collapsed {
                // Still streaming, the headline follows the reasoning: its
                // latest line, not the first.
                let line = if *streaming {
                    reasoning.lines().rev().find(|l| !l.trim().is_empty())
                } else {
                    reasoning.lines().next()
                };
                head.push(Span::styled(line.unwrap_or("").to_string(), dim));
                // Folded, the headline stands alone: the prefill and generation
                // lines are one unfold away.
                return row_with_meta(head, Vec::new(), true, width);
            }
            let indent: usize = head.iter().map(|s| width_of(&s.content)).sum();
            let mut lines = wrap_plain(
                reasoning,
                width.saturating_sub(indent as u16),
                dim,
                colors,
                is_light,
            );
            let mut head = Some(head);
            for line in &mut lines {
                let prefix = head
                    .take()
                    .unwrap_or_else(|| vec![Span::raw(" ".repeat(indent))]);
                for span in prefix.into_iter().rev() {
                    line.spans.insert(0, span);
                }
            }
            // The reasoning block shows only the prefill/generation indicators
            // (no wall-clock time — that belongs to the answer).
            if let Some(cost) = cost {
                lines.extend(cost_lines(width, cost, colors));
            }
            lines
        }
        Item::Assistant {
            text,
            error,
            at,
            cost,
            run_ms,
            ..
        } => {
            // The answer, marked like the user's message with an accent `›`,
            // shown in full and trimmed of the stray blank lines models lead
            // with. While it is still empty the live spinner stands in for it.
            let mark = Style::default()
                .fg(colors.info)
                .add_modifier(Modifier::BOLD);
            let mut lines: Vec<Line<'static>> = Vec::new();
            let answer = text.trim();
            if !answer.is_empty() {
                // Reserve the marker's width and indent wrapped lines under it,
                // so the accent `›` never pushes the first line over the edge.
                let body = render_markdown(answer, width.saturating_sub(2), colors, is_light);
                // Past the `› ` mark the answer's links sit two columns in.
                // Copied, the mark and the indent under it are decoration.
                marks
                    .copy
                    .extend(body.copy.iter().enumerate().map(|(i, row)| {
                        (
                            lines.len() + i,
                            RowCopy {
                                lead: row.lead + 2,
                                ..*row
                            },
                        )
                    }));
                marks
                    .links
                    .extend(body.links.into_iter().map(|link| RowLink {
                        line: lines.len() + link.line,
                        start: link.start as usize + 2,
                        end: link.end as usize + 2,
                        href: link.url,
                        id: link.id,
                    }));
                for (i, mut line) in body.lines.into_iter().enumerate() {
                    line.spans.insert(
                        0,
                        if i == 0 {
                            Span::styled("› ", mark)
                        } else {
                            Span::raw("  ")
                        },
                    );
                    lines.push(line);
                }
            }
            if let Some(error) = error {
                // Wrap the error under a `✗` marker, indenting continuation
                // lines, so a long message reflows instead of being clipped.
                let style = Style::default().fg(colors.error);
                let mut body = wrap_plain(error, width.saturating_sub(2), style, colors, is_light);
                for (i, line) in body.iter_mut().enumerate() {
                    line.spans.insert(
                        0,
                        if i == 0 {
                            Span::styled("✗ ", style)
                        } else {
                            Span::raw("  ")
                        },
                    );
                }
                lines.append(&mut body);
            }
            // An empty answer draws nothing at all — a bare turn (reasoning or
            // tools only) has no final message, and while streaming the live
            // spinner stands in for it.
            if lines.is_empty() {
                return lines;
            }
            // The answer carries the wall-clock time and status, like the user's
            // message; the prefill/generation indicators appear here only when a
            // turn does not reason (otherwise the reasoning block holds them).
            if !at.is_empty() {
                push_time_meta(
                    &mut lines,
                    width,
                    at,
                    error.is_none(),
                    colors.disabled,
                    colors,
                );
                if let Some(cost) = cost {
                    lines.extend(cost_lines(width, cost, colors));
                }
            }
            // The run's total from the request, marked `✻` so it does not read
            // as one more block's own `🕒`.
            if let Some(ms) = run_ms {
                lines.push(right_meta(
                    width,
                    // The clock at rest, dimmed apart from the animated one.
                    vec![Span::styled(format!("{RUN_GLYPH} {}", fmt_dur(*ms)), dim)],
                ));
            }
            lines
        }
        Item::Tool {
            call,
            result,
            live,
            at,
            duration_ms,
            waited_ms,
            waiting,
        } => {
            let running = is_live(item);
            let edit = result
                .as_ref()
                .filter(|_| call.name == "edit")
                .and_then(edit_view);
            let body = match (&edit, result, live) {
                (Some(edit), _, _) => edit.hunks.clone(),
                (None, Some(result), _) => result.plain_text(),
                (None, None, Some(live)) => live.clone(),
                (None, None, None) => String::new(),
            };
            // Trim the stray blank lines a command's output ends with, so the
            // padding stays even.
            let all: Vec<&str> = body.trim().lines().collect();
            // An edit's line counts stand at its headline's end, kept when a
            // long path is clipped.
            let stats: Vec<Span<'static>> = edit
                .as_ref()
                .map(|edit| {
                    let mut spans = Vec::new();
                    if edit.added > 0 {
                        spans.push(Span::styled(
                            format!("+{}", edit.added),
                            Style::default().fg(colors.success),
                        ));
                    }
                    if edit.removed > 0 {
                        if !spans.is_empty() {
                            spans.push(Span::raw(" "));
                        }
                        spans.push(Span::styled(
                            format!("−{}", edit.removed),
                            Style::default().fg(colors.error),
                        ));
                    }
                    spans
                })
                .unwrap_or_default();
            let marker = foldable.then(|| Span::styled(if collapsed { "▸ " } else { "▾ " }, dim));
            // The meta of a finished call is how long it took (`🕒`, when
            // known), never a wall-clock time. A single-row block carries the
            // `🕒` alone, its prefix's color telling how it ended; a taller one
            // ends with a meta row that adds the status glyph.
            let finished = !at.is_empty();
            let ok = result.as_ref().is_none_or(|r| !r.is_error);
            let state = tool_state_color(running, ok, colors);
            // A wait on a permission answer comes first, as the pause it was
            // (`‖`, marked while the question is up), then the call's own
            // duration.
            let mut clock: Vec<Span<'static>> = Vec::new();
            if let Some(ms) = waited_ms {
                let style = if *waiting {
                    Style::default().fg(colors.warning)
                } else {
                    dim
                };
                clock.push(Span::styled(
                    format!("{PAUSED_GLYPH} {}", fmt_dur(*ms)),
                    style,
                ));
            }
            // What a subagent's model calls spent, as the session's totals
            // show tokens.
            if let Some(spent) = result.as_ref().and_then(|r| r.spent()) {
                if !clock.is_empty() {
                    clock.push(Span::raw(" "));
                }
                clock.push(Span::styled(spent_label(&spent), dim));
            }
            if let Some(ms) = duration_ms {
                if !clock.is_empty() {
                    clock.push(Span::raw(" "));
                }
                clock.push(Span::styled(format!("🕒 {}", fmt_dur(*ms)), dim));
            }
            let one_row = |head: Vec<Span<'static>>, clip: bool| {
                row_with_meta(head, clock.clone(), clip, width)
            };
            if collapsed {
                // Folded, a finished call is its headline alone — a shell
                // call's first command line — and its timing is one unfold
                // away; a live one keeps the meta, where a wait on a
                // permission answer ticks.
                // A shell call's command clips to its first line; any other
                // call's subject wraps, read in full without unfolding.
                let meta = if finished { stats } else { clock };
                return match shell_command(call) {
                    Some(command) => {
                        let mut head = shell_prefix(call, state);
                        head.extend(marker);
                        let first = command.lines().next().unwrap_or("").to_string();
                        head.push(Span::styled(first, dim));
                        row_with_meta(head, meta, true, width)
                    }
                    None => {
                        let mut rows = Vec::new();
                        let lines = wrapped_row_with_meta(
                            tool_headline(call, marker, width, state, colors),
                            meta,
                            width,
                            &mut rows,
                        );
                        marks.links.extend(headline_links(call, rows));
                        lines
                    }
                };
            }
            let mut lines = match shell_command(call) {
                Some(command) => command_lines(call, &command, marker, width, state, colors),
                None => {
                    let mut rows = Vec::new();
                    let lines = wrapped_row_with_meta(
                        tool_headline(call, marker, width, state, colors),
                        stats,
                        width,
                        &mut rows,
                    );
                    marks.links.extend(headline_links(call, rows));
                    lines
                }
            };
            // An edit's result is a unified diff, painted like the git diff
            // panel paints one.
            let kinds = (call.name == "edit").then(|| diff_kinds(&all));
            let kind = |i: usize| kinds.as_ref().map(|k| k[i]);
            // A running call shows the newest output, where progress is; a
            // finished one shows its output from the top.
            let (start, end) = if running {
                (all.len().saturating_sub(EXPANDED_OUTPUT_LINES), all.len())
            } else {
                (0, all.len().min(EXPANDED_OUTPUT_LINES))
            };
            if start > 0 {
                lines.push(Line::styled(
                    format!("  {}", t.agent_more_lines_above(start)),
                    dim,
                ));
            }
            for (i, line) in all.iter().enumerate().take(end).skip(start) {
                if call.name == "question" || kind(i) == Some(DiffKind::Summary) {
                    // The answers, and an edit's text outside a diff (an
                    // error), are prose: they wrap rather than clip.
                    lines.extend(prose_output_lines(line, width, colors));
                } else {
                    lines.push(output_line(line, kind(i), width, colors));
                }
            }
            if end < all.len() {
                lines.push(Line::styled(
                    format!("  {}", t.agent_more_lines(all.len() - end)),
                    dim,
                ));
            }
            if edit.as_ref().is_some_and(|edit| edit.loose) {
                lines.extend(prose_output_lines(
                    t.agent_edit_loose_match(),
                    width,
                    colors,
                ));
            }
            // Who let the call run or refused it, unless the rules simply did;
            // a command the user ran says so, the one thing that sets its
            // block apart from the agent's.
            let note = if is_user_command(call) {
                Some(format!("✓ {}", t.agent_perm_note_user_ran()))
            } else {
                result
                    .as_ref()
                    .and_then(|r| r.permission.as_deref())
                    .and_then(permission_text)
            };
            if let Some(text) = note {
                lines.extend(prose_output_lines(&text, width, colors));
            }
            if !finished && !clock.is_empty() {
                // Still running, the wait is all there is to show.
                if lines.len() == 1 {
                    let row = lines.pop().unwrap_or_default();
                    lines = row_with_meta(row.spans, clock.clone(), false, width);
                } else {
                    lines.push(right_meta(width, clock.clone()));
                }
            }
            if finished {
                // A lone headline row takes the clock at its end, as when folded.
                if lines.len() == 1 {
                    let row = lines.pop().unwrap_or_default();
                    lines = one_row(row.spans, false);
                } else {
                    let mut meta = clock.clone();
                    if !meta.is_empty() {
                        meta.push(Span::raw(" "));
                    }
                    meta.push(status_span(ok, colors));
                    lines.push(right_meta(width, meta));
                }
            }
            lines
        }
        Item::Notice { text, kind } => {
            let (glyph, glyph_color, text_color) = match kind {
                NoticeKind::Info => ("·", colors.info, colors.disabled),
                NoticeKind::Warn => ("!", colors.warning, colors.warning),
                NoticeKind::Error => ("✗", colors.error, colors.error),
            };
            // No rule: a notice marks a moment in the flow, as the steps
            // around it do, and its glyph sets it apart.
            annotation(
                Span::styled(
                    format!("{glyph} "),
                    Style::default()
                        .fg(glyph_color)
                        .add_modifier(Modifier::BOLD),
                ),
                text.trim(),
                Style::default().fg(text_color),
                None,
                width,
                colors,
                is_light,
            )
        }
        Item::RunEnd {
            elapsed_ms,
            at,
            ok,
            paused,
            live,
        } => {
            // The live run clock, frozen where it stood: right-aligned under
            // the last block, with no rule, and the time the run ended. A pause
            // shows as `‖` in place of the resting `✻` and needs no status.
            // Only a pause still on stands out; a resting glyph is dimmed, so
            // it reads apart from the animated one.
            let glyph = if *paused { PAUSED_GLYPH } else { RUN_GLYPH };
            let glyph_style = if *live {
                Style::default()
                    .fg(colors.warning)
                    .add_modifier(Modifier::BOLD)
            } else {
                dim
            };
            let mut spans = vec![
                Span::styled(format!("{glyph} "), glyph_style),
                Span::styled(run_end_text(*elapsed_ms, at), dim),
            ];
            if !*paused || !*ok {
                spans.push(Span::raw(" "));
                spans.push(status_span(*ok, colors));
            }
            vec![right_meta(width, spans)]
        }
    }
}

/// The lines of an annotation: `glyph` then `text` wrapped under it, with an
/// optional trailing status glyph after the last word.
fn annotation(
    glyph: Span<'static>,
    text: &str,
    style: Style,
    status: Option<Span<'static>>,
    width: u16,
    colors: &ThemeColors,
    is_light: bool,
) -> Vec<Line<'static>> {
    let mut builder = Builder::new(width.saturating_sub(2), colors, is_light);
    builder.push_style(style);
    builder.text(text);
    builder.pop_style();
    if let Some(status) = status {
        builder.styled(status.content, status.style);
    }
    builder.end_paragraph();
    let mut lines = builder.finish().lines;
    for (i, line) in lines.iter_mut().enumerate() {
        let lead = if i == 0 {
            glyph.clone()
        } else {
            Span::raw("  ")
        };
        line.spans.insert(0, lead);
    }
    lines
}

/// The mark of a paused run, on its closing line and in the state strip.
pub(crate) const PAUSED_GLYPH: &str = "‖";

/// The frames of the live run clock, swelling and shrinking back; it comes to
/// rest on [`RUN_GLYPH`] when the run ends.
pub(crate) const RUN_FRAMES: [&str; 10] = ["·", "✢", "✳", "✶", "✻", "✽", "✻", "✶", "✳", "✢"];

/// The glyph of a run's total time: the live clock at rest, on the answer that
/// closed the run or on its closing line.
pub(crate) const RUN_GLYPH: &str = "✻";

/// The text of a run's closing line after its glyph: `3m41s · 21:03:41`, or
/// the duration alone for a pause, which carries no time of day.
pub(crate) fn run_end_text(elapsed_ms: u32, at: &str) -> String {
    if at.is_empty() {
        fmt_dur(elapsed_ms)
    } else {
        format!("{} · {at}", fmt_dur(elapsed_ms))
    }
}

/// One-line description of a call's arguments: the command for `bash`, the
/// path for file tools, the URL or query for web tools, compact JSON
/// otherwise.
#[must_use]
pub fn summarize_call(call: &ToolCall, max_chars: usize) -> String {
    let text = match call.name.as_str() {
        "bash" => call.arguments["command"].as_str().unwrap_or("").to_string(),
        "read" | "edit" | "write" => call.arguments["path"].as_str().unwrap_or("").to_string(),
        "fetch" => call.arguments["url"].as_str().unwrap_or("").to_string(),
        "web_search" => call.arguments["query"].as_str().unwrap_or("").to_string(),
        _ => match &call.arguments {
            Value::Object(map) if map.is_empty() => String::new(),
            other => other.to_string(),
        },
    };
    let text = text.replace('\n', " ");
    let max_chars = max_chars.max(8);
    if text.chars().count() > max_chars {
        let cut: String = text.chars().take(max_chars - 1).collect();
        format!("{cut}…")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: name.into(),
            arguments: args,
            extra_content: None,
        }
    }

    fn text_of(lines: &[Line<'_>]) -> Vec<String> {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn skill_task_and_mcp_calls_open_with_a_glyph_and_an_action() {
        let colors = ThemeColors::default();
        let headline = |name: &str, args: Value| -> String {
            tool_headline(&call(name, args), None, 80, colors.info, &colors)
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        };
        assert_eq!(
            headline("skill", json!({ "name": "review", "args": "src/x.rs" })),
            "/ Using skill review src/x.rs"
        );
        assert_eq!(
            headline("skill", json!({ "name": "deploy" })),
            "/ Using skill deploy"
        );
        assert_eq!(
            headline(
                "question",
                json!({ "questions": [{ "question": "Which approach?" }, { "question": "Name?" }] })
            ),
            "¿ Asking Which approach? (+1)"
        );
        assert_eq!(
            headline(
                "task",
                json!({ "agent": "reviewer", "prompt": "\ncheck the diff\nthen report" })
            ),
            "& Delegating to reviewer: check the diff"
        );
        assert_eq!(
            headline(
                "github__create_issue",
                json!({ "title": "Crash", "labels": ["bug"] })
            ),
            "* Using MCP github: create_issue labels=[\"bug\"] title=Crash"
        );
        // As an external agent reports an MCP call.
        assert_eq!(
            headline("mcp__termide__open", json!({})),
            "* Using MCP termide: open"
        );
        // Anything else keeps its name and a summary.
        assert_eq!(headline("lists", json!({})), "lists ");
        assert_eq!(mcp_parts("__x"), None);
        assert_eq!(mcp_parts("web_search"), None);
    }

    #[test]
    fn a_typed_command_heads_the_user_block_over_its_folded_expansion() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::User {
            text: "Review a.rs.\nQuote the lines.".into(),
            at: String::new(),
            command: Some("/review a.rs".into()),
        });
        let folded = text_of(transcript.lines(40, &colors, false)).join("\n");
        assert!(folded.contains("› /review a.rs"), "{folded}");
        assert!(folded.contains("… 2 more lines"), "{folded}");
        assert!(!folded.contains("Quote the lines."), "{folded}");

        transcript.toggle_expanded(0);
        let unfolded = text_of(transcript.lines(40, &colors, false)).join("\n");
        assert!(unfolded.contains("› /review a.rs"), "{unfolded}");
        assert!(unfolded.contains("  Review a.rs."), "{unfolded}");
        assert!(unfolded.contains("  Quote the lines."), "{unfolded}");
    }

    #[test]
    fn a_prompt_keeps_its_line_breaks_in_the_user_block() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::User {
            text: "first line\n\n- one\n- two and a much longer tail".into(),
            at: String::new(),
            command: None,
        });
        let rows: Vec<String> = text_of(transcript.lines(20, &colors, false))
            .into_iter()
            .map(|row| row.trim_end().to_string())
            .collect();
        let text = rows.join("\n");
        assert!(
            text.contains("› first line\n\n  - one\n  - two and a much\n  longer tail\n"),
            "{text}"
        );
    }

    #[test]
    fn the_live_footer_appears_after_the_last_line_and_clears() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::User {
            text: "hi".into(),
            at: String::new(),
            command: None,
        });
        let base = transcript.lines(40, &colors, false).len();

        transcript.set_live_footer(vec![
            Line::from("✍️ 3m39s (↓3215, 15 tok/s)"),
            Line::from("🕒 3m41s ⠙"),
        ]);
        let with = transcript.lines(40, &colors, false);
        assert_eq!(with.len(), base + 2);
        assert_eq!(text_of(with).last().map(String::as_str), Some("🕒 3m41s ⠙"));

        transcript.set_live_footer(Vec::new());
        assert_eq!(transcript.lines(40, &colors, false).len(), base);
    }

    #[test]
    fn a_finished_block_shows_its_byline_and_cost() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Assistant {
            text: "Done.".into(),
            streaming: false,
            error: None,
            at: "21:03:16".into(),
            cost: Some(Cost {
                prefill_ms: 600,
                gen_ms: 3600,
                input: 512,
                output: 40000,
            }),
            run_ms: None,
        });
        let lines = text_of(transcript.lines(60, &colors, false));
        // The answer (no reasoning here) carries the wall-clock time + status,
        // then the prefill / generation indicators.
        assert!(
            lines
                .iter()
                .any(|l| l.contains("21:03:16") && l.contains('✓')),
            "answer meta: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains("⏫") && l.contains("↑512")),
            "prefill line: {lines:?}"
        );
        // Large token counts are abbreviated (40000 -> 40k).
        assert!(
            lines.iter().any(|l| l.contains("✍") && l.contains("↓40k")),
            "generation line: {lines:?}"
        );

        // A finished tool folds to its headline; how long it took (`🕒`) and
        // its status wait under the fold.
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "cargo test" })),
            result: Some(ToolResultMessage::text(&call("bash", json!({})), "ok")),
            live: None,
            at: "21:03:20".into(),
            duration_ms: Some(1200),
            waited_ms: None,
            waiting: false,
        });
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(
            lines
                .last()
                .is_some_and(|l| !l.contains("🕒") && !l.contains('✓')),
            "tool meta: {lines:?}"
        );
        assert!(transcript.toggle_expanded(transcript.items().len() - 1));
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(
            lines.iter().any(|l| l.contains("🕒 1s") && l.contains('✓')),
            "tool meta: {lines:?}"
        );
        assert!(transcript.toggle_expanded(transcript.items().len() - 1));
        let lines = text_of(transcript.lines(60, &colors, false));
        // The shell headline uses the `$` prefix and the command.
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("$ Running ▸ cargo test")),
            "shell headline: {lines:?}"
        );
    }

    #[test]
    fn items_render_and_cache_per_width() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::OnFinish);
        transcript.push(Item::User {
            text: "Fix the bug".into(),
            at: String::new(),
            command: None,
        });
        // Reasoning and the answer stream into their own blocks.
        transcript
            .stream_thinking("mulling this over\nsecond line of it\nthird line of it\nl4\nl5\nl6");
        transcript.stream_answer("Looking at `main.rs` now.");
        transcript.push(Item::Tool {
            call: call("read", json!({ "path": "main.rs" })),
            result: None,
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        // user(0), thinking(1), assistant(2), tool(3)
        assert_eq!(transcript.items().len(), 4);

        let lines = text_of(transcript.lines(40, &colors, false));
        // A plain gap, then the plate's top padding row, then the message: the
        // text is on the third line.
        assert_eq!(lines[0].trim_end(), "");
        assert_eq!(lines[1].trim_end(), "");
        assert_eq!(lines[2].trim_end(), "› Fix the bug");
        // Streaming, the long reasoning and the running tool show in full
        // with no fold marker.
        assert!(lines.iter().all(|l| !l.contains('▸') && !l.contains('▾')));
        assert!(lines.iter().any(|l| l.contains("l6")));
        assert!(!lines.iter().any(|l| l.contains("more lines")));
        assert!(!transcript.toggle_expanded(1));
        // Once it finishes, the thinking folds to its first line behind the
        // `▸ @` marker; the answer shows behind its own accent mark.
        assert!(transcript.finish_thinking("12:00:00", None));
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(lines
            .iter()
            .any(|l| l.starts_with("@ Thinking ▸ mulling this")));
        assert!(!lines.iter().any(|l| l.contains("second line")));
        assert!(lines.iter().all(|l| !l.contains("thought for")));
        assert!(lines.iter().any(|l| l.contains("› Looking at")));
        assert!(lines.iter().any(|l| l.contains("< Reading main.rs")));
        assert_eq!(transcript.line_count(), lines.len());
        assert_eq!(transcript.item_at_line(0), Some(0));
        assert_eq!(transcript.item_at_line(lines.len() - 1), Some(3));

        // A multi-line result: collapsed shows the headline alone, expanded
        // shows all of it.
        let body = (1..=8)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(transcript.with_tool("c1", |item| {
            if let Item::Tool { result, .. } = item {
                *result = Some(ToolResultMessage::text(&call("read", json!({})), &body));
            }
        }));
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(lines.iter().any(|l| l.starts_with("< Reading ▸ main.rs")));
        assert!(!lines.iter().any(|l| l.contains("line ")));
        assert!(transcript.toggle_expanded(3));
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(lines.iter().any(|l| l.contains("line 1")));
        assert!(lines.iter().any(|l| l.contains("line 8")));
        assert!(transcript.any_expanded());
        // Expanding the thinking block shows the full reasoning text.
        assert!(transcript.toggle_expanded(1));
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(lines.iter().any(|l| l.contains("mulling this")));

        // A different width re-wraps the prose without losing items. Tool
        // summary lines are one-liners clipped at draw time, so only the
        // markdown-rendered ones are checked against the width.
        let narrow = text_of(transcript.lines(14, &colors, false));
        assert!(narrow
            .iter()
            .filter(|l| l.contains("Looking"))
            .all(|l| l.chars().count() <= 14));
        assert!(narrow.iter().any(|l| l.contains("Looking at")));
        assert_eq!(transcript.items().len(), 4);
    }

    #[test]
    fn an_unfolded_call_says_who_decided_it_unless_the_rules_did() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::Never);
        let finished = |note: Option<PermissionNote>| {
            let mut result = ToolResultMessage::text(&call("bash", json!({})), "ok");
            result.permission = note.map(Box::new);
            Item::Tool {
                call: call("bash", json!({ "command": "make" })),
                result: Some(result),
                live: None,
                at: "12:00".into(),
                duration_ms: Some(10),
                waited_ms: None,
                waiting: false,
            }
        };
        transcript.push(finished(Some(PermissionNote {
            lasting: Some(Lasting::Session),
            ..PermissionNote::new(DecidedBy::User, true)
        })));
        transcript.push(finished(Some(PermissionNote {
            reason: "not asked for".into(),
            ..PermissionNote::new(DecidedBy::Reviewer, false)
        })));
        transcript.push(finished(Some(PermissionNote::new(DecidedBy::Rules, true))));
        let lines = text_of(transcript.lines(80, &colors, false));
        let t = termide_i18n::t();
        assert!(lines
            .iter()
            .any(|l| l.contains(&format!("✓ {}", t.agent_perm_note_user_session()))));
        assert!(lines.iter().any(|l| l.contains(&format!(
            "✗ {}",
            t.agent_perm_note_reviewer_blocked_fmt("not asked for")
        ))));
        // What the rules allowed without asking is the common case, unsaid.
        let decisions = lines
            .iter()
            .filter(|l| l.starts_with("  ✓ ") || l.starts_with("  ✗ "))
            .count();
        assert_eq!(decisions, 2);
    }

    #[test]
    fn autofold_off_leaves_every_block_expanded() {
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::Never);
        transcript.push(Item::User {
            text: "hi".into(),
            at: String::new(),
            command: None,
        });
        // A long output is foldable, so autofold-off keeps it open.
        let body = (1..=8).map(|n| format!("line {n}")).collect::<Vec<_>>();
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "ls" })),
            result: Some(ToolResultMessage::text(
                &call("bash", json!({})),
                body.join("\n"),
            )),
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        assert!(transcript.any_expanded());
    }

    #[test]
    fn short_blocks_are_not_foldable() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        // A call with no output and a one-line command has nothing to hide.
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "echo hi" })),
            result: Some(ToolResultMessage::text(&call("bash", json!({})), "")),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(300),
            waited_ms: None,
            waiting: false,
        });
        transcript.push(Item::Assistant {
            text: "the answer".into(),
            streaming: false,
            error: None,
            at: "12:00:01".into(),
            cost: None,
            run_ms: None,
        });
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(lines.iter().all(|l| !l.contains('▸') && !l.contains('▾')));
        // Its meta shares the headline row.
        assert!(lines
            .iter()
            .any(|l| l.starts_with("$ Running echo hi") && l.contains("🕒") && !l.contains('✓')));
        assert!(lines.iter().any(|l| l.contains("› the answer")));
        // No small block folds on request (tool 0, answer 1).
        assert!(!transcript.toggle_expanded(0));
        assert!(!transcript.toggle_expanded(1));
        assert!(!transcript.any_expanded());
    }

    #[test]
    fn a_long_command_wraps_under_its_prompt() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let command = "cargo test --workspace --all-features -- --nocapture";
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": command })),
            result: None,
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        let lines = text_of(transcript.lines(30, &colors, false));
        // No row is clipped: every one fits, continuation rows indent under
        // the command, past `$ Running `, and the rows read back as it.
        assert!(lines.iter().all(|l| width_of(l) <= 30), "{lines:?}");
        // A tool call has no dividing rule above it.
        assert!(lines[0].starts_with("$ Running cargo test"), "{lines:?}");
        let indent = " ".repeat(width_of("$ Running "));
        assert!(lines[1].starts_with(&indent));
        let joined: String = lines
            .iter()
            .map(|l| {
                l.strip_prefix("$ Running ")
                    .or(l.strip_prefix(indent.as_str()))
                    .unwrap()
            })
            .collect();
        assert_eq!(joined, command);
    }

    #[test]
    fn a_long_command_folds_to_its_first_line() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let script = (1..=9)
            .map(|n| format!("echo {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": script })),
            result: Some(ToolResultMessage::text(&call("bash", json!({})), "ok")),
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        // Collapsed: the first command line behind the marker, nothing else.
        let lines = text_of(transcript.lines(40, &colors, false));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("$ Running ▸ echo 1"), "{lines:?}");
        assert!(transcript.toggle_expanded(0));
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(lines.iter().any(|l| l.trim() == "echo 9"));
        assert!(lines[0].starts_with("$ Running ▾ echo 1"));
        // Continuation lines align under the command, past `$ Running ▾ `.
        let indent = " ".repeat(width_of("$ Running ▾ "));
        assert!(
            lines[1].starts_with(&format!("{indent}echo 2")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_suggested_command_reads_as_a_shell_call_with_its_own_action() {
        // `[Run]` sends an offered command through the shell, so its block is
        // a shell call's — the `$`, the command wrapped under itself — and
        // only the action says the user, not the agent, runs it.
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let offer = call(
            "suggest_command",
            json!({ "command": "cd x\nmake install", "why": "needs sudo" }),
        );
        transcript.push(Item::Tool {
            call: offer.clone(),
            result: Some(ToolResultMessage::text(&offer, "ok")),
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        let lines = text_of(transcript.lines(40, &colors, false));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("$ Suggesting ▸ cd x"), "{lines:?}");
        assert!(transcript.toggle_expanded(0));
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(lines[0].starts_with("$ Suggesting ▾ cd x"), "{lines:?}");
        let indent = " ".repeat(width_of("$ Suggesting ▾ "));
        assert!(
            lines[1].starts_with(&format!("{indent}make install")),
            "{lines:?}"
        );
    }

    #[test]
    fn finished_reasoning_folds_to_one_line_without_a_rule() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::OnFinish);
        transcript.stream_thinking("a quick thought\nand a second one");
        let lines = text_of(transcript.lines(60, &colors, false));
        // Streaming: all of it, no marker, no dividing rule.
        assert!(
            lines[0].starts_with("@ Thinking a quick thought"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("second one")));
        assert!(lines.iter().all(|l| !l.starts_with('╌')));
        let cost = Cost {
            prefill_ms: 1000,
            gen_ms: 4000,
            input: 512,
            output: 300,
        };
        assert!(transcript.finish_thinking("12:00:00", Some(cost)));
        // Finished: the first line alone; the timing is one unfold away.
        let lines = text_of(transcript.lines(60, &colors, false));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("@ Thinking ▸ a quick thought"),
            "{lines:?}"
        );
        assert!(
            !lines[0].contains("🕒") && !lines[0].contains("⏫") && !lines[0].contains("✍"),
            "{lines:?}"
        );
        // Unfolded: the whole text and the full cost lines.
        assert!(transcript.toggle_expanded(0));
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(
            lines[0].starts_with("@ Thinking ▾ a quick thought"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("second one")));
        assert!(lines.iter().any(|l| l.contains("↑512")));
    }

    #[test]
    fn unfolded_reasoning_keeps_its_line_breaks() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::OnFinish);
        transcript.stream_thinking("first step\nsecond step\n\nafter a gap");
        let lines = text_of(transcript.lines(60, &colors, false));
        let lines: Vec<&str> = lines.iter().map(|l| l.trim_end()).collect();
        assert_eq!(
            lines,
            vec![
                "@ Thinking first step",
                "           second step",
                "",
                "           after a gap"
            ]
        );
    }

    #[test]
    fn a_highlight_covers_the_content_rows_of_a_block() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::User {
            text: "go".into(),
            at: String::new(),
            command: None,
        });
        transcript.stream_thinking("a thought\nand more");
        transcript.finish_thinking("12:00:00", None);
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "ls" })),
            result: Some(ToolResultMessage::text(&call("bash", json!({})), "a\nb")),
            live: None,
            at: "12:00:01".into(),
            duration_ms: Some(100),
            waited_ms: None,
            waiting: false,
        });
        transcript.stream_answer("done");
        transcript.finish_assistant("done".into(), None, None, "12:00:02".into(), false);
        let lines = text_of(transcript.lines(40, &colors, false));
        let rows = |index: usize| {
            let (first, last) = transcript.content_lines_of(index).unwrap();
            lines[first..=last].to_vec()
        };
        // The user plate without the plain gaps around it.
        let user = rows(0);
        assert!(user.iter().any(|l| l.starts_with("› go")), "{user:?}");
        assert!(user
            .first()
            .is_some_and(|l| l.trim().is_empty() && !l.is_empty()));
        // A folded step is its single row, the first row included.
        assert_eq!(rows(1).len(), 1);
        assert!(rows(1)[0].starts_with("@ Thinking ▸ a thought"));
        assert_eq!(rows(2).len(), 1);
        assert!(rows(2)[0].starts_with("$ Running ▸ ls"));
        // Unfolded, the headline row is part of it too.
        assert!(transcript.toggle_expanded(2));
        let lines = text_of(transcript.lines(40, &colors, false));
        let (first, _) = transcript.content_lines_of(2).unwrap();
        assert!(lines[first].starts_with("$ Running ▾ ls"), "{lines:?}");
        // The answer without the blank line above it.
        let (first, _) = transcript.content_lines_of(3).unwrap();
        assert!(lines[first].starts_with("› done"), "{lines:?}");
    }

    #[test]
    fn a_resting_clock_is_dim_and_a_live_pause_stands_out() {
        let colors = ThemeColors::default();
        let glyph_fg = |transcript: &mut Transcript, glyph: &str| {
            let lines = transcript.lines(40, &colors, false).to_vec();
            lines
                .iter()
                .rev()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content.starts_with(glyph))
                .and_then(|s| s.style.fg)
        };
        let mut transcript = Transcript::default();
        transcript.stream_answer("done");
        transcript.finish_assistant("done".into(), None, None, "12:00:00".into(), false);
        transcript.end_run(5_000, "12:00:00", true, false);
        assert_eq!(glyph_fg(&mut transcript, RUN_GLYPH), Some(colors.disabled));
        // A pause on is marked; once it ends it rests, dimmed.
        transcript.end_run(0, "", true, true);
        assert_eq!(
            glyph_fg(&mut transcript, PAUSED_GLYPH),
            Some(colors.warning)
        );
        transcript.finish_pause(3_000);
        assert_eq!(
            glyph_fg(&mut transcript, PAUSED_GLYPH),
            Some(colors.disabled)
        );
        // A pause that ended no longer ticks.
        assert!(!transcript.set_pause_length(9_000));
    }

    #[test]
    fn a_clean_run_puts_its_total_on_the_answer() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::User {
            text: "go".into(),
            at: "21:00:00".into(),
            command: None,
        });
        transcript.stream_answer("done");
        transcript.finish_assistant("done".into(), None, None, "21:03:41".into(), false);
        transcript.end_run(221_000, "21:03:41", true, false);
        assert_eq!(transcript.items().len(), 2, "no closing line");
        let lines = text_of(transcript.lines(40, &colors, false));
        // The times share their text's row when it fits; the total is `✻`.
        assert!(lines
            .iter()
            .any(|l| l.starts_with("› go") && l.contains("21:00:00 ✓")));
        assert!(lines
            .iter()
            .any(|l| l.starts_with("› done") && l.contains("21:03:41 ✓")));
        assert!(lines.iter().any(|l| l.trim() == "✻ 3m41s"), "{lines:?}");
        // Too narrow to share, the time takes its own row.
        let lines = text_of(transcript.lines(14, &colors, false));
        assert!(lines.iter().any(|l| l.trim() == "21:03:41 ✓"), "{lines:?}");
        // A paused or failed run, or one ending on a tool call, still closes
        // with a line that says how it ended.
        transcript.end_run(5_000, "21:04:00", false, false);
        assert!(matches!(
            transcript.items().last(),
            Some(Item::RunEnd { ok: false, .. })
        ));
    }

    #[test]
    fn a_failed_run_puts_its_total_on_the_failed_answer() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.finish_assistant(
            String::new(),
            Some("connection refused".into()),
            None,
            "11:35:56".into(),
            false,
        );
        transcript.end_run(3_000, "11:35:56", false, false);
        // No separate closing line: the answer already has the time and `✗`.
        assert_eq!(transcript.items().len(), 1);
        let lines = text_of(transcript.lines(50, &colors, false));
        assert_eq!(
            lines.iter().filter(|l| l.contains("11:35:56")).count(),
            1,
            "{lines:?}"
        );
        assert_eq!(lines.last().map(|l| l.trim()), Some("✻ 3s"), "{lines:?}");
        // An answer that went fine before the run was aborted keeps its `✓`,
        // so the run's `✗` needs its own line.
        transcript.stream_answer("partial");
        transcript.finish_assistant("partial".into(), None, None, "11:36:00".into(), false);
        transcript.end_run(1_000, "11:36:01", false, false);
        assert!(matches!(
            transcript.items().last(),
            Some(Item::RunEnd { ok: false, .. })
        ));
    }

    #[test]
    fn a_call_prefix_is_accent_while_running_then_success() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "true" })),
            result: None,
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        let fg_of = |lines: &[Line<'static>], content: &str| {
            lines[0]
                .spans
                .iter()
                .find(|s| s.content == content)
                .and_then(|s| s.style.fg)
        };
        let lines = transcript.lines(40, &colors, false).to_vec();
        assert_eq!(fg_of(&lines, "$ "), Some(colors.info));
        let mut transcript = Transcript::default();
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "true" })),
            result: Some(ToolResultMessage::text(&call("bash", json!({})), "")),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(500),
            waited_ms: None,
            waiting: false,
        });
        let lines = transcript.lines(40, &colors, false).to_vec();
        assert_eq!(fg_of(&lines, "$ "), Some(colors.success));
        assert_ne!(fg_of(&lines, "true"), Some(colors.success));
    }

    #[test]
    fn a_thinking_marker_is_accent_while_streaming_then_dim() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.stream_thinking("Weighing the options.");
        let at_fg = |transcript: &mut Transcript| {
            transcript
                .lines(60, &colors, false)
                .iter()
                .flat_map(|l| l.spans.iter())
                .find(|s| s.content == "@ ")
                .and_then(|s| s.style.fg)
        };
        assert_eq!(at_fg(&mut transcript), Some(colors.info));
        transcript.finish_thinking("12:00:00", None);
        assert_eq!(at_fg(&mut transcript), Some(colors.disabled));
    }

    #[test]
    fn a_failed_one_row_call_is_painted_instead_of_marked() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "exit 1" })),
            result: Some(ToolResultMessage::error(
                &call("bash", json!({})),
                "[exit code 1]",
            )),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(500),
            waited_ms: None,
            waiting: false,
        });
        let lines = transcript.lines(40, &colors, false).to_vec();
        assert_eq!(lines.len(), 1);
        let text = text_of(&lines)[0].clone();
        assert!(
            text.starts_with("$ Running ▸ exit 1") && !text.contains('✗'),
            "{text}"
        );
        // Only the prefix takes the error color; the command stays dim.
        let fg_of = |content: &str| {
            lines[0]
                .spans
                .iter()
                .find(|s| s.content == content)
                .unwrap()
                .style
                .fg
        };
        assert_eq!(fg_of("$ "), Some(colors.error));
        assert_eq!(fg_of("exit 1"), Some(colors.disabled));
        // Unfolded, the output is there and the meta row keeps the `✗`.
        assert!(transcript.toggle_expanded(0));
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(lines.iter().any(|l| l.contains("🕒") && l.contains('✗')));
    }

    #[test]
    fn durations_grow_into_hours_and_days() {
        assert_eq!(fmt_dur(1_400), "1s");
        assert_eq!(fmt_dur(73_000), "1m13s");
        assert_eq!(fmt_dur((2 * 3600 + 5 * 60 + 30) * 1000), "2h5m");
        assert_eq!(fmt_dur((3 * 86_400 + 4 * 3600) * 1000), "3d4h");
    }

    #[test]
    fn a_one_row_block_carries_no_fold_marker() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.stream_thinking("a quick thought");
        assert!(transcript.finish_thinking("12:00:00", None));
        // Unfolded it is a single row, so there is nothing to fold.
        let lines = text_of(transcript.lines(60, &colors, false));
        assert_eq!(lines, vec!["@ Thinking a quick thought".to_string()]);
        assert!(!transcript.toggle_expanded(0));
        assert!(!transcript.any_expanded());
        // Narrow enough to wrap, the same thought folds to its first row.
        let lines = text_of(transcript.lines(20, &colors, false));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("@ Thinking ▸ "), "{lines:?}");
        assert!(transcript.toggle_expanded(0));
    }

    #[test]
    fn folding_immediately_keeps_running_blocks_to_one_line() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        assert_eq!(transcript.fold, FoldMode::Immediately);
        transcript.stream_thinking("a first thought\nthe latest one");
        let lines = text_of(transcript.lines(60, &colors, false));
        // Streaming reasoning is one row, following its latest line.
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("▸ the latest one"), "{lines:?}");
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "cargo build" })),
            result: None,
            live: Some("Compiling a\nCompiling b\n".into()),
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        let lines = text_of(transcript.lines(60, &colors, false));
        // The running call is its headline alone, its output folded away.
        assert!(lines.iter().any(|l| l.contains("cargo build")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("Compiling")), "{lines:?}");
    }

    #[test]
    fn a_running_tool_shows_in_full_then_folds_to_one_line() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::OnFinish);
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "cargo build" })),
            result: None,
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        // While it runs, every line of the live output shows, with no fold
        // marker, and a click does not fold it.
        let output = (1..=8)
            .map(|n| format!("step {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let live_output = output.clone();
        transcript.with_tool("c1", |item| {
            if let Item::Tool { live, .. } = item {
                *live = Some(live_output);
            }
        });
        let lines = text_of(transcript.lines(40, &colors, false));
        assert_eq!(lines.len(), 9, "{lines:?}");
        assert_eq!(lines[0].trim_end(), "$ Running cargo build");
        assert!(lines.iter().any(|l| l.trim() == "step 1"));
        assert!(!transcript.toggle_expanded(0));
        // Finished, it folds to its headline with the meta at the row's end.
        transcript.with_tool("c1", |item| {
            if let Item::Tool {
                result,
                live,
                at,
                duration_ms,
                ..
            } = item
            {
                *result = Some(ToolResultMessage::text(&call("bash", json!({})), &output));
                *live = None;
                *at = "12:00:00".into();
                *duration_ms = Some(3000);
            }
        });
        let lines = text_of(transcript.lines(40, &colors, false));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("$ Running ▸ cargo build"), "{lines:?}");
        assert!(!lines[0].contains("🕒") && !lines[0].contains('✓'));
        assert!(width_of(&lines[0]) <= 40);
        // Too long for the row, the headline is clipped.
        let lines = text_of(transcript.lines(20, &colors, false));
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains('…'), "{lines:?}");
        assert!(width_of(&lines[0]) <= 20, "{lines:?}");
    }

    #[test]
    fn a_blank_line_sets_an_answer_apart_from_the_next_step() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Assistant {
            text: "Let me look.".into(),
            error: None,
            at: "12:00:00".into(),
            cost: None,
            run_ms: None,
            streaming: false,
        });
        // Last in the chat, the answer ends at its meta row.
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(!lines.last().is_some_and(String::is_empty), "{lines:?}");
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "ls" })),
            result: None,
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        let lines = text_of(transcript.lines(40, &colors, false));
        let tool = lines.iter().position(|l| l.starts_with("$ ")).unwrap();
        assert_eq!(lines[tool - 1], "", "{lines:?}");
        assert!(!lines[tool - 2].is_empty(), "{lines:?}");
        // The gap belongs to the answer, outside its highlight.
        assert_eq!(transcript.item_at_line(tool - 1), Some(0));
        assert_eq!(transcript.content_lines_of(0), Some((1, tool - 2)));
    }

    #[test]
    fn question_answers_wrap_under_their_item() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::Never);
        let body = "The user answered:\n- Which approach? → keep the channel open between turns";
        let question = call(
            "question",
            json!({ "questions": [{ "question": "Which approach?" }] }),
        );
        transcript.push(Item::Tool {
            result: Some(ToolResultMessage::text(&question, body)),
            call: question,
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        let lines = text_of(transcript.lines(30, &colors, false));
        let first = lines
            .iter()
            .position(|l| l.starts_with("  - Which"))
            .expect("the answer line");
        // Nothing is clipped: every row fits, and the words all come through.
        assert!(lines.iter().all(|l| width_of(l) <= 30), "{lines:#?}");
        assert!(lines[first + 1].starts_with("    "), "{lines:#?}");
        let joined: String = lines[first..]
            .iter()
            .take_while(|l| l.starts_with("  "))
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.contains("open between turns"), "{joined}");
    }

    #[test]
    fn an_edit_result_is_colored_like_a_diff() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::Never);
        let body = "Edited a.rs (1 replacement).\n\n--- a/a.rs\n+++ b/a.rs\n\
                    @@ -1,2 +1,2 @@\n keep\n-old\n+new\n--- dashes\n";
        transcript.push(Item::Tool {
            call: call("edit", json!({ "path": "a.rs" })),
            result: Some(ToolResultMessage::text(&call("edit", json!({})), body)),
            live: None,
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
        let lines = transcript.lines(30, &colors, false).to_vec();
        let style_of = |needle: &str| {
            let line = lines
                .iter()
                .find(|l| l.spans.iter().any(|s| s.content == needle))
                .unwrap_or_else(|| panic!("no line {needle:?}"));
            let span = line.spans.iter().find(|s| s.content == needle).unwrap();
            (span.style, line)
        };
        let (added, row) = style_of("+new");
        assert_eq!(added.fg, Some(colors.success));
        assert!(added.bg.is_some());
        // The tint runs across the whole row, as in the git diff panel.
        assert_eq!(width_of(&text_of(std::slice::from_ref(row))[0]), 30);
        let (removed, _) = style_of("-old");
        assert_eq!(removed.fg, Some(colors.error));
        // Inside a hunk a line of dashes is a removal, not a file header.
        assert_eq!(style_of("--- dashes").0.fg, Some(colors.error));
        assert_eq!(style_of("--- a/a.rs").0.fg, Some(colors.info));
        assert_eq!(style_of(" keep").0.fg, Some(colors.fg));
        // Without details (an older session), the text shows as it came.
        assert_eq!(style_of("Edited a.rs (1 replacement).").0.bg, None);
    }

    /// An edit result as the tool returns it: the model's text, which names
    /// the file, and the details the UI draws from.
    fn edit_result(text: &str, strategy: &str) -> ToolResultMessage {
        let diff = "--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,2 @@\n keep\n-old\n-gone\n+new\n";
        ToolResultMessage::text(&call("edit", json!({})), text).with_details(json!({
            "path": "a.rs",
            "replacements": 1,
            "strategy": strategy,
            "diff": diff,
        }))
    }

    fn push_edit(transcript: &mut Transcript, path: &str, result: ToolResultMessage) {
        transcript.push(Item::Tool {
            call: call("edit", json!({ "path": path })),
            result: Some(result),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(10),
            waited_ms: None,
            waiting: false,
        });
    }

    #[test]
    fn an_edit_shows_its_hunks_and_counts_from_the_details() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::Never);
        // The model was told the diff is too large; the user still sees it.
        push_edit(
            &mut transcript,
            "a.rs",
            edit_result(
                "Edited /abs/a.rs (1 replacement). The diff is too large to show inline.",
                "",
            ),
        );
        let lines = text_of(transcript.lines(40, &colors, false));
        assert!(
            lines[0].contains("a.rs") && lines[0].ends_with("+1 −2"),
            "{lines:?}"
        );
        assert!(lines.iter().any(|l| l.contains("@@ -1,3 +1,2 @@")));
        assert!(lines.iter().any(|l| l.contains("+new")));
        // The summary and the file header repeat the headline's path.
        assert!(!lines
            .iter()
            .any(|l| l.contains("Edited") || l.contains("--- a/")));
        assert!(!lines.iter().any(|l| l.contains("whitespace")));
    }

    #[test]
    fn an_edit_wraps_a_long_path_and_keeps_its_counts() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let path = "/a/very/long/path/that/does/not/fit/the/row/at/all/a.rs";
        push_edit(
            &mut transcript,
            path,
            edit_result("Edited a.rs (1 replacement).", ""),
        );
        // Folded, and unfolded, the path wraps whole and the counts follow it.
        let folded = text_of(transcript.lines(40, &colors, false));
        assert!(folded.len() > 1, "{folded:?}");
        assert!(folded.iter().all(|row| width_of(row) <= 40), "{folded:?}");
        assert!(!folded.concat().contains('…'), "{folded:?}");
        let joined: String = folded.iter().map(|row| row.trim()).collect();
        assert!(joined.contains(path), "{folded:?}");
        assert!(folded.last().unwrap().ends_with("+1 −2"), "{folded:?}");
        assert!(transcript.toggle_expanded(0));
        let unfolded = text_of(transcript.lines(40, &colors, false));
        assert!(
            unfolded.iter().any(|row| row.ends_with("+1 −2")),
            "{unfolded:?}"
        );
    }

    #[test]
    fn a_folded_task_wraps_its_subject_under_the_action() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Tool {
            call: call(
                "task",
                json!({ "agent": "clear", "prompt": "check every call site of the parser and report which ones skip validation" }),
            ),
            result: Some(ToolResultMessage::text(&call("task", json!({})), "done")),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(10),
            waited_ms: None,
            waiting: false,
        });
        let rows = text_of(transcript.lines(40, &colors, false));
        assert!(rows.len() > 1, "{rows:?}");
        assert!(rows.iter().all(|row| width_of(row) <= 40), "{rows:?}");
        assert!(!rows.concat().contains('…'), "{rows:?}");
        let joined = rows
            .iter()
            .map(|row| row.trim())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(joined.ends_with("which ones skip validation"), "{rows:?}");
        // Continuation rows line up under the subject, past the action.
        let indent = rows[0].find("clear:").unwrap();
        let indent = width_of(&rows[0][..indent]);
        assert!(rows[1].starts_with(&" ".repeat(indent)), "{rows:?}");
    }

    #[test]
    fn a_wrapped_fetch_headline_links_every_row_to_its_url() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let url = "https://docs.rs/termide/latest/termide/links/index.html";
        transcript.push(Item::Tool {
            call: call("fetch", json!({ "url": url })),
            result: Some(ToolResultMessage::text(&call("fetch", json!({})), "page")),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(10),
            waited_ms: None,
            waiting: false,
        });
        let rows = text_of(transcript.lines(40, &colors, false));
        assert!(rows.len() > 1, "{rows:?}");
        // The second row's piece of the address opens the whole of it, and
        // lights the rows above and below with it.
        let col = rows[1].len() - rows[1].trim_start().len();
        let (href, regions) = transcript.link_at(1, col).expect("a link");
        assert_eq!(href, url);
        assert_eq!(regions.len(), rows.len(), "{regions:?}");
        // The indent before it is not the link.
        assert!(transcript.link_at(1, col - 1).is_none());
    }

    #[test]
    fn an_answer_link_sits_past_the_mark_and_the_gap() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Assistant {
            text: "see [x](a.md)".into(),
            error: None,
            at: String::new(),
            cost: None,
            run_ms: None,
            streaming: false,
        });
        let rows = text_of(transcript.lines(40, &colors, false));
        let line = rows.iter().position(|r| r.contains("see x")).unwrap();
        let row = &rows[line];
        let col = row[..row.find(" x").unwrap()].chars().count() + 1;
        assert_eq!(
            transcript
                .link_at(line, col)
                .map(|(href, _)| href)
                .as_deref(),
            Some("a.md")
        );
        assert!(transcript.link_at(line, col - 1).is_none());
    }

    #[test]
    fn folded_reasoning_still_clips_to_one_row() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Thinking {
            text: "a long first line of reasoning that goes well past the row\nmore".into(),
            streaming: false,
            at: "12:00:00".into(),
            cost: None,
        });
        let rows = text_of(transcript.lines(40, &colors, false));
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert!(rows[0].ends_with('…'), "{rows:?}");
    }

    #[test]
    fn a_loose_edit_match_is_noted() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::Never);
        push_edit(
            &mut transcript,
            "a.rs",
            edit_result(
                "Edited a.rs (1 replacement, matched ignoring indentation).",
                "ignoring indentation",
            ),
        );
        let joined = text_of(transcript.lines(60, &colors, false)).join("\n");
        assert!(joined.contains("whitespace"), "{joined}");
    }

    #[test]
    fn a_failed_edit_wraps_its_error() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.set_fold(FoldMode::Never);
        push_edit(
            &mut transcript,
            "a.rs",
            ToolResultMessage::error(
                &call("edit", json!({})),
                "old_string not found in /a/long/path/to/a.rs; read the file again.",
            ),
        );
        let joined = text_of(transcript.lines(30, &colors, false)).join(" ");
        assert!(joined.contains("again."), "{joined}");
    }

    #[test]
    fn a_run_closes_with_its_total_time() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::User {
            text: "go".into(),
            at: "21:00:00".into(),
            command: None,
        });
        transcript.push(Item::RunEnd {
            elapsed_ms: 221_000,
            at: "21:03:41".into(),
            ok: true,
            paused: false,
            live: false,
        });
        let lines = text_of(transcript.lines(60, &colors, false));
        // The frozen run clock, right-aligned with no rule above it.
        let last = lines.last().unwrap();
        assert_eq!(last.trim(), "✻ 3m41s · 21:03:41 ✓", "{lines:?}");
        assert!(lines.iter().all(|l| !l.starts_with('╌')), "{lines:?}");
        // A paused run rests on `‖` and carries no status.
        transcript.push(Item::RunEnd {
            elapsed_ms: 72_000,
            at: String::new(),
            ok: true,
            paused: true,
            live: true,
        });
        let lines = text_of(transcript.lines(60, &colors, false));
        assert_eq!(lines.last().unwrap().trim(), "‖ 1m12s");
        // It is a closing line, not a block: never folded, never selected.
        assert!(!transcript.toggle_expanded(1));
        assert!(!transcript.is_selectable(1));
        assert_eq!(transcript.selectable_near(1), Some(0));
    }

    #[test]
    fn notices_carry_no_rule() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.push(Item::Assistant {
            text: "done".into(),
            streaming: false,
            error: None,
            at: String::new(),
            cost: None,
            run_ms: None,
        });
        transcript.push(Item::RunEnd {
            elapsed_ms: 5000,
            at: "12:00:05".into(),
            ok: false,
            paused: false,
            live: false,
        });
        transcript.push(Item::Notice {
            text: "goal stopped".into(),
            kind: NoticeKind::Warn,
        });
        transcript.push(Item::Notice {
            text: "queue cleared".into(),
            kind: NoticeKind::Info,
        });
        let rule = |lines: &[String]| lines.iter().filter(|l| l.starts_with('╌')).count();
        let lines = text_of(transcript.lines(40, &colors, false));
        // The answer opens with a blank line; neither the run's closing line
        // nor the notices draw a rule: each notice is its glyph and text.
        assert_eq!(lines[0], "", "{lines:?}");
        assert_eq!(rule(&lines), 0, "{lines:?}");
        let n = lines.len();
        assert!(lines[n - 3].trim().starts_with("✻ ") && lines[n - 3].ends_with('✗'));
        assert_eq!(lines[n - 2], "! goal stopped");
        assert_eq!(lines[n - 1], "· queue cleared");
        // A long notice wraps under its glyph.
        transcript.push(Item::User {
            text: "next".into(),
            at: String::new(),
            command: None,
        });
        transcript.push(Item::Notice {
            text: "a long notice that has to wrap onto a second row".into(),
            kind: NoticeKind::Info,
        });
        let lines = text_of(transcript.lines(20, &colors, false));
        let at = lines
            .iter()
            .position(|l| l.starts_with("· a long"))
            .unwrap();
        assert!(!lines[at - 1].starts_with('╌'));
        assert!(lines[at + 1].starts_with("  "), "{lines:?}");
        assert!(lines.iter().all(|l| width_of(l) <= 20), "{lines:?}");
    }

    #[test]
    fn errors_and_notices_are_visible() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        transcript.finish_assistant(
            String::new(),
            Some("HTTP 500".into()),
            None,
            "12:00:00".into(),
            false,
        );
        transcript.push(Item::Notice {
            text: "compacted 1200 tokens".into(),
            kind: NoticeKind::Info,
        });
        transcript.push(Item::Tool {
            call: call("bash", json!({ "command": "exit 1" })),
            result: Some(ToolResultMessage::error(
                &call("bash", json!({})),
                "[exit code 1]",
            )),
            live: None,
            at: "12:00:05".into(),
            duration_ms: Some(500),
            waited_ms: None,
            waiting: false,
        });
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(lines.iter().any(|l| l.contains("✗ HTTP 500")));
        assert!(lines.iter().any(|l| l.contains("· compacted 1200 tokens")));
        // A notice can be selected, so an error can be copied.
        assert!(transcript.is_selectable(1));
        // Shell headline uses `$`; folded to one row, it shows neither how
        // long it took nor a status glyph.
        assert!(lines
            .iter()
            .any(|l| l.contains("$ Running ▸ exit 1") && !l.contains("🕒") && !l.contains('✗')));
    }

    #[test]
    fn a_turn_error_after_reasoning_shows_on_the_answer() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        // Reasoning streams, then the turn fails with no answer text.
        transcript.stream_thinking("weighing it\nl2\nl3\nl4\nl5\nl6");
        transcript.finish_thinking("12:00:00", None);
        transcript.finish_assistant(
            String::new(),
            Some("HTTP 500".into()),
            None,
            "12:00:01".into(),
            true,
        );
        // The error and its failed status land on an answer block, even though
        // the answer text is empty; the reasoning block carries no status.
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(lines.iter().any(|l| l.contains("✗ HTTP 500")));
        assert!(lines
            .iter()
            .any(|l| l.contains("12:00:01") && l.contains('✗')));
    }

    #[test]
    fn a_long_error_wraps_to_the_width() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let error = "the request failed because the endpoint is unreachable and \
             the retries were exhausted after several attempts"
            .to_string();
        transcript.finish_assistant(String::new(), Some(error), None, "12:00:01".into(), true);
        let width = 30;
        let lines = text_of(transcript.lines(width, &colors, false));
        // The error is marked with `✗` and reflows instead of spilling past the
        // width; every wrapped row stays within it.
        assert!(lines.iter().any(|l| l.contains('✗')));
        let wrapped = lines
            .iter()
            .filter(|l| l.contains("request") || l.contains("retries"))
            .count();
        assert!(wrapped >= 2, "a long error should wrap to several rows");
        assert!(lines.iter().all(|l| l.chars().count() <= width as usize));
    }

    #[test]
    fn the_system_prompt_folds_with_a_marker() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let prompt = (1..=8)
            .map(|n| format!("rule {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        transcript.push(Item::System { text: prompt });
        // Collapsed: a `▸` marker after the `#`, the first line, a note, then
        // the last few — the ellipsis sits between the first and the last few.
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(lines.iter().any(|l| l.contains("# ▸ rule 1")));
        assert!(lines.iter().any(|l| l.contains("… 3 more lines")));
        assert!(lines.iter().any(|l| l.contains("rule 5")));
        assert!(lines.iter().any(|l| l.contains("rule 8")));
        // The middle lines are the hidden ones.
        assert!(!lines.iter().any(|l| l.contains("rule 2")));
        assert!(!lines.iter().any(|l| l.contains("rule 4")));
        // Expanded: a `▾` marker and the whole prompt.
        assert!(transcript.toggle_expanded(0));
        let lines = text_of(transcript.lines(60, &colors, false));
        assert!(lines.iter().any(|l| l.contains("# ▾ rule 1")));
        assert!(lines.iter().any(|l| l.contains("rule 4")));
        assert!(lines.iter().any(|l| l.contains("rule 8")));
        // The `#` is dim like the prompt itself, not an accent.
        let hash_fg = transcript
            .lines(60, &colors, false)
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "# ")
            .and_then(|s| s.style.fg);
        assert_eq!(hash_fg, Some(colors.disabled));
    }

    #[test]
    fn web_tools_head_with_their_glyph_and_subject() {
        let colors = ThemeColors::default();
        let text = |call: &ToolCall| -> String {
            tool_headline(call, None, 80, colors.info, &colors)
                .iter()
                .map(|span| span.content.as_ref())
                .collect()
        };
        let t = termide_i18n::t();
        assert_eq!(
            text(&call("fetch", json!({ "url": "https://docs.rs" }))),
            format!("↓ {} https://docs.rs", t.agent_tool_fetch())
        );
        // The fetched URL is dim, as a link.
        let spans = tool_headline(
            &call("fetch", json!({ "url": "https://docs.rs" })),
            None,
            80,
            colors.info,
            &colors,
        );
        let url = spans.last().unwrap();
        assert_eq!(url.content, "https://docs.rs");
        assert_eq!(url.style.fg, Some(colors.disabled));
        assert_eq!(
            text(&call(
                "web_search",
                json!({ "query": "rust tui", "limit": 3 })
            )),
            format!("? {} rust tui", t.agent_tool_web_search())
        );
        assert_eq!(
            text(&call(
                "recall",
                json!({ "queries": ["why no tokio", "почему без tokio"] })
            )),
            format!("↺ {} why no tokio (+1)", t.agent_tool_recall())
        );
        assert_eq!(
            text(&call("recall", json!({ "open": "commit:.@08c2e3b1" }))),
            format!("↺ {} commit:.@08c2e3b1", t.agent_tool_recall())
        );
    }

    #[test]
    fn call_summaries_truncate_and_flatten() {
        let long = "x".repeat(50);
        let summary = summarize_call(
            &call("bash", json!({ "command": format!("echo {long}\nls") })),
            20,
        );
        assert_eq!(summary.chars().count(), 20);
        assert!(summary.ends_with('…'));
        assert!(!summary.contains('\n'));
        assert_eq!(
            summarize_call(&call("edit", json!({ "path": "a.rs" })), 40),
            "a.rs"
        );
        assert_eq!(
            summarize_call(&call("fetch", json!({ "url": "https://docs.rs" })), 40),
            "https://docs.rs"
        );
        assert_eq!(
            summarize_call(
                &call("web_search", json!({ "query": "rust tui", "limit": 5 })),
                40
            ),
            "rust tui"
        );
        assert_eq!(summarize_call(&call("mcp", json!({})), 40), "");
        assert_eq!(
            summarize_call(&call("mcp", json!({ "q": 1 })), 40),
            "{\"q\":1}"
        );
    }

    /// A finished `task` shows under its fold what the subagent spent, as
    /// the session's totals show tokens, beside how long it took.
    #[test]
    fn a_subagents_block_shows_what_it_spent() {
        let colors = ThemeColors::default();
        let mut transcript = Transcript::default();
        let task = call("task", json!({ "agent": "search", "prompt": "find it" }));
        let spent = termide_agent_core::Usage {
            input: 1200,
            output: 300,
            cache_read: 2000,
            cache_write: 0,
        };
        transcript.push(Item::Tool {
            call: task.clone(),
            result: Some(ToolResultMessage::text(&task, "found").with_spent(spent)),
            live: None,
            at: "21:03:20".into(),
            duration_ms: Some(4000),
            waited_ms: None,
            waiting: false,
        });
        assert!(transcript.toggle_expanded(0));
        let lines = text_of(transcript.lines(80, &colors, false));
        assert!(
            lines.iter().any(|l| l.contains("↑1k ↻2k ↓300 🕒 4s")),
            "{lines:?}"
        );
    }
}
