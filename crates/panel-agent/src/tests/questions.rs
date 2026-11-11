//! The model's questions and the permission cards.

use super::*;

#[test]
fn the_models_questions_are_answered_one_card_at_a_time() {
    let mut panel = panel(vec![]);
    let mut first = question("Which approach?", &["Channel", "Slot"], false);
    first.header = "Approach".into();
    let worker = ask_in_worker(
        &mut panel,
        vec![
            first,
            question("Which crates?", &["core", "ui", "app"], true),
        ],
    );
    {
        let form = panel.pending.as_ref().unwrap().form();
        assert_eq!(form.title(), "Agent asks: Approach (1/2)");
        assert_eq!(form.detail(), Some("Which approach?"));
        assert_eq!(form.options(), ["Channel", "Slot"]);
    }
    let rows = render_text(&mut panel, 60, 16);
    assert!(
        rows.iter().any(|r| r.contains("1. Channel  about Channel")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r.contains("3. Type your own answer")));
    // A single choice answers the question and brings up the next.
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(
        panel.pending.as_ref().unwrap().form().title(),
        "Agent asks (← 2/2)"
    );
    // Several picks, and an answer of the user's own among them.
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
    type_text(&mut panel, "docs");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let rows = render_text(&mut panel, 60, 16);
    assert!(rows.iter().any(|r| r.contains("[✓] core")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("[ ] ui")), "{rows:?}");
    panel.handle_key(chord(KeyCode::Char('5'), KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(
        worker.join().unwrap(),
        QuestionReply::Answered(vec![
            QuestionAnswer {
                chosen: vec!["Slot".into()],
                custom: None,
            },
            QuestionAnswer {
                chosen: vec!["core".into(), "app".into()],
                custom: Some("docs".into()),
            },
        ])
    );
}

#[test]
fn a_paste_while_an_answer_is_typed_goes_into_the_answer() {
    let mut panel = panel(vec![]);
    let worker = ask_in_worker(
        &mut panel,
        vec![question("Which approach?", &["Channel", "Slot"], false)],
    );
    // Before the answer row is chosen a paste still goes to the prompt.
    panel.handle_command(PanelCommand::PasteText { text: "p".into() });
    assert_eq!(panel.input_text(), "p");
    panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
    type_text(&mut panel, "use ");
    panel.handle_command(PanelCommand::PasteText {
        text: "a\nchannel".into(),
    });
    // Ctrl chords edit the answer: a word back, then a word selected.
    panel.handle_key(chord(KeyCode::Left, KeyModifiers::CONTROL));
    panel.handle_key(chord(
        KeyCode::Right,
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ));
    assert_eq!(
        panel
            .pending
            .as_ref()
            .unwrap()
            .form()
            .selected_text()
            .as_deref(),
        Some("channel")
    );
    assert_eq!(panel.input_text(), "p");
    panel.handle_key(chord(KeyCode::End, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        worker.join().unwrap(),
        QuestionReply::Answered(vec![QuestionAnswer {
            chosen: vec![],
            custom: Some("use a channel".into()),
        }])
    );
}

#[test]
fn the_mouse_places_the_cursor_and_selects_in_a_typed_answer() {
    let mut panel = panel(vec![]);
    let worker = ask_in_worker(
        &mut panel,
        vec![question("Which approach?", &["Channel", "Slot"], false)],
    );
    panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
    panel.handle_command(PanelCommand::PasteText {
        text: "hello world".into(),
    });
    let rows = render_text(&mut panel, 60, 16);
    let row = rows
        .iter()
        .position(|r| r.contains(": hello world"))
        .expect("the answer is drawn") as u16;
    let column = rows[row as usize]
        .chars()
        .collect::<Vec<_>>()
        .windows(5)
        .position(|w| w.iter().collect::<String>() == "world")
        .unwrap() as u16;
    let at = |kind, column| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let area = Rect::new(0, 0, 60, 16);
    // Pressed twice, as a double click: the word under it is selected and
    // the answer stays as typed, not confirmed.
    for _ in 0..2 {
        panel.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), column), area);
        panel.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), column), area);
    }
    let form = panel.pending.as_ref().unwrap().form();
    assert_eq!(form.typed(), Some("hello world"));
    assert_eq!(form.selected_text().as_deref(), Some("world"));
    // A press on another character and a drag select from there.
    panel.handle_mouse(
        at(MouseEventKind::Down(MouseButton::Left), column + 1),
        area,
    );
    panel.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 59), area);
    panel.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 59), area);
    let form = panel.pending.as_ref().unwrap().form();
    assert_eq!(form.typed(), Some("hello world"));
    assert_eq!(form.selected_text().as_deref(), Some("orld"));
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    let _ = worker.join().unwrap();
}

