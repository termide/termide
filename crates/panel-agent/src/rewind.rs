//! Rewinding the session to before one of the user's messages (`Esc` in an
//! idle, empty prompt, or `F4`): the messages are listed right above the
//! input, as the `/command` completions are; the conversation continues from
//! the one picked, the message comes back into the input, and the files the
//! requests since then changed can be put back too, as Claude Code's rewind
//! does.

use std::path::PathBuf;
use std::sync::PoisonError;

use crossterm::event::{KeyEvent, KeyModifiers};

use termide_agent_core::{EntryKind, Message};
use termide_core::PanelEvent;
use termide_ui::{ChoiceForm, CompletionAction, CompletionItem, CompletionList};

use crate::pending::Pending;
use crate::{truncate_title, AgentPanel, NoticeKind};

/// The open list of messages to rewind to, and what each row stands for.
pub(crate) struct RewindPicker {
    pub(crate) list: CompletionList,
    points: Vec<RewindPoint>,
}

/// A user message on the current branch the session can be rewound to.
#[derive(Debug, Clone)]
pub(crate) struct RewindPoint {
    /// Where the branch continues from: the message's parent.
    leaf: Option<String>,
    /// What the user typed, put back into the input.
    typed: String,
    /// The command, when the message is the output of one the user ran.
    ran: Option<String>,
    /// How many checkpoints, newest first, the requests since the message
    /// left.
    checkpoints: usize,
    /// The files those requests changed.
    files: Vec<PathBuf>,
}

/// What a rewind puts back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RewindScope {
    /// The files and the conversation.
    All,
    /// The conversation alone; the files stay as they are.
    Conversation,
    /// The files alone; the conversation goes on where it stands.
    Code,
}

impl RewindScope {
    /// The scope of a row of the rewind card, in the order it lists them.
    pub(crate) fn of_choice(index: usize) -> Self {
        match index {
            0 => Self::All,
            1 => Self::Conversation,
            _ => Self::Code,
        }
    }
}

