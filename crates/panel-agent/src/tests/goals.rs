//! `/loop` and `/goal`.

use super::*;

#[test]
fn loop_args_split_interval_from_prompt() {
    assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
    assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
    assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
    assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
    assert_eq!(parse_duration("x"), None);
    assert_eq!(parse_duration("0"), None);
    assert_eq!(
        parse_loop_args("5m run the tests"),
        (Some(Duration::from_secs(300)), "run the tests")
    );
    assert_eq!(parse_loop_args("keep improving"), (None, "keep improving"));
}

#[test]
fn slash_loop_starts_and_stops() {
    let mut panel = panel(vec![reply("a")]);
    type_text(&mut panel, "/loop keep going");
    panel.submit();
    assert!(panel.loop_task.is_some(), "the loop started");

    type_text(&mut panel, "/loop stop");
    panel.submit();
    assert!(panel.loop_task.is_none(), "/loop stop ended it");

    // Esc also ends a waiting loop.
    type_text(&mut panel, "/loop 5m again");
    panel.submit();
    assert!(panel.loop_task.is_some());
    settle(&mut panel);
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.loop_task.is_none(), "Esc ended the loop");
}

#[test]
fn slash_completion_is_alphabetical() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "/c");
    let list = panel.completion.as_ref().expect("a completion popup");
    let names: Vec<&str> = list.items().iter().map(|i| i.value.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "the /-command list should be alphabetical");
    assert!(names.contains(&"clear") && names.contains(&"compact"));
}

#[test]
fn slash_pause_and_continue_report_when_idle() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/pause");
    panel.submit();
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("nothing is running"))));
    type_text(&mut panel, "/continue");
    panel.submit();
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("nothing to continue"))));
}

#[test]
fn goal_works_turn_by_turn_until_the_judge_says_done() {
    // Provider calls, in order: work turn, judge (continue), work turn,
    // judge (done).
    let mut panel = panel(vec![
        reply("starting the work"),
        reply("CONTINUE\ntests are still red"),
        reply("more work done"),
        reply("DONE\neverything is green"),
    ]);
    type_text(&mut panel, "/goal get the build green");
    panel.submit();
    assert!(panel.goal_task.is_some());
    settle_goal(&mut panel);

    // Two work turns were sent (the goal, then a continuation).
    let users = panel
        .transcript()
        .items()
        .iter()
        .filter(|i| matches!(i, Item::User { .. }))
        .count();
    assert_eq!(users, 2);
    // The first turn heads its block with the command as typed, and the
    // notice does not repeat the goal.
    let items = panel.transcript().items();
    assert!(
        items
            .iter()
            .any(|i| matches!(i, Item::User { command: Some(c), text, .. }
            if c == "/goal get the build green" && text == "get the build green")),
        "{items:?}"
    );
    assert!(items.iter().any(|i| matches!(i, Item::Notice { text, .. }
        if text == termide_i18n::t().agent_notice_goal_working())));
    assert!(!items
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("get the build green"))));
    // The judge ran and the goal ended with the success reason.
    assert!(panel.transcript().items().iter().any(
        |i| matches!(i, Item::Notice { text, .. } if text.contains("checking whether the goal"))
    ));
    assert!(panel.transcript().items().iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("goal reached: everything is green"))
        ));
}

/// A judge that echoes its instructions gives no verdict: the goal is not
/// dropped, the check is tried again, and the answer is in the log.
#[test]
fn an_echoed_instruction_from_the_judge_keeps_the_goal() {
    let dir = tempfile::tempdir().unwrap();
    let echo = "`DONE` if the goal is fully achieved";
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("working"), reply(echo)])
    });
    type_text(&mut panel, "/goal get the build green");
    panel.submit();
    settle(&mut panel);
    panel.goal_task.as_mut().unwrap().judge_at = Some(Instant::now());
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.goal_task.as_ref().unwrap().judge_failures == 0 {
        panel.tick();
        assert!(Instant::now() < deadline, "the check never came back");
        std::thread::sleep(Duration::from_millis(5));
    }
    let task = panel.goal_task.as_ref().expect("the goal stays");
    assert!(task.judge_at.is_some(), "the check is tried again");
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("goal reached"))));
    let logged = Session::open_read_only(panel.session.as_ref().unwrap().path()).unwrap();
    let verdicts: Vec<&EntryKind> = logged
        .branch()
        .into_iter()
        .map(|e| &e.kind)
        .filter(|k| matches!(k, EntryKind::GoalVerdict { .. }))
        .collect();
    assert!(
        matches!(
            verdicts.as_slice(),
            [EntryKind::GoalVerdict { outcome: None, error, reply, .. }]
                if error.contains("no verdict") && reply == echo
        ),
        "{verdicts:?}"
    );
}

