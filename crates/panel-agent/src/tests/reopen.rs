//! Reopening a session: restored runs, waits, goals, queues and failure cards.

use super::*;

/// A running subagent keeps a row in the state strip — its agent, task,
/// latest progress and clock — newest first, and past five the oldest fold
/// into `+N`.
#[test]
fn running_subagents_show_in_the_state_strip() {
    let colors = ThemeColors::default();
    let mut panel = panel(vec![]);
    for n in 1..=7 {
        panel.transcript.push(Item::Tool {
            call: ToolCall {
                id: format!("call-{n}"),
                name: "task".into(),
                arguments: serde_json::json!({ "agent": "search", "prompt": format!("find thing {n}") }),
                extra_content: None,
            },
            result: None,
            live: Some("started\n· read src/lib.rs".into()),
            at: String::new(),
            duration_ms: None,
            waited_ms: None,
            waiting: false,
        });
    }
    let text = |line: &Line| {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    };
    let rows = panel.strip_rows(80);
    let lines = crate::strip::strip_lines(&rows, 80, &colors);
    assert_eq!(lines.len(), 6);
    // Newest first: the oldest are the ones folded away.
    let first = text(&lines[0]);
    let head = format!(
        "& {} · search  find thing 7",
        termide_i18n::t().agent_tool_task()
    );
    assert!(first.starts_with(&head), "{first}");
    // Its last step, then the time; no spinner, and no `✕` yet.
    assert!(first.contains("· read src/lib.rs"), "{first}");
    let unit = termide_i18n::t().time_unit_secs();
    assert!(first.trim_end().ends_with(unit), "{first}");
    assert!(termide_ui::str_display_width(&first) <= 80, "{first}");
    assert!(text(&lines[4]).contains("find thing 3"));
    assert_eq!(text(&lines[5]).trim_end(), "& +2");
}

#[test]
fn a_reopened_session_shows_what_a_compaction_took_out_of_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/w")).unwrap();
    for text in ["one", "two", "three"] {
        session
            .append_message(&Message::User(UserMessage::text(text)))
            .unwrap();
        session
            .append_message(&Message::Assistant(reply(&format!("answer {text}"))))
            .unwrap();
    }
    session.append_compaction("the gist", 5000, 2).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("four")))
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);

    // As the live run showed it: every block, the compaction's notice where
    // it happened, and no summary posing as a message of the user's.
    let panel = reopen(&path, dir.path(), vec![]);
    let shown: Vec<String> = panel
        .transcript
        .items()
        .iter()
        .filter_map(|item| match item {
            Item::User { text, .. } | Item::Assistant { text, .. } => Some(text.clone()),
            Item::Notice { text, .. } => Some(format!("· {text}")),
            _ => None,
        })
        .collect();
    let notice = format!("· {}", crate::runtime::compacted_notice(5000, 2));
    assert_eq!(
        shown,
        [
            "one",
            "answer one",
            "two",
            "answer two",
            "three",
            "answer three",
            notice.as_str(),
            "four"
        ]
    );
}

/// A run another termide has under way — its window out of sight — is not
/// taken for one cut off: the panel shows the session read-only, offers
/// nothing, sends nothing, and takes it up once that termide lets it go.
#[test]
fn a_session_another_termide_holds_is_shown_read_only_until_let_go() {
    let dir = tempfile::tempdir().unwrap();
    let path = logged_request(dir.path(), |session| {
        session.append_run_start().unwrap();
    });
    let other = hold_elsewhere(&path, 4242);
    let session = Session::open_read_only(&path).unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(session),
        ..setup(vec![reply("built")])
    });
    assert!(panel.is_held_elsewhere());
    assert_eq!(panel.peer_state(), PeerState::Held);
    assert!(panel.pending.is_none(), "no carry-on card");
    assert!(!panel.paused);
    let held = termide_i18n::t().agent_notice_session_held_fmt("4242");
    let notices = |panel: &AgentPanel, wanted: &str| {
        panel
            .transcript()
            .items()
            .iter()
            .filter(|i| matches!(i, Item::Notice { text, .. } if text == wanted))
            .count()
    };
    assert_eq!(notices(&panel, &held), 1);
    // The conversation shows.
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::User { text, .. } if text == "build it")));

    // Nothing is sent, and the text waits in the input.
    let before = std::fs::read_to_string(&path).unwrap();
    type_text(&mut panel, "go on");
    panel.submit();
    assert!(!panel.is_busy());
    assert_eq!(panel.input_text(), "go on");
    assert_eq!(notices(&panel, &held), 2);
    assert!(panel.deliver(UserMessage::text("from panel 2")).is_err());
    // Still held: a look changes nothing.
    panel.held.as_mut().unwrap().next_check = Instant::now();
    panel.tick();
    assert!(panel.is_held_elsewhere());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        before,
        "nothing is written to the other termide's log"
    );

    // Let go, it is taken up, and its cut-off run is asked about.
    drop(other);
    panel.held.as_mut().unwrap().next_check = Instant::now();
    panel.tick();
    assert!(!panel.is_held_elsewhere());
    assert!(!panel.session.as_ref().unwrap().is_read_only());
    assert!(Session::is_open(&path));
    assert!(panel.paused);
    assert_eq!(panel.restored, Some(Restored::Resume));
    carry_on_card(&panel);
    assert!(panel.transcript().items().iter().any(|i| matches!(
        i,
        Item::Notice { text, .. } if text == termide_i18n::t().agent_notice_session_released()
    )));
}