#[test]
fn the_arrows_step_back_to_an_answered_question_and_its_answer_shows() {
    let mut panel = panel(vec![]);
    let worker = ask_in_worker(
        &mut panel,
        vec![
            question("Which approach?", &["Channel", "Slot"], false),
            question("Which crates?", &["core", "ui", "app"], true),
            question("Name?", &[], false),
        ],
    );
    let title = |panel: &AgentPanel| panel.pending.as_ref().unwrap().form().title().to_string();
    let key = |panel: &mut AgentPanel, code| panel.handle_key(chord(code, KeyModifiers::NONE));
    // The first question has nothing before it: `←` is not a step there.
    assert_eq!(title(&panel), "Agent asks (1/3)");
    key(&mut panel, KeyCode::Char('2'));
    key(&mut panel, KeyCode::Char('1'));
    key(&mut panel, KeyCode::Char('3'));
    key(&mut panel, KeyCode::Char('5'));
    assert_eq!(title(&panel), "Agent asks (← 3/3)");

    // Back to the second: its picks show, and `→` leads on again.
    key(&mut panel, KeyCode::Left);
    assert_eq!(title(&panel), "Agent asks (← 2/3 →)");
    let form = panel.pending.as_ref().unwrap().form();
    assert!(form.is_checked(0) && !form.is_checked(1) && form.is_checked(2));
    // Back to the first: its choice is selected; a new one moves on to the
    // second, which keeps its answer and can be stepped past.
    key(&mut panel, KeyCode::Left);
    assert_eq!(title(&panel), "Agent asks (1/3 →)");
    assert_eq!(panel.pending.as_ref().unwrap().form().selected(), 1);
    key(&mut panel, KeyCode::Char('1'));
    assert_eq!(title(&panel), "Agent asks (← 2/3 →)");
    key(&mut panel, KeyCode::Right);
    assert_eq!(title(&panel), "Agent asks (← 3/3)");
    // The last is answered in the user's own words; the set goes back.
    key(&mut panel, KeyCode::Char('1'));
    type_text(&mut panel, "relay");
    key(&mut panel, KeyCode::Enter);
    assert!(panel.pending.is_none());
    assert_eq!(
        worker.join().unwrap(),
        QuestionReply::Answered(vec![
            QuestionAnswer {
                chosen: vec!["Channel".into()],
                custom: None,
            },
            QuestionAnswer {
                chosen: vec!["core".into(), "app".into()],
                custom: None,
            },
            QuestionAnswer {
                chosen: vec![],
                custom: Some("relay".into()),
            },
        ])
    );
}

#[test]
fn escape_declines_the_models_question() {
    let mut panel = panel(vec![]);
    let worker = ask_in_worker(&mut panel, vec![question("Name?", &[], false)]);
    assert!(panel.captures_escape());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), QuestionReply::Declined);
}

/// An external agent's calls reach the panel as a message of calls alone
/// once one has run, for the log: it leaves no answer block behind, nor
/// closes the reasoning streaming meanwhile.
#[test]
fn a_message_of_calls_alone_leaves_no_answer_block() {
    let mut panel = panel(vec![]);
    panel.external = true;
    let call = termide_agent_core::ToolCall {
        id: "c".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "ls" }),
        extra_content: None,
    };
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&call, "a.rs"),
    });
    // The agent has started reasoning about what came back.
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(
        "so".into(),
    )));
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(
        AssistantMessage {
            content: vec![AssistantContent::ToolCall(call.clone())],
            stop_reason: StopReason::ToolUse,
            ..reply("")
        },
    )));
    panel.apply(AgentEvent::MessageEnd(Message::ToolResult(
        ToolResultMessage::text(&call, "a.rs"),
    )));
    assert!(
        !panel
            .transcript
            .items()
            .iter()
            .any(|item| matches!(item, Item::Assistant { .. })),
        "{:?}",
        panel.transcript.items()
    );
    assert!(
        matches!(
            panel.transcript.items().last(),
            Some(Item::Thinking {
                streaming: true,
                ..
            })
        ),
        "{:?}",
        panel.transcript.items()
    );
}