/// A session whose goal the judge ended shows how it ended when reopened.
#[test]
fn a_reached_goal_is_told_again_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("working"), reply("DONE\neverything is green")])
    });
    type_text(&mut panel, "/goal get the build green");
    panel.submit();
    settle_goal(&mut panel);
    panel.tick();
    let path = panel.session.as_ref().unwrap().path().to_path_buf();
    drop(panel);
    let panel = reopen(&path, dir.path(), vec![]);
    let reached = termide_i18n::t().agent_notice_goal_reached_reason_fmt("everything is green");
    assert!(matches!(
        panel.transcript().items().last(),
        Some(Item::Notice { text, .. }) if *text == reached
    ));
    assert!(panel.goal_task.is_none());
}

/// A goal's work turn that the judge takes up next rings no bell, however
/// long it ran; the goal's end does.
#[test]
fn a_goal_rings_only_when_it_ends() {
    let mut panel = panel(vec![]);
    panel.busy = true;
    type_text(&mut panel, "/goal finish the refactor");
    panel.submit();
    panel.busy = false;
    end_run_of(&mut panel, 60);
    assert!(panel.goal_task.as_ref().unwrap().judge_at.is_some());
    assert!(!panel.rung);
    assert!(!panel.needs_attention());
    panel.apply(AgentEvent::GoalJudged {
        outcome: termide_agent_core::GoalOutcome::Done,
        reason: "done".into(),
        reply: "DONE\ndone".into(),
    });
    assert!(panel.goal_task.is_none());
    assert!(panel.rung);
}

/// A loop's run with the next one scheduled rings no bell either.
#[test]
fn a_loop_run_with_another_to_come_rings_no_bell() {
    let mut panel = panel(vec![]);
    panel.busy = true;
    type_text(&mut panel, "/loop 5m check the build");
    panel.submit();
    panel.busy = false;
    assert!(panel.loop_task.is_some());
    end_run_of(&mut panel, 60);
    assert!(panel.loop_task.as_ref().unwrap().next_at.is_some());
    assert!(!panel.rung);
}

/// While the judge decides, its call shows the ticking live line; the
/// verdict takes it away.
#[test]
fn the_goal_check_shows_a_live_line_until_the_verdict() {
    let mut panel = panel(vec![reply("DONE\nall green")]);
    panel.busy = true;
    type_text(&mut panel, "/goal get the build green");
    panel.submit();
    panel.busy = false;
    panel.goal_task.as_mut().unwrap().first_turn = None;
    panel.run_goal_judge();
    assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Judge));
    assert!(!panel.live_footer_lines(60).is_empty());
    settle_goal(&mut panel);
    assert!(panel.activity.is_none());
}

/// A judge that finds the goal out of reach ends it, with the reason.
#[test]
fn an_impossible_goal_ends_with_the_reason() {
    let mut panel = panel(vec![
        reply("looked into it"),
        reply("IMPOSSIBLE\nthe API it needs was removed"),
    ]);
    type_text(&mut panel, "/goal use the old API");
    panel.submit();
    settle_goal(&mut panel);
    let wanted =
        termide_i18n::t().agent_notice_goal_impossible_reason_fmt("the API it needs was removed");
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if *text == wanted)));
}

/// A judge that finds the agent waiting on the user pauses the goal at the
/// first such verdict, still set, instead of sending it back to restate the
/// wait: a turn that polls with a tool would otherwise defeat the idle count.
#[test]
fn a_goal_waiting_on_the_user_pauses_at_once() {
    let mut panel = panel(vec![
        reply("waiting until you close the other session"),
        reply("WAITING\nthe agent waits for the user to close a session"),
    ]);
    type_text(&mut panel, "/goal land the slice");
    panel.submit();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !panel.paused {
        panel.tick();
        assert!(Instant::now() < deadline, "the goal did not pause");
        std::thread::sleep(Duration::from_millis(5));
    }
    let task = panel.goal_task.as_ref().expect("the goal is still set");
    assert_eq!(task.iterations, 1, "no turn was sent back");
    assert_eq!(panel.restored, Some(Restored::Autorun));
    let wanted = termide_i18n::t()
        .agent_notice_goal_waiting_fmt("the agent waits for the user to close a session");
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if *text == wanted)));
}

