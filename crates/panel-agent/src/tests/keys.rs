//! The prompt keys and the F-key commands: rename, delete, new, fork, summary and rewind.

use super::*;

#[test]
fn shift_enter_adds_a_line_and_esc_clears() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "one");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::SHIFT));
    type_text(&mut panel, "two");
    assert_eq!(panel.input_text(), "one\ntwo");
    let rows = render_text(&mut panel, 30, 6);
    assert!(rows[4].starts_with("› one"));
    assert!(rows[5].starts_with("  two"));

    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.input_text().is_empty());
    // With nothing to rewind to either, Esc does nothing, and the panel
    // keeps it rather than being closed by it.
    assert!(panel.captures_escape());
    assert!(panel
        .handle_key(chord(KeyCode::Esc, KeyModifiers::NONE))
        .is_empty());
}

/// Mid-run, the first Esc clears what is typed; only the next one stops.
#[test]
fn esc_clears_the_typed_prompt_before_stopping_a_run() {
    let mut panel = panel(vec![]);
    panel.busy = true;
    type_text(&mut panel, "next request");
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.input_text().is_empty());
    assert!(panel.stop_requested.is_none());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.stop_requested.is_some());
}

#[test]
fn a_large_paste_is_held_as_a_placeholder_and_expanded() {
    let mut panel = panel(vec![]);
    let big = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");

    // A large paste shows a short placeholder, not the whole block.
    panel.handle_command(PanelCommand::PasteText { text: big.clone() });
    assert_eq!(panel.input_text(), "[#1 pasted 40 lines]");
    // Its full text is spliced back for the message.
    assert_eq!(panel.expand_pastes(&panel.input_text()), big);

    // A small paste is inlined as-is, beside the placeholder.
    panel.handle_command(PanelCommand::PasteText {
        text: " review this".into(),
    });
    assert_eq!(panel.input_text(), "[#1 pasted 40 lines] review this");
    assert_eq!(
        panel.expand_pastes(&panel.input_text()),
        format!("{big} review this")
    );

    // Submitting sends the expanded text and drops the held paste.
    panel.submit();
    assert!(panel.input_text().is_empty());
    assert!(panel.pastes.is_empty());
    assert_eq!(panel.paste_seq, 0);
}

#[test]
fn f2_opens_the_rename_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    let events = panel.handle_key(chord(KeyCode::F(2), KeyModifiers::NONE));
    let Some(PanelEvent::ShowInput { on_submit, .. }) = events.first() else {
        panic!("F2 should open the rename prompt, got {events:?}");
    };
    assert!(
        matches!(on_submit, InputAction::Custom(a) if a == RENAME_ACTION),
        "{on_submit:?}"
    );
}

#[test]
fn f8_confirms_in_a_modal_then_deletes_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let first_path = panel.session_path().unwrap().to_path_buf();

    // F8 asks through an app confirmation modal — nothing is deleted yet,
    // and the panel raises no in-panel card of its own.
    let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    let Some(PanelEvent::ShowConfirm {
        on_confirm,
        message,
    }) = events.first()
    else {
        panic!("F8 should raise a confirmation modal, got {events:?}");
    };
    // An unnamed session is named in the UI language, its id below.
    let t = termide_i18n::t();
    let id = first_path.file_stem().unwrap().to_str().unwrap();
    assert_eq!(
        *message,
        format!(
            "{}\n{id}",
            t.agent_delete_confirm_fmt(t.agent_delete_this_session())
        )
    );
    assert!(
        matches!(on_confirm, ConfirmAction::Custom(a) if a == DELETE_SESSION_ACTION),
        "{on_confirm:?}"
    );
    assert!(panel.pending.is_none());
    assert!(first_path.exists());

    // The confirmed answer comes back as a Confirmed command: the log is
    // removed and a fresh session starts.
    panel.handle_command(PanelCommand::Confirmed {
        action: DELETE_SESSION_ACTION.to_string(),
    });
    settle(&mut panel);
    assert!(!first_path.exists(), "the session log is removed");
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert!(panel.transcript().items().is_empty());

    // Cancelling the modal (no Confirmed command) keeps the session.
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let path = panel.session_path().unwrap().to_path_buf();
    panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    assert!(path.exists(), "cancelled delete keeps the log");
}

