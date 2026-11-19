//! The state strip between the transcript and the input, and the run controls on the prompt border.

use super::*;

#[test]
fn the_state_strip_shows_queued_messages_until_the_agent_takes_them() {
    let mut panel = AgentPanel::new(setup(vec![]));
    assert!(panel.state_lines(40).is_empty());
    panel.apply(AgentEvent::AgentStart);
    for text in ["first\nsecond line", "two", "three", "four", "five"] {
        panel.send(text.to_string());
    }
    // Queued while busy: nothing in the transcript, all of it in the strip.
    assert!(panel
        .transcript()
        .items()
        .iter()
        .all(|i| !matches!(i, Item::Notice { .. } | Item::User { .. })));
    let lines = strip_text(&panel.state_lines(40));
    assert!(lines[0].starts_with('╌'));
    assert!(lines[1].starts_with("› Queued  first"), "{lines:?}");
    assert!(!lines[1].contains("second line"));
    assert!(lines[2].starts_with("› Queued  two"), "{lines:?}");
    // How long each has waited, then the `✕` that takes the queue back.
    let unit = termide_i18n::t().time_unit_secs();
    let tail = lines[2].trim_end();
    assert!(tail.ends_with('✕'), "{lines:?}");
    assert!(
        tail.trim_end_matches('✕').trim_end().ends_with(unit),
        "{lines:?}"
    );
    assert!(lines[4].starts_with("› 2 more queued"), "{lines:?}");
    assert!(lines.iter().all(|l| termide_ui::str_display_width(l) <= 40));
    // The agent takes the two oldest; the strip follows its count.
    panel.apply(AgentEvent::QueueUpdate {
        steering: 3,
        follow_up: 0,
    });
    let lines = strip_text(&panel.state_lines(40));
    assert!(lines[1].starts_with("› Queued  three"), "{lines:?}");
    assert_eq!(lines.len(), 4);
    panel.apply(AgentEvent::QueueUpdate {
        steering: 0,
        follow_up: 0,
    });
    assert!(panel.state_lines(40).is_empty());
}

#[test]
fn the_state_strip_sits_between_the_transcript_and_the_input() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.send("waiting its turn".to_string());
    let rows = render_text(&mut panel, 40, 12);
    let at = rows
        .iter()
        .position(|r| r.starts_with("› Queued  waiting its turn"))
        .expect("queued row");
    assert!(rows[at - 1].starts_with('╌'), "{rows:?}");
    // The input bar's titled border follows the strip directly.
    assert!(
        !rows[at + 1].trim().is_empty() && !rows[at + 1].starts_with('›'),
        "{rows:?}"
    );
    assert_eq!(at, rows.len() - 1 - panel.input_area.height as usize);
}

#[test]
fn a_pending_pause_lives_in_the_state_strip_and_a_pause_in_the_transcript() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    type_text(&mut panel, "/pause");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let t = termide_i18n::t();
    let lines = strip_text(&panel.state_lines(60));
    assert!(
        lines[1].starts_with(&format!(
            "‖ {}  {}",
            t.agent_strip_pause(),
            t.agent_strip_after_step()
        )),
        "{lines:?}"
    );
    assert!(lines[1].trim_end().ends_with('✕'), "{lines:?}");
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    // Once the pause takes effect the strip clears: the transcript's `‖`
    // line stands for it.
    assert!(panel.state_lines(80).is_empty());
    // History keeps only the event: the run's closing line, marked paused.
    let items = panel.transcript().items();
    assert!(items.iter().all(|i| !matches!(i, Item::Notice { .. })));
    assert!(matches!(
        items.last(),
        Some(Item::RunEnd {
            paused: true,
            ok: true,
            ..
        })
    ));
}

#[test]
fn a_new_session_does_not_inherit_a_pause() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    assert_eq!(
        panel.run_buttons(),
        vec![RunButton::Continue, RunButton::Stop]
    );
    // The paused run went with the old runtime: the fresh session is idle.
    assert!(panel.switch_session(None));
    assert!(panel.run_buttons().is_empty());
    assert!(!panel.paused);
    assert!(panel.pause_start.is_none());
    assert!(panel.run_start.is_none());
}

#[test]
fn a_pause_ticks_its_own_length_and_the_run_clock_resumes_from_the_request() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    let start = panel.run_start.expect("the run started");
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    // Paused: the run keeps its start, and the pause's line counts the
    // pause itself.
    assert_eq!(panel.run_start, Some(start));
    panel.pause_start = Some(Instant::now() - Duration::from_secs(3));
    panel.tick();
    let lines = strip_text(panel.transcript.lines(40, &panel.colors, false));
    // The duration alone, no time of day.
    assert_eq!(lines.last().map(|l| l.trim()), Some("‖ 3s"));
    // Resumed: the pause's length stays on its line, the run's clock goes
    // on from the request.
    panel.paused = false;
    panel.resuming = true;
    panel.apply(AgentEvent::AgentStart);
    assert_eq!(panel.run_start, Some(start));
    assert!(panel.pause_start.is_none());
    assert!(matches!(
        panel.transcript.items().last(),
        Some(Item::RunEnd { paused: true, elapsed_ms, .. }) if *elapsed_ms >= 3000
    ));
    // A fresh run (not a resume) starts its own clock.
    panel.apply(AgentEvent::AgentEnd);
    panel.apply(AgentEvent::AgentStart);
    assert_ne!(panel.run_start, Some(start));
}

