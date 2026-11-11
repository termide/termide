//! Prompt history, slash completion, command scripts, undo and plan mode.

use super::*;

#[test]
fn arrow_keys_recall_earlier_requests_and_bring_the_draft_back() {
    let mut panel = panel(vec![reply("a"), reply("b")]);
    for request in ["first request", "second request"] {
        type_text(&mut panel, request);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
    }
    type_text(&mut panel, "half typ");
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "second request");
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "first request");
    // Past the oldest it stays; back down it returns to the draft.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "first request");
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "half typ");
    assert!(panel.history_pos.is_none());
    // Typing ends browsing; the arrows then move inside a multi-line input.
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::SHIFT));
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "half typ\nmore");
    assert_eq!(format_tokens(32_000), "32k");
    assert_eq!(format_tokens(262_144), "262k");
    assert_eq!(format_tokens(1_000_000), "1M");
    assert_eq!(format_tokens(1_250_000), "1.2M");
    assert_eq!(format_tokens(512), "512");
}

#[test]
fn up_walks_past_multi_line_requests_without_stepping_through_their_rows() {
    let mut panel = panel(vec![reply("a"), reply("b")]);
    for request in [["one", "two"], ["three", "four"]] {
        type_text(&mut panel, request[0]);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::SHIFT));
        type_text(&mut panel, request[1]);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
    }
    // Going back lands at the start, so the next ↑ is already history.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "three\nfour");
    {
        let c = panel.input_area().cursor();
        assert_eq!((c.row, c.col), (0, 0));
    }
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "one\ntwo");
    // ↓ first walks the rows, then moves on, landing at the end.
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "one\ntwo");
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "three\nfour");
    {
        let c = panel.input_area().cursor();
        assert_eq!((c.row, c.col), (1, 4));
    }
}

#[test]
fn up_moves_through_a_soft_wrapped_line_before_recalling() {
    let mut panel = panel(vec![reply("a")]);
    type_text(&mut panel, "earlier");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // One logical line, several rows once wrapped to a narrow panel.
    let long = "word ".repeat(30);
    type_text(&mut panel, &long);
    render_text(&mut panel, 40, 20);
    let end = panel.input_area().cursor();
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), long, "↑ stays in the wrapped draft");
    let up = panel.input_area().cursor();
    assert_eq!(up.row, 0);
    assert!(up.col < end.col, "{up:?} vs {end:?}");
    // Up to the first visual row, then history.
    for _ in 0..10 {
        render_text(&mut panel, 40, 20);
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    }
    assert_eq!(panel.input_text(), "earlier");
}

#[test]
fn typing_a_slash_offers_templates_and_tab_or_enter_completes() {
    let mut panel = panel(vec![reply("ok")]);
    type_text(&mut panel, "/re");
    let popup = panel.completion.as_ref().expect("popup");
    assert_eq!(popup.items()[0].value, "review");
    let rows = render_text(&mut panel, 60, 12);
    assert!(
        rows.iter()
            .any(|r| r.contains("/review <path>  Review a file")),
        "{rows:?}"
    );
    // Tab completes and closes the popup; the space invites arguments.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "/review ");
    assert!(panel.completion.is_none());

    // Enter on a partial name completes; on the full name it sends.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    type_text(&mut panel, "/rev");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "/review ");
    assert!(panel.transcript().items().is_empty());
    panel.handle_key(chord(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(
        panel.completion.is_some(),
        "a lone /review shows the popup again"
    );
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(matches!(
        panel.transcript().items().first(),
        Some(Item::User { text, .. }) if text == "Review  carefully."
    ));

    // No match, no popup; Esc closes an open one.
    type_text(&mut panel, "/zzz");
    assert!(panel.completion.is_none());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    type_text(&mut panel, "/r");
    assert!(panel.completion.is_some());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.completion.is_none());
    assert_eq!(
        panel.input_text(),
        "/r",
        "Esc closes the popup, not the input"
    );
}

#[test]
fn slash_compact_is_built_in_and_reports_through_the_transcript() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/comp");
    let popup = panel.completion.as_ref().expect("popup");
    assert!(popup.items().iter().any(|i| i.value == "compact"));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "/compact ");
    type_text(&mut panel, "the tests");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "", "the command is consumed, not sent");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.tick();
        let failed = panel.transcript().items().iter().any(
            |item| matches!(item, Item::Notice { text, .. } if text.contains("too few messages")),
        );
        if failed {
            break;
        }
        assert!(Instant::now() < deadline, "no compaction notice");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(panel
        .transcript()
        .items()
        .iter()
        .all(|i| !matches!(i, Item::User { .. })));
}