#[test]
fn f7_starts_a_new_session_and_f6_opens_the_switcher() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "task one");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let first = panel.session_path().unwrap().to_path_buf();

    // F7 opens a fresh session, keeping the used one.
    panel.handle_key(chord(KeyCode::F(7), KeyModifiers::NONE));
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first);

    // F6 opens the session switcher.
    let events = panel.handle_key(chord(KeyCode::F(6), KeyModifiers::NONE));
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
}

#[test]
fn f5_forks_the_session_behind_a_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let first = panel.session_path().unwrap().to_path_buf();

    // F5 asks through an app confirmation modal — nothing is copied yet.
    let events = panel.handle_key(chord(KeyCode::F(5), KeyModifiers::NONE));
    let Some(PanelEvent::ShowConfirm {
        on_confirm,
        message,
    }) = events.first()
    else {
        panic!("F5 should raise a confirmation modal, got {events:?}");
    };
    assert!(
        matches!(on_confirm, ConfirmAction::Custom(a) if a == FORK_SESSION_ACTION),
        "{on_confirm:?}"
    );
    let t = termide_i18n::t();
    let id = first.file_stem().unwrap().to_str().unwrap();
    assert_eq!(
        *message,
        format!(
            "{}\n{id}",
            t.agent_fork_confirm_fmt(t.agent_fork_this_session())
        )
    );
    assert_eq!(session_files(&dir), vec![first.clone()]);

    // The confirmed answer asks the app to open a copy; this panel keeps its
    // own session, and the copy is the app's to make.
    panel.handle_command(PanelCommand::Confirmed {
        action: FORK_SESSION_ACTION.to_string(),
    });
    let event = wait_for(&mut panel, |e| {
        matches!(e, PanelEvent::ForkAgentSession { .. })
    });
    let PanelEvent::ForkAgentSession {
        session,
        cwd,
        focus,
    } = event
    else {
        unreachable!()
    };
    assert_eq!(session, first);
    assert_eq!(cwd, panel.cwd);
    assert_eq!(focus, None);
    assert_eq!(panel.session_path().unwrap(), first);
    assert_eq!(session_files(&dir), vec![first.clone()]);

    // Cancelling the modal asks and leaves the panel where it stood.
    panel.handle_key(chord(KeyCode::F(5), KeyModifiers::NONE));
    assert_eq!(panel.session_path().unwrap(), first);
}

#[test]
fn fork_copies_the_conversation_into_a_log_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let first = panel.session_path().unwrap().to_path_buf();

    // What the app does on the confirmed fork: copy, then open the copy. The
    // name it gives the copy is the app's concern (`fork_name`); `None` here
    // takes the mechanics alone.
    let copy = Session::open(&first)
        .unwrap()
        .fork(&panel.cwd, None)
        .unwrap();
    let forked = Session::open_exclusive(&copy).unwrap();
    assert_ne!(forked.id(), panel.session.as_ref().unwrap().id());
    assert_eq!(forked.context_messages().len(), 2);
    // No second panel takes the same copy, and the source stays open here.
    assert!(Session::open_exclusive(&copy).is_err());
    assert!(Session::is_open(&first));

    // The two logs part at once: a later request of this panel is not in the
    // copy, and the copy's own directory is where its work goes.
    assert_eq!(forked.header().cwd, panel.cwd);
    drop(forked);
    assert!(Session::open_exclusive(&copy).is_ok());
}

#[test]
fn fork_without_a_session_log_says_so() {
    // No session directory: the panel runs without a log to copy.
    let mut panel = panel(vec![reply("ok")]);
    let events = panel.handle_key(chord(KeyCode::F(5), KeyModifiers::NONE));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PanelEvent::ShowConfirm { .. })),
        "{events:?}"
    );
    assert!(panel.transcript().items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text == termide_i18n::t().agent_notice_fork_no_session()
    )));
}