#[test]
fn a_run_cut_off_by_closing_is_asked_about_when_reopened() {
    let dir = tempfile::tempdir().unwrap();
    let path = logged_request(dir.path(), |session| {
        session.append_run_start().unwrap();
    });
    let mut panel = reopen(&path, dir.path(), vec![reply("built")]);
    assert!(panel.paused);
    assert_eq!(panel.restored, Some(Restored::Resume));
    assert!(carry_on_card(&panel).contains(
        &termide_i18n::t().agent_unfinished_run_cut_fmt(termide_i18n::t().time_just_now())
    ));
    assert!(panel.attention, "the panel asks to be looked at");
    // The run's line says it was cut off.
    assert!(matches!(
        panel.transcript().items().last(),
        Some(Item::RunEnd { cut: true, .. })
    ));
    // Nothing runs on its own; the card rings the bell.
    let events = panel.tick();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PanelEvent::RequestAttention)),
        "the carry-on card rings"
    );
    assert!(!panel.is_busy());

    // Continue on the card resumes the run.
    panel.apply_form_action(ChoiceAction::Chosen(0));
    settle(&mut panel);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Assistant { text, .. } if text == "built")));
    assert!(!panel.paused);
    assert_eq!(panel.session.as_ref().unwrap().last_run(), LastRun::Ended);
}

#[test]
fn a_call_a_cut_off_run_left_is_closed_not_run_again() {
    let dir = tempfile::tempdir().unwrap();
    let call = ToolCall {
        id: "c1".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "git push" }),
        extra_content: None,
    };
    let path = logged_request(dir.path(), |session| {
        session.append_run_start().unwrap();
        session
            .append_message(&Message::Assistant(reply_calling(call.clone())))
            .unwrap();
    });
    let mut panel = reopen(&path, dir.path(), vec![reply("checked first")]);
    // The call has its result already, saying it may have run in part.
    let results: Vec<String> = panel
        .session
        .as_ref()
        .unwrap()
        .context_messages()
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult(r) if r.tool_call_id == "c1" => Some(r.plain_text()),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 1);
    assert!(results[0].contains("may have run in part"), "{results:?}");
    panel.apply_form_action(ChoiceAction::Chosen(0));
    settle(&mut panel);
    // The model answered the result; nothing ran the call again.
    let logged = panel.session.as_ref().unwrap().context_messages();
    let results = logged
        .iter()
        .filter(|m| matches!(m, Message::ToolResult(r) if r.tool_call_id == "c1"))
        .count();
    assert_eq!(results, 1);
    assert!(
        matches!(logged.last(), Some(Message::Assistant(a)) if a.plain_text() == "checked first")
    );
}

#[test]
fn a_paused_run_reopens_in_the_mode_it_ran_in() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::create(dir.path(), Path::new("/tmp")).unwrap()),
        ..setup(vec![])
    });
    let _ = panel.set_mode(Mode::Edit);
    let path = {
        let session = panel.session.as_mut().unwrap();
        session
            .append_message(&Message::User(UserMessage::text("build it")))
            .unwrap();
        session.append_run_start().unwrap();
        session.append_run_end(true).unwrap();
        session.path().to_path_buf()
    };
    drop(panel);

    let mut panel = reopen(&path, dir.path(), vec![]);
    assert!(panel.paused);
    assert_eq!(panel.mode.get(), Mode::Edit);
    assert_eq!(panel.rules.mode, Mode::Edit);

    // An undo goes back past the change, yet the mode the user is in stays.
    let _ = panel.set_mode(Mode::All);
    panel.session.as_mut().unwrap().rewind_to(None).unwrap();
    panel.log_mode(panel.mode.get());
    let session = panel.session.take();
    panel.switch_session(session);
    assert_eq!(panel.mode.get(), Mode::All);
}