impl AgentPanel {
    /// The user's messages on the current branch, newest first, each with
    /// the checkpoints left since it.
    pub(crate) fn rewind_points(&self) -> Vec<RewindPoint> {
        let Some(session) = &self.session else {
            return Vec::new();
        };
        let branch = session.branch();
        // A leaf's place on the branch, counting the empty start as 0, so an
        // entry's parent sits at the entry's own index.
        let place = |leaf: Option<&str>| match leaf {
            None => Some(0),
            Some(id) => branch.iter().position(|e| e.id == id).map(|at| at + 1),
        };
        let checkpoints = self
            .checkpoints
            .as_ref()
            .map(|store| {
                store
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .checkpoints()
            })
            .unwrap_or_default();
        let started: Vec<Option<usize>> = checkpoints
            .iter()
            .map(|checkpoint| place(checkpoint.leaf_before.as_deref()))
            .collect();
        branch
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(at, entry)| {
                let EntryKind::Message {
                    message: Message::User(user),
                    ..
                } = &entry.kind
                else {
                    return None;
                };
                // Checkpoints are newest first, so those of requests that
                // started at or after the message lead the list. One whose
                // start is off the branch ends the count.
                let count = started
                    .iter()
                    .take_while(|start| start.is_some_and(|start| start >= at))
                    .count();
                let mut files: Vec<PathBuf> = Vec::new();
                for path in checkpoints[..count].iter().flat_map(|c| &c.files) {
                    if !files.contains(path) {
                        files.push(path.clone());
                    }
                }
                Some(RewindPoint {
                    leaf: entry.parent_id.clone(),
                    typed: user.typed(),
                    ran: user.ran.clone(),
                    checkpoints: count,
                    files,
                })
            })
            .collect()
    }

    /// Offer the user's messages to rewind the session to before one of
    /// them, in a list above the input: the newest at the bottom, next to the
    /// input and selected, `↑` going back in time. A row names the files
    /// rewinding there can put back.
    pub(crate) fn ask_rewind(&mut self) -> Vec<PanelEvent> {
        let points = self.rewind_points();
        self.offer_rewind(points)
    }

    /// [`Self::ask_rewind`] over `points` already collected, oldest first.
    pub(crate) fn offer_rewind(&mut self, mut points: Vec<RewindPoint>) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        if self.is_busy() {
            self.notice(t.agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        points.reverse();
        if points.is_empty() {
            self.notice(t.agent_notice_nothing_to_rollback(), NoticeKind::Info);
            return vec![PanelEvent::NeedsRedraw];
        }
        let items = points
            .iter()
            .enumerate()
            .map(|(index, point)| {
                CompletionItem::new(index.to_string())
                    .with_label(truncate_title(&point.typed))
                    .with_description(self.changed_files(&point.files))
            })
            .collect();
        self.completion = None;
        self.chat_focus = false;
        let mut list = CompletionList::new(items);
        list.select(points.len() - 1);
        self.rewind_picker = Some(RewindPicker { list, points });
        vec![PanelEvent::NeedsRedraw]
    }

    /// `files` as a row or a card names them: relative to the panel's
    /// directory, counted when there are several; empty for none.
    fn changed_files(&self, files: &[PathBuf]) -> String {
        let names: Vec<String> = files
            .iter()
            .map(|path| {
                path.strip_prefix(&self.cwd)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
            .collect();
        match names.as_slice() {
            [] => String::new(),
            [name] => name.clone(),
            _ => termide_i18n::t().agent_undo_changed_files_fmt(names.len(), &names.join(", ")),
        }
    }

    /// A key while the rewind list is open: the arrows move through it,
    /// `Enter` or `Tab` picks the message, `Esc` closes it. Any other key
    /// closes it too and goes on to the input (`None`), so typing starts a
    /// new prompt.
    pub(crate) fn rewind_picker_key(&mut self, key: KeyEvent) -> Option<Vec<PanelEvent>> {
        let picker = self.rewind_picker.as_mut()?;
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        let action = if plain {
            picker.list.handle_key(key)
        } else {
            CompletionAction::NotHandled
        };
        match action {
            CompletionAction::Handled => Some(vec![PanelEvent::NeedsRedraw]),
            CompletionAction::Accept => {
                let index = picker.list.selected();
                Some(self.choose_rewind(index))
            }
            CompletionAction::Dismiss => {
                self.rewind_picker = None;
                Some(vec![PanelEvent::NeedsRedraw])
            }
            CompletionAction::NotHandled => {
                self.rewind_picker = None;
                None
            }
        }
    }

    /// The message picked in the rewind list: with files to put back, a card
    /// asks what to restore; without, the conversation rewinds at once.
    pub(crate) fn choose_rewind(&mut self, index: usize) -> Vec<PanelEvent> {
        let point = self.rewind_picker.take().and_then(|mut picker| {
            (index < picker.points.len()).then(|| picker.points.swap_remove(index))
        });
        let Some(point) = point else {
            return Vec::new();
        };
        if point.files.is_empty() {
            return self.rewind(&point, RewindScope::Conversation);
        }
        let t = termide_i18n::t();
        let changed = self.changed_files(&point.files);
        let form = ChoiceForm::new(
            t.agent_rewind_confirm_fmt(&truncate_title(&point.typed), &changed),
            vec![
                t.agent_undo_restore().to_string(),
                t.agent_rewind_conversation_only().to_string(),
                t.agent_rewind_files_only().to_string(),
            ],
        )
        .with_cancel(t.agent_undo_keep());
        self.pending = Some(Pending::Rewind { form, point });
        vec![PanelEvent::NeedsRedraw]
    }

    /// Rewind to before `point`'s message: put back what `scope` names. The
    /// checkpoints since the message go either way — restored, or dropped
    /// with the branch they belonged to.
    pub(crate) fn rewind(&mut self, point: &RewindPoint, scope: RewindScope) -> Vec<PanelEvent> {
        let t = termide_i18n::t();
        if self.is_busy() {
            self.notice(t.agent_notice_busy(), NoticeKind::Warn);
            return vec![PanelEvent::NeedsRedraw];
        }
        let mut events = Vec::new();
        let mut restored = 0usize;
        if let Some(store) = &self.checkpoints {
            let mut store = store.lock().unwrap_or_else(PoisonError::into_inner);
            for _ in 0..point.checkpoints {
                if scope == RewindScope::Conversation {
                    if !store.forget_last() {
                        break;
                    }
                    continue;
                }
                let Ok(undone) = store.undo_last() else {
                    break;
                };
                restored += undone.files.len();
                events.extend(undone.files.into_iter().map(PanelEvent::FileChangedOnDisk));
            }
        }
        if scope != RewindScope::Code {
            if let Some(session) = &mut self.session {
                if let Err(error) = session.rewind_to(point.leaf.as_deref()) {
                    log::warn!("agent session rewind failed: {error}");
                }
            }
            let session = self.session.take();
            self.switch_session(session);
            // The message comes back to be edited and sent again, unless the
            // user has started typing something else.
            if self.input_area().is_empty() {
                match &point.ran {
                    Some(command) => {
                        self.shell_mode = true;
                        self.set_input(command);
                    }
                    None => self.set_input(&point.typed),
                }
                self.after_edit();
            }
        }
        let plural = t.pluralize(restored, "file");
        let notice = match scope {
            RewindScope::All => t.agent_notice_rolled_back_fmt(restored, plural),
            RewindScope::Conversation => t.agent_notice_rewound().to_string(),
            RewindScope::Code => t.agent_notice_files_restored_fmt(restored, plural),
        };
        self.notice(notice, NoticeKind::Info);
        events.push(PanelEvent::NeedsRedraw);
        events
    }
}
