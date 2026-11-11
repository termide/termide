use super::*;
use crate::agent::test_support::text_reply;
use crate::failure::FailureKind;
use crate::message::{AssistantMessage, UserMessage};

#[test]
fn a_fork_counter_is_only_the_suffix_forks_write() {
    let split = Session::split_fork_counter;
    assert_eq!(split("Refactor (2)"), ("Refactor", Some(2)));
    assert_eq!(split("Refactor (12)"), ("Refactor", Some(12)));
    // The counter starts at two: one fork makes the second of a kind.
    assert_eq!(split("Refactor (1)"), ("Refactor (1)", None));
    // Not a counter: other brackets, other contents, no brackets.
    assert_eq!(split("work (draft)"), ("work (draft)", None));
    assert_eq!(split("fix (+2)"), ("fix (+2)", None));
    assert_eq!(split("x(2)"), ("x(2)", None));
    assert_eq!(split("2 of 3"), ("2 of 3", None));
    assert_eq!(split("()"), ("()", None));
    assert_eq!(split("Refactor"), ("Refactor", None));
}

#[test]
fn is_empty_tracks_conversation_and_discard_removes_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    // Setup entries alone (model, agent) leave the session empty.
    session
        .append_model_change("local", "qwen", Some(32_000))
        .unwrap();
    session.append_agent_change("default").unwrap();
    assert!(session.is_empty());
    // A user-given name marks intent to keep it.
    session.set_name("keep me").unwrap();
    assert!(!session.is_empty());
    // Clearing the name gives the default label back and the intent up.
    session.set_name(" ").unwrap();
    assert_eq!(session.name(), None);
    assert!(session.is_empty());
    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(reopened.name(), None);
    assert_eq!(SessionSummary::from(&reopened).name, None);
    session.set_name("keep me").unwrap();
    assert!(!session.is_empty());

    // A fresh session with a message is not empty either.
    let mut chatted = Session::create(dir.path(), Path::new("/work")).unwrap();
    chatted
        .append_message(&Message::User(UserMessage::text("hi")))
        .unwrap();
    assert!(!chatted.is_empty());

    // Discard deletes the file (and a missing file is not an error).
    let path = chatted.path().to_path_buf();
    assert!(path.exists());
    chatted.discard().unwrap();
    assert!(!path.exists());
}

/// A reply of the external agent, as its backend logs it.
fn agent_reply(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        provider: ACP_PROVIDER.into(),
        ..text_reply(text)
    })
}

#[test]
fn runs_and_unattended_work_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    assert_eq!(session.last_run(), LastRun::Ended);
    let before = session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    session.append_run_start().unwrap();
    let autorun = Autorun {
        goal: Some(GoalRecord {
            goal: "ship".into(),
            iterations: 2,
        }),
        repeat: Some(LoopRecord {
            prompt: "check".into(),
            interval_ms: None,
            iterations: 5,
            due_ms: None,
        }),
    };
    session.append_autorun(&autorun).unwrap();
    let path = session.path().to_path_buf();
    drop(session);

    let mut session = Session::open(&path).unwrap();
    assert_eq!(session.last_run(), LastRun::CutOff);
    assert_eq!(session.autorun(), autorun);
    // None of it reaches the model.
    assert_eq!(session.context_messages().len(), 1);
    session.append_run_end(true).unwrap();
    assert_eq!(session.last_run(), LastRun::Paused);
    session.append_autorun(&Autorun::default()).unwrap();
    assert!(Session::open(&path).unwrap().autorun().is_empty());
    // Undone back to before the run, the branch holds none of it.
    session.rewind_to(Some(&before)).unwrap();
    assert_eq!(session.last_run(), LastRun::Ended);
}

#[test]
fn a_run_whose_last_reply_finished_counts_as_ended() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    session.append_run_start().unwrap();
    assert_eq!(session.last_run(), LastRun::CutOff);
    // The reply came in whole; only the run's end was not written.
    session.append_message(&agent_reply("answer")).unwrap();
    assert_eq!(session.last_run(), LastRun::Ended);
    assert!(!SessionSummary::from(&session).unfinished);
}

/// The external agent closes its text before a call and logs the call
/// only with its result: a run cut off while that call ran ends on the
/// text, which waits on the call, so the run is still open.
#[test]
fn a_run_cut_off_in_an_external_agents_call_stays_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("deploy")))
        .unwrap();
    session.append_run_start().unwrap();
    session
        .append_message(&Message::Assistant(AssistantMessage {
            provider: ACP_PROVIDER.into(),
            stop_reason: StopReason::ToolUse,
            ..text_reply("Running the script.")
        }))
        .unwrap();
    assert_eq!(session.last_run(), LastRun::CutOff);
    assert!(SessionSummary::from(&session).unfinished);
}