#[test]
fn a_finished_run_reopens_idle() {
    let dir = tempfile::tempdir().unwrap();
    let path = logged_request(dir.path(), |session| {
        session.append_run_start().unwrap();
        session
            .append_message(&Message::Assistant(reply("done")))
            .unwrap();
    });
    // Its end was never written, but its reply came in whole.
    let panel = reopen(&path, dir.path(), vec![]);
    assert!(!panel.paused);
    assert!(panel.restored.is_none());
    assert!(panel.pending.is_none());
    assert!(panel.run_buttons().is_empty());
}

#[test]
fn a_goal_left_going_is_judged_again_on_continue() {
    let dir = tempfile::tempdir().unwrap();
    let path = logged_request(dir.path(), |session| {
        session.append_run_start().unwrap();
        session
            .append_message(&Message::Assistant(reply("halfway")))
            .unwrap();
        session.append_run_end(false).unwrap();
        session
            .append_autorun(&Autorun {
                goal: Some(GoalRecord {
                    goal: "build it".into(),
                    iterations: 3,
                }),
                repeat: None,
            })
            .unwrap();
    });
    let mut panel = reopen(&path, dir.path(), vec![reply("DONE\nit builds")]);
    // The run itself ended: only the goal waits to go on.
    assert_eq!(panel.restored, Some(Restored::Autorun));
    assert_eq!(panel.goal_task.as_ref().map(|t| t.iterations), Some(3));
    assert!(
        carry_on_card(&panel).contains(&termide_i18n::t().agent_unfinished_goal_fmt("build it", 3))
    );
    // Decide later: the card goes, the pause stays.
    panel.apply_form_action(ChoiceAction::Cancelled);
    assert!(panel.pending.is_none());
    assert!(panel.paused);
    panel.tick();
    assert!(
        panel.goal_task.is_some(),
        "nothing goes on before /continue"
    );

    type_text(&mut panel, "/continue");
    panel.submit();
    settle_goal(&mut panel);
    assert!(!panel.paused);
    panel.tick();
    // The goal reached is written to the log, so it is not offered again.
    assert!(panel.session.as_ref().unwrap().autorun().is_empty());
}

#[test]
fn a_loop_waits_only_what_is_left_of_its_interval() {
    let dir = tempfile::tempdir().unwrap();
    let due = now_millis() + 600_000;
    let path = logged_request(dir.path(), |session| {
        session.append_run_end(false).unwrap();
        session
            .append_autorun(&Autorun {
                goal: None,
                repeat: Some(LoopRecord {
                    prompt: "check".into(),
                    interval_ms: Some(3_600_000),
                    iterations: 2,
                    due_ms: Some(due),
                }),
            })
            .unwrap();
    });
    let mut panel = reopen(&path, dir.path(), vec![]);
    assert!(carry_on_card(&panel).contains("1h"));
    panel.apply_form_action(ChoiceAction::Chosen(0));
    let next = panel.loop_task.as_ref().and_then(|t| t.next_at).unwrap();
    let wait = next.saturating_duration_since(Instant::now());
    assert!(
        wait > Duration::from_secs(590) && wait <= Duration::from_secs(600),
        "{wait:?}"
    );
}

#[test]
fn dropping_restored_work_takes_it_out_of_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = logged_request(dir.path(), |session| {
        session.append_run_end(false).unwrap();
        session
            .append_autorun(&Autorun {
                goal: None,
                repeat: Some(LoopRecord {
                    prompt: "check".into(),
                    interval_ms: Some(60_000),
                    iterations: 2,
                    due_ms: None,
                }),
            })
            .unwrap();
    });
    assert!(Session::list(dir.path()).unwrap()[0]
        .display_label()
        .starts_with("⏸ "));
    let mut panel = reopen(&path, dir.path(), vec![]);
    assert_eq!(
        panel.loop_task.as_ref().map(|t| t.interval),
        Some(Some(Duration::from_secs(60)))
    );
    panel.apply_form_action(ChoiceAction::Chosen(1));
    panel.tick();
    assert!(panel.loop_task.is_none());
    assert!(panel.run_buttons().is_empty());
    drop(panel);
    let session = Session::open(&path).unwrap();
    assert!(session.autorun().is_empty());
    assert!(!Session::list(dir.path()).unwrap()[0].unfinished);
}