#[test]
fn fork_with_a_focus_hands_it_to_the_copy() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    type_text(&mut panel, "/fork the log format");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    panel.handle_command(PanelCommand::Confirmed {
        action: FORK_SESSION_ACTION.to_string(),
    });
    let event = wait_for(&mut panel, |e| {
        matches!(e, PanelEvent::ForkAgentSession { .. })
    });
    let PanelEvent::ForkAgentSession { focus, .. } = event else {
        unreachable!()
    };
    assert_eq!(focus.as_deref(), Some("the log format"));

    // The focus goes with that confirmation alone: F5 forks the copy whole.
    panel.handle_key(chord(KeyCode::F(5), KeyModifiers::NONE));
    panel.handle_command(PanelCommand::Confirmed {
        action: FORK_SESSION_ACTION.to_string(),
    });
    let event = wait_for(&mut panel, |e| {
        matches!(e, PanelEvent::ForkAgentSession { .. })
    });
    assert!(matches!(
        event,
        PanelEvent::ForkAgentSession { focus: None, .. }
    ));
}

#[test]
fn a_forks_first_request_waits_for_its_compaction() {
    let mut panel = panel(vec![reply("on it")]);
    // Too short to compact: the summary call fails, and the request goes on
    // the copy left whole — but only once the compaction is over.
    panel.start_fork(Some("the log format".into()), Some(fork_request("migrate")));
    assert!(!panel.is_busy());
    assert!(panel.fork_first.is_some());
    settle(&mut panel);
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.fork_first.is_some() || panel.is_busy() {
        assert!(Instant::now() < deadline, "the request did not go");
        panel.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    let failed =
        termide_i18n::t().agent_notice_compaction_failed_fmt("too few messages to compact");
    let items = panel.transcript().items();
    let notice = items
        .iter()
        .position(|item| matches!(item, Item::Notice { text, .. } if *text == failed))
        .expect("the compaction's notice");
    let request = items
        .iter()
        .position(
            |item| matches!(item, Item::Tool { call, .. } if call.arguments["kind"] == "fork"),
        )
        .expect("the fork's request, as the agent's message");
    assert!(notice < request);
    // Another agent's words: the count of messages passed without the user
    // goes on from the session that forked.
    assert_eq!(panel.agent_hops(), 1);
}

#[test]
fn a_fork_without_a_focus_starts_at_once() {
    let mut panel = panel(vec![reply("on it")]);
    panel.start_fork(None, Some(fork_request("migrate")));
    assert!(panel.is_busy());
    assert!(panel.fork_first.is_none());
    settle(&mut panel);
    // One the user forks by hand starts nothing.
    let mut quiet = self::panel(vec![]);
    quiet.start_fork(None, None);
    assert!(!quiet.is_busy());
}

#[test]
fn f3_shows_a_session_summary() {
    let mut panel = panel(vec![]);
    let events = panel.handle_key(chord(KeyCode::F(3), KeyModifiers::NONE));
    let Some(PanelEvent::ShowSessionUsage { rows }) = events.first() else {
        panic!("F3 should show a summary modal, got {events:?}");
    };
    for label in ["Provider", "Model", "Agent", "Directory", "Tokens"] {
        assert!(
            rows.iter().any(|(key, _)| key == label),
            "missing {label}: {rows:?}"
        );
    }
    assert!(
        rows.iter()
            .any(|(key, value)| key == "Model" && value == panel.model.id.as_str()),
        "{rows:?}"
    );
}

#[test]
fn f4_offers_the_messages_to_rewind_to() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    // Nothing asked yet: F4 says there is nothing to rewind to.
    panel.handle_key(chord(KeyCode::F(4), KeyModifiers::NONE));
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("rewind"))));

    type_text(&mut panel, "task one");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.handle_key(chord(KeyCode::F(4), KeyModifiers::NONE));
    assert_eq!(rewind_rows(&panel), [("task one".into(), String::new())]);
}

