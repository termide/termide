//! Connections, the toolset guard, the queue, and permission grants.

use super::*;

/// A connection edited in the settings reaches the panel already open on it,
/// without a reopen — and asks for nothing while a run is going on, which
/// keeps the endpoint it started on and takes the settings when it is over.
#[test]
fn an_edited_connection_reaches_the_panels_already_open_on_it() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = Arc::new(Moving::new());
    let mut panel = catalog.panel(dir.path());

    // Nothing moved: the panel keeps its endpoint and asks for no redraw. It
    // does read the connection to know that, which builds one it then drops;
    // that happens only on a settings change, which is what this follows.
    assert!(!panel
        .handle_command(PanelCommand::RefreshAiConnection)
        .needs_redraw());
    assert_eq!(panel.provider.name(), "ep0");

    // The run going on keeps its endpoint; the ask waits for it.
    catalog.move_to("https://openrouter.ai/api/v1");
    panel.busy = true;
    assert!(!panel
        .handle_command(PanelCommand::RefreshAiConnection)
        .needs_redraw());
    assert_eq!(panel.provider.name(), "ep0");
    assert!(panel.connection_refresh, "kept for the run's end");

    // The run is over: the panel takes an endpoint other than the one it
    // started on, and the settings the connection now names.
    panel.apply(AgentEvent::AgentEnd);
    assert_ne!(panel.provider.name(), "ep0", "a new endpoint");
    assert_eq!(panel.connection, "local");
    assert_eq!(
        panel.connection_settings.base_url,
        "https://openrouter.ai/api/v1"
    );
    assert!(!panel.connection_refresh);

    // Asked again with nothing changed, it keeps the endpoint it took.
    let taken = panel.provider.name().to_string();
    assert!(!panel
        .handle_command(PanelCommand::RefreshAiConnection)
        .needs_redraw());
    assert_eq!(panel.provider.name(), taken);
}

#[test]
fn a_new_session_records_the_connection_it_starts_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    let first = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(first.current_connection(), Some("local".to_string()));
    // A new session in the panel too.
    panel.switch_session(None);
    let second = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(second.current_connection(), Some("local".to_string()));
}

#[test]
fn the_connection_picker_switches_the_endpoint_and_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    let events = panel.handle_status_action(CONNECTION_ACTION);
    let Some(PanelEvent::ShowSelect { options, .. }) = events.first() else {
        panic!("a picker, got {events:?}");
    };
    assert!(options[0].starts_with("● local"), "{options:?}");
    assert!(options[1].contains("cloud") && options[1].contains("claude-x"));
    panel.handle_command(PanelCommand::SelectionMade {
        action: CONNECTION_ACTION.to_string(),
        index: 1,
    });
    // The connection's endpoint and model replace the ones in use.
    assert_eq!(panel.connection, "cloud");
    assert_eq!(panel.provider_kind, "anthropic_compatible");
    assert_eq!(panel.model.id, "claude-x");
    assert_eq!(panel.model.context_window, 200_000);
    assert!(!panel.external);
    // The banner names the connection beside its protocol, the chip by name.
    assert_eq!(panel.connection_display(), "cloud · Anthropic Compatible");
    assert_eq!(panel.connection_chip(), "cloud");
    // The log keeps it, so a reopened session reconnects there.
    let session = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(session.current_connection(), Some("cloud".to_string()));
    assert_eq!(
        session.current_model().map(|m| m.id),
        Some("claude-x".to_string())
    );
}

#[test]
fn a_connection_switch_keeps_the_typed_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    type_text(&mut panel, "fix the bug");
    assert!(panel.switch_connection("cloud"));
    assert_eq!(panel.input.field_value(0), "fix the bug");
    // As does a switch onto a CLI agent.
    assert!(panel.switch_connection("cli"));
    assert_eq!(panel.input.field_value(0), "fix the bug");
}

#[test]
fn an_unsent_session_saves_its_setup_and_a_used_one_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    assert!(panel.switch_connection("cloud"));
    assert!(panel.switch_model("claude-y", None));
    let state = |panel: &AgentPanel| match panel.to_state(Path::new("/unused")) {
        Some(termide_core::PanelState::Agent { setup, .. }) => setup,
        other => panic!("agent state expected, got {other:?}"),
    };
    let saved = state(&panel);
    assert_eq!(saved.connection.as_deref(), Some("cloud"));
    assert_eq!(saved.model.as_deref(), Some("claude-y"));
    // The connection's own model is not pinned.
    assert!(panel.switch_model("claude-x", None));
    assert_eq!(state(&panel).model, None);

    // Once the log holds a conversation, it is what a restore reads.
    let mut used = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("done")])
    });
    type_text(&mut used, "task");
    used.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut used);
    assert!(state(&used).is_empty());
}