/// With no turn cap, a goal whose turns call no tool is what stops it: after
/// [`GOAL_IDLE_TURNS`] such turns it waits at a pause, still set.
#[test]
fn a_goal_that_only_talks_pauses_after_the_idle_turns() {
    let mut replies = Vec::new();
    for _ in 0..GOAL_IDLE_TURNS {
        replies.push(reply("thinking about it"));
        replies.push(reply("CONTINUE\nnothing has changed"));
    }
    let mut panel = panel(replies);
    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !panel.paused {
        panel.tick();
        assert!(Instant::now() < deadline, "the goal did not pause");
        std::thread::sleep(Duration::from_millis(5));
    }
    let task = panel.goal_task.as_ref().expect("the goal is still set");
    assert_eq!(task.iterations, GOAL_IDLE_TURNS);
    assert_eq!(task.idle_turns, 0, "the count starts again after the pause");
    assert_eq!(panel.restored, Some(Restored::Autorun));
    let wanted = termide_i18n::t().agent_notice_goal_idle_paused_fmt(GOAL_IDLE_TURNS);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if *text == wanted)));
}

#[test]
fn goal_can_be_set_while_the_agent_works() {
    let mut panel = panel(vec![reply("refactored")]);
    // A run is in flight: the goal is taken at once, nothing joins the run.
    panel.busy = true;
    type_text(&mut panel, "/goal finish the refactor");
    panel.submit();
    assert!(panel.goal_task.is_some());
    assert!(panel.queued_texts.is_empty());
    assert!(panel.runtime.take_queued().is_empty());
    let notices = |panel: &AgentPanel, wanted: &str| {
        panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text == wanted))
    };
    let t = termide_i18n::t();
    assert!(notices(&panel, t.agent_notice_goal_working()));
    assert!(!notices(&panel, t.agent_notice_busy()));
    // Once the run ends, its first turn goes in place of the judge, headed
    // by the `/goal …` typed.
    panel.busy = false;
    panel.goal_task.as_mut().unwrap().judge_at = Some(Instant::now());
    panel.tick();
    let task = panel.goal_task.as_ref().unwrap();
    assert_eq!(task.iterations, 1);
    assert!(task.first_turn.is_none());
    assert!(!task.judging);
    settle(&mut panel);
    assert!(panel.transcript().items().iter().any(|i| matches!(
        i,
        Item::User { command: Some(c), .. } if c == "/goal finish the refactor"
    )));
}

#[test]
fn goal_stop_ends_an_active_goal() {
    let mut panel = panel(vec![reply("working")]);
    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    settle(&mut panel);
    assert!(panel.goal_task.is_some());
    // Stop it before the judge would send another turn.
    type_text(&mut panel, "/goal stop");
    panel.submit();
    assert!(panel.goal_task.is_none());
    assert!(panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text.contains(termide_i18n::t().agent_notice_goal_stopped()))));
}

