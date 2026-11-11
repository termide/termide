//! Messages between agents, tasks, delegates and trust.

use super::*;

#[test]
fn another_agents_message_starts_a_turn_and_counts_its_hops() {
    let mut panel = panel(vec![reply("noted"), reply("ok")]);
    assert_eq!(panel.peer_state(), PeerState::Free);
    panel
        .deliver(from_peer(termide_agent_core::OriginKind::Info, 3))
        .unwrap();
    assert_eq!(panel.peer_state(), PeerState::Working);
    assert_eq!(panel.agent_hops(), 3);
    settle(&mut panel);
    assert_eq!(panel.peer_state(), PeerState::Idle);
    // It shows as a step of the agent's, never as the user's plate.
    assert!(panel.transcript().items().iter().any(|item| matches!(
        item,
        Item::Tool { call, .. } if crate::transcript::is_peer_message(call)
            && call.arguments["agent"] == "reviewer"
            && call.arguments["kind"] == "info"
            && call.arguments["panel"] == 7
            && call.arguments["subject"] == "the config loader moved"
    )));
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::User { .. })));
    // The user writing starts the count again.
    type_text(&mut panel, "thanks");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.agent_hops(), 0);
    settle(&mut panel);
}

#[test]
fn a_free_panel_takes_no_question() {
    let mut panel = panel(vec![reply("noted")]);
    assert_eq!(panel.peer_state(), PeerState::Free);
    let refused = panel
        .deliver(from_peer(termide_agent_core::OriginKind::Question, 1))
        .unwrap_err();
    assert!(refused.contains("nothing in its session"), "{refused}");
    assert_eq!(panel.peer_state(), PeerState::Free);
}

#[test]
fn a_panel_rebuilt_in_place_keeps_its_number() {
    let panel = AgentPanel::new(AgentPanelSetup {
        panel_id: Some(42),
        ..setup(vec![])
    });
    assert_eq!(panel.panel_id(), 42);
    assert_ne!(AgentPanel::new(setup(vec![])).panel_id(), 42);
}

#[test]
fn clicking_a_running_tasks_row_asks_for_its_panel() {
    let mut panel = panel(vec![]);
    panel.transcript.push(Item::Tool {
        call: ToolCall {
            id: "call-9".into(),
            name: "task".into(),
            arguments: serde_json::json!({ "agent": "search", "prompt": "find the loader" }),
            extra_content: None,
        },
        result: None,
        live: Some("working".into()),
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let rows = render_text(&mut panel, 80, 20);
    // The strip's row, below the block that stands for the call.
    let y = rows
        .iter()
        .rposition(|row| row.contains("· search  find the loader") && row.starts_with('&'))
        .expect("the task's row") as u16;
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: y,
            modifiers: KeyModifiers::NONE,
        },
        ratatui::layout::Rect::new(0, 0, 80, 20),
    );
    let own = panel.panel_id();
    assert!(events.iter().any(|event| matches!(
        event,
        PanelEvent::FocusDelegate { panel, call } if *panel == own && call == "call-9"
    )));
}

#[test]
fn a_command_moved_to_the_background_shows_in_the_state_strip() {
    let mut panel = panel(vec![]);
    let call = ToolCall {
        id: "t1".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "cargo test --workspace" }),
        extra_content: None,
    };
    panel.transcript.push(Item::Tool {
        call: call.clone(),
        result: Some(ToolResultMessage::text(&call, "moved to the background")),
        live: None,
        at: String::new(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    });
    // A block after it, so selecting it is seen.
    let read = ToolCall {
        id: "r1".into(),
        name: "read".into(),
        arguments: serde_json::json!({ "path": "a.rs" }),
        extra_content: None,
    };
    panel.transcript.push(Item::Tool {
        call: read.clone(),
        result: Some(ToolResultMessage::text(&read, "text")),
        live: None,
        at: String::new(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    });
    panel.selected = 1;
    panel.runtime = Box::<Backgrounded>::default();
    let rows = render_text(&mut panel, 80, 20);
    let head = format!(
        "$ {}  cargo test --workspace",
        termide_i18n::t().agent_tool_bash()
    );
    let clock = crate::transcript::fmt_dur(134_000);
    let y = rows
        .iter()
        .rposition(|row| row.starts_with(&head) && row.contains(&clock))
        .unwrap_or_else(|| panic!("the command's row: {rows:#?}")) as u16;
    panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: y,
            modifiers: KeyModifiers::NONE,
        },
        ratatui::layout::Rect::new(0, 0, 80, 20),
    );
    assert_eq!(panel.selected, 0);
    assert!(panel.chat_focus);
}

/// The `✕` of a command's row in the background stops it, once confirmed.
#[test]
fn a_command_in_the_background_is_stopped_from_its_row() {
    let mut panel = panel(vec![]);
    let call = ToolCall {
        id: "t1".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "cargo test --workspace" }),
        extra_content: None,
    };
    panel.transcript.push(Item::Tool {
        call: call.clone(),
        result: Some(ToolResultMessage::text(&call, "moved to the background")),
        live: None,
        at: String::new(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    });
    let stopped = Arc::new(Mutex::new(Vec::new()));
    panel.runtime = Box::new(Backgrounded(Arc::clone(&stopped)));
    let (width, height) = (80, 20);
    let rows = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::Background(0));
    let line = &rows[row as usize];
    assert!(line.trim_end().ends_with('✕'), "{line}");
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: panel.strip_stop_x,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    let Some(PanelEvent::ShowConfirm {
        message,
        on_confirm: ConfirmAction::Custom(action),
    }) = events.into_iter().next()
    else {
        panic!("no confirmation");
    };
    assert!(message.contains("cargo test --workspace"), "{message}");
    assert!(stopped.lock().unwrap().is_empty(), "asked first");
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert_eq!(*stopped.lock().unwrap(), ["t1"]);
}

