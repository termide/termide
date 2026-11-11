//! Modes, models, agents, the system prompt and folding blocks.

use super::*;

#[test]
fn delegated_tasks_follow_the_session_mode() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(ModeWatcher(Arc::clone(&seen))),
        ..setup(vec![])
    });
    // Told the starting mode, then every change.
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert!(panel.switch_agent("review"));
    assert_eq!(
        *seen.lock().unwrap(),
        vec![Mode::Configured, Mode::Auto, Mode::Edit]
    );
}

#[test]
fn mode_switches_from_the_chip_and_with_shift_tab() {
    let mut panel = panel(vec![]);
    let hooks_mode = panel.mode.clone();
    assert_eq!(chip(&panel, MODE_ACTION), "configured");

    // Shift+Tab cycles and reports the new mode in the status line.
    let events = panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert!(events.iter().any(|e| matches!(
        e,
        PanelEvent::SetStatusMessage { message, .. } if message.ends_with("auto")
    )));
    assert_eq!(chip(&panel, MODE_ACTION), "auto");
    assert_eq!(hooks_mode.get(), Mode::Auto);

    // The chip opens a picker with the current mode marked.
    let events = panel.handle_status_action(MODE_ACTION);
    let picker = events.first().expect("picker");
    let PanelEvent::ShowSelect { options, .. } = picker else {
        panic!("expected a picker, got {picker:?}");
    };
    assert_eq!(options.len(), 6);
    assert!(options[4].starts_with("● auto"), "{:?}", options[4]);
    assert!(options[1].starts_with("  plan"), "{:?}", options[1]);
    assert!(matches!(
        select(&mut panel, picker, 2),
        CommandResult::Handled(true)
    ));
    assert_eq!(chip(&panel, MODE_ACTION), "edit");
    assert_eq!(hooks_mode.get(), Mode::Edit);
    let events = panel.tick();
    assert!(events.iter().any(|e| matches!(
        e,
        PanelEvent::SetStatusMessage { message, .. } if message.ends_with("edit")
    )));

    // Cycling wraps from all to ask, and a rebuilt agent starts in the
    // chosen mode.
    for _ in 0..3 {
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    }
    assert_eq!(chip(&panel, MODE_ACTION), "all");
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(chip(&panel, MODE_ACTION), "ask");
    panel.switch_session(None);
    assert_eq!(chip(&panel, MODE_ACTION), "ask");
    assert_eq!(panel.mode.get(), Mode::Ask);
}

#[test]
fn model_switches_are_recorded_and_followed_on_resume() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(Scripted::new(vec![reply("one"), reply("two")]));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup_with(Arc::clone(&provider))
    });
    let first_path = panel.session_path().unwrap().to_path_buf();
    // A fresh session records the model it starts on.
    assert_eq!(
        Session::open(&first_path).unwrap().current_model(),
        Some(termide_agent_core::SessionModel {
            provider: "openai_compatible".into(),
            id: "m".into(),
            context_window: Some(1000),
        })
    );
    assert_eq!(chip(&panel, MODEL_ACTION), "m");

    // The chip fetches the list off-thread; the picker marks the current
    // model, names a model by the name the endpoint gives it beside its id,
    // and ends with the typed-id entry.
    let events = panel.handle_status_action(MODEL_ACTION);
    assert!(matches!(
        events.first(),
        Some(PanelEvent::SetStatusMessage { .. })
    ));
    let picker = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowSelect { .. }));
    let PanelEvent::ShowSelect { options, .. } = &picker else {
        unreachable!()
    };
    assert_eq!(
        options,
        &["  Big Model · big", "● m", "  Enter a model id…"]
    );
    select(&mut panel, &picker, 0);
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    // The endpoint's context window comes along with the id.
    assert_eq!(panel.model.context_window, 64_000);
    // A fresh session updates the banner in place: the switch leaves no
    // notice, but it is still recorded in the session log.
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Notice { .. })));
    assert_eq!(
        Session::open(&first_path)
            .unwrap()
            .current_model()
            .map(|m| m.id),
        Some("big".to_string())
    );

    // The next run goes to the new model.
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(
        *provider.seen_models.lock().unwrap(),
        vec!["big".to_string()]
    );

    // A new session starts on the current model; the last picker entry
    // asks for an id by hand.
    panel.handle_status_action(NEW_SESSION_ACTION);
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    panel.handle_status_action(MODEL_ACTION);
    let picker = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowSelect { .. }));
    select(&mut panel, &picker, 2);
    let input = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowInput { .. }));
    let PanelEvent::ShowInput {
        initial_value,
        on_submit: InputAction::Custom(action),
        ..
    } = input
    else {
        panic!("expected an input prompt, got {input:?}");
    };
    assert_eq!(initial_value, "big");
    panel.handle_command(PanelCommand::InputSubmitted {
        action,
        text: " typed ".to_string(),
    });
    assert_eq!(chip(&panel, MODEL_ACTION), "typed");
    // A typed id keeps the window of the model it replaced.
    assert_eq!(panel.model.context_window, 64_000);

    // Reopening the first session returns to the model it last used.
    panel.session_choices = panel.session_list();
    let index = panel
        .session_choices
        .iter()
        .position(|s| s.path == first_path)
        .unwrap();
    panel.resume_choice(index);
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    // The window comes back from the log too.
    assert_eq!(panel.model.context_window, 64_000);
}