#[test]
fn esc_in_the_idle_empty_prompt_rewinds_to_a_chosen_message() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = asked(dir.path(), &["task one", "task two", "task three"], &[]);
    assert!(panel.captures_escape(), "Esc does not close the panel");
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    // The list stands right above the input, as the `/command` completions
    // do, the newest message next to it, so it is a single Enter away.
    let labels: Vec<String> = rewind_rows(&panel).into_iter().map(|(l, _)| l).collect();
    assert_eq!(labels, ["task one", "task two", "task three"]);
    let rows = render_text(&mut panel, 40, 16);
    let rule = rows.iter().rposition(|r| r.starts_with('─')).unwrap();
    assert!(rows[rule - 1].contains("task three"), "{rows:#?}");
    assert!(rows[rule - 3].contains("task one"), "{rows:#?}");

    // No files to put back: the conversation rewinds at once, and the
    // message comes back to be edited.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.rewind_picker.is_none());
    assert!(panel.pending.is_none());
    assert_eq!(panel.input_text(), "task two");
    assert_eq!(users(&panel), ["task one"]);
    assert_eq!(panel.session.as_ref().unwrap().context_messages().len(), 2);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text == "conversation rewound")));
    let reopened = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(reopened.context_messages().len(), 2);
}

#[test]
fn the_rewind_list_closes_on_esc_and_on_typing() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = asked(dir.path(), &["task one"], &[]);
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.rewind_picker.is_some());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.rewind_picker.is_none(), "Esc closes the list");
    assert_eq!(users(&panel), ["task one"], "and rewinds nothing");

    // Typing closes it and goes into the input: a new prompt.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    type_text(&mut panel, "next");
    assert!(panel.rewind_picker.is_none());
    assert_eq!(panel.input_text(), "next");
    assert_eq!(users(&panel), ["task one"]);
}

#[test]
fn rewinding_past_changed_files_asks_what_to_put_back() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "before").unwrap();
    let mut panel = asked(
        dir.path(),
        &["task one", "task two", "task three"],
        &[(1, file.as_path())],
    );
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    // Only rewinding to before task two or earlier takes the file back.
    assert_eq!(
        rewind_rows(&panel),
        [
            ("task one".into(), "notes.txt".into()),
            ("task two".into(), "notes.txt".into()),
            ("task three".into(), String::new()),
        ]
    );

    // Rewinding to before task three changes no file: no card.
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(users(&panel), ["task one", "task two"]);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "after task two");

    // Before task two, now the newest, the card names the file; the first
    // row puts back both the file and the conversation.
    panel.clear_input();
    rewind_with_esc(&mut panel, 0);
    let form = panel.pending.as_ref().expect("rewind card").form();
    assert!(form.title().contains("notes.txt"), "{}", form.title());
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    let events = panel.tick();
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::FileChangedOnDisk(p) if p == &file)));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    assert_eq!(users(&panel), ["task one"]);
    assert_eq!(panel.input_text(), "task two");
    assert_eq!(
        panel
            .checkpoints
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .undoable(),
        0
    );
}

#[test]
fn a_rewind_can_leave_the_files_or_the_conversation_alone() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "before").unwrap();
    let mut panel = asked(
        dir.path(),
        &["task one", "task two"],
        &[(1, file.as_path())],
    );
    let rewind_to = |panel: &mut AgentPanel, index: usize, row: char| {
        rewind_with_esc(panel, index);
        panel.handle_key(chord(KeyCode::Char(row), KeyModifiers::NONE));
        panel.tick()
    };

    // The files alone: the conversation goes on where it stands, and the
    // checkpoint is used up.
    rewind_to(&mut panel, 0, '3');
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    assert_eq!(users(&panel), ["task one", "task two"]);
    assert!(panel.input_text().is_empty());
    assert_eq!(
        panel
            .checkpoints
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .undoable(),
        0
    );

    // The conversation alone: the file keeps what the request wrote, and
    // the checkpoint goes with the branch it belonged to.
    std::fs::write(&file, "before").unwrap();
    let mut panel = asked(
        &dir.path().join("again"),
        &["task one", "task two"],
        &[(1, file.as_path())],
    );
    rewind_to(&mut panel, 0, '2');
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "after task two");
    assert_eq!(users(&panel), ["task one"]);
    assert_eq!(panel.input_text(), "task two");
    assert_eq!(
        panel
            .checkpoints
            .as_ref()
            .unwrap()
            .lock()
            .unwrap()
            .undoable(),
        0
    );
}