/// The answers held for the next request survive a reopen, and the
/// last record wins: once they went, none is held.
#[test]
fn held_reports_survive_reopen_until_they_go() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    assert!(session.held_reports().is_empty());
    let report = UserMessage::text("The background task is over");
    session
        .append_held_reports(std::slice::from_ref(&report))
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);

    let mut reopened = Session::open(&path).unwrap();
    assert_eq!(reopened.held_reports(), vec![report]);
    assert_eq!(reopened.last_run(), LastRun::Ended, "no run of its own");
    reopened.append_held_reports(&[]).unwrap();
    drop(reopened);
    assert!(Session::open(&path).unwrap().held_reports().is_empty());
}

/// A held request survives a reopen and is taken back when it is
/// retried or given up; the branch's own end says nothing about it.
#[test]
fn a_held_retry_survives_reopen_and_is_taken_back() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("build it")))
        .unwrap();
    session.append_run_start().unwrap();
    session.append_run_end(false).unwrap();
    assert_eq!(session.pending_retry(), None);
    assert!(!SessionSummary::from(&session).unfinished);

    let failure = Failure {
        kind: FailureKind::QuotaExhausted,
        resets_at: Some(1_800),
    };
    session.append_retry(Some(failure)).unwrap();
    let path = session.path().to_path_buf();
    assert_eq!(session.pending_retry(), Some(failure));
    assert!(SessionSummary::from(&session).unfinished);
    drop(session);

    // Read without opening, as a project not loaded yet is asked.
    assert_eq!(Session::pending_retry_at(&path).unwrap(), Some(failure));
    let reopened = Session::open(&path).unwrap();
    assert_eq!(reopened.pending_retry(), Some(failure));
    // The run itself ended: the held request is a choice, not a run.
    assert_eq!(reopened.last_run(), LastRun::Ended);
    drop(reopened);

    let mut session = Session::open(&path).unwrap();
    session.append_retry(None).unwrap();
    assert_eq!(session.pending_retry(), None);
    assert_eq!(Session::pending_retry_at(&path).unwrap(), None);
    assert!(!SessionSummary::from(&session).unfinished);
}

#[test]
fn an_external_session_is_resumed_while_the_branch_is_still_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session.append_agent_change("claude").unwrap();
    assert_eq!(session.external_session(), None);
    session.append_external_session("claude", "s1").unwrap();
    let start = session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    session.append_message(&agent_reply("answer")).unwrap();
    // What the session holds does not change with these.
    session.append_toolset(&["shell".into()]).unwrap();
    session.set_name("named").unwrap();
    let expected = Some(ExternalSessionRef {
        agent: "claude".into(),
        session_id: "s1".into(),
    });
    assert_eq!(session.external_session(), expected);
    // It survives a reopen.
    let path = session.path().to_path_buf();
    drop(session);
    let mut session = Session::open(&path).unwrap();
    assert_eq!(session.external_session(), expected);

    // An undone request is still in the agent's session.
    session.rewind_to(Some(&start)).unwrap();
    assert_eq!(session.external_session(), None);
    // A new one after it is resumed again.
    session.append_external_session("claude", "s2").unwrap();
    assert_eq!(
        session.external_session().map(|r| r.session_id),
        Some("s2".into())
    );
    // The built-in loop answered in between: the agent never saw it.
    session
        .append_message(&Message::Assistant(text_reply("built-in")))
        .unwrap();
    assert_eq!(session.external_session(), None);
}

#[test]
fn an_external_agents_settings_are_kept_until_another_agent_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session.append_agent_change("claude").unwrap();
    session.append_external_session("claude", "s1").unwrap();
    session.append_agent_option("effort", "low").unwrap();
    session.append_agent_option("model", "opus").unwrap();
    session.append_agent_option("effort", "high").unwrap();
    let expected = vec![
        ("model".to_string(), "opus".to_string()),
        ("effort".to_string(), "high".to_string()),
    ];
    assert_eq!(session.agent_options(), expected);
    // They change nothing the agent's own session holds.
    assert!(session.external_session().is_some());
    let path = session.path().to_path_buf();
    drop(session);
    let mut session = Session::open(&path).unwrap();
    assert_eq!(session.agent_options(), expected);
    // Another agent's settings are its own.
    session.append_agent_change("codex").unwrap();
    assert!(session.agent_options().is_empty());
}