/// A question for a call the external agent gave up comes down by itself,
/// and the next call's question is shown rather than turned away.
#[test]
fn a_withdrawn_permission_question_comes_down() {
    let mut panel = panel(vec![]);
    let (prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let request = |command: &str| termide_agent_core::PermissionRequest {
        tool: "shell".into(),
        subject: command.into(),
        call: termide_agent_core::ToolCall {
            id: "c".into(),
            name: "shell".into(),
            arguments: serde_json::json!({ "command": command }),
            extra_content: None,
        },
        suggested_pattern: String::new(),
        can_persist: false,
        can_allow_session: true,
        parts: Vec::new(),
    };
    let wait_for = |panel: &mut AgentPanel, subject: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.tick();
            if let Some(Pending::Permission { envelope, .. }) = &panel.pending {
                if envelope.request.subject == subject {
                    return;
                }
            }
            assert!(Instant::now() < deadline, "no card for {subject}");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let withdrawn = CancelToken::new();
    let mut first = prompter.clone();
    let token = withdrawn.clone();
    let worker = std::thread::spawn(move || first.ask_for_call(&request("git push"), Some(&token)));
    wait_for(&mut panel, "git push");

    withdrawn.cancel();
    assert_eq!(worker.join().unwrap(), PermissionAnswer::Deny);
    let mut second = prompter.clone();
    let worker = std::thread::spawn(move || second.ask(&request("git fetch")));
    // The given-up card goes and the next question takes its place.
    wait_for(&mut panel, "git fetch");
    assert!(panel.answer_permission(PermissionAnswer::AllowOnce));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowOnce);
}

/// Delegates working side by side ask at once: each request gets its card in
/// turn, headed with the delegate's name, none turned away.
#[test]
fn delegates_asking_at_once_take_turns_on_the_card() {
    let mut panel = panel(vec![]);
    let (prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let ask = |agent: &str, command: &str| {
        let mut delegate = prompter.for_delegate(agent, CancelToken::new());
        let request = termide_agent_core::PermissionRequest {
            tool: "shell".into(),
            subject: command.into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": command }),
                extra_content: None,
            },
            suggested_pattern: String::new(),
            can_persist: false,
            can_allow_session: false,
            parts: Vec::new(),
        };
        std::thread::spawn(move || delegate.ask(&request))
    };
    let card = |panel: &mut AgentPanel| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.tick();
            if let Some(Pending::Permission { form, envelope, .. }) = &panel.pending {
                return (form.title().to_string(), envelope.request.subject.clone());
            }
            assert!(Instant::now() < deadline, "no card");
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let first = ask("search", "git fetch");
    let (title, subject) = card(&mut panel);
    assert!(title.starts_with("& search · "), "{title}");
    let second = ask("review", "git push");
    // The second waits while the first is on screen.
    std::thread::sleep(Duration::from_millis(50));
    panel.tick();
    let (_, still) = card(&mut panel);
    assert_eq!(still, subject);
    assert!(panel.answer_permission(PermissionAnswer::AllowOnce));
    assert_eq!(first.join().unwrap(), PermissionAnswer::AllowOnce);
    let (title, subject) = card(&mut panel);
    assert!(title.starts_with("& review · "), "{title}");
    assert_eq!(subject, "git push");
    assert!(panel.answer_permission(PermissionAnswer::Deny));
    assert_eq!(second.join().unwrap(), PermissionAnswer::Deny);
}

#[test]
fn permission_prompt_is_answered_in_the_panel() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "shell".into(),
            subject: "git push".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": "git push" }),
                extra_content: None,
            },
            suggested_pattern: "git push *".into(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "git push".into(),
                pattern: Some("git push *".into()),
            }],
        })
    });

    // The question arrives as a form in the panel and a status line.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = panel.tick();
        if panel.pending.is_some() {
            assert!(events.iter().any(|e| matches!(
                e,
                PanelEvent::SetStatusMessage { message, .. } if message.contains("git push")
            )));
            // The call waits on the user, so the panel asks for the bell.
            assert!(rings(&events));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    {
        let form = panel.pending.as_ref().unwrap().form();
        assert_eq!(form.title(), "Agent wants to run shell:");
        assert_eq!(form.detail(), Some("git push"));
        assert_eq!(
            form.options()[2],
            "Allow always in this project (git push *)"
        );
        assert_eq!(form.options()[3], "Allow always everywhere (git push *)");
        assert_eq!(form.options()[5], "Deny for this session (git push *)");
    }
    assert!(panel.captures_escape());
    let rows = render_text(&mut panel, 60, 14);
    assert!(
        rows.iter().any(|r| r.contains("2. Allow for this session")),
        "{rows:?}"
    );
    // Typing goes nowhere while the question is open; Down + Enter answer it.
    type_text(&mut panel, "x");
    assert_eq!(panel.input_text(), "");
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);

    // A digit answers at once; Esc declines.
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let request = termide_agent_core::PermissionRequest {
        tool: "edit".into(),
        subject: "src/x.rs".into(),
        call: termide_agent_core::ToolCall {
            id: "c".into(),
            name: "edit".into(),
            arguments: serde_json::json!({ "path": "src/x.rs" }),
            extra_content: None,
        },
        suggested_pattern: "src/x.rs".into(),
        can_persist: true,
        can_allow_session: true,
        parts: Vec::new(),
    };
    let asked = request.clone();
    let worker = std::thread::spawn(move || {
        let first = prompter.ask(&asked);
        let second = prompter.ask(&asked);
        (first, second)
    });
    let wait = |panel: &mut AgentPanel| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while panel.pending.is_none() {
            panel.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait(&mut panel);
    panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
    // The fifth row takes a reason the model gets to read.
    wait(&mut panel);
    {
        let form = panel.pending.as_ref().unwrap().form();
        assert_eq!(
            form.height(60),
            12,
            "a detail line and divider, six answers, a reason row and a stop row"
        );
    }
    panel.handle_key(chord(KeyCode::Char('7'), KeyModifiers::NONE));
    type_text(&mut panel, "edit the test instead");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        worker.join().unwrap(),
        (
            PermissionAnswer::AllowAlways,
            PermissionAnswer::DenyWithReason("edit the test instead".into())
        )
    );
    let _ = request;
}