#[cfg(unix)]
#[test]
fn command_scripts_run_and_a_project_one_asks_first() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = |name: &str, body: &str, trusted: bool| {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        CommandScript::from_file(&path, trusted).unwrap()
    };
    let gather = script(
            "gather",
            "#!/bin/sh\n# description: Gather context\n# argument-hint: <topic>\necho \"Context about $1\"\n",
            true,
        );
    let project = script(
        "scan",
        "#!/bin/sh\n# description: Scan\necho scanned\n",
        false,
    );
    *COMMANDS.lock().unwrap() = vec![gather, project];
    let mut panel = panel(vec![reply("a"), reply("b")]);
    let wait_user = |panel: &mut AgentPanel, text: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.tick();
            if panel
                .transcript()
                .items()
                .iter()
                .any(|i| matches!(i, Item::User { text: t, .. } if t == text))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{:?}",
                panel.transcript().items()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    // The trusted script runs unasked and its output is the request.
    type_text(&mut panel, "/ga");
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "gather"));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    type_text(&mut panel, "parsers");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "");
    wait_user(&mut panel, "Context about parsers");
    settle(&mut panel);

    // The project's script asks; "run for this session" runs it now and
    // next time without asking.
    type_text(&mut panel, "/scan");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let form = panel.pending.as_ref().expect("a card asks").form();
    assert!(form.title().starts_with("Run the project command /scan"));
    assert_eq!(form.options().len(), 4);
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    wait_user(&mut panel, "scanned");
    settle(&mut panel);
    assert!(panel.allowed_commands.contains("scan"));

    // "Don't run" leaves nothing behind.
    *COMMANDS.lock().unwrap() = vec![script("other", "#!/bin/sh\necho x\n", false)];
    type_text(&mut panel, "/other");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.pending.is_some());
    panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
    assert!(panel.pending.is_none() && panel.command_run.is_none());
    *COMMANDS.lock().unwrap() = Vec::new();
}

#[test]
fn undo_restores_the_files_and_rewinds_the_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "before").unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.path().to_path_buf(),
        session_dir: Some(sessions),
        ..setup(vec![reply("a"), reply("b"), reply("c")])
    });
    type_text(&mut panel, "task one");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // Nothing changed files yet: /undo has nothing to offer.
    type_text(&mut panel, "/undo");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("nothing to undo"))));

    // The second request "edits" the file: the store records it the way
    // the hook does for the edit tool.
    let leaf = panel
        .session
        .as_ref()
        .unwrap()
        .leaf_id()
        .map(str::to_string);
    {
        let store = panel.checkpoints.as_ref().unwrap();
        let mut store = store.lock().unwrap();
        store.begin_run(leaf);
        store.save(&file).unwrap();
        std::fs::write(&file, "after").unwrap();
        store.end_run();
    }
    type_text(&mut panel, "task two");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(panel.session.as_ref().unwrap().context_messages().len(), 4);

    type_text(&mut panel, "/un");
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "undo"));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE)); // completes
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE)); // sends /undo
    let form = panel.pending.as_ref().expect("undo card").form();
    assert!(form.title().contains("notes.txt"), "{}", form.title());
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let events = panel.tick();
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::FileChangedOnDisk(p) if p == &file)));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    // The conversation is back at the end of task one, on disk too.
    assert_eq!(panel.session.as_ref().unwrap().context_messages().len(), 2);
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::User { text, .. } if text == "task two")));
    assert!(panel.transcript().items().iter().any(
        |i| matches!(i, Item::Notice { text, .. } if text.contains("undid the last request"))
    ));
    let reopened = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(reopened.context_messages().len(), 2);
}

#[test]
fn plan_mode_adds_its_instructions_and_offers_to_carry_the_plan_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        system_prompt: "Base prompt.".into(),
        plan_prompt: PlanPrompt::from_file("---\nrequest: Do it.\n---\nPlan first."),
        ..setup(vec![reply("1. change a\n2. change b"), reply("done")])
    });
    // configured → auto → all → ask → plan
    for _ in 0..4 {
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    }
    assert_eq!(panel.mode.get(), Mode::Plan);
    assert_eq!(chip(&panel, MODE_ACTION), "plan");
    let shown = panel.write_system_prompt().unwrap();
    assert_eq!(
        std::fs::read_to_string(shown).unwrap(),
        "Base prompt.\n\nPlan first."
    );

    type_text(&mut panel, "add a feature");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let form = panel.pending.as_ref().expect("plan card").form();
    assert!(form.title().starts_with("Plan mode"), "{}", form.title());

    // Esc keeps planning; the next answer offers again.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(panel.mode.get(), Mode::Plan);

    // The plan is carried out in the mode it was planned from.
    panel.offer_plan();
    let t = termide_i18n::t();
    assert_eq!(
        panel.pending.as_ref().expect("plan card").form().options(),
        [
            t.agent_plan_clean_mode_fmt("configured"),
            t.agent_plan_mode_fmt("configured"),
            t.agent_plan_accept_edits().to_string(),
        ]
    );
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    let _ = panel.tick();
    assert_eq!(panel.mode.get(), Mode::Configured);
    settle(&mut panel);
    let users: Vec<&str> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|i| match i {
            Item::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["add a feature", "Do it."]);
    let shown = panel.write_system_prompt().unwrap();
    assert_eq!(std::fs::read_to_string(shown).unwrap(), "Base prompt.");
    assert!(panel.pending.is_none(), "no card outside plan mode");
}