#[test]
fn model_picker_falls_back_to_a_typed_id() {
    let mut provider = Scripted::new(vec![]);
    provider.models = Err("HTTP 404: no such route".into());
    let mut panel = AgentPanel::new(setup_with(Arc::new(provider)));
    panel.handle_status_action(MODEL_ACTION);
    let input = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowInput { .. }));
    assert!(matches!(input, PanelEvent::ShowInput { initial_value, .. } if initial_value == "m"));
    assert!(panel.transcript().items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text.contains("HTTP 404")
    )));
}

#[test]
fn saved_state_names_the_directory_and_the_session_log() {
    let no_log = panel(vec![]);
    assert_eq!(
        no_log.to_state(Path::new("/unused")),
        Some(termide_core::PanelState::Agent {
            cwd: PathBuf::from("/tmp"),
            session: None,
            agent: None,
            setup: termide_core::AgentSetupState::default(),
        })
    );

    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("done")])
    });
    type_text(&mut panel, "task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let Some(termide_core::PanelState::Agent { cwd, session, .. }) =
        panel.to_state(Path::new("/unused"))
    else {
        panic!("agent state expected");
    };
    assert_eq!(cwd, PathBuf::from("/tmp"));
    let session = session.expect("session path");
    assert_eq!(panel.session_path(), Some(session.as_path()));

    // Rebuilding from that state brings the conversation back.
    let restored = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::open(&session).unwrap()),
        ..setup(vec![])
    });
    assert_eq!(restored.title(), "Agent: task");
    assert_eq!(restored.session_path(), Some(session.as_path()));
}

#[test]
fn a_successful_edit_reports_the_changed_file() {
    use termide_agent_core::ToolCall;
    let mut panel = panel(vec![]);
    let call = |name: &str| ToolCall {
        id: format!("{name}-1"),
        name: name.into(),
        arguments: serde_json::json!({}),
        extra_content: None,
    };
    let changed = |events: &[PanelEvent]| -> Vec<PathBuf> {
        events
            .iter()
            .filter_map(|e| match e {
                PanelEvent::FileChangedOnDisk(path) => Some(path.clone()),
                _ => None,
            })
            .collect()
    };

    let edit = call("edit");
    panel.apply(AgentEvent::ToolExecutionStart { call: edit.clone() });
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&edit, "Edited")
            .with_details(serde_json::json!({ "path": "/tmp/f.rs", "replacements": 1 })),
    });
    assert_eq!(changed(&panel.tick()), vec![PathBuf::from("/tmp/f.rs")]);

    // A failed edit, a read and a shell command report nothing.
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::error(&edit, "no match")
            .with_details(serde_json::json!({ "path": "/tmp/f.rs" })),
    });
    let read = call("read");
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&read, "…")
            .with_details(serde_json::json!({ "path": "/tmp/g.rs" })),
    });
    let bash = call("shell");
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&bash, "ok"),
    });
    assert!(changed(&panel.tick()).is_empty());
}