#[test]
fn a_compound_command_card_lists_the_parts_it_asks_about() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let command = "cd $BUILD && ./run && make install";
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "shell".into(),
            subject: command.into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": command }),
                extra_content: None,
            },
            suggested_pattern: "make install *".into(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![
                termide_agent_core::AskedPart {
                    text: "./run".into(),
                    pattern: None,
                },
                termide_agent_core::AskedPart {
                    text: "make install".into(),
                    pattern: Some("make install *".into()),
                },
            ],
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let form = panel.pending.as_ref().unwrap().form();
    let detail = form.detail().unwrap().to_string();
    assert!(detail.starts_with(command), "{detail}");
    let once = termide_i18n::t().agent_perm_part_once();
    assert!(detail.contains(&format!("• ./run ({once})")), "{detail}");
    assert!(detail.contains("• make install\n") || detail.ends_with("• make install"));
    // The recorded rule is the part's that can have one.
    assert!(
        form.options()[1].contains("(make install *)"),
        "{:?}",
        form.options()
    );
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);
    assert_eq!(
        panel
            .effective_rules()
            .evaluate_session("shell", "make install x"),
        Some(Decision::Allow)
    );
    assert_eq!(
        panel.effective_rules().evaluate_session("shell", "./run"),
        None
    );

    // With no part a rule can stand for, nothing is offered to remember.
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "shell".into(),
            subject: "cd $BUILD && ./run".into(),
            call: termide_agent_core::ToolCall {
                id: "d".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": "cd $BUILD && ./run" }),
                extra_content: None,
            },
            suggested_pattern: String::new(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "./run".into(),
                pattern: None,
            }],
        })
    });
    while panel.pending.is_none() {
        panel.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    let options = panel.pending.as_ref().unwrap().form().options().to_vec();
    assert_eq!(options.len(), 2, "allow once and deny only: {options:?}");
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowOnce);
}

#[test]
fn always_is_not_offered_where_the_configured_rules_do_not_count() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "shell".into(),
            subject: "make".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": "make" }),
                extra_content: None,
            },
            suggested_pattern: "make *".into(),
            can_persist: false,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "make".into(),
                pattern: Some("make *".into()),
            }],
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let options = panel.pending.as_ref().unwrap().form().options().to_vec();
    assert_eq!(options.len(), 4, "{options:?}");
    assert!(options.iter().all(|o| !o.contains("always")), "{options:?}");
    // The last row refuses for the session.
    panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::DenySession);
    assert_eq!(
        panel
            .effective_rules()
            .evaluate_session("shell", "make all"),
        Some(Decision::Deny)
    );
}

#[test]
fn a_single_click_selects_and_a_double_click_answers_a_permission() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "shell".into(),
            subject: "git push".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": "git push" }),
                extra_content: None,
            },
            suggested_pattern: "git push *".into(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "git push".into(),
                pattern: Some("git push *".into()),
            }],
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Render so the form's row geometry exists, then find the second option.
    let rows = render_text(&mut panel, 60, 16);
    let y = rows
        .iter()
        .position(|r| r.contains("2. Allow for this session"))
        .expect("the second option is on screen") as u16;
    let area = Rect::new(0, 0, 60, 16);
    let click = |panel: &mut AgentPanel, y: u16| {
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 5,
                row: y,
                modifiers: KeyModifiers::NONE,
            },
            area,
        );
    };

    // A single click only moves the selection; the question stays open.
    click(&mut panel, y);
    assert!(panel.pending.is_some(), "a single click does not answer");
    assert_eq!(panel.pending.as_ref().unwrap().form().selected(), 1);

    // A second click on the same row (a double click) answers it.
    click(&mut panel, y);
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);
}