#[test]
fn closing_while_work_is_under_way_asks_first() {
    let mut panel = panel(vec![reply("working")]);
    assert!(panel.needs_close_confirmation().is_none());
    type_text(&mut panel, "/loop 5m check");
    panel.submit();
    assert_eq!(
        panel.needs_close_confirmation().as_deref(),
        Some(termide_i18n::t().agent_close_confirm_working())
    );
    settle(&mut panel);
    // Waiting out its interval, the loop is still going.
    assert!(panel.needs_close_confirmation().is_some());
    type_text(&mut panel, "/loop stop");
    panel.submit();
    assert!(panel.needs_close_confirmation().is_none());
}

#[test]
fn a_goal_under_way_is_in_the_log_until_it_stops() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("working")])
    });
    let path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    settle(&mut panel);
    panel.goal_task.as_mut().unwrap().judge_at = None;
    panel.tick();
    // The work turn's start and the goal are logged right after the
    // request, which hangs off where the request started.
    let logged = Session::open(&path).unwrap();
    assert_eq!(logged.last_run(), LastRun::Ended);
    let branch = logged.branch();
    let user = branch
        .iter()
        .position(|e| {
            matches!(
                &e.kind,
                EntryKind::Message {
                    message: Message::User(_),
                    ..
                }
            )
        })
        .unwrap();
    assert!(matches!(branch[user + 1].kind, EntryKind::RunStart));
    assert!(matches!(branch[user + 2].kind, EntryKind::Autorun { .. }));
    assert_eq!(
        logged.autorun().goal,
        Some(GoalRecord {
            goal: "do the thing".into(),
            iterations: 1
        })
    );

    type_text(&mut panel, "/goal stop");
    panel.submit();
    panel.tick();
    assert!(Session::open(&path).unwrap().autorun().is_empty());
}

/// A limit the panel asked about and termide closed on: reopening the session
/// asks the same again, with the same rows, and runs nothing by itself.
#[test]
fn a_limit_asked_about_comes_back_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![limited()])
    });
    let path = panel.session_path().unwrap().to_path_buf();
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));
    // Closing the panel gives the log back to the file.
    drop(panel);

    let mut panel = reopen(&path, dir.path(), vec![reply("done")]);
    let (rows, detail) = failure_card(&panel);
    assert!(rows[0].starts_with("Wait until"), "{rows:?}");
    assert!(rows.iter().any(|row| row == "Try again now"), "{rows:?}");
    assert!(detail.contains("Resets at"), "{detail}");
    assert!(
        !detail.contains("session limit"),
        "the error's text stands in the transcript above: {detail}"
    );
    assert!(panel.retry_ready, "`/continue` takes it too");
    assert!(!panel.paused, "a failed request is not a pause");
    // Nothing runs by itself.
    panel.tick();
    assert!(!panel.is_busy());

    // The row picked does what it names.
    let now = rows.iter().position(|row| row == "Try again now").unwrap();
    panel.apply_form_action(ChoiceAction::Chosen(now));
    settle(&mut panel);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, error: None, .. } if text == "done")));
    // The held request is taken back out of the log.
    assert_eq!(Session::open(&path).unwrap().pending_retry(), None);
}

/// Stopping the restored card gives the request up for good: a third reopen
/// asks nothing, since the user has answered.
#[test]
fn stopping_a_restored_limit_card_asks_nothing_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![limited()])
    });
    let path = panel.session_path().unwrap().to_path_buf();
    panel.send("do it".to_string());
    settle(&mut panel);

    drop(panel);
    let mut panel = reopen(&path, dir.path(), vec![reply("done")]);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));
    panel.apply_form_action(ChoiceAction::Cancelled);
    assert!(panel.pending.is_none());
    // Esc gives the request up for the card, and `/continue` still holds it
    // until a new request takes over — as it does before any close.
    assert!(panel.retry_ready);
    drop(panel);

    let panel = reopen(&path, dir.path(), vec![reply("done")]);
    assert!(
        !matches!(panel.pending, Some(Pending::Failure { .. })),
        "the user answered: nothing to ask"
    );
}

