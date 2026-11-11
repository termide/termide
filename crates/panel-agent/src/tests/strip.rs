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