/// A subagent the agent waits on has a row with a `✕` as one handed over
/// in the background does: it stops it once confirmed.
#[test]
fn a_running_task_is_stopped_from_its_row() {
    let mut panel = panel(vec![]);
    let id = panel.panel_id();
    let call = ToolCall {
        id: "call-5".into(),
        name: "task".into(),
        arguments: serde_json::json!({ "agent": "search", "prompt": "find the loader" }),
        extra_content: None,
    };
    panel.transcript.push(Item::Tool {
        call,
        result: None,
        live: Some("working".into()),
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let registration = crate::live::register(crate::live::LiveTask {
        panel: Some(id),
        call: "call-5",
        from: "default",
        agent: "search",
        task: "find the loader",
        session: None,
        background: false,
    });
    let (width, height) = (80, 20);
    let rows = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::Task(0));
    let line = &rows[row as usize];
    assert!(line.trim_end().ends_with('✕'), "{line}");
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: panel.strip_stop_x,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    let Some(PanelEvent::ShowConfirm {
        message,
        on_confirm: ConfirmAction::Custom(action),
    }) = events.into_iter().next()
    else {
        panic!("no confirmation");
    };
    assert!(message.contains("search"), "{message}");
    assert!(registration.control().ended_by_user().is_none());
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert_eq!(
        registration.control().ended_by_user(),
        Some("the user stopped the subagent")
    );
}

/// A retry waited for is a row like any other: the failure, when it goes,
/// how long it has waited — and `✕` gives it up.
#[test]
fn the_strip_stop_button_gives_up_a_retry_wait() {
    let mut panel = panel(vec![limited(), reply("done")]);
    panel.on_limit = LimitPolicy::Wait;
    panel.send("do it".to_string());
    settle(&mut panel);
    assert!(panel.retry_wait.is_some());
    let (width, height) = (80, 30);
    let rows = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::RetryWait);
    let line = &rows[row as usize];
    assert!(
        line.starts_with("↻ Usage limit reached  trying again at"),
        "{line}"
    );
    assert!(line.trim_end().ends_with('✕'), "{line}");
    panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: panel.strip_stop_x,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    assert!(panel.retry_wait.is_none());
}

/// A panel following a task that runs out of sight shows what its log
/// gains, stops it on `Esc`, and opens its session once it is over.
#[test]
fn a_followed_task_shows_its_log_and_opens_when_over() {
    let dir = tempfile::tempdir().unwrap();
    let parent = termide_agent_core::SessionParent {
        session: Some("p1".into()),
        agent: "lead".into(),
        call: "call-1".into(),
    };
    let mut log = Session::create_delegate(dir.path(), dir.path(), parent).unwrap();
    log.append_message(&Message::User(UserMessage::text("find the loader")))
        .unwrap();
    let path = log.path().to_path_buf();

    let mut panel = panel(vec![]);
    let control = DelegateControl::new();
    panel.start_watch(Watch::new(
        "lead",
        "find the loader\nmore",
        path.clone(),
        control.clone(),
        3,
        "call-1",
    ));
    assert!(panel.is_delegate() && panel.runs_task(3, "call-1"));
    assert!(!panel.runs_task(3, "call-2"));
    assert_eq!(panel.delegated_by(), Some(3));
    assert!(panel.transcript.items().iter().any(|item| matches!(
        item,
        Item::User { text, .. } if text.contains("find the loader")
    )));

    // What the delegate says next shows up.
    log.append_message(&Message::Assistant(reply("found it")))
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    panel.tick();
    assert!(panel.transcript.items().iter().any(|item| matches!(
        item,
        Item::Assistant { text, .. } if text.contains("found it")
    )));

    // `Esc` only goes back to the panel that delegated the task.
    let events = panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(
        events.as_slice(),
        [PanelEvent::FocusAgentPanel(3)]
    ));
    assert!(control.ended_by_user().is_none());
    // The border offers to take the task over and to stop it; stopping asks
    // first.
    let (width, height) = (80, 30);
    let rows = render_text(&mut panel, width, height);
    assert!(rows
        .iter()
        .any(|line| line.contains("[✎]") && line.contains("[■]")));
    assert!(!rows.iter().any(|line| line.contains("[⧉]")));
    let events = panel.ask_stop_watched();
    let Some(PanelEvent::ShowConfirm {
        on_confirm: ConfirmAction::Custom(action),
        ..
    }) = events.into_iter().next()
    else {
        panic!("no confirmation");
    };
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert_eq!(
        control.ended_by_user(),
        Some("the user stopped the subagent")
    );
    assert!(panel.is_delegate());

    // Over: the session opens here as any other.
    drop(log);
    control.finish();
    panel.tick();
    assert!(!panel.is_delegate() && !panel.is_delegate());
    assert_eq!(panel.session_path(), Some(path.as_path()));
}

/// A task handed over in the background has no running block: the strip
/// lists it from the registry, its `✕` stops it, a click elsewhere asks for
/// a panel following it, and closing the panel asks first.
#[test]
fn a_background_task_has_a_row_that_stops_or_follows_it() {
    let mut panel = panel(vec![]);
    let id = panel.panel_id();
    let registration = crate::live::register(crate::live::LiveTask {
        panel: Some(id),
        call: "bg-1",
        from: "default",
        agent: "search",
        task: "find the loader\nin src/",
        session: None,
        background: true,
    });
    registration.set_status("· read src/main.rs");
    let (width, height) = (80, 30);
    let rows = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::Delegate(0));
    let line = &rows[row as usize];
    assert!(line.starts_with("& "), "{line}");
    assert!(
        line.contains("search") && line.contains("find the loader"),
        "{line}"
    );
    assert!(line.contains("· read src/main.rs"), "{line}");
    assert!(line.trim_end().ends_with('✕'), "{line}");
    assert!(panel
        .needs_close_confirmation()
        .is_some_and(|text| text.contains('1')));

    // Elsewhere on the row: a panel following it is asked for.
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    assert!(events.iter().any(|event| matches!(
        event,
        PanelEvent::FocusDelegate { panel, call } if *panel == id && call == "bg-1"
    )));
    // `✕` asks first, then stops it.
    render_text(&mut panel, width, height);
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: panel.strip_stop_x,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    let Some(PanelEvent::ShowConfirm {
        message,
        on_confirm: ConfirmAction::Custom(action),
    }) = events.into_iter().next()
    else {
        panic!("no confirmation");
    };
    assert!(message.contains("search"), "{message}");
    assert!(registration.control().ended_by_user().is_none());
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert_eq!(
        registration.control().ended_by_user(),
        Some("the user stopped the subagent")
    );
    // Over: the row and the question on closing go.
    drop(registration);
    let rows = render_text(&mut panel, width, height);
    assert!(rows.iter().all(|line| !line.contains("find the loader")));
    assert!(panel.needs_close_confirmation().is_none());
}