/// A wait out of a limit that termide closed over: reopening asks rather than
/// waiting on its own or sending anything — the reset may have come and gone.
#[test]
fn a_wait_closed_over_asks_on_reopen_and_waits_for_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        on_limit: LimitPolicy::Wait,
        ..setup(vec![limited()])
    });
    let path = panel.session_path().unwrap().to_path_buf();
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(panel.pending.is_none(), "the policy asks nothing");
    assert!(panel.retry_wait.is_some(), "the reset is waited for");
    drop(panel);

    let mut panel = reopen(&path, dir.path(), vec![reply("done")]);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));
    assert!(panel.retry_wait.is_none(), "the wait does not go on unseen");
    panel.tick();
    assert!(!panel.is_busy(), "nothing goes to the model unasked");

    // A reset already gone by takes its row away, as on any open card.
    set_reset(&mut panel, crate::failure::unix_now() - 60);
    panel.failure_card_shown = None;
    assert!(panel.refresh_failure_card());
    let (rows, detail) = failure_card(&panel);
    assert!(
        !rows.iter().any(|row| row.starts_with("Wait until")),
        "a reset already past is not waited for: {rows:?}"
    );
    assert!(detail.contains("has passed"), "{detail}");
}

/// A goal left waiting on a failure card comes back with it, so the choice
/// carries both.
#[test]
fn a_goal_waiting_on_a_limit_card_comes_back_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![limited()])
    });
    let path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "/goal get it done");
    panel.submit();
    settle(&mut panel);
    drop(panel);

    let panel = reopen(&path, dir.path(), vec![reply("done")]);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));
    assert!(panel.goal_task.is_some(), "the goal waits on the card");
}

#[test]
fn an_external_agent_cut_off_is_told_to_carry_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/tmp")).unwrap();
    session.append_agent_change("outside").unwrap();
    session
        .append_message(&Message::User(UserMessage::text("build it")))
        .unwrap();
    session.append_run_start().unwrap();
    let path = session.path().to_path_buf();
    drop(session);
    let mut panel = reopen(&path, dir.path(), vec![]);
    assert!(panel.external);
    // It cannot be resumed between steps: it is told to go on instead.
    assert_eq!(panel.restored, Some(Restored::Prompt));
    panel.apply_form_action(ChoiceAction::Chosen(0));
    settle(&mut panel);
    let items = panel.transcript().items();
    assert!(
        items
            .iter()
            .any(|i| matches!(i, Item::User { command: Some(c), .. } if c == "/continue")),
        "{items:?}"
    );
    assert!(items
        .iter()
        .any(|i| matches!(i, Item::Assistant { text, .. } if text == "from outside")));
    assert_eq!(panel.session.as_ref().unwrap().last_run(), LastRun::Ended);
}

/// An external agent logs a call only with its result. Closed while a long
/// call ran, the panel leaves the call in the log with a result saying it was
/// cut off, so the reopened panel shows it and asks to carry the run on.
#[test]
fn an_external_agent_closed_in_a_call_leaves_it_logged_cut_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/tmp")).unwrap();
    session.append_agent_change("outside").unwrap();
    let path = session.path().to_path_buf();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(session),
        ..setup(vec![])
    });
    assert!(panel.external);
    let call = ToolCall {
        id: "c1".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "./deploy.sh" }),
        extra_content: None,
    };
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageEnd(Message::User(UserMessage::text(
        "deploy",
    ))));
    // The text closed before the call, as the external agent logs it.
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(
        AssistantMessage {
            provider: ACP_PROVIDER.into(),
            stop_reason: StopReason::ToolUse,
            ..reply("Running the deploy.")
        },
    )));
    panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
    assert!(panel.is_busy());
    drop(panel);

    let panel = reopen(&path, dir.path(), vec![]);
    assert_eq!(panel.restored, Some(Restored::Prompt));
    carry_on_card(&panel);
    let logged = panel.session.as_ref().unwrap().context_messages();
    let calls: Vec<&ToolCall> = logged
        .iter()
        .filter_map(|m| match m {
            Message::Assistant(a) => Some(a),
            _ => None,
        })
        .flat_map(AssistantMessage::tool_calls)
        .collect();
    assert_eq!(calls, [&call]);
    assert!(matches!(
        logged.last(),
        Some(Message::ToolResult(r)) if r.tool_call_id == "c1"
            && r.plain_text().contains("may have run in part")
    ));
    // The transcript shows the call with its result, not still running.
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Tool { call, result: Some(_), .. } if call.id == "c1")));
}