#[test]
fn the_prompt_border_carries_the_run_controls() {
    let mut panel = AgentPanel::new(setup(vec![]));
    let (width, height) = (40, 12);
    // The controls on the border, as text, and a click on one by glyph.
    let border = |panel: &mut AgentPanel| {
        let rows = render_text(panel, width, height);
        rows[panel.input_area.y as usize].clone()
    };
    let click = |panel: &mut AgentPanel, glyph: &str| {
        let row = border(panel);
        let col = row[..row.find(glyph).expect("the control is on the border")]
            .chars()
            .count() as u16;
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: col,
                row: panel.input_area.y,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, width, height),
        );
    };
    // Idle: no controls.
    assert!(!border(&mut panel).contains('['));
    // Working: pause and stop.
    panel.apply(AgentEvent::AgentStart);
    let row = border(&mut panel);
    assert!(row.ends_with("[‖][■]─"), "{row:?}");
    click(&mut panel, "[‖]");
    assert!(panel.pause_requested, "the pause control asks to pause");
    // A pause asked for: continue withdraws it.
    assert!(border(&mut panel).contains("[▶][■]"));
    click(&mut panel, "[▶]");
    assert!(
        !panel.pause_requested,
        "continue withdraws the pending pause"
    );
    // The strip's pending-pause line withdraws it too.
    click(&mut panel, "[‖]");
    let _ = render_text(&mut panel, width, height);
    let row = strip_row(&panel, crate::strip::StripTarget::Pause);
    panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    assert!(!panel.pause_requested);
    // `/continue` withdraws a pending pause as well.
    click(&mut panel, "[‖]");
    type_text(&mut panel, "/continue");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !panel.pause_requested,
        "/continue withdraws the pending pause"
    );
    // Paused: continue alone, and the pause's line resumes too.
    click(&mut panel, "[‖]");
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    assert!(border(&mut panel).contains("[▶][■]"));
    let line = panel.transcript.line_count() - 1;
    assert!(panel.transcript.is_live_pause_line(line));
    // Stop while paused gives the run up: no controls, the pause rests.
    click(&mut panel, "[■]");
    assert!(!panel.paused);
    assert!(panel.pause_start.is_none());
    assert!(!border(&mut panel).contains('['));
    assert!(!panel.transcript.is_live_pause_line(line));
}

#[test]
fn the_stop_control_is_red_only_while_a_stop_is_under_way() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    let colors = panel.colors;
    assert_eq!(
        panel.run_button_color(RunButton::Pause, true),
        colors.border_focused
    );
    assert_eq!(
        panel.run_button_color(RunButton::Stop, true),
        colors.border_focused
    );
    panel.request_pause();
    panel.abort();
    // A stop under way leaves stop alone, red, and a second press adds
    // no second notice.
    assert_eq!(panel.run_buttons(), vec![RunButton::Stop]);
    assert_eq!(panel.run_button_color(RunButton::Stop, true), colors.error);
    // In the background every control dims with the frame, red stop too.
    assert_eq!(
        panel.run_button_color(RunButton::Stop, false),
        colors.border
    );
    let lines = panel.transcript.line_count();
    panel.abort();
    assert_eq!(panel.transcript.line_count(), lines);
    panel.apply(AgentEvent::AgentEnd);
    panel.apply(AgentEvent::AgentStart);
    assert_eq!(panel.run_buttons(), vec![RunButton::Pause, RunButton::Stop]);
    assert_eq!(
        panel.run_button_color(RunButton::Stop, true),
        colors.border_focused
    );
    assert_eq!(
        panel.run_button_color(RunButton::Pause, false),
        colors.border
    );
}

fn steps(list: &[(&str, ChecklistStatus)]) -> Vec<ChecklistItem> {
    list.iter()
        .map(|(text, status)| ChecklistItem::new(*text, *status))
        .collect()
}

#[test]
fn the_agents_checklist_shows_in_the_state_strip() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::Checklist {
        items: steps(&[
            ("Read the code", ChecklistStatus::Done),
            ("Fix the bug", ChecklistStatus::InProgress),
            ("Run the tests", ChecklistStatus::Pending),
        ]),
    });
    // Queued behind it, a message: the list heads the strip, under its
    // title on the rule.
    panel.apply(AgentEvent::AgentStart);
    panel.send("and the docs".to_string());
    let lines = strip_text(&panel.state_lines(40));
    let title = termide_i18n::t().agent_strip_checklist();
    assert!(
        lines[0].starts_with(&format!("╌╌ {title} 1/3 ╌")),
        "{lines:?}"
    );
    assert_eq!(
        termide_ui::str_display_width(&lines[0]),
        termide_ui::str_display_width(
            &strip_text(&[crate::transcript::separator(40, &panel.colors)])[0]
        )
    );
    assert!(lines[1].starts_with("✓ Read the code"), "{lines:?}");
    assert!(lines[2].starts_with("▸ Fix the bug"), "{lines:?}");
    assert!(lines[3].starts_with("○ Run the tests"), "{lines:?}");
    assert!(lines[4].starts_with("› Queued  and the docs"), "{lines:?}");
    // An empty list clears it, and the rule its title.
    panel.apply(AgentEvent::Checklist { items: Vec::new() });
    let lines = strip_text(&panel.state_lines(40));
    assert!(lines[0].chars().all(|c| c == '╌'), "{lines:?}");
    assert!(lines[1].starts_with("› Queued"), "{lines:?}");
}