/// A delegate parked at a limit keeps its row, says when it goes again, and
/// asks what to do: try now, keep waiting, or give the task up. Giving it up
/// tells the panel's own agent, so it waits on no answer that will not come.
#[test]
fn a_parked_delegate_holds_a_row_and_asks_what_to_do() {
    let mut panel = panel(vec![]);
    let id = panel.panel_id();
    let now = crate::failure::unix_now();
    crate::live::park(
        id,
        crate::live::Parked {
            call: "task-1".into(),
            agent: "methodical".into(),
            task: "cut the release\nand tag it".into(),
            session: Some(std::path::PathBuf::from("/logs/d.jsonl")),
            failure: Failure::new(FailureKind::QuotaExhausted).with_reset(Some(now + 600)),
            wake_at: now + 600,
            connection: "claude-code".into(),
            attempts: 1,
            since: now,
            asked: false,
            launching: false,
        },
    );

    let (width, height) = (80, 30);
    // The tick raises the card; the strip draws the row.
    assert!(panel.poll_parked());
    let rows = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::Parked(0));
    let line = &rows[row as usize];
    assert!(line.starts_with("\u{21bb} "), "{line}");
    assert!(line.contains("methodical"), "{line}");
    assert!(line.contains("cut the release"), "{line}");
    assert!(line.trim_end().ends_with('\u{2715}'), "{line}");

    // The card asks once, and names the reset.
    assert!(matches!(panel.pending, Some(Pending::DelegateLimit { .. })));
    let detail = panel
        .pending
        .as_ref()
        .unwrap()
        .form()
        .detail()
        .unwrap_or_default();
    assert!(detail.contains("methodical"), "{detail}");

    // The first row sends it again now; the registry is told, not the run.
    assert!(panel.apply_form_action(ChoiceAction::Chosen(0)));
    assert!(crate::live::parked_of(id)[0].asked);

    // The same try is not asked about twice: it is on its way.
    panel.poll_parked();
    assert!(!matches!(
        panel.pending,
        Some(Pending::DelegateLimit { .. })
    ));
    // Parked at a later try, the card comes back.
    let mut again = crate::live::parked_of(id)
        .into_iter()
        .next()
        .expect("parked");
    again.attempts = 2;
    again.launching = false;
    crate::live::park(id, again);
    panel.poll_parked();
    assert!(matches!(panel.pending, Some(Pending::DelegateLimit { .. })));
    // Leaving it: the task waits on, unasked.
    panel.apply_form_action(ChoiceAction::Cancelled);
    assert!(panel.pending.is_none());
    assert!(crate::live::parked_of(id)[0].asked, "it still waits");

    // `\u{2715}` asks first, then gives the task up and tells the panel's agent.
    render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::Parked(0));
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: panel.strip_stop_x,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    let Some(PanelEvent::ShowConfirm {
        on_confirm: ConfirmAction::Custom(action),
        ..
    }) = events.into_iter().next()
    else {
        panic!("no confirmation");
    };
    panel.pending = None;
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert!(crate::live::parked_of(id).is_empty(), "given up");
}

/// `on_limit = "wait"` asks nothing: the parked delegate only holds its row,
/// and goes again at its reset on its own.
#[test]
fn a_parked_delegate_asks_nothing_when_the_policy_waits() {
    let mut panel = panel(vec![]);
    panel.on_limit = LimitPolicy::Wait;
    let id = panel.panel_id();
    let now = crate::failure::unix_now();
    crate::live::park(
        id,
        crate::live::Parked {
            call: "task-2".into(),
            agent: "clear".into(),
            task: "sweep the docs".into(),
            session: None,
            failure: Failure::new(FailureKind::RateLimited),
            wake_at: now + 120,
            connection: "omlx".into(),
            attempts: 2,
            since: now,
            asked: false,
            launching: false,
        },
    );
    render_text(&mut panel, 80, 30);
    assert!(matches!(
        panel.pending,
        None | Some(Pending::DelegateLimit { .. })
    ));
    panel.poll_parked();
    assert!(!matches!(
        panel.pending,
        Some(Pending::DelegateLimit { .. })
    ));
    assert_eq!(crate::live::parked_of(id).len(), 1);
    crate::live::clear_parked_of(id);
}

/// Past five running subagents the strip counts the oldest; a click on the
/// count shows them all, and the row after them folds them back.
#[test]
fn the_strip_count_of_subagents_unfolds_and_folds_back() {
    let mut panel = panel(vec![]);
    let id = panel.panel_id();
    let calls: Vec<String> = (0..7).map(|n| format!("more-{n}")).collect();
    let tasks: Vec<String> = (0..7).map(|n| format!("task number {n}")).collect();
    let registrations: Vec<_> = calls
        .iter()
        .zip(&tasks)
        .map(|(call, task)| {
            crate::live::register(crate::live::LiveTask {
                panel: Some(id),
                call,
                from: "default",
                agent: "search",
                task,
                session: None,
                background: true,
            })
        })
        .collect();
    let (width, height) = (80, 40);
    let shown = |panel: &mut AgentPanel| {
        let rows = render_text(panel, width, height);
        (0..7)
            .filter(|n| {
                rows.iter()
                    .any(|line| line.contains(&format!("task number {n}")))
            })
            .collect::<Vec<_>>()
    };
    let click = |panel: &mut AgentPanel| {
        let row = strip_row(panel, crate::strip::StripTarget::MoreTasks);
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 1,
                row,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, width, height),
        );
        row
    };
    // Folded, the newest stay in sight.
    assert_eq!(shown(&mut panel), [2, 3, 4, 5, 6]);
    let rows = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::MoreTasks);
    assert!(
        rows[row as usize].starts_with("& +2"),
        "{}",
        rows[row as usize]
    );
    click(&mut panel);
    assert_eq!(shown(&mut panel).len(), 7);
    let rows = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::MoreTasks);
    assert!(
        rows[row as usize].starts_with("& ▴"),
        "{}",
        rows[row as usize]
    );
    click(&mut panel);
    assert_eq!(shown(&mut panel).len(), 5);
    drop(registrations);
}