#[test]
fn the_system_prompt_can_be_opened_as_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        system_prompt: "You are terse.\n".into(),
        ..setup(vec![])
    });
    // It has no menu slot; `/prompt` is its entry point.
    type_text(&mut panel, "/prompt");
    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let Some(PanelEvent::ViewFile(path)) = events.first() else {
        panic!("expected a viewer, got {events:?}");
    };
    assert_eq!(path, &dir.path().join("system-prompt.md"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "You are terse.\n");
    // The prompt file is not mistaken for a session.
    assert!(panel.session_list().iter().all(|s| s.path != *path));
}

#[test]
fn a_system_prompt_block_shows_once_and_on_change() {
    let mut panel = AgentPanel::new(AgentPanelSetup {
        system_prompt: "Base prompt.".into(),
        ..setup(vec![reply("a"), reply("b"), reply("c")])
    });
    let count = |p: &AgentPanel| {
        p.transcript()
            .items()
            .iter()
            .filter(|i| matches!(i, Item::System { .. }))
            .count()
    };

    type_text(&mut panel, "hi");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(count(&panel), 1, "shown with the first message");

    // Unchanged prompt: not repeated.
    type_text(&mut panel, "again");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(count(&panel), 1, "not repeated when unchanged");

    // A different agent has a different prompt: shown again.
    assert!(panel.switch_agent("review"));
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(count(&panel), 2, "shown again after it changed");
}

#[test]
fn the_header_shows_the_agents_own_icon_or_the_robot() {
    let mut panel = panel(vec![]);
    let (theme, config) = (Theme::default(), Arc::new(Config::default()));
    panel.prepare_render(&theme, &config);
    assert_eq!(panel.icon(), Some(AGENT_ICON));

    let events = panel.handle_status_action(AGENT_ACTION);
    let picker = events.first().expect("picker");
    select(&mut panel, picker, 1);
    // Until the next frame looks the agent up, the header keeps the robot
    // rather than another agent's icon.
    assert_eq!(panel.icon(), Some(AGENT_ICON));
    panel.prepare_render(&theme, &config);
    assert_eq!(panel.icon(), Some("🔍"));
}

#[test]
fn switching_agents_changes_prompt_model_and_mode_and_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(Scripted::new(vec![reply("ok")]));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup_with(Arc::clone(&provider))
    });
    assert_eq!(chip(&panel, AGENT_ACTION), "default");

    let events = panel.handle_status_action(AGENT_ACTION);
    let picker = events.first().expect("picker");
    let PanelEvent::ShowSelect { options, .. } = picker else {
        panic!("expected a picker, got {picker:?}");
    };
    assert_eq!(
        options,
        &[
            "● default",
            "  review · Reviews diffs",
            "  outside · An external agent"
        ]
    );
    assert!(matches!(
        select(&mut panel, picker, 1),
        CommandResult::Handled(true)
    ));

    assert_eq!(chip(&panel, AGENT_ACTION), "review");
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    assert_eq!(chip(&panel, MODE_ACTION), "edit");
    assert_eq!(panel.system_prompt, "You review diffs.");
    // A fresh session keeps its banner: the switch leaves no notice (it is
    // still applied and recorded, checked below on resume).
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Notice { .. })));

    // The next run goes to the reviewer's model, and the layout state
    // names the agent so a restore comes back as it.
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(
        *provider.seen_models.lock().unwrap(),
        vec!["big".to_string()]
    );
    let Some(termide_core::PanelState::Agent { agent, .. }) = panel.to_state(Path::new("/unused"))
    else {
        panic!("agent state expected");
    };
    assert_eq!(agent.as_deref(), Some("review"));
    assert_eq!(
        Session::open(panel.session_path().unwrap())
            .unwrap()
            .current_model()
            .unwrap()
            .id,
        "big"
    );

    // A new session starts as the current agent; reopening the first
    // one comes back as the agent it last ran as, prompt and tools too.
    let first_path = panel.session_path().unwrap().to_path_buf();
    panel.handle_status_action(NEW_SESSION_ACTION);
    assert_eq!(chip(&panel, AGENT_ACTION), "review");
    panel.switch_agent("default");
    assert_eq!(panel.system_prompt, "default prompt");
    assert_eq!(chip(&panel, MODEL_ACTION), "big", "the model stays");
    assert_eq!(chip(&panel, MODE_ACTION), "edit", "the mode stays");
    panel.session_choices = panel.session_list();
    let index = panel
        .session_choices
        .iter()
        .position(|s| s.path == first_path)
        .unwrap();
    panel.resume_choice(index);
    assert_eq!(chip(&panel, AGENT_ACTION), "review");
    assert_eq!(panel.system_prompt, "You review diffs.");
    assert_eq!(
        Session::open(&first_path)
            .unwrap()
            .current_agent()
            .as_deref(),
        Some("review")
    );
    assert!(!panel.switch_agent("missing"));
}