#[test]
fn another_agent_a_compaction_a_fork_or_a_move_leave_the_external_session_behind() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = |dir: &Path| {
        let mut session = Session::create(dir, Path::new("/work")).unwrap();
        session.append_external_session("claude", "s1").unwrap();
        session
            .append_message(&Message::User(UserMessage::text("one")))
            .unwrap();
        assert!(session.external_session().is_some());
        session
    };
    let mut switched = fresh(dir.path());
    switched.append_agent_change("default").unwrap();
    assert_eq!(switched.external_session(), None);
    let mut reconnected = fresh(dir.path());
    reconnected.append_connection_change("cloud").unwrap();
    assert_eq!(reconnected.external_session(), None);
    let mut compacted = fresh(dir.path());
    compacted.append_compaction("gist", 100, 1).unwrap();
    assert_eq!(compacted.external_session(), None);
    let mut cleared = fresh(dir.path());
    cleared.append_pruned().unwrap();
    assert_eq!(cleared.external_session(), None);

    let original = fresh(dir.path());
    let fork = Session::open(&original.fork(Path::new("/work"), None).unwrap()).unwrap();
    assert_eq!(fork.external_session(), None);
    assert!(original.external_session().is_some());
    let elsewhere = tempfile::tempdir().unwrap();
    let moved = original
        .move_into(elsewhere.path(), Path::new("/elsewhere"))
        .unwrap();
    assert_eq!(Session::open(&moved).unwrap().external_session(), None);
}

#[test]
fn create_append_reopen_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session
        .append_model_change("local", "qwen", Some(32_000))
        .unwrap();
    session.append_agent_change("review").unwrap();
    session
        .append_message(&Message::User(UserMessage::text("hi")))
        .unwrap();
    session
        .append_message(&Message::Assistant(text_reply("hello")))
        .unwrap();

    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(reopened.id(), session.id());
    assert_eq!(reopened.header().cwd, PathBuf::from("/work"));
    assert_eq!(reopened.entries().len(), 4);
    assert_eq!(reopened.current_agent().as_deref(), Some("review"));
    assert_eq!(reopened.leaf_id(), session.leaf_id());
    assert_eq!(
        reopened.current_model(),
        Some(SessionModel {
            provider: "local".into(),
            id: "qwen".into(),
            context_window: Some(32_000),
        })
    );
    let messages = reopened.context_messages();
    assert_eq!(messages.len(), 2);
    assert!(matches!(&messages[0], Message::User(u) if u.plain_text() == "hi"));

    let first_line = std::fs::read_to_string(session.path())
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();
    assert!(first_line.starts_with("{\"type\":\"session\""));
}

#[test]
fn moving_the_leaf_starts_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    let first = session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    session
        .append_message(&Message::Assistant(text_reply("answer one")))
        .unwrap();
    session.set_leaf(Some(&first)).unwrap();
    session
        .append_message(&Message::Assistant(text_reply("answer two")))
        .unwrap();

    let branch = session.context_messages();
    assert_eq!(branch.len(), 2);
    assert!(matches!(&branch[1], Message::Assistant(a) if a.plain_text() == "answer two"));
    assert_eq!(session.entries().len(), 3, "nothing was rewritten");
    assert!(session.set_leaf(Some("nope")).is_err());
    session.set_leaf(None).unwrap();
    assert!(session.context_messages().is_empty());
}

#[test]
fn a_fork_made_mid_run_is_settled_to_rest() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    let call = |id: &str, name: &str| crate::message::ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: serde_json::json!({}),
        extra_content: None,
    };
    let mut forking = text_reply("splitting the work");
    for open in [call("f1", "fork"), call("r1", "read")] {
        forking
            .content
            .push(crate::message::AssistantContent::ToolCall(open));
    }
    session
        .append_message(&Message::User(UserMessage::text("do both")))
        .unwrap();
    session.append_run_start().unwrap();
    session
        .append_message(&Message::Assistant(forking))
        .unwrap();
    assert_eq!(session.last_run(), LastRun::CutOff);

    session
        .settle(|open| crate::message::ToolResultMessage::text(open, open.name.clone()))
        .unwrap();
    // Each open call answered in its order, and the run ended.
    let messages = session.context_messages();
    assert!(crate::agent::unanswered_calls(&messages).is_empty());
    let answers: Vec<String> = messages
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some(result.plain_text()),
            _ => None,
        })
        .collect();
    assert_eq!(answers, ["fork", "read"]);
    assert_eq!(session.last_run(), LastRun::Ended);
    // Settled once, it stays as it is.
    let entries = session.entries().len();
    session.settle(|_| unreachable!()).unwrap();
    assert_eq!(session.entries().len(), entries);
}