/// Shown over the layout, a view has buttons in place of the prompt — into
/// a panel (`F9`), take over, stop, close — and a task that ended stays as
/// it ended, to read, with only the way into a panel and out.
#[test]
fn a_view_over_the_layout_docks_and_keeps_an_ended_task() {
    let dir = tempfile::tempdir().unwrap();
    let parent = termide_agent_core::SessionParent {
        session: Some("p1".into()),
        agent: "lead".into(),
        call: "call-ov".into(),
    };
    let mut log = Session::create_delegate(dir.path(), dir.path(), parent).unwrap();
    log.append_message(&Message::User(UserMessage::text("find the loader")))
        .unwrap();
    let path = log.path().to_path_buf();
    let mut panel = panel(vec![]);
    let control = DelegateControl::new();
    panel.set_in_overlay(true);
    panel.start_watch(Watch::new(
        "lead",
        "find the loader",
        path.clone(),
        control.clone(),
        4,
        "call-ov",
    ));
    let (width, height) = (80, 30);
    let rows = render_text(&mut panel, width, height);
    let buttons = rows
        .iter()
        .position(|line| {
            line.contains("[ In a panel F9 ]")
                && line.contains("[ Take over Ctrl+T ]")
                && line.contains("[ Stop ]")
                && line.contains("[ Close Esc ]")
        })
        .expect("the buttons are under the view");
    assert!(rows[buttons - 1].chars().all(|c| c == '─'));
    assert!(!rows
        .iter()
        .any(|line| line.contains("[✎]") || line.contains("[■]")));
    let events = panel.handle_key(chord(KeyCode::F(9), KeyModifiers::NONE));
    assert!(matches!(events.as_slice(), [PanelEvent::DockOverlay]));
    let click = |panel: &mut AgentPanel, label: &str| {
        let row = &rows[buttons];
        let column = row[..row.find(label).unwrap()].chars().count() as u16 + 2;
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column,
                row: buttons as u16,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, width, height),
        )
    };
    assert!(matches!(
        click(&mut panel, "[ In a panel").as_slice(),
        [PanelEvent::DockOverlay]
    ));
    assert!(matches!(
        click(&mut panel, "[ Close").as_slice(),
        [PanelEvent::FocusAgentPanel(4)]
    ));

    // Over: still the task as it ended, with only the way into a panel.
    drop(log);
    control.finish();
    panel.tick();
    assert!(panel.is_delegate());
    let rows = render_text(&mut panel, width, height);
    assert!(rows
        .iter()
        .any(|line| line.contains("[ In a panel F9 ]") && line.contains("[ Close Esc ]")));
    assert!(!rows
        .iter()
        .any(|line| line.contains("[ Stop ]") || line.contains("Take over")));
    // In a panel, its session opens.
    panel.set_in_overlay(false);
    panel.tick();
    assert!(!panel.is_delegate());
    assert_eq!(panel.session_path(), Some(path.as_path()));
}

/// A session reopened after termide closed names the delegated tasks left
/// unfinished in their own logs on the card that asks to carry the work on.
#[test]
fn the_carry_on_card_names_the_delegates_left_behind() {
    let dir = tempfile::tempdir().unwrap();
    let parent_path = {
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("build it")))
            .unwrap();
        session.append_run_start().unwrap();
        session.path().to_path_buf()
    };
    let id = Session::open_read_only(&parent_path)
        .unwrap()
        .id()
        .to_string();
    {
        let mut log = Session::create_delegate(
            dir.path(),
            dir.path(),
            termide_agent_core::SessionParent {
                // The parent names the agent that delegated, not the delegate.
                session: Some(id.clone()),
                agent: "default".into(),
                call: "call-1".into(),
            },
        )
        .unwrap();
        log.append_message(&Message::User(UserMessage::text("find the loader")))
            .unwrap();
        log.append_agent_change("search").unwrap();
        log.append_run_start().unwrap();
    }
    // Another panel's delegate, and this panel's finished one: neither shows.
    {
        let mut other = Session::create_delegate(
            dir.path(),
            dir.path(),
            termide_agent_core::SessionParent {
                session: Some("elsewhere".into()),
                agent: "default".into(),
                call: "call-9".into(),
            },
        )
        .unwrap();
        other
            .append_message(&Message::User(UserMessage::text("read it")))
            .unwrap();
        other.append_run_start().unwrap();
    }

    let panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::open(&parent_path).unwrap()),
        ..setup(vec![reply("ok")])
    });
    let left = panel.unfinished_delegates();
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(
        (left[0].agent.as_str(), left[0].call.as_str()),
        ("search", "call-1")
    );
    let detail = carry_on_card(&panel);
    // The delegate's own name, not the delegating agent's.
    assert!(detail.contains("search"), "{detail}");
    assert!(!detail.contains("default"), "{detail}");
}

/// A session whose own run ended cleanly, but which left delegated tasks
/// unfinished in their logs, opens those sessions as panels of their own:
/// each asks there whether to carry its run on, with a prompter of its own.
#[test]
fn a_finished_panel_opens_the_sessions_of_its_unfinished_delegates() {
    let dir = tempfile::tempdir().unwrap();
    let parent_path = {
        let mut session = Session::create(dir.path(), dir.path()).unwrap();
        session
            .append_message(&Message::User(UserMessage::text("build it")))
            .unwrap();
        session.append_run_start().unwrap();
        session
            .append_message(&Message::Assistant(reply("done")))
            .unwrap();
        session.append_run_end(false).unwrap();
        session.path().to_path_buf()
    };
    let id = Session::open_read_only(&parent_path)
        .unwrap()
        .id()
        .to_string();
    let delegate_path = {
        let mut log = Session::create_delegate(
            dir.path(),
            dir.path(),
            termide_agent_core::SessionParent {
                // The parent names the agent that delegated, not the delegate.
                session: Some(id.clone()),
                agent: "default".into(),
                call: "call-1".into(),
            },
        )
        .unwrap();
        log.append_message(&Message::User(UserMessage::text("find the loader")))
            .unwrap();
        log.append_agent_change("search").unwrap();
        log.append_run_start().unwrap();
        log.path().to_path_buf()
    };

    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::open(&parent_path).unwrap()),
        ..setup(vec![reply("ok")])
    });
    // The panel's own run is over, so no card asks about it; the delegate
    // holds a row above the input instead of opening a panel of its own.
    let events = panel.tick();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PanelEvent::OpenAgentSession(_))),
        "{events:?}"
    );
    assert_eq!(
        panel.restored_delegates.len(),
        1,
        "the delegate holds a row"
    );
    assert_eq!(panel.restored_delegates[0].agent, "search");
    // The idle panel asks about the delegates alone.
    assert!(
        matches!(panel.pending, Some(Pending::Unfinished { .. })),
        "the delegates are asked about"
    );
    // Continue carries them on: the app restarts each on the panel's runner,
    // under the `task` call that handed it over.
    panel.apply_form_action(ChoiceAction::Chosen(0));
    let events = panel.tick();
    assert!(
        events.iter().any(|event| matches!(
            event,
            PanelEvent::RestartDelegates { panel: _, items }
                if items.len() == 1
                    && items[0].agent == "search"
                    && items[0].session == delegate_path
                    && items[0].call == "call-1"
                    && !items[0].task.is_empty()
        )),
        "{events:?}"
    );
    assert!(
        panel.restored_delegates.is_empty(),
        "a task carried on leaves its row"
    );
}