#[test]
fn a_failing_goal_check_is_tried_again_then_pauses_the_goal() {
    let mut panel = panel(vec![reply("working")]);
    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    settle(&mut panel);
    let fail = || AgentEvent::GoalJudgeFailed {
        error: "session/prompt: Internal error (code -32603)".into(),
        reply: String::new(),
    };
    // The first failures wait and try again; the goal stays.
    for attempt in 1..=GOAL_JUDGE_RETRIES {
        panel.goal_task.as_mut().unwrap().judging = true;
        panel.apply(fail());
        let task = panel.goal_task.as_ref().expect("the goal stays");
        assert_eq!(task.judge_failures, attempt);
        assert!(!task.judging);
        assert!(task.judge_at.is_some_and(|at| at > Instant::now()));
        assert!(!panel.paused);
    }
    // Once they are used up, the goal waits at a pause instead of ending.
    panel.apply(fail());
    let task = panel
        .goal_task
        .as_ref()
        .expect("the goal is paused, not dropped");
    assert!(task.judge_at.is_none());
    assert_eq!(task.judge_failures, 0);
    assert!(panel.paused);
    // `/continue` judges it again now.
    type_text(&mut panel, "/continue");
    panel.submit();
    assert!(!panel.paused);
    assert!(panel.goal_task.is_some());
}

#[test]
fn a_goal_paused_by_failing_checks_ends_with_goal_stop() {
    let mut panel = panel(vec![reply("working")]);
    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    settle(&mut panel);
    for _ in 0..=GOAL_JUDGE_RETRIES {
        panel.apply(AgentEvent::GoalJudgeFailed {
            error: "boom".into(),
            reply: String::new(),
        });
    }
    assert!(panel.paused);
    type_text(&mut panel, "/goal stop");
    panel.submit();
    panel.tick();
    assert!(panel.goal_task.is_none());
    assert!(!panel.paused);
}

#[test]
fn a_goal_set_during_a_run_ends_with_a_stop_and_hands_back_the_queue() {
    let mut panel = panel(vec![]);
    panel.busy = true;
    type_text(&mut panel, "/goal finish the refactor");
    panel.submit();
    type_text(&mut panel, "are you stuck?");
    panel.submit();
    assert_eq!(
        panel
            .queued_texts
            .iter()
            .map(|(text, _)| text)
            .collect::<Vec<_>>(),
        ["are you stuck?"]
    );
    panel.abort();
    assert!(panel.goal_task.is_none());
    // The goal took nothing from the queue; the stop hands it back.
    assert!(panel.runtime.take_queued().is_empty());
    assert_eq!(panel.input_text(), "are you stuck?");
}

#[test]
fn stopping_withdraws_the_queued_loop_turn() {
    let mut panel = panel(vec![]);
    panel.busy = true;
    type_text(&mut panel, "/loop 5m check the build");
    panel.submit();
    type_text(&mut panel, "are you stuck?");
    panel.submit();
    panel.abort();
    assert!(panel.loop_task.is_none());
    // The loop's turn is gone; the user's own message comes back to the input.
    assert_eq!(panel.input_text(), "are you stuck?");
    assert!(panel.queued_texts.is_empty());
}

#[test]
fn stopping_takes_the_queue_back_into_the_input() {
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    for text in ["fix the test", "and the docs"] {
        type_text(&mut panel, text);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    }
    assert_eq!(panel.runtime.queue_lens(), (2, 0));
    // Esc on an empty input stops the run.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.stop_requested.is_some());
    assert_eq!(panel.input_text(), "fix the test\n\nand the docs");
    assert_eq!(panel.runtime.queue_lens(), (0, 0));
    panel.apply(AgentEvent::AgentEnd);
    assert!(
        panel.state_lines(60).is_empty(),
        "nothing is left waiting in the strip"
    );
    assert_eq!(panel.input_text(), "fix the test\n\nand the docs");
}

#[test]
fn a_queued_row_shows_its_wait_and_its_cross_takes_it_back() {
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    type_text(&mut panel, "fix the test");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let rows = panel.strip_rows(60);
    let row = rows
        .iter()
        .find(|row| row.target == crate::strip::StripTarget::Queued)
        .expect("a queued row");
    assert!(row.time.is_some(), "it shows how long it waits");
    assert!(row.stop, "it has a ✕");
    // Elsewhere on the row nothing happens; the ✕ takes it back.
    panel.click_strip(crate::strip::StripTarget::Queued, false);
    assert_eq!(panel.runtime.queue_lens(), (1, 0));
    panel.click_strip(crate::strip::StripTarget::Queued, true);
    assert_eq!(panel.input_text(), "fix the test");
    assert_eq!(panel.runtime.queue_lens(), (0, 0));
    assert!(panel.state_lines(60).is_empty());
    assert!(panel.is_busy(), "the run goes on");
}

