//! Who spent a session's tokens — the conversation itself, its subagents,
//! the `auto` reviewer, recall's solver, summaries, the `/goal` judge, a
//! `/handoff` brief — and on which model, added up from the session log.
//!
//! Each kind of call leaves its usage where the log already keeps it: an
//! assistant message, a tool result's details ([`crate::ToolResultMessage::spent`]),
//! a compaction's call; the calls that leave nothing else behind (the
//! reviewer, the judge, the brief) are logged as [`EntryKind::Spent`].

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::message::{Message, ToolResultMessage, Usage, SUBAGENT_SESSION_KEY};
use crate::session::{own_start, read_file, Entry, EntryKind};

/// How many levels of delegates' logs are read: a subagent's, its own
/// subagent's, and so on.
const SUBAGENT_DEPTH: usize = 4;

/// What a model call was made for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Spender {
    /// The conversation's own turns.
    Main,
    /// Agents a `task` handed work to.
    Subagent,
    /// The `auto` mode reviewer judging tool calls.
    Reviewer,
    /// The `recall` tool's solver.
    Recall,
    /// Summaries of the conversation that make room in its context.
    Compaction,
    /// The `/goal` judge.
    GoalJudge,
    /// A `/handoff` brief.
    Handoff,
    /// Any other tool that reports what its own model calls spent.
    Tool,
    /// A CLI agent's calls beyond its conversation's own turns, which it
    /// reports only as a sum per model: Claude Code's own subagents, its
    /// summaries, its helpers on a smaller model.
    AgentInternal,
}

impl Spender {
    /// Who spent what tool `name` reports it spent.
    #[must_use]
    pub fn of_tool(name: &str) -> Self {
        match name {
            "task" => Self::Subagent,
            "recall" => Self::Recall,
            _ => Self::Tool,
        }
    }
}

/// A session's tokens by who spent them and by model. A model left empty is
/// one the log does not tell (a subagent's, say).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spending {
    pub by: BTreeMap<Spender, Usage>,
    pub models: BTreeMap<String, Usage>,
}

impl Spending {
    /// Count `usage` as `by`'s, on `model`; nothing spent counts nothing.
    pub fn add(&mut self, by: Spender, model: &str, usage: &Usage) {
        if usage.total() == 0 {
            return;
        }
        self.by.entry(by).or_default().add(usage);
        self.models.entry(model.to_string()).or_default().add(usage);
    }

    /// Everything spent.
    #[must_use]
    pub fn total(&self) -> Usage {
        let mut total = Usage::default();
        for usage in self.by.values() {
            total.add(usage);
        }
        total
    }

    /// What every entry of a log spent, in file order — the branches an
    /// undo left behind too, since their tokens were spent all the same. A
    /// subagent counts as its task's result says, all of it its own.
    ///
    /// Only entries from index `start` on count: a fork carries its
    /// source's branch first, and what was spent on it is the source's.
    #[must_use]
    pub fn of_entries(entries: &[Entry], start: usize) -> Self {
        Self::read(entries, start, None)
    }

    /// [`Self::of_entries`] with each subagent's log read for what its task
    /// spent: on which models, and by its own reviewer, summaries and
    /// subagents. Its turns count as the subagent's; what its result says
    /// beyond its log (an older log, one gone) counts as the subagent's on
    /// no known model.
    #[must_use]
    pub fn with_subagents(entries: &[Entry], start: usize) -> Self {
        Self::read(entries, start, Some((&mut HashSet::new(), SUBAGENT_DEPTH)))
    }

    /// What tool result `result` counts for in [`Self::with_subagents`]:
    /// what it says it spent, or what its subagent's log holds when that is
    /// more. Reads that log, so for a result as it comes, not every frame.
    #[must_use]
    pub fn of_result(result: &ToolResultMessage) -> Usage {
        let entry = Entry {
            id: String::new(),
            parent_id: None,
            timestamp: 0,
            kind: EntryKind::Message {
                message: Message::ToolResult(result.clone()),
                timing: None,
            },
        };
        Self::with_subagents(std::slice::from_ref(&entry), 0).total()
    }