#[test]
fn a_cli_agent_connection_is_taken_on_mid_conversation_and_left_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    let meter = panel.reviewer.spent.clone();
    panel.transcript.push(Item::User {
        text: "go".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    // The CLI agent takes the conversation over (told a recap of it).
    assert!(panel.switch_connection("cli"));
    assert!(panel.external);
    assert_eq!(panel.provider_kind, "codex");
    // And the built-in loop takes it back.
    assert!(panel.switch_connection("local"));
    assert!(!panel.external);
    // The connection's reviewer came along each time.
    assert_eq!(
        panel.reviewer.model.session_model.as_deref(),
        Some("local-reviewer")
    );
    // What its reviews spend still reaches the panel's totals.
    meter.add(&termide_agent_core::Usage {
        input: 9,
        ..Default::default()
    });
    assert_eq!(panel.reviewer.spent.take().input, 9);
}

#[test]
fn the_toolset_guard_refuses_what_is_switched_off_in_context() {
    let blocked: Blocked = Arc::new(RwLock::new(
        ["shell".to_string(), "skill:review".to_string()].into(),
    ));
    let mut guard = ToolsetGuard { blocked };
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    let call = |name: &str, args: serde_json::Value| ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: args,
        extra_content: None,
    };
    let refused = |d: ToolDecision| matches!(d, ToolDecision::Block { .. });
    assert!(refused(
        guard.before_tool_call(&call("shell", serde_json::json!({})), &ctx)
    ));
    assert!(refused(guard.before_tool_call(
        &call("skill", serde_json::json!({ "name": "review" })),
        &ctx
    )));
    assert!(!refused(guard.before_tool_call(
        &call("skill", serde_json::json!({ "name": "deploy" })),
        &ctx
    )));
    assert!(!refused(
        guard.before_tool_call(&call("read", serde_json::json!({})), &ctx)
    ));
}

#[test]
fn switching_off_before_the_first_request_keeps_it_out_of_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    panel.offered_tools = vec!["read".into(), "shell".into()];
    panel.offered_skills = vec!["review".into()];
    assert!(panel.is_fresh());
    panel.apply_toolset(&["read".to_string()]);
    let off: BTreeSet<String> = ["shell".to_string(), "skill:review".to_string()].into();
    assert_eq!(panel.toolset_off, off);
    // Rebuilt at once without them: out of the context, nothing to refuse.
    assert_eq!(panel.context_off, off);
    assert!(panel.blocked.read().unwrap().is_empty());
    // The log keeps the set for a reopened session.
    let path = panel.session_path().unwrap().to_path_buf();
    let session = Session::open(&path).unwrap();
    assert_eq!(
        session.current_toolset(),
        Some(vec!["shell".to_string(), "skill:review".to_string()])
    );
    // Reopened, the session comes back with it, built without it: a new
    // worker has no cache to keep.
    drop(panel);
    let reopened = AgentPanel::new(AgentPanelSetup {
        session: Some(session),
        ..setup(vec![])
    });
    assert_eq!(reopened.toolset_off, off);
    assert_eq!(reopened.context_off, off);
    assert!(reopened.blocked.read().unwrap().is_empty());
}

#[test]
fn a_new_session_keeps_what_the_last_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    panel.offered_tools = vec!["read".into(), "shell".into()];
    panel.apply_toolset(&["read".to_string()]);
    let off: BTreeSet<String> = ["shell".to_string()].into();
    let first = panel.session_path().unwrap().to_path_buf();
    assert!(panel.switch_session(None));
    assert_ne!(panel.session_path().unwrap(), first.as_path());
    assert_eq!(panel.toolset_off, off);
    assert_eq!(panel.context_off, off);
    // The new log records it, so a reopen comes back with it.
    let session = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(session.current_toolset(), Some(vec!["shell".to_string()]));
}