/// A delegate row above the input opens the session it waits in, where that
/// session's own card asks to carry it on; `Later` leaves the rows there.
#[test]
fn a_row_of_an_unfinished_delegate_opens_its_session_and_later_keeps_it() {
    let dir = tempfile::tempdir().unwrap();
    let parent_path = {
        let log = Session::create(dir.path(), dir.path()).unwrap();
        log.path().to_path_buf()
    };
    let delegate_path = {
        let mut log = Session::create_delegate(
            dir.path(),
            dir.path(),
            termide_agent_core::SessionParent {
                session: Some(
                    Session::open_read_only(&parent_path)
                        .unwrap()
                        .id()
                        .to_string(),
                ),
                agent: "default".into(),
                call: "call-1".into(),
            },
        )
        .unwrap();
        log.append_message(&Message::User(UserMessage::text("find the loader")))
            .unwrap();
        log.append_agent_change("search").unwrap();
        log.append_run_start().unwrap();
        log.path().to_path_buf()
    };
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::open(&parent_path).unwrap()),
        ..setup(vec![reply("ok")])
    });
    panel.tick();
    // Later: nothing runs, and the row stays for another time.
    panel.apply_form_action(ChoiceAction::Chosen(1));
    let events = panel.tick();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, PanelEvent::RestartDelegates { .. })),
        "{events:?}"
    );
    assert_eq!(panel.restored_delegates.len(), 1, "the row waits on");
    // A click on the row opens the session the delegate waits in.
    let rows = render_text(&mut panel, 80, 30);
    let row = strip_row(&panel, crate::strip::StripTarget::DelegateSession(0));
    let line = &rows[row as usize];
    assert!(
        line.contains("search") && line.contains("find the loader"),
        "{line}"
    );
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, 80, 30),
    );
    assert!(
        events.iter().any(
            |event| matches!(event, PanelEvent::OpenAgentSession(path) if path == &delegate_path)
        ),
        "{events:?}"
    );
}

/// A delegate's session reopened and carried on answers the panel that
/// delegated the task: its last words go back there at the run's end, which
/// is what the `task` call was made for.
#[test]
fn a_carried_on_delegate_answers_the_panel_that_delegated_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let mut log = Session::create_delegate(
            dir.path(),
            Path::new("/tmp"),
            termide_agent_core::SessionParent {
                session: Some("parent-1".into()),
                // The parent names the agent that delegated, not the delegate.
                agent: "default".into(),
                call: "call-1".into(),
            },
        )
        .unwrap();
        log.append_message(&Message::User(UserMessage::text("find the loader")))
            .unwrap();
        log.append_agent_change("search").unwrap();
        log.append_run_start().unwrap();
        log.path().to_path_buf()
    };
    let mut panel = reopen(
        &path,
        dir.path(),
        vec![reply("the loader is in src/loader.rs")],
    );
    assert_eq!(panel.restored, Some(Restored::Resume));
    // Whose task this run answers, and that nothing has answered yet.
    assert_eq!(
        panel.carry_back.as_ref().map(|carry| (
            carry.parent_session.as_str(),
            carry.agent.as_str(),
            carry.running
        )),
        Some(("parent-1", "search", false)),
    );

    panel.apply_form_action(ChoiceAction::Chosen(0));
    let event = wait_for(&mut panel, |event| {
        matches!(event, PanelEvent::DelegateAnswerToParent { .. })
    });
    match event {
        PanelEvent::DelegateAnswerToParent {
            parent_session,
            agent,
            text,
            ..
        } => {
            assert_eq!(parent_session, "parent-1");
            // The delegate's own name, not the delegating agent's.
            assert_eq!(agent, "search");
            assert!(text.contains("src/loader.rs"), "{text}");
        }
        other => panic!("not a carried-on answer: {other:?}"),
    }
    assert!(panel.carry_back.is_none(), "the answer is owed once");
}

/// The panel's pause holds the tasks it handed over, the one its run waits on
/// too, and its withdrawal lets them go; the pause's row counts them.
#[test]
fn pausing_the_panel_holds_its_delegates_and_resumes_them() {
    let mut panel = panel(vec![]);
    let id = panel.panel_id();
    let held = crate::live::register(crate::live::LiveTask {
        panel: Some(id),
        call: "call-hold",
        from: "default",
        agent: "search",
        task: "find the loader",
        session: None,
        background: true,
    });
    let waited = crate::live::register(crate::live::LiveTask {
        panel: Some(id),
        call: "call-sync",
        from: "default",
        agent: "claude",
        task: "review the diff",
        session: None,
        background: false,
    });

    panel.busy = true;
    assert!(panel.request_pause());
    assert!(
        held.control().is_paused(),
        "the delegate waits with the run"
    );
    assert!(
        waited.control().is_paused(),
        "so does the one the run waits on"
    );
    let rows = panel.strip_rows(80);
    let notes = &rows
        .iter()
        .find(|r| r.target == crate::strip::StripTarget::Pause)
        .expect("the pause row")
        .notes;
    assert_eq!(notes.len(), 1, "{notes:?}: the held ones counted");

    panel.cancel_pause();
    assert!(!held.control().is_paused(), "withdrawing frees it");
    assert!(!waited.control().is_paused());
}