    /// Add up `entries`; with `delegates`, the logs already read and the
    /// levels left, the subagents' logs are read too.
    fn read(
        entries: &[Entry],
        start: usize,
        mut delegates: Option<(&mut HashSet<PathBuf>, usize)>,
    ) -> Self {
        let mut spending = Self::default();
        // What the tasks said each delegate's log spent, a log carried on
        // by several tasks counted once.
        let mut delegated: BTreeMap<PathBuf, Usage> = BTreeMap::new();
        // A summary runs on the model the session ran on at the time.
        let mut model = String::new();
        for (index, entry) in entries.iter().enumerate() {
            // A model change before `start` still sets the model a later
            // summary runs on.
            if let EntryKind::ModelChange { model: changed, .. } = &entry.kind {
                model.clone_from(changed);
            }
            if index < start {
                continue;
            }
            match &entry.kind {
                EntryKind::Message { message, .. } => match message {
                    Message::Assistant(reply) => {
                        let on = if reply.model.is_empty() {
                            &model
                        } else {
                            &reply.model
                        };
                        spending.add(Spender::Main, on, &reply.usage);
                    }
                    Message::ToolResult(result) => {
                        let log = delegates.as_ref().and_then(|_| delegate_log(result));
                        match (result.spent(), log) {
                            (spent, Some(log)) => delegated
                                .entry(log)
                                .or_default()
                                .add(&spent.unwrap_or_default()),
                            (Some(spent), None) => {
                                spending.add(Spender::of_tool(&result.tool_name), "", &spent);
                            }
                            (None, None) => {}
                        }
                    }
                    Message::User(_) => {}
                },
                EntryKind::Compaction {
                    call: Some(call), ..
                }
                | EntryKind::CompactionFailed { call, .. } => {
                    spending.add(Spender::Compaction, &model, &call.usage);
                }
                EntryKind::Spent {
                    by,
                    model: on,
                    usage,
                } => spending.add(*by, on, usage),
                _ => {}
            }
        }
        if let Some((seen, depth)) = delegates.as_mut() {
            for (log, said) in delegated {
                spending.add_delegate(&log, said, seen, *depth);
            }
        }
        spending
    }

    /// Count delegate log `log`, whose tasks said they spent `said`.
    fn add_delegate(&mut self, log: &Path, said: Usage, seen: &mut HashSet<PathBuf>, depth: usize) {
        let read = (depth > 0 && seen.insert(log.to_path_buf()))
            .then(|| read_file(log).ok())
            .flatten();
        let Some((header, entries)) = read else {
            self.add(Spender::Subagent, "", &said);
            return;
        };
        let start = own_start(&header, &entries);
        let theirs = Self::read(&entries, start, Some((seen, depth - 1)));
        for (by, usage) in &theirs.by {
            let by = match by {
                Spender::Main => Spender::Subagent,
                other => *other,
            };
            self.by.entry(by).or_default().add(usage);
        }
        for (model, usage) in &theirs.models {
            self.models.entry(model.clone()).or_default().add(usage);
        }
        let rest = said.less(&theirs.total());
        self.add(Spender::Subagent, "", &rest);
    }
}