#[test]
fn a_pruned_entry_replays_as_the_live_pruning_and_a_rewind_past_it_undoes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    let read = crate::message::ToolCall {
        id: "r1".into(),
        name: "read".into(),
        arguments: serde_json::json!({}),
        extra_content: None,
    };
    let mut looking = text_reply("looking");
    looking
        .content
        .push(crate::message::AssistantContent::ToolCall(read.clone()));
    session
        .append_message(&Message::User(UserMessage::text("plan it")))
        .unwrap();
    session
        .append_message(&Message::Assistant(looking))
        .unwrap();
    session
        .append_message(&Message::ToolResult(
            crate::message::ToolResultMessage::text(&read, "contents"),
        ))
        .unwrap();
    let plan = session
        .append_message(&Message::Assistant(text_reply("the plan")))
        .unwrap();
    let whole = session.context_messages();
    session.append_pruned().unwrap();
    // One message, logged and expected alike: two would differ in their
    // timestamps whenever a millisecond passes between them.
    let next = Message::User(UserMessage::text("do it"));
    session.append_message(&next).unwrap();

    let mut expected = prune_to_decisions(&whole);
    expected.push(next);
    assert_eq!(expected.len(), 3);
    assert_eq!(session.context_messages(), expected);
    let timed: Vec<Message> = session
        .context_messages_with_times(&CompactionPrompts::default())
        .into_iter()
        .map(|logged| logged.message)
        .collect();
    assert_eq!(timed, expected);

    session.rewind_to(Some(&plan)).unwrap();
    assert_eq!(session.context_messages(), whole);

    // A request made right after a clearing undoes to before it.
    let cleared = session.append_pruned().unwrap();
    assert_eq!(session.leaf_id(), Some(cleared.as_str()));
    let point = session.undo_point().map(str::to_string);
    assert_ne!(point.as_deref(), Some(cleared.as_str()));
    session.rewind_to(point.as_deref()).unwrap();
    assert_eq!(session.context_messages(), whole);
}

#[test]
fn read_branch_gives_the_live_path_and_leaves_dead_branches_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    let first = session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    session
        .append_message(&Message::Assistant(text_reply("undone")))
        .unwrap();
    session.rewind_to(Some(&first)).unwrap();
    session
        .append_message(&Message::Assistant(text_reply("kept")))
        .unwrap();
    let path = session.path().to_path_buf();

    // Read while the session is still open for appending.
    let (header, branch) = Session::read_branch(&path).unwrap();
    assert_eq!(&header.id, session.id());
    let texts: Vec<String> = branch
        .iter()
        .filter_map(|entry| match &entry.kind {
            EntryKind::Message {
                message: Message::User(user),
                ..
            } => Some(user.plain_text()),
            EntryKind::Message {
                message: Message::Assistant(reply),
                ..
            } => Some(reply.plain_text()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["one", "kept"]);
    assert!(branch.iter().any(|e| matches!(e.kind, EntryKind::Rewind)));
}

/// A crash mid-write leaves a torn last line. Reopening finishes it, so
/// the next entry starts its own line, and the conversation before the
/// tear stays whole.
#[test]
fn a_torn_last_line_does_not_swallow_the_next_entry() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    for text in ["one", "two"] {
        session
            .append_message(&Message::User(UserMessage::text(text)))
            .unwrap();
    }
    let path = session.path().to_path_buf();
    drop(session);
    // Torn inside a two-byte character, as a crash may leave it.
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(b"{\"id\":\"lost\",\"parent_id\":null,\"text\":\"\xd0")
        .unwrap();
    drop(file);

    let mut reopened = Session::open(&path).unwrap();
    assert_eq!(reopened.entries().len(), 2);
    assert!(std::fs::read(&path).unwrap().ends_with(b"\xd0\n"));
    reopened
        .append_message(&Message::User(UserMessage::text("three")))
        .unwrap();
    drop(reopened);

    let again = Session::open(&path).unwrap();
    let texts: Vec<_> = again
        .context_messages()
        .iter()
        .map(|m| match m {
            Message::User(user) => user.plain_text(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(texts, ["one", "two", "three"]);
}

/// A log written before the mend has the next entry glued to the torn
/// line: that entry is recovered, and when an entry is lost outright the
/// branch carries on from the one written before it rather than ending.
#[test]
fn a_glued_entry_is_recovered_and_a_lost_parent_does_not_cut_the_branch() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    let first = session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);

    let entry = |id: &str, parent: &str, text: &str| {
        let entry = Entry {
            id: id.into(),
            parent_id: Some(parent.into()),
            timestamp: 1,
            kind: EntryKind::Message {
                message: Message::User(UserMessage::text(text)),
                timing: None,
            },
        };
        serde_json::to_string(&Line::Entry(entry)).unwrap()
    };
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    // A torn write with "two" glued on, then "three" on it.
    writeln!(file, "{{\"id\":\"torn\",\"pa{}", entry("b", &first, "two")).unwrap();
    writeln!(file, "{}", entry("c", "b", "three")).unwrap();
    // "four" lost outright; "five" names it as its parent.
    writeln!(file, "{{\"id\":\"d\",\"parent_id\":\"c\",\"tim").unwrap();
    writeln!(file, "{}", entry("e", "d", "five")).unwrap();
    drop(file);

    let reopened = Session::open(&path).unwrap();
    let texts: Vec<_> = reopened
        .context_messages()
        .iter()
        .map(|m| match m {
            Message::User(user) => user.plain_text(),
            _ => String::new(),
        })
        .collect();
    assert_eq!(texts, ["one", "two", "three", "five"]);
}

#[test]
fn malformed_lines_are_skipped_and_missing_header_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("ok")))
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(file, "{{ this is not json").unwrap();
    let reopened = Session::open(&path).unwrap();
    assert_eq!(reopened.entries().len(), 1);

    let headerless = dir.path().join("bad.jsonl");
    std::fs::write(&headerless, "{\"id\":\"x\",\"parent_id\":null,\"timestamp\":1,\"type\":\"model_change\",\"provider\":\"p\",\"model\":\"m\"}\n").unwrap();
    assert!(Session::open(&headerless).is_err());
}