/// Stopping the run stops the work it handed over, held at a pause or not,
/// and the answer of a background task stopped with it waits for the next
/// request rather than starting a run.
#[test]
fn stopping_the_run_stops_its_delegates_and_holds_their_answers() {
    let mut panel = panel(vec![]);
    let id = panel.panel_id();
    let background = crate::live::register(crate::live::LiveTask {
        panel: Some(id),
        call: "call-bg",
        from: "default",
        agent: "search",
        task: "find the loader",
        session: None,
        background: true,
    });
    panel.busy = true;
    assert!(panel.request_pause());
    panel.abort();
    assert!(background.control().stop_token().is_cancelled());
    assert!(!background.control().is_paused(), "stopped, not paused");
    assert!(crate::live::take_quiet(id, "call-bg"));

    // The run has not ended yet: the answer is held, not queued to it.
    let answer = UserMessage::text("The background task is over");
    panel.deliver_held(answer.clone()).unwrap();
    assert_eq!(panel.held_reports.len(), 1);
    assert_eq!(panel.queued, (0, 0));

    // The next request takes it along.
    panel.busy = false;
    panel.stop_requested = None;
    panel.send_message(UserMessage::text("go on with the docs"));
    assert!(panel.held_reports.is_empty());
    assert!(
        panel
            .queued_texts
            .iter()
            .any(|(text, _)| text.contains("background task is over")),
        "queued to the new run"
    );
}

/// Held at its panel's pause, a view says so on the rule above its buttons:
/// the log stops growing, and nothing else on screen tells why.
#[test]
fn a_held_delegate_says_it_stands_still() {
    let dir = tempfile::tempdir().unwrap();
    let parent = termide_agent_core::SessionParent {
        session: Some("p1".into()),
        agent: "lead".into(),
        call: "call-hold".into(),
    };
    let mut log = Session::create_delegate(dir.path(), dir.path(), parent).unwrap();
    log.append_message(&Message::User(UserMessage::text("find the loader")))
        .unwrap();
    let mut panel = panel(vec![]);
    let control = DelegateControl::new();
    panel.set_in_overlay(true);
    panel.start_watch(Watch::new(
        "lead",
        "find the loader",
        log.path().to_path_buf(),
        control.clone(),
        4,
        "call-hold",
    ));
    let rows = render_text(&mut panel, 80, 30);
    let rule = rows
        .iter()
        .position(|line| line.contains("[ In a panel F9 ]"))
        .expect("the buttons are under the view")
        - 1;
    assert!(!rows[rule].contains("paused"));

    control.pause();
    let rows = render_text(&mut panel, 80, 30);
    assert!(rows[rule].contains("paused"), "{:?}", rows[rule]);

    control.resume();
    assert!(!render_text(&mut panel, 80, 30)[rule].contains("paused"));
}

/// While a view follows a task, the panel that delegated it sets the task's
/// cards aside for it; the view shows them, and gives back what it holds
/// when it goes.
#[test]
fn a_delegates_cards_follow_the_view_of_its_task() {
    let mut parent = panel(vec![]);
    let id = parent.panel_id();
    let (prompter, rx) = permission_channel(CancelToken::new());
    parent.permission_rx = rx;
    let registration = crate::live::register(crate::live::LiveTask {
        panel: Some(id),
        call: "call-cards",
        from: "default",
        agent: "search",
        task: "find it",
        session: Some(std::path::PathBuf::from("/nonexistent/x.jsonl")),
        background: false,
    });
    let watch = crate::live::watch(id, "call-cards").unwrap();
    let mut view = panel(vec![]);
    view.start_watch(watch);

    let mut delegate = prompter
        .for_delegate("search", CancelToken::new())
        .for_task("call-cards");
    let worker = std::thread::spawn(move || {
        delegate.ask(&termide_agent_core::PermissionRequest {
            tool: "shell".into(),
            subject: "git fetch".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": "git fetch" }),
                extra_content: None,
            },
            suggested_pattern: String::new(),
            can_persist: false,
            can_allow_session: false,
            parts: Vec::new(),
        })
    });
    // The parent sets it aside rather than showing it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut out = Vec::new();
    while out.is_empty() {
        parent.tick();
        out = parent.take_cards_out();
        assert!(Instant::now() < deadline, "no card set aside");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(parent.pending.is_none());
    let (call, card) = out.pop().unwrap();
    assert_eq!(call, "call-cards");
    // The view shows it.
    view.give_card(card);
    view.tick();
    assert!(matches!(view.pending, Some(Pending::Permission { .. })));
    // The view goes: the card goes back, and the parent shows it.
    let back = view.release_cards();
    drop(view);
    for card in back {
        parent.give_card(card);
    }
    parent.tick();
    assert!(matches!(parent.pending, Some(Pending::Permission { .. })));
    assert!(parent.answer_permission(PermissionAnswer::AllowOnce));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowOnce);
    drop(registration);
}

/// While the panel follows another agent's task, its `[≡]` menu offers only
/// what reads the session: the rest would do nothing.
#[test]
fn a_panel_following_a_task_offers_only_what_reads_its_session() {
    let mut panel = panel(vec![]);
    let full = panel.context_menu_items().len();
    assert!(full > 2);
    panel.start_watch(Watch::new(
        "lead",
        "find it",
        std::path::PathBuf::from("/nonexistent/x.jsonl"),
        DelegateControl::new(),
        5,
        "call-menu",
    ));
    let actions: Vec<String> = panel
        .context_menu_items()
        .into_iter()
        .map(|(_, action)| action)
        .collect();
    assert_eq!(actions, [SESSION_INFO_ACTION, SAVE_CHAT_ACTION]);
}

/// A task handed over in the background asks and answers through the
/// panel's session: the session does not change under it, as it does not
/// under a run.
#[test]
fn the_session_waits_for_the_tasks_handed_over_in_the_background() {
    let mut panel = panel(vec![]);
    let registration = crate::live::register(crate::live::LiveTask {
        panel: Some(panel.panel_id()),
        call: "bg-session",
        from: "default",
        agent: "search",
        task: "find it",
        session: None,
        background: true,
    });
    assert!(!panel.switch_session(None));
    assert!(panel.transcript.items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text.contains('1')
    )));
    drop(registration);
    assert!(panel.switch_session(None));
}