/// An external agent holds no copy of the request across a close, so its
/// restored retry is told to carry on rather than sent the same turn twice.
#[test]
fn a_restored_retry_of_an_external_agent_carries_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/tmp")).unwrap();
    session.append_agent_change("outside").unwrap();
    session
        .append_message(&Message::User(UserMessage::text("build it")))
        .unwrap();
    session.append_run_start().unwrap();
    session.append_run_end(false).unwrap();
    session
        .append_retry(Some(termide_agent_core::Failure {
            kind: termide_agent_core::FailureKind::QuotaExhausted,
            resets_at: Some(crate::failure::unix_now() + 600),
        }))
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);

    let mut panel = reopen(&path, dir.path(), vec![reply("from outside")]);
    assert!(panel.external);
    let (rows, _) = failure_card(&panel);
    let now = rows.iter().position(|row| row == "Try again now").unwrap();
    panel.apply_form_action(ChoiceAction::Chosen(now));
    settle(&mut panel);
    // The request goes once, as a carry-on; the user's prompt is not sent a
    // second time.
    let items = panel.transcript().items();
    let users: Vec<_> = items
        .iter()
        .filter(|i| matches!(i, Item::User { .. }))
        .collect();
    assert_eq!(users.len(), 1, "the prompt is not sent twice: {items:?}");
    assert!(items
        .iter()
        .any(|i| matches!(i, Item::Assistant { text, .. } if text == "from outside")));
}

/// A wait whose monotonic clock stood still in a sleep goes by the wall clock.
#[test]
fn a_wait_goes_by_the_wall_clock_after_a_sleep() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.on_limit = LimitPolicy::Wait;
    panel.send("do it".to_string());
    settle(&mut panel);
    // As after a sleep: the wall clock passed the reset, the monotonic one not.
    panel.retry_wait.as_mut().unwrap().wall = crate::failure::unix_now();
    panel.tick();
    settle(&mut panel);
    assert!(panel.retry_wait.is_none());
    assert!(answered_done(&panel));
}

/// A request that went through answers the failure it follows: the log keeps
/// no retry for it, so a session reopened later asks about nothing — least of
/// all about a limit the request outran on another connection.
#[test]
fn a_run_that_goes_takes_the_held_failure_out_of_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![limited(), reply("done")])
    });
    let path = panel.session_path().unwrap().to_path_buf();
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));
    assert!(
        panel.session.as_ref().unwrap().pending_retry().is_some(),
        "the card holds the failure in the log"
    );

    // The user answers by sending a new request, as one does to carry on.
    panel.apply_form_action(ChoiceAction::Cancelled);
    panel.send("go on".to_string());
    settle(&mut panel);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, error: None, .. } if text == "done")));

    assert!(
        panel.session.as_ref().unwrap().pending_retry().is_none(),
        "the request went: no failure waits on the user"
    );
    drop(panel);
    let reopened = reopen(&path, dir.path(), vec![reply("done")]);
    assert!(
        !matches!(reopened.pending, Some(Pending::Failure { .. })),
        "nothing is asked at a reopen"
    );
}

/// A limit belongs to the endpoint that hit it, not to the conversation:
/// switching connections rebuilds the agent and keeps the card up, so the
/// request can be tried again there — which is how a subscription limit is
/// got past.
#[test]
fn a_connection_switch_keeps_the_failure_card_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = on_endpoint(dir.path(), vec![limited(), reply("done")]);
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));

    assert!(panel.switch_connection("cloud"));
    assert!(
        matches!(panel.pending, Some(Pending::Failure { .. })),
        "the choice is still owed: the endpoint changed, not the request"
    );
    assert!(panel.retry_ready, "`/continue` still holds it");

    let rows = failure_rows(&panel);
    let now = rows.iter().position(|row| row == "Try again now").unwrap();
    panel.apply_form_action(ChoiceAction::Chosen(now));
    settle(&mut panel);
    assert!(
        answered_done(&panel),
        "the request went on the new endpoint"
    );
}

/// The same across a switch onto a CLI agent, whose runtime is rebuilt whole.
#[test]
fn a_switch_to_a_cli_agent_keeps_the_failure_card_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = on_endpoint(dir.path(), vec![limited(), reply("done")]);
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(panel.switch_connection("cli"));
    assert!(panel.external);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));
}