#[test]
fn a_compactions_details_are_optional_and_a_refused_call_changes_nothing() {
    // A line written before the details existed still reads.
    let old: EntryKind = serde_json::from_str(
        r#"{"type":"compaction","summary":"s","tokens_before":5,"keep_last":1}"#,
    )
    .unwrap();
    assert_eq!(
        old,
        EntryKind::Compaction {
            summary: "s".into(),
            tokens_before: 5,
            keep_last: 1,
            tokens_after: None,
            call: None,
        }
    );
    // Without details it is written as before.
    assert_eq!(
        serde_json::to_string(&old).unwrap(),
        r#"{"type":"compaction","summary":"s","tokens_before":5,"keep_last":1}"#
    );

    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session.append_external_session("claude", "s1").unwrap();
    session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    session.append_message(&agent_reply("answer")).unwrap();
    let before = session.context_messages();
    let call = CompactionCall {
        reason: crate::compaction::CompactionReason::Threshold,
        stop_reason: StopReason::ToolUse,
        duration_ms: 198_000,
        usage: crate::message::Usage::default(),
        thinking_chars: 0,
        tool_calls: vec!["shell".into()],
        input_cut: Vec::new(),
    };
    session
        .append_compaction_failed("degenerate", 247_861, call.clone(), "Looking:")
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);
    let session = Session::open(&path).unwrap();
    assert_eq!(session.context_messages(), before);
    assert!(session.external_session().is_some());
    assert!(session.branch().iter().any(|e| e.kind
        == EntryKind::CompactionFailed {
            error: "degenerate".into(),
            tokens_before: 247_861,
            call: call.clone(),
            reply: "Looking:".into(),
        }));
}

#[test]
fn the_judges_answers_are_logged_and_the_ending_one_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("work")))
        .unwrap();
    session.append_message(&agent_reply("did it")).unwrap();
    let before = session.context_messages();
    // A failed check and a verdict to go on end no goal.
    session
        .append_goal_verdict(Err("the judge gave no verdict"), "`DONE` if the goal…")
        .unwrap();
    assert_eq!(session.goal_ending(), None);
    session
        .append_goal_verdict(
            Ok((GoalOutcome::Continue, "tests fail")),
            "CONTINUE\ntests fail",
        )
        .unwrap();
    assert_eq!(session.goal_ending(), None);
    session
        .append_goal_verdict(Ok((GoalOutcome::Done, "tests pass")), "DONE\ntests pass")
        .unwrap();
    session.append_run_end(false).unwrap();
    let path = session.path().to_path_buf();
    drop(session);

    let mut session = Session::open(&path).unwrap();
    // The entries change nothing in the context.
    assert_eq!(session.context_messages(), before);
    let logged: Vec<&EntryKind> = session
        .branch()
        .into_iter()
        .map(|e| &e.kind)
        .filter(|kind| matches!(kind, EntryKind::GoalVerdict { .. }))
        .collect();
    assert_eq!(
        logged,
        [
            &EntryKind::GoalVerdict {
                outcome: None,
                reason: String::new(),
                error: "the judge gave no verdict".into(),
                reply: "`DONE` if the goal…".into(),
            },
            &EntryKind::GoalVerdict {
                outcome: Some(GoalOutcome::Continue),
                reason: "tests fail".into(),
                error: String::new(),
                reply: "CONTINUE\ntests fail".into(),
            },
            &EntryKind::GoalVerdict {
                outcome: Some(GoalOutcome::Done),
                reason: "tests pass".into(),
                error: String::new(),
                reply: "DONE\ntests pass".into(),
            },
        ]
    );
    assert_eq!(
        session.goal_ending(),
        Some((GoalOutcome::Done, "tests pass".into()))
    );
    // Once the conversation goes on, the ending is behind it.
    session
        .append_message(&Message::User(UserMessage::text("more")))
        .unwrap();
    assert_eq!(session.goal_ending(), None);

    // On disk, with a stable tag; an older log has none and reads as before.
    let line = std::fs::read_to_string(&path).unwrap();
    assert!(
        line.contains(r#""type":"goal_verdict","outcome":"done""#),
        "{line}"
    );
}