#[test]
fn an_external_agent_following_the_modes_is_offered_to_carry_the_plan_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        connections: Some(Arc::new(Connections)),
        plan_prompt: PlanPrompt::from_file(
            "---\nrequest: Do it.\nclean_request: Do it afresh.\n---\nPlan first.",
        ),
        ..setup(vec![])
    });
    assert!(panel.switch_connection("cli"));
    let _ = panel.set_mode(Mode::Plan);
    type_text(&mut panel, "add a feature");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    let form = panel.pending.as_ref().expect("plan card").form();
    assert!(form.title().starts_with("Plan mode"), "{}", form.title());
    let t = termide_i18n::t();
    assert_eq!(
        form.options(),
        [
            t.agent_plan_clean_mode_fmt("configured"),
            t.agent_plan_mode_fmt("configured"),
            t.agent_plan_accept_edits().to_string(),
        ]
    );

    // The second row goes back to the mode planned from and sends the
    // request that keeps the context, in the same session of the agent's.
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    let _ = panel.tick();
    assert_eq!(panel.mode.get(), Mode::Configured);
    settle(&mut panel);
    let users: Vec<&str> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|i| match i {
            Item::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["add a feature", "Do it."]);
}

#[test]
fn an_external_agent_carries_a_plan_out_from_a_clean_context_in_a_new_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        connections: Some(Arc::new(Connections)),
        system_prompt: "Base prompt.".into(),
        plan_prompt: PlanPrompt::from_file(
            "---\nrequest: Do it.\nclean_request: Do it afresh.\n---\nPlan first.",
        ),
        ..setup(vec![])
    });
    assert!(panel.switch_connection("cli"));
    let _ = panel.set_mode(Mode::Plan);
    // Started in plan mode, it is still handed the prompt without the plan
    // instructions: one fixed for its session would outlive the mode.
    assert!(panel.rebuild_keeping_prompt());
    let (prompt, _) = EXTERNAL_PROMPT.with(|p| p.borrow_mut().take()).unwrap();
    assert_eq!(prompt, "Base prompt.");
    type_text(&mut panel, "add a feature");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = panel.tick();
    assert!(
        panel.session.as_ref().unwrap().external_session().is_some(),
        "its session is recorded"
    );

    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    let _ = panel.tick();
    assert_eq!(panel.mode.get(), Mode::Configured);
    settle(&mut panel);
    // A new session of the agent's, told what the clearing kept, its log
    // known to have been in plan mode.
    let (resume, history) = EXTERNAL_START.with(|s| s.borrow_mut().take()).unwrap();
    assert_eq!(resume, None);
    assert_eq!(history, ["user:add a feature", "assistant:from outside"]);
    let (prompt, plan_seen) = EXTERNAL_PROMPT.with(|p| p.borrow_mut().take()).unwrap();
    assert_eq!(prompt, "Base prompt.");
    assert!(plan_seen);
    let users: Vec<&str> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|i| match i {
            Item::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users.last(), Some(&"Do it afresh."));
}

#[test]
fn an_external_agent_on_its_own_modes_gets_no_plan_card() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    assert!(panel.switch_agent("outside"));
    let _ = panel.set_mode(Mode::Plan);
    type_text(&mut panel, "add a feature");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(panel.pending.is_none());
}