/// A wait out of a limit survives it too, so `ask_else_wait` keeps counting
/// toward the reset while the endpoint is changed under it.
#[test]
fn a_connection_switch_keeps_a_wait_and_the_goal_waiting_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = on_endpoint(dir.path(), vec![limited(), reply("done")]);
    panel.on_limit = LimitPolicy::Wait;
    type_text(&mut panel, "/goal get it done");
    panel.submit();
    settle(&mut panel);
    assert!(panel.retry_wait.is_some(), "the reset is waited for");

    assert!(panel.switch_connection("cloud"));
    let wait = panel.retry_wait.expect("the wait is kept");
    assert!(wait.wall > crate::failure::unix_now() + 3_000);
    assert!(panel.goal_task.is_some(), "the goal keeps waiting on it");
}

/// Leaving for another session is not a rebuild: the failed request stays
/// with the log it failed on, so its card and its wait go with it.
#[test]
fn another_session_takes_the_failure_card_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = on_endpoint(dir.path(), vec![limited()]);
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(matches!(panel.pending, Some(Pending::Failure { .. })));

    panel.switch_session(None);
    assert!(
        !matches!(panel.pending, Some(Pending::Failure { .. })),
        "a fresh session owes no answer for another's request"
    );
    assert!(!panel.retry_ready);
    assert!(panel.retry_wait.is_none());
}

/// `ask_else_wait`: the card asks, and left unanswered until the reset it
/// tries the request again by itself.
#[test]
fn an_unanswered_card_tries_again_at_the_reset() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.on_limit = LimitPolicy::AskElseWait;
    panel.send("do it".to_string());
    settle(&mut panel);

    let at = auto_at(&panel).expect("the card has a time");
    assert!(
        at > crate::failure::unix_now() + 3_000,
        "the reset, not sooner"
    );
    let (_, detail) = failure_card(&panel);
    assert!(detail.contains("No answer by then"), "{detail}");
    panel.tick();
    assert!(!panel.is_busy(), "nothing goes before its time");

    // The reset comes, the card still unanswered.
    set_auto_at(&mut panel, crate::failure::unix_now());
    panel.tick();
    settle(&mut panel);
    assert!(panel.pending.is_none());
    assert!(answered_done(&panel));
}

#[test]
fn an_answered_card_does_not_try_again_by_itself() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.on_limit = LimitPolicy::AskElseWait;
    panel.send("do it".to_string());
    settle(&mut panel);
    set_auto_at(&mut panel, crate::failure::unix_now());
    panel.apply_form_action(ChoiceAction::Cancelled);
    panel.tick();
    settle(&mut panel);
    assert!(!answered_done(&panel), "stopping is the answer");
}

#[test]
fn the_ask_policy_never_tries_again_by_itself() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.send("do it".to_string());
    settle(&mut panel);
    assert_eq!(auto_at(&panel), None);
}

/// Without a stated reset the card's time is the backoff a wait would take.
#[test]
fn an_unanswered_card_without_a_reset_waits_the_backoff() {
    let outage = AssistantMessage::failed("scripted", "m", StopReason::Error, "network down")
        .with_failure(termide_agent_core::Failure::new(
            termide_agent_core::FailureKind::Transient,
        ));
    let mut panel = panel(vec![outage, reply("done")]);
    panel.on_limit = LimitPolicy::AskElseWait;
    let before = crate::failure::unix_now();
    panel.send("do it".to_string());
    settle(&mut panel);
    let at = auto_at(&panel).expect("the card has a time");
    assert!(
        (before + 15..=crate::failure::unix_now() + 15).contains(&at),
        "{at} vs {before}"
    );
}

/// A card brought back on reopen whose reset passed while termide was
/// closed tries again after a short grace, not at once.
#[test]
fn a_reopened_card_past_its_reset_tries_again_after_a_grace() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![limited()])
    });
    let path = panel.session_path().unwrap().to_path_buf();
    panel.send("do it".to_string());
    settle(&mut panel);
    set_reset(&mut panel, crate::failure::unix_now() - 60);
    // Logged with the reset gone by, as after a long close.
    panel.logged_retry = None;
    panel.sync_retry();
    drop(panel);

    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::open_exclusive(&path).unwrap()),
        on_limit: LimitPolicy::AskElseWait,
        ..setup(vec![reply("done")])
    });
    let now = crate::failure::unix_now();
    let at = auto_at(&panel).expect("the reopened card has a time");
    assert!(at > now && at <= now + 31, "{at} vs {now}");
    panel.tick();
    assert!(!panel.is_busy(), "not at once");
    set_auto_at(&mut panel, now);
    panel.tick();
    settle(&mut panel);
    assert!(answered_done(&panel));
}