#[test]
fn a_fitted_entry_replays_the_cut_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session.append_external_session("claude", "s1").unwrap();
    for i in 0..10 {
        session
            .append_message(&Message::User(UserMessage::text(format!(
                "request {i} {}",
                "x".repeat(100)
            ))))
            .unwrap();
        session
            .append_message(&agent_reply(&format!("answer {i}")))
            .unwrap();
    }
    let whole = session.context_messages();
    let expected = crate::fit::fit(&whole, 600, 50);
    assert!(expected.changed());
    session
        .append_fitted(600, 50, expected.steps.clone(), 1_000)
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);
    let session = Session::open(&path).unwrap();
    assert_eq!(session.context_messages(), expected.messages);
    // The external agent's session holds what was there before.
    assert_eq!(session.external_session(), None);
    let timed = session.context_messages_with_times(&CompactionPrompts::default());
    let plain: Vec<Message> = timed.iter().map(|l| l.message.clone()).collect();
    assert_eq!(plain, expected.messages);
    // The note takes the time of the first message; the rest keep theirs.
    let all = Session::open(&path).unwrap();
    let first = all
        .branch()
        .into_iter()
        .find(|e| matches!(e.kind, EntryKind::Message { .. }))
        .unwrap()
        .timestamp;
    assert_eq!(timed[0].timestamp, first);
    assert!(timed.windows(2).all(|w| w[0].timestamp <= w[1].timestamp));
}

#[test]
fn compaction_entry_rebuilds_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    for text in ["one", "two", "three"] {
        session
            .append_message(&Message::User(UserMessage::text(text)))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply(&format!("answer {text}"))))
            .unwrap();
    }
    session.append_compaction("the gist", 5000, 2).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("four")))
        .unwrap();

    let reopened = Session::open(session.path()).unwrap();
    let context = reopened.context_messages();
    assert_eq!(context.len(), 4);
    assert!(matches!(
        &context[0],
        Message::User(u) if u.plain_text().contains("the gist")
    ));
    assert!(matches!(&context[1], Message::User(u) if u.plain_text() == "three"));
    assert!(matches!(&context[3], Message::User(u) if u.plain_text() == "four"));
    assert_eq!(SessionSummary::from(&reopened).message_count, 7);
}

#[test]
fn history_keeps_what_a_compaction_or_a_fitting_took_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    for text in ["one", "two", "three"] {
        session
            .append_message(&Message::User(UserMessage::text(text)))
            .unwrap();
        session
            .append_message(&Message::Assistant(text_reply(&format!("answer {text}"))))
            .unwrap();
    }
    session.append_compaction("the gist", 5000, 2).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("four")))
        .unwrap();
    session.append_fitted(1_000, 10, Vec::new(), 900).unwrap();

    let reopened = Session::open(session.path()).unwrap();
    let prompts = CompactionPrompts::default();
    let steps = reopened.history_with_times(&prompts);
    let texts: Vec<String> = steps
        .iter()
        .map(|step| match step {
            HistoryStep::Message(logged) => match &logged.message {
                Message::User(user) => user.plain_text(),
                Message::Assistant(assistant) => assistant.plain_text(),
                Message::ToolResult(_) => "tool result".into(),
            },
            HistoryStep::Compacted {
                tokens_before,
                kept,
            } => {
                format!("compacted {tokens_before}, kept {kept}")
            }
            HistoryStep::Fitted { tokens_before, .. } => format!("fitted {tokens_before}"),
        })
        .collect();
    assert_eq!(
        texts,
        [
            "one",
            "answer one",
            "two",
            "answer two",
            "three",
            "answer three",
            "compacted 5000, kept 2",
            "four",
            "fitted 900"
        ]
    );
    // A fitting's size after it is the context's, as the agent measured it.
    let Some(HistoryStep::Fitted { tokens_after, .. }) = steps.last() else {
        panic!("no fitting")
    };
    assert_eq!(
        *tokens_after,
        context_tokens(&reopened.context_messages_with(&prompts))
    );
}

/// Another termide's claim, as this process sees one: the lock of the
/// lease file taken on a handle of its own, with the holder's id in it.
fn hold_elsewhere(log: &Path, pid: u32) -> File {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(lease_path(log))
        .unwrap();
    file.try_lock().unwrap();
    write!(file, "{pid}").unwrap();
    file
}