#[test]
fn o_opens_the_selected_block_in_a_read_only_panel() {
    let mut panel = AgentPanel::new(setup(vec![reply("the answer here")]));
    type_text(&mut panel, "do it");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE)); // chat focus, last block (assistant)
    let events = panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::NONE));
    let path = events.iter().find_map(|e| match e {
        PanelEvent::ViewFile(path) => Some(path.clone()),
        _ => None,
    });
    let path = path.expect("o should open a ViewFile panel");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("the answer here"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn tab_moves_focus_to_the_chat_and_arrows_fold_blocks() {
    // The answer folds only when its reasoning is long enough to be worth
    // hiding.
    let thinking = "l1\nl2\nl3\nl4\nl5\nl6";
    let mut panel = AgentPanel::new(setup(vec![reply_thinking("the answer", thinking)]));
    type_text(&mut panel, "do it");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // A user block and an assistant block exist.
    assert!(panel.transcript().items().len() >= 2);
    assert!(!panel.chat_focus);

    // Tab moves focus to the chat, on the last (answer) block, which is not
    // foldable. Blocks are user(0), thinking(1), answer(2); the clean run
    // closed on the answer, so its total sits there, not on a closing line.
    assert!(matches!(
        panel.transcript().items().last(),
        Some(Item::Assistant {
            run_ms: Some(_),
            ..
        })
    ));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(panel.chat_focus);
    assert_eq!(panel.selected, 2);
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.selected, 2);

    // Up walks to the reasoning block, which folds; Space expands it, again
    // folds it.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.selected, 1);
    assert!(!panel.transcript().any_expanded());
    panel.handle_key(chord(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(panel.transcript().any_expanded());
    panel.handle_key(chord(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(!panel.transcript().any_expanded());

    // Right unfolds it and Left folds it, as in the file manager's tree; a
    // second press in the same direction leaves it as it is.
    panel.handle_key(chord(KeyCode::Right, KeyModifiers::NONE));
    assert!(panel.transcript.is_expanded(1));
    panel.handle_key(chord(KeyCode::Right, KeyModifiers::NONE));
    assert!(panel.transcript.is_expanded(1));
    panel.handle_key(chord(KeyCode::Left, KeyModifiers::NONE));
    assert!(!panel.transcript.is_expanded(1));
    panel.handle_key(chord(KeyCode::Left, KeyModifiers::NONE));
    assert!(!panel.transcript.is_expanded(1));
    assert!(panel.input_text().is_empty(), "the arrows stay in the chat");

    // Up walks to the user block.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.selected, 0);

    // Tab returns focus to the input.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(!panel.chat_focus);
    type_text(&mut panel, "hi");
    assert_eq!(panel.input_text(), "hi");
}

#[test]
fn ctrl_o_sets_how_fresh_blocks_fold() {
    let thinking = "l1\nl2";
    let mut panel = AgentPanel::new(setup(vec![
        reply_thinking("one", thinking),
        reply_thinking("two", thinking),
        reply_thinking("three", thinking),
    ]));
    let turn = |panel: &mut AgentPanel, text: &str| {
        type_text(panel, text);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(panel);
    };
    let reasoning_expanded = |panel: &AgentPanel| -> Vec<bool> {
        let items = panel.transcript().items();
        (0..items.len())
            .filter(|&i| matches!(items[i], Item::Thinking { .. }))
            .map(|i| panel.transcript.is_expanded(i))
            .collect()
    };
    turn(&mut panel, "first");
    assert_eq!(reasoning_expanded(&panel), [false]);
    // Unfolding everything has the next turn arrive unfolded too.
    panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::CONTROL));
    turn(&mut panel, "second");
    assert_eq!(reasoning_expanded(&panel), [true, true]);
    // Folding everything back returns fresh blocks to the configured mode.
    panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::CONTROL));
    turn(&mut panel, "third");
    assert_eq!(reasoning_expanded(&panel), [false, false, false]);
}
