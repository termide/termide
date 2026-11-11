//! Sessions: resuming, naming, listing, switching, and the session slash commands.

use super::*;

#[test]
fn a_resumed_block_shows_its_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("earlier question")))
        .unwrap();
    session
        .append_message(&Message::Assistant(reply("earlier answer")))
        .unwrap();
    let path = session.path().to_path_buf();
    // Reopen: the restored blocks carry the wall-clock time from the log.
    let session = Session::open(&path).unwrap();
    let mut transcript = Transcript::default();
    for logged in &session.context_messages_with_times(&CompactionPrompts::default()) {
        push_history(&mut transcript, logged);
    }
    let has_time = transcript
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { at, .. } if !at.is_empty()));
    assert!(has_time, "a resumed answer keeps its time");
}

#[test]
fn a_resumed_session_keeps_its_timing() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    let tool_call = ToolCall {
        id: "t1".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "ls" }),
        extra_content: None,
    };
    let mut turn = reply("");
    turn.content = vec![
        AssistantContent::thinking("let me look"),
        AssistantContent::ToolCall(tool_call.clone()),
    ];
    session
        .append_timed_message(
            &Message::Assistant(turn),
            Some(Timing::Turn {
                prefill_ms: 1000,
                gen_ms: 2000,
            }),
        )
        .unwrap();
    session
        .append_timed_message(
            &Message::ToolResult(ToolResultMessage::text(&tool_call, "a\nb")),
            Some(Timing::Tool {
                duration_ms: 4000,
                waited_ms: Some(9000),
            }),
        )
        .unwrap();
    let session = Session::open(session.path()).unwrap();
    let mut transcript = Transcript::default();
    for logged in &session.context_messages_with_times(&CompactionPrompts::default()) {
        push_history(&mut transcript, logged);
    }
    // The turn's cost comes back from the timing and the turn's usage.
    assert!(transcript.items().iter().any(|item| matches!(
        item,
        Item::Thinking { cost: Some(cost), .. }
            if cost.prefill_ms == 1000 && cost.gen_ms == 2000 && cost.input == 100
    )));
    assert_eq!(transcript.tool_duration("t1"), Some(4000));
    assert_eq!(transcript.tool_wait("t1"), Some(9000));
}

#[test]
fn a_custom_agent_names_the_title() {
    let mut panel = panel(vec![reply("ok")]);
    // The default agent shows the localized "Agent" label.
    assert_eq!(panel.title(), "Agent: /tmp");
    // A custom agent replaces the label with its own capitalized name.
    assert!(panel.switch_agent("review"));
    assert_eq!(panel.title(), "Review: /tmp");
}

#[test]
fn a_named_session_titles_the_panel() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });

    type_text(&mut panel, "first request");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(panel.title(), "Agent: first request");

    // The context menu raises an input prompt carrying our action.
    let (label, action) = panel
        .context_menu_items()
        .into_iter()
        .find(|(_, action)| *action == RENAME_ACTION)
        .expect("a rename entry in the menu");
    assert_eq!(label, "Rename session");
    let events = panel.handle_status_action(&action);
    let Some(PanelEvent::ShowInput { on_submit, .. }) = events.first() else {
        panic!("expected an input prompt, got {events:?}");
    };
    let InputAction::Custom(submit_action) = on_submit else {
        panic!("expected a custom action");
    };
    assert_eq!(submit_action, RENAME_ACTION);

    // The submitted name wins over the first prompt and survives a reopen.
    let result = panel.handle_command(PanelCommand::InputSubmitted {
        action: submit_action.clone(),
        text: "  timeout work  ".into(),
    });
    assert!(matches!(result, CommandResult::Handled(true)));
    assert_eq!(panel.title(), "Agent: timeout work");
    let path = panel.session_path().unwrap().to_path_buf();
    drop(panel);
    assert_eq!(Session::open(&path).unwrap().name(), Some("timeout work"));

    // Without a session log there is nothing to record the name in.
    let mut logless = AgentPanel::new(setup(vec![]));
    assert!(!logless.rename_session("x"));
}

#[test]
fn sessions_can_be_listed_switched_and_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    // A session is created eagerly, so the log exists before the first
    // prompt.
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // The panel menu is trimmed to the actions with no home elsewhere;
    // "New session" starts an empty one and leaves the old log alone.
    let items = panel.context_menu_items();
    let labels: Vec<&str> = items.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "Session info",
            "Rename session",
            "Save chat as Markdown…",
            "Fork session",
            "Delete session"
        ]
    );
    panel.handle_status_action(NEW_SESSION_ACTION);
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert_eq!(panel.title(), "Agent: /tmp");

    // The picker lists both, newest first, marking the current one.
    let events = panel.handle_status_action(RESUME_ACTION);
    let Some(PanelEvent::ShowSelect {
        options, on_select, ..
    }) = events.first()
    else {
        panic!("expected a picker, got {events:?}");
    };
    assert_eq!(options.len(), 2);
    assert!(options[0].starts_with("● "), "{:?}", options[0]);
    assert!(options[1].contains("first task"), "{:?}", options[1]);
    let SelectAction::Custom(action) = on_select else {
        panic!("expected a custom action");
    };

    // Choosing the older one replays its transcript into the panel.
    let result = panel.handle_command(PanelCommand::SelectionMade {
        action: action.clone(),
        index: 1,
    });
    assert!(matches!(result, CommandResult::Handled(true)));
    assert_eq!(panel.session_path().unwrap(), first_path);
    assert_eq!(panel.title(), "Agent: first task");
    let items = panel.transcript().items();
    assert!(matches!(&items[0], Item::User { text, .. } if text == "first task"));
    assert!(items
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, .. } if text == "one")));

    // The resumed agent keeps the old messages as context.
    type_text(&mut panel, "second task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let reopened = Session::open(&first_path).unwrap();
    assert_eq!(
        roles(&reopened.context_messages()),
        vec!["user", "assistant", "user", "assistant"]
    );
}

#[test]
fn slash_new_starts_a_fresh_session_and_keeps_the_old() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one")])
    });
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // /new opens a fresh session; the used one is left on disk to resume.
    type_text(&mut panel, "/new");
    panel.submit();
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert!(first_path.exists(), "the previous session log is kept");
    assert_eq!(panel.session_list().len(), 2);
}

#[test]
fn slash_clear_discards_the_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one")])
    });
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // /clear deletes the current session and starts a fresh one, so there
    // is nothing to resume back to: only the new empty session remains.
    type_text(&mut panel, "/clear");
    panel.submit();
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert!(!first_path.exists(), "the previous session log is removed");
    assert_eq!(panel.session_list().len(), 1);
}

#[test]
fn slash_rename_and_name_set_the_session_title() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    // With an argument, both /rename and /name set the title directly.
    type_text(&mut panel, "/rename my work");
    panel.submit();
    assert_eq!(panel.title(), "Agent: my work");
    type_text(&mut panel, "/name other");
    panel.submit();
    assert_eq!(panel.title(), "Agent: other");
    assert!(panel.input_text().is_empty());
    // A fork's counter moves next to the label, where cutting the title
    // from its end leaves it; brackets that are no counter stay put.
    type_text(&mut panel, "/rename other (2)");
    panel.submit();
    assert_eq!(panel.title(), "Agent (2): other");
    type_text(&mut panel, "/rename fix (1)");
    panel.submit();
    assert_eq!(panel.title(), "Agent: fix (1)");

    // Without an argument, it opens the rename prompt instead of sending.
    type_text(&mut panel, "/rename");
    let events = panel.submit();
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowInput { .. })));
    assert!(panel.transcript().items().is_empty(), "nothing was sent");
}