#[test]
fn a_long_checklist_keeps_the_work_at_hand_in_sight() {
    let mut list: Vec<(String, ChecklistStatus)> = (1..=10)
        .map(|n| (format!("step {n}"), ChecklistStatus::Pending))
        .collect();
    for step in &mut list[..5] {
        step.1 = ChecklistStatus::Done;
    }
    list[5].1 = ChecklistStatus::InProgress;
    let items: Vec<ChecklistItem> = list
        .iter()
        .map(|(text, status)| ChecklistItem::new(text.clone(), *status))
        .collect();
    let rows = crate::strip::checklist_rows(&items);
    let bodies: Vec<&str> = rows.iter().map(|row| row.body.as_str()).collect();
    // The last step done, the one in progress and what follows; the rest
    // folded into counts.
    assert_eq!(bodies, ["+4", "step 5", "step 6", "step 7", "step 8", "+2"]);
    assert_eq!(rows.len(), crate::strip::STRIP_CHECKLIST_ROWS);
    // At the start nothing is folded before it.
    let fresh: Vec<ChecklistItem> = (1..=10)
        .map(|n| ChecklistItem::new(format!("step {n}"), ChecklistStatus::Pending))
        .collect();
    let bodies: Vec<String> = crate::strip::checklist_rows(&fresh)
        .into_iter()
        .map(|row| row.body)
        .collect();
    assert_eq!(bodies, ["step 1", "step 2", "step 3", "step 4", "+6"]);
}

#[test]
fn a_finished_checklist_stays_until_the_next_request_and_reopens_as_it_stood() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::create(dir.path(), Path::new("/tmp")).unwrap()),
        ..setup(vec![reply("next")])
    });
    let list = steps(&[
        ("one", ChecklistStatus::Done),
        ("two", ChecklistStatus::InProgress),
    ]);
    panel.apply(AgentEvent::Checklist {
        items: list.clone(),
    });
    let path = {
        let session = panel.session.as_mut().unwrap();
        session
            .append_message(&Message::User(UserMessage::text("do it")))
            .unwrap();
        session.path().to_path_buf()
    };
    drop(panel);

    // A reopened session shows the list as the agent last reported it.
    let mut panel = reopen(&path, dir.path(), vec![reply("next")]);
    assert_eq!(panel.checklist, list);
    let done = steps(&[
        ("one", ChecklistStatus::Done),
        ("two", ChecklistStatus::Done),
    ]);
    panel.apply(AgentEvent::Checklist {
        items: done.clone(),
    });
    // Carried through, it stays in sight with every step checked...
    let lines = strip_text(&panel.state_lines(40));
    assert!(lines[2].starts_with("✓ two"), "{lines:?}");
    // ...until the next request, which clears it, in the log too.
    panel.send("something else".to_string());
    assert!(panel.checklist.is_empty());
    assert!(Session::open(&path).unwrap().checklist().is_empty());
}

#[test]
fn an_unfinished_checklist_stays_through_the_next_request() {
    let mut panel = AgentPanel::new(setup(vec![reply("ok")]));
    let list = steps(&[
        ("one", ChecklistStatus::Done),
        ("two", ChecklistStatus::Pending),
    ]);
    panel.apply(AgentEvent::Checklist {
        items: list.clone(),
    });
    panel.send("go on".to_string());
    assert_eq!(panel.checklist, list);
}

/// The built-in agent's `checklist` call sets the strip's list from what its
/// result carries, and the reminder of each model call reads the same list;
/// a call that failed leaves both as they were.
#[test]
fn the_checklist_tool_sets_the_list_the_strip_shows_and_the_agent_is_reminded_of() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    panel.send("plan it".to_string());
    let list = steps(&[
        ("Read", ChecklistStatus::Done),
        ("Fix", ChecklistStatus::InProgress),
    ]);
    let call = ToolCall {
        id: "c1".into(),
        name: "checklist".into(),
        arguments: serde_json::json!({ "items": list }),
        extra_content: None,
    };
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&call, "Checklist")
            .with_details(serde_json::json!({ "checklist": list })),
    });
    assert_eq!(panel.checklist, list);
    let id = panel.session.as_ref().expect("a session").id().to_string();
    assert_eq!(termide_agent_core::checklist::current(&id), list);

    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::error(&call, "bad status")
            .with_details(serde_json::json!({ "checklist": [] })),
    });
    assert_eq!(panel.checklist, list);
}