#[test]
fn trust_lists_what_the_project_brings_and_approves_it() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Untrusted::new(&asked)),
        ..setup(vec![])
    });
    // The held hook is said when the panel opens.
    assert_eq!(
        notices(&panel),
        ["hook check: from the project, runs `./check.sh` — /trust check to let it run"]
    );
    for command in ["/trust", "/trust ext", "/trust check", "/trust nope"] {
        type_text(&mut panel, command);
        panel.submit();
    }
    assert_eq!(
        *asked.lock().unwrap(),
        ["trust ext", "trust check", "trust nope"]
    );
    let seen = notices(&panel);
    assert_eq!(
        &seen[1..],
        [
            "mcp ext: from the project, runs `npx ext-server` — /trust ext to start it",
            "hook check: from the project, runs `./check.sh` — /trust check to let it run",
            "ext: approved for this project",
            "mcp ext: connecting",
            "check: approved for this project",
            "Nothing named nope waits for approval",
        ]
    );
    assert!(!panel.is_busy());
}

/// `/trust <name>` approves the launch last shown for it: one never shown,
/// or changed since, is shown instead, and approved only when asked again.
#[test]
fn trust_approves_only_the_launch_that_was_shown() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let catalog = Untrusted::new(&asked);
    let ext = Arc::clone(&catalog.ext);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(catalog),
        ..setup(vec![])
    });
    // Never shown: shown now, not approved.
    type_text(&mut panel, "/trust ext");
    panel.submit();
    // Shown by the listing, then changed before the approval.
    type_text(&mut panel, "/trust");
    panel.submit();
    *ext.lock().unwrap() = "npx evil-server";
    type_text(&mut panel, "/trust ext");
    panel.submit();
    // What was shown last is what this one approves.
    type_text(&mut panel, "/trust ext");
    panel.submit();
    // The hook shown when the panel opened is approved as it was.
    type_text(&mut panel, "/trust check");
    panel.submit();
    assert_eq!(
        &notices(&panel)[1..],
        [
            "mcp ext: from the project, runs `npx ext-server` — /trust ext to start it",
            "mcp ext: from the project, runs `npx ext-server` — /trust ext to start it",
            "hook check: from the project, runs `./check.sh` — /trust check to let it run",
            "mcp ext: from the project, runs `npx evil-server` — /trust ext to start it",
            "ext: approved for this project",
            "mcp ext: connecting",
            "check: approved for this project",
        ]
    );
}

/// A tick on a held server approves the launch shown beside the box: one
/// changed since is shown instead, and its tools are not switched on.
#[test]
fn a_tick_on_a_server_changed_since_the_list_opened_approves_nothing() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let catalog = Untrusted::new(&asked);
    let ext = Arc::clone(&catalog.ext);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(catalog),
        ..setup(vec![])
    });
    let events = panel.handle_status_action(crate::toolset::TOOLSET_ACTION);
    let Some(PanelEvent::ShowChecklist { groups, .. }) = events.first() else {
        panic!("the toolset list opens: {events:?}");
    };
    let key = groups
        .iter()
        .find_map(|g| g.switch.as_ref())
        .unwrap()
        .key
        .clone();
    *ext.lock().unwrap() = "npx evil-server";
    panel.handle_command(PanelCommand::ChecklistButton {
        action: crate::toolset::TOOLSET_ACTION.into(),
        id: key,
    });
    assert_eq!(*asked.lock().unwrap(), ["trust ext"]);
    assert!(panel.mcp_enable.is_empty());
    assert_eq!(
        notices(&panel).last().unwrap(),
        "mcp ext: from the project, runs `npx evil-server` — /trust ext to start it"
    );
    // `/trust ext` now approves what that notice showed.
    type_text(&mut panel, "/trust ext");
    panel.submit();
    assert_eq!(notices(&panel).last().unwrap(), "mcp ext: connecting");
}

#[test]
fn a_held_server_ticked_in_the_toolset_list_is_approved_and_comes_on() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Untrusted::new(&asked)),
        late_tools: Some(rx),
        ..setup(vec![])
    });
    // The session had switched the server's tools off, and something else.
    panel.toolset_off = ["ext__a", "ext__b", "read"]
        .into_iter()
        .map(String::from)
        .collect();
    let events = panel.handle_status_action(crate::toolset::TOOLSET_ACTION);
    let Some(PanelEvent::ShowChecklist { groups, .. }) = events.first() else {
        panic!("the toolset list opens: {events:?}");
    };
    // The held server's heading: a checkbox, unticked, what ticking it
    // approves beside it, and no buttons.
    let ext = groups.iter().find(|g| g.name == "MCP: ext").unwrap();
    assert_eq!(
        ext.switch,
        Some(termide_core::ChecklistSwitch {
            key: "mcp-trust:fp-npx-ext-server:ext".into(),
            checked: false,
        })
    );
    assert_eq!(ext.note, "npx ext-server");
    assert!(ext.buttons.is_empty(), "{:?}", ext.buttons);

    // Left unticked, nothing is approved.
    panel.handle_command(PanelCommand::ChecklistDone {
        action: crate::toolset::TOOLSET_ACTION.into(),
        checked: Vec::new(),
    });
    assert!(asked.lock().unwrap().is_empty());

    // Ticked: approved at once, the list staying open and brought up to
    // date; its tools come on as they arrive and are drawn into the open
    // list; the rest of what the session switched off stays off.
    panel.handle_status_action(crate::toolset::TOOLSET_ACTION);
    panel.pending_events.clear();
    panel.handle_command(PanelCommand::ChecklistButton {
        action: crate::toolset::TOOLSET_ACTION.into(),
        id: "mcp-trust:fp-npx-ext-server:ext".into(),
    });
    assert_eq!(*asked.lock().unwrap(), ["trust ext"]);
    assert!(panel.toolset_list_open);
    assert!(panel
        .pending_events
        .iter()
        .any(|e| matches!(e, PanelEvent::RefreshChecklist(_))));
    tx.send(LateTools::Ready {
        source: "ext".into(),
        tools: vec![
            Arc::new(Late("ext__a")) as Arc<dyn termide_agent_core::Tool>,
            Arc::new(Late("ext__b")) as Arc<dyn termide_agent_core::Tool>,
        ],
    })
    .unwrap();
    let events = panel.tick();
    let Some(PanelEvent::RefreshChecklist(refresh)) = events
        .iter()
        .rev()
        .find(|e| matches!(e, PanelEvent::RefreshChecklist(_)))
    else {
        panic!("the open list is brought up to date: {events:?}");
    };
    let ext: Vec<(&str, bool)> = refresh
        .items
        .iter()
        .filter(|i| i.key.starts_with("ext__"))
        .map(|i| (i.key.as_str(), i.checked))
        .collect();
    assert_eq!(ext, [("ext__a", true), ("ext__b", true)]);
    assert_eq!(
        panel.toolset_off.iter().collect::<Vec<_>>(),
        ["read"],
        "{:?}",
        panel.toolset_off
    );
    assert!(!panel.blocked.read().unwrap().contains("ext__a"));

    // Its next set, after a reconnect, keeps what the user chooses then.
    panel.toolset_off.insert("ext__a".into());
    tx.send(LateTools::Ready {
        source: "ext".into(),
        tools: vec![Arc::new(Late("ext__a")) as Arc<dyn termide_agent_core::Tool>],
    })
    .unwrap();
    panel.tick();
    assert!(panel.toolset_off.contains("ext__a"));
}