/// A bare `/goal` or `/loop` tells what is going on and leaves it going;
/// without one it shows the usage. Only `stop` (or `off`) ends either.
#[test]
fn bare_goal_and_loop_report_rather_than_stop() {
    let t = termide_i18n::t();
    let last_notice = |panel: &AgentPanel| {
        panel
            .transcript()
            .items()
            .iter()
            .rev()
            .find_map(|i| match i {
                Item::Notice { text, .. } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };
    let mut panel = panel(vec![reply("working"), reply("again")]);
    type_text(&mut panel, "/goal");
    panel.submit();
    assert_eq!(last_notice(&panel), t.agent_notice_goal_usage());
    type_text(&mut panel, "/loop");
    panel.submit();
    assert_eq!(last_notice(&panel), t.agent_notice_loop_usage());

    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    settle(&mut panel);
    type_text(&mut panel, "/goal");
    panel.submit();
    assert!(panel.goal_task.is_some(), "a bare /goal left it going");
    let status = last_notice(&panel);
    assert!(status.contains("do the thing"), "{status}");
    let turns = panel.goal_task.as_ref().unwrap().iterations;
    assert_eq!(status, t.agent_unfinished_goal_fmt("do the thing", turns));

    let mut panel = looping_panel();
    type_text(&mut panel, "/loop");
    panel.submit();
    assert!(panel.loop_task.is_some(), "a bare /loop left it going");
    let status = last_notice(&panel);
    assert!(status.contains("check the build"), "{status}");
    assert!(status.contains("5m"), "{status}");
}

/// An active goal and loop stay pinned in the state strip: the command as
/// typed on the left, the count (and the wait for the next run) at the right.
#[test]
fn an_active_goal_and_loop_show_in_the_state_strip() {
    let mut panel = looping_panel();
    type_text(&mut panel, "/goal do the thing");
    panel.submit();

    let rows = panel.strip_rows(80);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let colors = ThemeColors::default();
    let lines = crate::strip::strip_lines(&rows, 80, &colors);
    let text = |line: &Line| {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    };
    let goal = text(&lines[0]);
    assert!(
        goal.starts_with("◎ Heading for the goal  do the thing"),
        "{goal}"
    );
    assert_eq!(
        lines[0].spans[0].style.fg,
        Some(colors.info),
        "label in accent"
    );
    assert_eq!(
        lines[0].spans[2].style.fg,
        Some(colors.disabled),
        "goal dim"
    );
    let stop = lines[0].spans.last().unwrap();
    assert_eq!(
        (stop.content.as_ref(), stop.style.fg),
        (" ✕", Some(colors.info))
    );
    // No turn count and no cap: only how long the goal has run, then `✕`.
    assert!(!goal.contains('/'), "{goal}");
    let unit = termide_i18n::t().time_unit_secs();
    assert!(goal.trim_end().ends_with(&format!("{unit} ✕")), "{goal}");
    // The loop's period and count to the left, its time at the right.
    let looped = text(&lines[1]);
    assert!(
        looped.starts_with(&format!(
            "↺ Repeating every 5m · 1/{LOOP_MAX_ITERATIONS} · next in"
        )),
        "{looped}"
    );
    assert!(looped.contains("  check the build"), "{looped}");
    assert!(
        looped.trim_end().ends_with(&format!("{unit} ✕")),
        "{looped}"
    );
    for line in &lines {
        let text = text(line);
        assert!(termide_ui::str_display_width(&text) <= 80);
        // The button sits where a click looks for it.
        let column = crate::strip::strip_stop_column(80) as usize;
        assert_eq!(text.chars().nth(column), Some('✕'), "{text:?}");
    }

    type_text(&mut panel, "/goal stop");
    panel.submit();
    type_text(&mut panel, "/loop stop");
    panel.submit();
    assert!(panel.strip_rows(80).is_empty());
}

/// A click on the `✕` of a goal or loop row stops just that one.
#[test]
fn the_strip_stop_button_ends_a_goal_or_loop() {
    let mut panel = looping_panel();
    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    let (width, height) = (80, 30);
    let click_stop = |panel: &mut AgentPanel, kind: crate::render::Autorun| {
        let _ = render_text(panel, width, height);
        let row = strip_row(panel, crate::strip::StripTarget::Autorun(kind));
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: panel.strip_stop_x,
                row,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, width, height),
        );
    };
    // A click on the row away from `✕` brings the loop's run into view.
    let _ = render_text(&mut panel, width, height);
    let row = strip_row(
        &panel,
        crate::strip::StripTarget::Autorun(crate::render::Autorun::Loop),
    );
    let block = panel
        .autorun_block(crate::render::Autorun::Loop)
        .expect("the loop's run is in the transcript");
    panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    assert!(panel.loop_task.is_some(), "the row click does not stop it");
    assert!(panel.chat_focus);
    assert_eq!(panel.selected, block);
    click_stop(&mut panel, crate::render::Autorun::Loop);
    assert!(panel.loop_task.is_none());
    assert!(panel.goal_task.is_some(), "only the loop stopped");
    click_stop(&mut panel, crate::render::Autorun::Goal);
    assert!(panel.goal_task.is_none());
    let _ = render_text(&mut panel, width, height);
    assert!(panel.strip_hits.is_empty());
}