#[test]
fn switching_off_mid_session_refuses_until_a_compaction_takes_it_out() {
    let mut panel = panel(vec![]);
    panel.offered_tools = vec!["read".into(), "shell".into()];
    panel.transcript.push(Item::User {
        text: "go".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    assert!(!panel.is_fresh());
    panel.apply_toolset(&["read".to_string()]);
    // Still in the context (the prompt cache stays): refused, not gone.
    assert!(panel.context_off.is_empty());
    assert!(panel.blocked.read().unwrap().contains("shell"));
    let items = panel.toolset_items();
    let bash = items.iter().find(|i| i.key == "shell").unwrap();
    assert!(
        bash.enabled && !bash.checked && !bash.note.is_empty(),
        "{bash:?}"
    );
    // A compaction lets the next moment between runs rebuild without it.
    panel.apply(AgentEvent::Compacted {
        summary: "earlier".into(),
        kept: 1,
        tokens_before: 1000,
        tokens_after: 200,
        call: summary_call(),
    });
    assert!(panel.context_stale);
    panel.tick();
    assert!(panel.context_off.contains("shell"));
    assert!(panel.blocked.read().unwrap().is_empty());
    // Out of the context now, it cannot come back in this session.
    let items = panel.toolset_items();
    let bash = items.iter().find(|i| i.key == "shell").unwrap();
    assert!(!bash.enabled);
    panel.apply_toolset(&["read".to_string(), "shell".to_string()]);
    assert!(
        panel.toolset_off.contains("shell"),
        "a locked item keeps its state"
    );
}

#[test]
fn up_takes_the_queue_back_into_the_input() {
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    for text in ["fix the test", "and the docs"] {
        type_text(&mut panel, text);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    }
    assert_eq!(panel.runtime.queue_lens(), (2, 0));
    type_text(&mut panel, "also");
    // Up at the top of the input takes both back, ahead of what is typed.
    for _ in 0..2 {
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    }
    assert_eq!(panel.input_text(), "fix the test\n\nand the docs\n\nalso");
    assert_eq!(panel.runtime.queue_lens(), (0, 0));
    assert!(panel.queued_texts.is_empty());
    assert!(
        panel.state_lines(60).is_empty(),
        "the strip no longer lists them"
    );
}

#[test]
fn a_pasted_picture_path_attaches_the_picture_and_survives_the_queue() {
    let dir = tempfile::tempdir().unwrap();
    let shot = dir.path().join("dot.png");
    std::fs::write(&shot, DOT_PNG).unwrap();
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    type_text(&mut panel, "what is ");
    panel.paste(&shot.display().to_string());
    let placeholder = termide_i18n::t()
        .agent_paste_placeholder_fmt(1, &termide_i18n::t().agent_paste_image_fmt(1, 1));
    assert_eq!(panel.input_text(), format!("what is {placeholder}"));
    assert_eq!(panel.held_pictures(&panel.input_text()).len(), 1);

    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.runtime.queue_lens(), (1, 0));
    assert!(panel.input_text().is_empty());

    // Taken back into the input, the picture is held under its placeholder.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), format!("what is {placeholder}"));
    let held = panel.held_pictures(&panel.input_text());
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].0, "image/png");
    // Deleting the placeholder drops the picture.
    assert!(panel.held_pictures("what is").is_empty());
}

#[test]
fn a_call_that_fails_before_its_first_token_shows_no_cost() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    let failed = AssistantMessage::failed("p", "m", StopReason::Error, "connection refused");
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(failed)));
    let answer = panel
        .transcript
        .items()
        .iter()
        .find(|item| matches!(item, Item::Assistant { .. }))
        .expect("the failure shows on an answer block");
    assert!(matches!(
        answer,
        Item::Assistant {
            cost: None,
            error: Some(_),
            ..
        }
    ));
}