#[test]
fn a_plan_carried_out_from_a_clean_context_leaves_the_exploration_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut looking = reply("");
    looking.stop_reason = StopReason::ToolUse;
    looking.content = vec![
        AssistantContent::Text {
            text: "Let me look.".into(),
        },
        AssistantContent::ToolCall(ToolCall {
            id: "r1".into(),
            name: "read".into(),
            arguments: serde_json::json!({ "path": "src/main.rs" }),
            extra_content: None,
        }),
    ];
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        plan_prompt: PlanPrompt::from_file(
            "---\nrequest: Do it.\nclean_request: Do it afresh.\n---\nPlan first.",
        ),
        ..setup(vec![looking, reply("the plan"), reply("done")])
    });
    for _ in 0..4 {
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    }
    assert_eq!(panel.mode.get(), Mode::Plan);
    type_text(&mut panel, "add a feature");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(panel.pending.is_some(), "plan card");

    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(panel.mode.get(), Mode::Configured);
    // The request "edits" a file, so it has a checkpoint to undo. The run
    // began inside `handle_key` and ends only when a tick drains its events,
    // so the file is saved before any tick, however fast the reply comes.
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "before").unwrap();
    panel
        .checkpoints
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .save(&file)
        .unwrap();
    settle(&mut panel);

    // The log keeps the exploration; the context rebuilt from it does not.
    let reopened = Session::open(panel.session_path().unwrap()).unwrap();
    assert!(reopened
        .branch()
        .iter()
        .any(|entry| matches!(entry.kind, EntryKind::Pruned)));
    let shape: Vec<String> = reopened
        .context_messages()
        .iter()
        .map(|message| match message {
            Message::User(user) => format!("user:{}", user.plain_text()),
            Message::Assistant(assistant) => format!("assistant:{}", assistant.plain_text()),
            Message::ToolResult(result) => format!("result:{}", result.tool_name),
        })
        .collect();
    assert_eq!(
        shape,
        [
            "user:add a feature",
            "assistant:the plan",
            "user:Do it afresh.",
            "assistant:done",
        ]
    );

    // Undoing the request leads back to before the clearing, so the
    // exploration comes back with it.
    let branch = reopened.branch();
    let cleared = branch
        .iter()
        .find(|entry| matches!(entry.kind, EntryKind::Pruned))
        .unwrap();
    let checkpoints = panel
        .checkpoints
        .as_ref()
        .unwrap()
        .lock()
        .unwrap()
        .checkpoints();
    assert_eq!(checkpoints.len(), 1);
    assert_eq!(checkpoints[0].leaf_before, cleared.parent_id);
}

/// Work in `from`, go on to plan mode with `Shift+Tab` through the modes
/// between, let the agent answer, and return the plan card's rows with the
/// panel.
fn planned_from(from: Mode) -> (AgentPanel, Vec<String>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        plan_prompt: PlanPrompt::from_file("---\nrequest: Do it.\n---\nPlan first."),
        ..setup(vec![reply("looked"), reply("the plan"), reply("done")])
    });
    let _ = panel.set_mode(from);
    type_text(&mut panel, "look around");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    while panel.mode.get() != Mode::Plan {
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    }
    type_text(&mut panel, "add a feature");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let rows = panel
        .pending
        .as_ref()
        .expect("plan card")
        .form()
        .options()
        .to_vec();
    (panel, rows, dir)
}

#[test]
fn a_plan_is_carried_out_in_the_mode_it_was_planned_from() {
    let t = termide_i18n::t();
    let (mut panel, rows, _dir) = planned_from(Mode::Auto);
    assert_eq!(
        rows,
        [
            t.agent_plan_clean_mode_fmt("auto"),
            t.agent_plan_mode_fmt("auto"),
            t.agent_plan_accept_edits().to_string(),
        ]
    );
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(panel.mode.get(), Mode::Auto);

    // Accepting edits stays on offer as its own row.
    let (mut panel, _, _dir) = planned_from(Mode::Auto);
    panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
    assert_eq!(panel.mode.get(), Mode::Edit);
}

#[test]
fn a_plan_made_from_edit_offers_no_second_edit_row() {
    let t = termide_i18n::t();
    let (_panel, rows, _dir) = planned_from(Mode::Edit);
    assert_eq!(
        rows,
        [
            t.agent_plan_clean_mode_fmt("edit"),
            t.agent_plan_mode_fmt("edit"),
        ]
    );
}

#[test]
fn a_plan_made_from_ask_is_carried_out_under_the_configured_rules() {
    let (mut panel, _, _dir) = planned_from(Mode::Ask);
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(panel.mode.get(), Mode::Configured);
}

#[test]
fn a_session_reopened_in_plan_mode_returns_to_the_mode_planned_from() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::create(dir.path(), Path::new("/tmp")).unwrap()),
        ..setup(vec![])
    });
    let _ = panel.set_mode(Mode::Auto);
    panel
        .session
        .as_mut()
        .unwrap()
        .append_message(&Message::User(UserMessage::text("look around")))
        .unwrap();
    let _ = panel.set_mode(Mode::Ask);
    let _ = panel.set_mode(Mode::Plan);
    let path = panel.session.as_ref().unwrap().path().to_path_buf();
    drop(panel);

    let panel = reopen(&path, dir.path(), vec![]);
    assert_eq!(panel.mode.get(), Mode::Plan);
    assert_eq!(panel.plan_carry_mode(), Mode::Auto);
}