#[test]
fn a_session_held_by_another_termide_opens_only_to_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("hi")))
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);

    let other = hold_elsewhere(&path, 4242);
    assert_eq!(Session::held_elsewhere(&path), Some(Some(4242)));
    assert!(Session::is_open(&path));
    let error = Session::open_exclusive(&path).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
    assert!(error.to_string().contains("4242"), "{error}");
    // The refusal leaves no claim behind in this process.
    assert!(!open_registry().lock().unwrap().contains(&lock_key(&path)));

    // Read, it shows the conversation, and nothing is written to it.
    let mut view = Session::open_read_only(&path).unwrap();
    assert!(view.is_read_only());
    assert_eq!(view.context_messages().len(), 1);
    let before = std::fs::read_to_string(&path).unwrap();
    assert!(view
        .append_message(&Message::User(UserMessage::text("more")))
        .is_err());
    view.discard().unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

    // Once the other lets it go, it is this one's to take.
    drop(other);
    assert_eq!(Session::held_elsewhere(&path), None);
    let taken = Session::open_exclusive(&path).unwrap();
    assert!(!taken.is_read_only());
    assert_eq!(
        lease_holder_id(&lease_path(&path)),
        Some(std::process::id())
    );
    // This process's own claim is not another termide's.
    assert_eq!(Session::held_elsewhere(&path), None);
    drop(taken);
    assert!(
        !lease_path(&path).exists(),
        "the lease file goes with the claim"
    );
}

#[test]
fn a_new_session_is_leased_from_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::create_exclusive(dir.path(), dir.path()).unwrap();
    let lock = lease_path(session.path());
    assert_eq!(lease_holder_id(&lock), Some(std::process::id()));
    // Another process trying for it now finds it taken.
    let probe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    assert!(matches!(
        probe.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    drop(probe);
    drop(session);
    assert!(!lock.exists());
}

#[test]
fn list_sorts_newest_first_with_first_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let mut older = Session::create(dir.path(), Path::new("/w")).unwrap();
    older
        .append_message(&Message::User(UserMessage::text("first task")))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut newer = Session::create(dir.path(), Path::new("/w")).unwrap();
    newer
        .append_message(&Message::User(UserMessage::text("second task")))
        .unwrap();
    std::fs::write(dir.path().join("notes.txt"), "ignored").unwrap();

    newer.set_name("the important one").unwrap();

    let list = Session::list(dir.path()).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].first_prompt.as_deref(), Some("second task"));
    assert_eq!(list[1].first_prompt.as_deref(), Some("first task"));
    assert_eq!(list[0].message_count, 1);
    // A named session shows its name; an unnamed one its first prompt.
    assert_eq!(list[0].label(), "the important one");
    assert_eq!(list[1].label(), "first task");
    assert!(Session::list(&dir.path().join("missing"))
        .unwrap()
        .is_empty());
}

#[test]
fn a_message_timing_survives_reopen_and_is_optional() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("hi")))
        .unwrap();
    let timing = Timing::Turn {
        prefill_ms: 800,
        gen_ms: 2400,
    };
    session
        .append_timed_message(&Message::Assistant(text_reply("hello")), Some(timing))
        .unwrap();
    let reopened = Session::open(session.path()).unwrap();
    let logged = reopened.context_messages_with_times(&CompactionPrompts::default());
    assert_eq!(logged.len(), 2);
    assert_eq!(logged[0].timing, None);
    assert_eq!(logged[1].timing, Some(timing));
    // An untimed entry writes no `timing` key, so older logs read the same.
    let text = std::fs::read_to_string(session.path()).unwrap();
    assert_eq!(text.matches("\"timing\"").count(), 1, "{text}");
}

#[test]
fn the_connection_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    assert_eq!(session.current_connection(), None);
    session.append_connection_change("cloud").unwrap();
    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(reopened.current_connection(), Some("cloud".to_string()));
}

#[test]
fn the_reasoning_level_survives_reopen_and_reads_the_old_switch() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    assert_eq!(session.current_thinking(), None);
    // A log written before levels holds the on/off switch.
    session
        .append(EntryKind::ReasoningChange { reasoning: false })
        .unwrap();
    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::Off));
    session
        .append(EntryKind::ReasoningChange { reasoning: true })
        .unwrap();
    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::High));

    session
        .append_thinking_change(ThinkingLevel::XHigh)
        .unwrap();
    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::XHigh));
}

#[test]
fn the_permission_mode_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    assert_eq!(session.current_mode(), None);
    session.append_mode_change(Mode::Plan).unwrap();
    session.append_mode_change(Mode::Edit).unwrap();
    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(reopened.current_mode(), Some(Mode::Edit));
    // A setup entry: the session stays empty.
    assert!(reopened.is_empty());
}

#[test]
fn the_toolset_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    assert_eq!(session.current_toolset(), None);
    session.append_toolset(&["shell".into()]).unwrap();
    session
        .append_toolset(&["shell".into(), "mcp:github".into()])
        .unwrap();
    let reopened = Session::open(session.path()).unwrap();
    assert_eq!(
        reopened.current_toolset(),
        Some(vec!["shell".to_string(), "mcp:github".to_string()])
    );
}

#[test]
fn ids_are_unique() {
    let ids: std::collections::HashSet<String> = (0..1000).map(|_| new_id()).collect();
    assert_eq!(ids.len(), 1000);
    assert!(ids.iter().all(|id| id.len() == 8));
}