#[test]
fn a_permission_wait_is_timed_apart_from_the_call() {
    let mut panel = panel(vec![]);
    let call = termide_agent_core::ToolCall {
        id: "c".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "cat notes.txt" }),
        extra_content: None,
    };
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
    // The call started 7s ago and its question has been up for 5s.
    panel
        .tool_starts
        .insert("c".into(), Instant::now() - Duration::from_secs(7));
    let (reply, _rx) = std::sync::mpsc::channel();
    panel.pending = Some(Pending::Permission {
        envelope: PermissionEnvelope {
            id: 1,
            request: termide_agent_core::PermissionRequest {
                tool: "shell".into(),
                subject: "cat notes.txt".into(),
                call: call.clone(),
                suggested_pattern: "cat *".into(),
                can_persist: true,
                can_allow_session: true,
                parts: vec![termide_agent_core::AskedPart {
                    text: "cat notes.txt".into(),
                    pattern: Some("cat *".into()),
                }],
            },
            reply,
            withdrawn: termide_agent_core::CancelToken::default(),
            delegate: None,
            task: None,
        },
        form: ChoiceForm::new("", vec![]),
        answers: Vec::new(),
    });
    panel.permission_wait = Some((Instant::now() - Duration::from_secs(5), 0));
    panel.tick();
    // While the question is up, the call's wait ticks as a pause.
    let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
    assert!(rows.iter().any(|r| r.contains("‖ 5s")), "{rows:?}");
    assert!(matches!(
        panel.transcript.items().last(),
        Some(Item::Tool { waiting: true, .. })
    ));
    // Answered: the wait rests, and the call's duration leaves it out.
    assert!(panel.answer_permission(PermissionAnswer::AllowOnce));
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&call, "notes"),
    });
    let Some(Item::Tool {
        waited_ms: Some(waited),
        waiting: false,
        duration_ms: Some(duration),
        ..
    }) = panel.transcript.items().last().cloned()
    else {
        panic!("the call keeps its wait and duration");
    };
    assert!((5000..6000).contains(&waited), "{waited}");
    assert!((1500..2500).contains(&duration), "{duration}");
    // Folded, the finished call hides both; unfolded, they show together.
    let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
    assert!(rows.iter().all(|r| !r.contains('‖')), "{rows:?}");
    let last = panel.transcript.items().len() - 1;
    assert!(panel.transcript.toggle_expanded(last));
    let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
    assert!(rows.iter().any(|r| r.contains("‖ 5s 🕒 2s")), "{rows:?}");
}

#[test]
fn a_grant_reaches_the_panel_rules_so_it_survives_a_rebuild() {
    let pending_from = |panel: &mut AgentPanel, delegate: Option<&str>| {
        let (reply, _rx) = std::sync::mpsc::channel();
        panel.pending = Some(Pending::Permission {
            envelope: PermissionEnvelope {
                id: 1,
                request: termide_agent_core::PermissionRequest {
                    tool: "shell".into(),
                    subject: "cat notes.txt".into(),
                    call: termide_agent_core::ToolCall {
                        id: "c".into(),
                        name: "shell".into(),
                        arguments: serde_json::json!({ "command": "cat notes.txt" }),
                        extra_content: None,
                    },
                    suggested_pattern: "cat *".into(),
                    can_persist: true,
                    can_allow_session: true,
                    parts: vec![termide_agent_core::AskedPart {
                        text: "cat notes.txt".into(),
                        pattern: Some("cat *".into()),
                    }],
                },
                reply,
                withdrawn: termide_agent_core::CancelToken::default(),
                delegate: delegate.map(str::to_string),
                task: None,
            },
            form: ChoiceForm::new("", vec![]),
            answers: Vec::new(),
        });
    };
    let pending = |panel: &mut AgentPanel| pending_from(panel, None);

    // "Allow for the session" lands in the session rules, which
    // `effective_rules` — what every rebuilt agent starts from — carries
    // apart from the persistent rules; "deny for the session" too.
    let mut session = panel(vec![]);
    pending(&mut session);
    assert!(session.answer_permission(PermissionAnswer::AllowSession));
    assert_eq!(
        session.effective_rules().evaluate_session("shell", "cat x"),
        Some(Decision::Allow)
    );
    assert_eq!(session.effective_rules().evaluate("shell", "cat x"), None);
    pending(&mut session);
    assert!(session.answer_permission(PermissionAnswer::DenySession));
    assert_eq!(
        session.effective_rules().evaluate_session("shell", "cat x"),
        Some(Decision::Deny)
    );

    // "Allow always" lands in the persistent rules.
    let mut always = panel(vec![]);
    pending(&mut always);
    assert!(always.answer_permission(PermissionAnswer::AllowAlways));
    assert_eq!(
        always.rules.evaluate("shell", "cat x"),
        Some(Decision::Allow)
    );

    // What a delegate is granted stays with its task: the panel's own
    // rules are left as they were.
    let mut delegated = panel(vec![]);
    pending_from(&mut delegated, Some("search"));
    assert!(delegated.answer_permission(PermissionAnswer::AllowSession));
    assert_eq!(
        delegated
            .effective_rules()
            .evaluate_session("shell", "cat x"),
        None
    );
}