#[test]
fn a_run_is_told_as_it_starts_and_ends() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::AgentEnd);
    let runs: Vec<Option<bool>> = panel
        .pending_events
        .iter()
        .filter_map(|event| match event {
            PanelEvent::AgentRun { finished, .. } => Some(*finished),
            _ => None,
        })
        .collect();
    assert_eq!(runs, [None, Some(true)]);
}

/// A pause asked for on a run that then ends on its own — its last reply
/// left nothing to do — leaves no pause to continue from: the delegates it
/// held go on, rather than wait for a `/continue` that has nothing to resume.
#[test]
fn a_pause_the_run_never_reached_frees_its_delegates() {
    let mut panel = panel(vec![]);
    let id = panel.panel_id();
    let held = crate::live::register(crate::live::LiveTask {
        panel: Some(id),
        call: "call-free",
        from: "default",
        agent: "search",
        task: "find the loader",
        session: None,
        background: true,
    });
    panel.apply(AgentEvent::AgentStart);
    assert!(panel.request_pause());
    assert!(held.control().is_paused());
    panel.apply(AgentEvent::AgentEnd);
    assert!(!panel.paused, "the run ended, not paused");
    assert!(
        !held.control().is_paused(),
        "nothing holds the delegate now"
    );
}

/// A carried-on delegate paused on the way owes its answer still: only the
/// run's real end reports back to the panel that delegated it.
#[test]
fn a_carried_on_delegate_paused_on_the_way_still_owes_its_answer() {
    let mut panel = panel(vec![]);
    panel.carry_back = Some(crate::unfinished::CarryBack {
        parent_session: "parent-1".into(),
        agent: "search".into(),
        running: true,
    });
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    assert!(panel.carry_back.is_some(), "a pause is not the task's end");
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::AgentEnd);
    assert!(panel.carry_back.is_none(), "the real end settles it");
}

/// Leaving a session leaves its unfinished delegates and the answer it owed
/// behind: a new session's carry-on must not restart another's tasks.
#[test]
fn a_new_session_forgets_the_unfinished_delegates_of_the_last() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let log = Session::create(dir.path(), Path::new("/tmp")).unwrap();
        log.path().to_path_buf()
    };
    let mut panel = reopen(&path, dir.path(), vec![]);
    panel
        .restored_delegates
        .push(crate::unfinished::RestoredDelegate {
            agent: "search".into(),
            session: dir.path().join("delegate.jsonl"),
            call: "call-1".into(),
            task: "find the loader".into(),
        });
    panel.carry_back = Some(crate::unfinished::CarryBack {
        parent_session: "parent-1".into(),
        agent: "search".into(),
        running: false,
    });
    assert!(panel.switch_session(None));
    assert!(panel.restored_delegates.is_empty());
    assert!(panel.carry_back.is_none());
}

/// The answers held for the next request are in the session's log: a panel
/// reopened on it after termide closed still sends them with that request.
#[test]
fn held_answers_survive_a_reopen_and_go_with_the_next_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let mut log = Session::create(dir.path(), Path::new("/tmp")).unwrap();
        log.append_message(&Message::User(UserMessage::text("hand the search over")))
            .unwrap();
        log.path().to_path_buf()
    };
    {
        let mut panel = reopen(&path, dir.path(), vec![]);
        panel
            .deliver_held(UserMessage::text("The background task is over"))
            .unwrap();
        assert_eq!(panel.held_reports.len(), 1);
    }
    let mut panel = reopen(&path, dir.path(), vec![reply("noted")]);
    assert_eq!(panel.held_reports.len(), 1, "taken up from the log");
    panel.send_message(UserMessage::text("go on with the docs"));
    assert!(panel.held_reports.is_empty());
    assert!(
        panel.session.as_ref().unwrap().held_reports().is_empty(),
        "the log no longer holds them"
    );
}

/// Every delegate row is drawn from where its task stands: the same label
/// and agent for all, a paused or unfinished one saying so, a parked one
/// under the retry wait's glyph — not `⏸`, which marks unfinished sessions
/// in the lists — with when it goes again.
#[test]
fn a_delegate_row_says_where_its_task_stands() {
    use crate::strip::{delegate_row, DelegateState, StripTarget};
    let t = termide_i18n::t();
    let row = |state| {
        delegate_row(
            "finder".into(),
            "\n  find it\nthen more",
            state,
            StripTarget::Delegate(0),
        )
    };

    let running = row(DelegateState::Running {
        status: "· read a.rs".into(),
    });
    assert_eq!(running.glyph, "&");
    assert_eq!(running.body, "find it");
    assert_eq!(running.notes, vec!["finder".to_string()]);
    assert_eq!(running.status, "· read a.rs");

    let paused = row(DelegateState::Paused);
    assert_eq!(paused.glyph, "&");
    assert_eq!(paused.notes[1], t.agent_strip_paused());
    assert!(paused.status.is_empty());

    let unfinished = row(DelegateState::Unfinished);
    assert_eq!(unfinished.notes[1], t.agent_strip_left_unfinished());

    let parked = row(DelegateState::Parked { wakes_at: 0 });
    assert_eq!(parked.glyph, crate::failure::WAIT_GLYPH);
    assert_eq!(parked.label, t.agent_parked_strip());
    assert_eq!(parked.notes.len(), 2);
}