/// The log of the delegate a task's `result` ran in, when it notes one.
fn delegate_log(result: &ToolResultMessage) -> Option<PathBuf> {
    let path = result
        .details
        .as_ref()?
        .get(SUBAGENT_SESSION_KEY)?
        .as_str()?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::agent::test_support::text_reply;
    use crate::compaction::{CompactionCall, CompactionReason};
    use crate::message::{StopReason, ToolCall, ToolResultMessage, UserMessage};
    use crate::session::Session;

    fn spent(input: u64, output: u64) -> Usage {
        Usage {
            input,
            output,
            ..Default::default()
        }
    }

    fn result(tool: &str, usage: Usage) -> Message {
        let call = ToolCall {
            id: format!("{tool}-1"),
            name: tool.into(),
            arguments: serde_json::json!({}),
            extra_content: None,
        };
        Message::ToolResult(ToolResultMessage::text(&call, "done").with_spent(usage))
    }

    /// A fork carries its source's branch but not what the source spent on
    /// it; what the fork spends after counts, on the model it carried over.
    #[test]
    fn a_fork_counts_only_what_it_spent_itself() {
        let dir = tempfile::tempdir().unwrap();
        let mut source = Session::create(dir.path(), Path::new("/work")).unwrap();
        source.append_model_change("local", "big", None).unwrap();
        let mut reply = text_reply("ok");
        reply.usage = spent(100, 10);
        source.append_message(&Message::Assistant(reply)).unwrap();
        source
            .append_spent(Spender::Reviewer, "small", spent(30, 1))
            .unwrap();
        // Copied within the same millisecond: the header counts what it
        // copied, the clock is not asked.
        let path = source.fork(Path::new("/work"), None).unwrap();
        let mut fork = Session::open(&path).unwrap();
        assert_eq!(fork.header().copied, Some(3));
        assert_eq!(fork.spending().total(), Usage::default());
        let mut reply = text_reply("again");
        reply.usage = spent(7, 1);
        reply.model.clear();
        fork.append_message(&Message::Assistant(reply)).unwrap();
        let spending = fork.spending_with_subagents();
        assert_eq!(spending.total().input, 7);
        assert_eq!(spending.models.keys().collect::<Vec<_>>(), ["big"]);
        assert_eq!(source.spending().total().input, 130);
    }

    /// A log older than the header's count of copied entries tells them by
    /// their timestamps, made before its header.
    #[test]
    fn an_older_fork_tells_its_copied_entries_by_their_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        let mut reply = text_reply("ok");
        reply.usage = spent(100, 10);
        session.append_message(&Message::Assistant(reply)).unwrap();
        let mut header = session.header().clone();
        let entries = session.entries();
        assert_eq!(crate::session::own_start(&header, entries), 0);
        header.copied = None;
        header.created = entries[0].timestamp + 1;
        assert_eq!(crate::session::own_start(&header, entries), 1);
        header.created = entries[0].timestamp;
        assert_eq!(crate::session::own_start(&header, entries), 0);
    }

    /// Each kind of call is read from where the log keeps it, and a reopened
    /// log adds up the same.
    #[test]
    fn a_log_tells_who_spent_what_on_which_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session
            .append_model_change("local", "big", Some(32_000))
            .unwrap();
        session
            .append_message(&Message::User(UserMessage::text("go")))
            .unwrap();
        let mut reply = text_reply("ok");
        reply.usage = spent(100, 10);
        session.append_message(&Message::Assistant(reply)).unwrap();
        session
            .append_message(&result("task", spent(50, 5)))
            .unwrap();
        session
            .append_message(&result("recall", spent(20, 2)))
            .unwrap();
        session
            .append_spent(Spender::Reviewer, "small", spent(30, 1))
            .unwrap();
        let call = CompactionCall {
            reason: CompactionReason::Threshold,
            stop_reason: StopReason::Stop,
            duration_ms: 1,
            usage: spent(40, 4),
            thinking_chars: 0,
            tool_calls: Vec::new(),
            input_cut: Vec::new(),
        };
        session
            .append_compaction_with("summary", 1_000, 0, Some(100), Some(call))
            .unwrap();
        let reopened = Session::open(session.path()).unwrap();
        for spending in [session.spending(), reopened.spending()] {
            let by: Vec<(Spender, u64)> = spending
                .by
                .iter()
                .map(|(by, usage)| (*by, usage.input))
                .collect();
            assert_eq!(
                by,
                [
                    (Spender::Main, 100),
                    (Spender::Subagent, 50),
                    (Spender::Reviewer, 30),
                    (Spender::Recall, 20),
                    (Spender::Compaction, 40),
                ]
            );
            // The reply names its model; the summary ran on the session's;
            // what the tools spent is not told.
            let models: Vec<(&str, u64)> = spending
                .models
                .iter()
                .map(|(model, usage)| (model.as_str(), usage.input))
                .collect();
            assert_eq!(
                models,
                [("", 70), ("big", 40), ("small", 30), ("test", 100)]
            );
            assert_eq!(spending.total(), spent(240, 22));
        }
    }

    fn task_result(log: &Path, usage: Usage) -> Message {
        let Message::ToolResult(result) = result("task", usage) else {
            unreachable!()
        };
        let mut details = result.details.clone().unwrap();
        details[SUBAGENT_SESSION_KEY] = serde_json::json!(log.to_string_lossy());
        Message::ToolResult(ToolResultMessage {
            details: Some(details),
            ..result
        })
    }

    /// A subagent's log tells its model and what its own reviewer spent; a
    /// log two tasks ran in counts once; what the results say beyond the
    /// log, or of a log gone, counts as the subagent's on no known model.
    #[test]
    fn subagents_are_read_from_their_own_logs() {
        let dir = tempfile::tempdir().unwrap();
        let mut theirs = Session::create(dir.path(), Path::new("/work")).unwrap();
        theirs
            .append_model_change("local", "small", Some(32_000))
            .unwrap();
        let mut reply = text_reply("found");
        reply.model = String::new();
        reply.usage = spent(60, 6);
        theirs.append_message(&Message::Assistant(reply)).unwrap();
        theirs
            .append_spent(Spender::Reviewer, "tiny", spent(10, 1))
            .unwrap();
        let gone = dir.path().join("gone.jsonl");

        let mut ours = Session::create(dir.path(), Path::new("/work")).unwrap();
        // Two runs of one task log, which said 80 between them: 10 more than
        // the log holds, from before its reviewer was logged.
        ours.append_message(&task_result(theirs.path(), spent(50, 4)))
            .unwrap();
        ours.append_message(&task_result(theirs.path(), spent(30, 3)))
            .unwrap();
        ours.append_message(&task_result(&gone, spent(5, 1)))
            .unwrap();

        let deep = ours.spending_with_subagents();
        let by: Vec<(Spender, u64)> = deep.by.iter().map(|(b, u)| (*b, u.input)).collect();
        assert_eq!(by, [(Spender::Subagent, 75), (Spender::Reviewer, 10)]);
        let models: Vec<(&str, u64)> = deep
            .models
            .iter()
            .map(|(m, u)| (m.as_str(), u.input))
            .collect();
        assert_eq!(models, [("", 15), ("small", 60), ("tiny", 10)]);
        // The totals agree with what the results said.
        assert_eq!(deep.total(), spent(85, 8));
        assert_eq!(ours.spending().total(), spent(85, 8));

        // A result that says nothing of what its subagent spent counts its
        // log, as the totals with the subagents do.
        let call = ToolCall {
            id: "task-2".into(),
            name: "task".into(),
            arguments: serde_json::json!({}),
            extra_content: None,
        };
        let mut silent = ToolResultMessage::text(&call, "done").with_details(serde_json::json!({
            SUBAGENT_SESSION_KEY: theirs.path().to_string_lossy(),
        }));
        assert_eq!(Spending::of_result(&silent), spent(70, 7));
        silent.details = None;
        assert_eq!(Spending::of_result(&silent), Usage::default());
    }
}