#[test]
fn a_fork_copies_the_branch_under_a_new_id_and_leaves_the_source_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create_exclusive(dir.path(), Path::new("/work")).unwrap();
    session.append_connection_change("cloud").unwrap();
    session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    session
        .append_message(&Message::Assistant(text_reply("a")))
        .unwrap();
    session.set_name("keep me").unwrap();
    let source = session.path().to_path_buf();

    let copy = session
        .fork(Path::new("/other"), Some("keep me (2)"))
        .unwrap();
    assert_ne!(copy, source);
    let forked = Session::open_exclusive(&copy).unwrap();
    // A new log of its own: new id, the asking panel's cwd, and the name
    // it was given — not the source's, which would leave two alike rows.
    assert_ne!(forked.id(), session.id());
    assert_eq!(forked.header().cwd, PathBuf::from("/other"));
    assert_eq!(forked.name(), Some("keep me (2)"));
    // The conversation came across whole, entries and ids included.
    assert_eq!(forked.current_connection().as_deref(), Some("cloud"));
    // The copy's leaf is the name entry it was given, hanging off the
    // source's leaf: what came across keeps its ids and its shape.
    assert_eq!(forked.entries().len(), session.entries().len() + 1);
    let names: Vec<&str> = forked
        .entries()
        .iter()
        .rev()
        .filter_map(|e| match &e.kind {
            EntryKind::SessionName { name } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(names, ["keep me (2)", "keep me"]);
    let texts: Vec<String> = forked
        .context_messages()
        .iter()
        .map(|m| match m {
            Message::User(u) => u.plain_text(),
            Message::Assistant(a) => a.plain_text(),
            Message::ToolResult(_) => String::new(),
        })
        .collect();
    assert_eq!(texts, ["one", "a"]);

    // Writing to one log leaves the other where it was: the source is
    // still claimed by this panel and still holds two messages.
    assert!(Session::is_open(&source));
    session
        .append_message(&Message::User(UserMessage::text("later")))
        .unwrap();
    assert_eq!(Session::open(&copy).unwrap().context_messages().len(), 2);
    assert_eq!(Session::open(&source).unwrap().context_messages().len(), 3);
}

#[test]
fn a_fork_takes_the_current_branch_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    let keep = session
        .append_message(&Message::User(UserMessage::text("keep")))
        .unwrap();
    session
        .append_message(&Message::Assistant(text_reply("a")))
        .unwrap();
    session.rewind_to(Some(&keep)).unwrap();

    let forked = Session::open(&session.fork(Path::new("/w"), None).unwrap()).unwrap();
    let texts: Vec<String> = forked
        .context_messages()
        .iter()
        .map(|m| match m {
            Message::User(u) => u.plain_text(),
            Message::Assistant(a) => a.plain_text(),
            Message::ToolResult(_) => String::new(),
        })
        .collect();
    assert_eq!(texts, ["keep"]);
}

#[test]
fn a_move_refiles_the_whole_log_under_the_new_cwd_and_removes_the_old_one() {
    let root = tempfile::tempdir().unwrap();
    let mut session = Session::create(&root.path().join("a"), Path::new("/a")).unwrap();
    session.append_connection_change("cloud").unwrap();
    session.set_name("named").unwrap();
    let source = session.path().to_path_buf();
    let other = root.path().join("b");

    let moved = session.move_into(&other, Path::new("/b")).unwrap();
    assert_eq!(moved, other.join(source.file_name().unwrap()));
    assert!(!source.exists());
    let reopened = Session::open(&moved).unwrap();
    assert_eq!(reopened.id(), session.id());
    assert_eq!(reopened.header().cwd, PathBuf::from("/b"));
    assert_eq!(reopened.current_connection().as_deref(), Some("cloud"));
    assert_eq!(reopened.name(), Some("named"));
}

#[test]
fn a_rewind_drops_the_undone_messages_from_the_branch_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("one")))
        .unwrap();
    let keep = session
        .append_message(&Message::Assistant(text_reply("a")))
        .unwrap();
    session
        .append_message(&Message::User(UserMessage::text("two")))
        .unwrap();
    session
        .append_message(&Message::Assistant(text_reply("b")))
        .unwrap();
    session.rewind_to(Some(&keep)).unwrap();
    assert_eq!(session.context_messages().len(), 2);
    session
        .append_message(&Message::User(UserMessage::text("three")))
        .unwrap();
    let reopened = Session::open(session.path()).unwrap();
    let texts: Vec<String> = reopened
        .context_messages()
        .iter()
        .map(|m| match m {
            Message::User(u) => u.plain_text(),
            Message::Assistant(a) => a.plain_text(),
            Message::ToolResult(_) => String::new(),
        })
        .collect();
    assert_eq!(texts, ["one", "a", "three"]);
    assert!(session.rewind_to(Some("missing")).is_err());
}
