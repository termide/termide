//! Shell mode, hand-run commands and suggested commands.

use super::*;

#[test]
fn text_opening_with_a_dollar_is_a_message() {
    // Only the mode makes a command: a pasted `$PATH …` goes to the model.
    let (mut panel, seen, provider) = panel_with_runner(vec![reply("ok")]);
    panel.paste("$PATH misses cargo");
    panel.submit();
    settle(&mut panel);
    assert!(seen.lock().unwrap().is_empty(), "nothing ran in the shell");
    assert!(!provider.seen_models.lock().unwrap().is_empty());
}

#[test]
fn bang_runs_in_the_shell_and_never_reaches_the_model() {
    let (mut panel, seen, provider) = panel_with_runner(vec![reply("should not be asked")]);
    type_text(&mut panel, "$git status");
    let events = panel.submit();
    assert!(panel.input_text().is_empty(), "the input is cleared");
    assert!(
        events.iter().any(|e| matches!(
            e,
            PanelEvent::SetStatusMessage {
                is_error: false,
                ..
            }
        )),
        "{events:?}"
    );
    // The command reached the shell as written, without the `!`.
    wait_for_commands(&mut panel, &seen, 1);
    assert_eq!(seen.lock().unwrap().as_slice(), ["git status"]);

    // Its output shows under the command as a user block, and no model turn
    // was spent on it: the scripted reply is still queued.
    wait_for_bang_output(&mut panel, "$git status", "out of git status");
    wait_for_bang_idle(&mut panel);
    assert!(
        provider.seen_models.lock().unwrap().is_empty(),
        "no model call was made"
    );
}

#[test]
fn shell_mode_walks_the_commands_and_messages_their_own() {
    let (mut panel, seen, _) = panel_with_runner(vec![reply("ok")]);
    type_text(&mut panel, "hello");
    panel.submit();
    settle(&mut panel);
    for (i, command) in ["$ls", "$git status"].into_iter().enumerate() {
        type_text(&mut panel, command);
        panel.submit();
        wait_for_commands(&mut panel, &seen, i + 1);
        wait_for_bang_idle(&mut panel);
    }
    let up = |panel: &mut AgentPanel| {
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    };
    // Outside the mode the arrows give back messages only.
    up(&mut panel);
    assert!(!panel.shell_mode);
    assert_eq!(panel.input_text(), "hello");
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));

    // In it, the commands, newest first, and the mode holds.
    type_text(&mut panel, "$");
    up(&mut panel);
    assert!(panel.shell_mode);
    assert_eq!(panel.input_text(), "git status");
    up(&mut panel);
    assert_eq!(panel.input_text(), "ls");
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert!(panel.shell_mode);
    assert_eq!(panel.input_text(), "", "past the newest, the draft returns");
}

#[test]
fn bang_without_a_runner_is_refused_with_a_notice() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "$ls");
    panel.submit();
    let notices: Vec<String> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|i| match i {
            Item::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|n| n.contains("shell")),
        "no notice about the shell: {notices:?}"
    );
}

#[test]
fn escape_stops_a_hand_run_command() {
    let (runner, seen) = blocking_runner();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        shell_run: Some(runner),
        ..setup(vec![])
    });
    type_text(&mut panel, "$sleep 100");
    panel.submit();
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().is_empty() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(panel.captures_escape(), "Esc belongs to the command");
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.tick();
        if panel.shell_job.is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "the command never stopped");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_suggested_command_shows_on_a_card_with_the_exact_command() {
    let mut panel = panel(vec![]);
    let worker = suggest_in_worker(&mut panel, "gh issue comment 59", "post the reply");
    let form = panel.pending.as_ref().unwrap().form();
    assert_eq!(form.title(), "Run this command?");
    let detail = form.detail().unwrap();
    // The command shows in full and unmodified, with why and where.
    assert!(detail.contains("gh issue comment 59"), "{detail}");
    assert!(detail.contains("post the reply"), "{detail}");
    assert_eq!(form.options(), ["Run", "Edit first", "Copy"]);
    // It is marked as the agent's, so it cannot read as one the user wrote.
    assert!(detail.contains("offered by the agent"), "{detail}");
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), SuggestionReply::Declined);
}

#[test]
fn a_suggested_command_denied_by_a_rule_withholds_run() {
    termide_ui::clipboard::use_fake_clipboard("");
    let mut panel = panel(vec![]);
    panel.rules.add("shell", "git push*", Decision::Deny);
    let worker = suggest_in_worker(&mut panel, "git push origin main", "publish");
    let form = panel.pending.as_ref().unwrap().form();
    // No `[Run]`, no `[Edit first]`: only the text may be taken away.
    assert_eq!(form.options(), ["Copy"]);
    assert!(
        form.detail().unwrap().contains("withheld"),
        "{:?}",
        form.detail()
    );
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), SuggestionReply::Copied);
}

#[test]
fn the_dismiss_row_declines_a_suggestion() {
    let mut panel = panel(vec![]);
    let worker = suggest_in_worker(&mut panel, "git push", "publish");
    // Row 4, after `[Run]`, `[Edit first]` and `[Copy]`, is `[Don't run]`.
    panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), SuggestionReply::Declined);
    assert!(panel.input_text().is_empty(), "nothing goes to the input");
}

#[test]
fn a_denied_suggestion_still_offers_dismiss() {
    let mut panel = panel(vec![]);
    panel.mode.set(Mode::Plan);
    let worker = suggest_in_worker(&mut panel, "touch new.rs", "create it");
    // Row 2, after `[Copy]`, is `[Don't run]`.
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), SuggestionReply::Declined);
}

#[test]
fn edit_first_puts_the_command_in_the_input_to_run_by_hand() {
    let mut panel = panel(vec![]);
    let worker = suggest_in_worker(&mut panel, "git push", "publish");
    // Row 2 is `[Edit first]`.
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), SuggestionReply::Edit);
    assert!(panel.shell_mode, "it waits as a command, not a message");
    assert_eq!(panel.input_text(), "git push");
}

#[test]
fn a_second_suggestion_while_a_card_is_up_declines() {
    let mut panel = panel(vec![]);
    let (suggester, rx) = suggestion_channel(CancelToken::new());
    panel.suggestion_rx = rx;
    let first = {
        let suggester = suggester.clone();
        std::thread::spawn(move || {
            suggester.suggest(Suggestion {
                kind: SuggestionKind::Command,
                command: "ls".into(),
                why: "look".into(),
            })
        })
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline, "the card never appeared");
        std::thread::sleep(Duration::from_millis(5));
    }

    // A second offer cannot be shown while the first card stands; polling it
    // declines it at once, so that call learns nothing ran.
    let (tx, settled) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let reply = suggester.suggest(Suggestion {
            kind: SuggestionKind::Command,
            command: "pwd".into(),
            why: String::new(),
        });
        let _ = tx.send(reply);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let reply = loop {
        panel.tick();
        if let Ok(reply) = settled.try_recv() {
            break reply;
        }
        assert!(Instant::now() < deadline, "the second offer never settled");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(reply, SuggestionReply::Declined);
    // The first card is untouched by it.
    assert_eq!(
        panel.pending.as_ref().unwrap().form().title(),
        "Run this command?"
    );
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(first.join().unwrap(), SuggestionReply::Declined);
}

/// A goal the agent offers shows on a card of its own; `[Start]` starts it
/// as the typed `/goal` line would, and only then answers the call.
#[test]
fn an_offered_goal_starts_on_start() {
    let mut panel = panel(vec![reply("working"), reply("DONE\nall green")]);
    let worker = offer_in_worker(
        &mut panel,
        SuggestionKind::Goal,
        "the build is green",
        "many steps",
    );
    let form = panel.pending.as_ref().unwrap().form();
    assert_eq!(form.title(), "Start this goal?");
    assert_eq!(form.options(), ["Start", "Edit first"]);
    let detail = form.detail().unwrap();
    assert!(detail.contains("the build is green"), "{detail}");
    assert!(detail.contains("many steps"), "{detail}");
    assert!(detail.contains("does not start unless"), "{detail}");
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), SuggestionReply::Run);
    assert_eq!(
        panel.goal_task.as_ref().map(|task| task.goal.as_str()),
        Some("the build is green")
    );
    settle_goal(&mut panel);
    assert!(panel.transcript().items().iter().any(|i| matches!(
        i,
        Item::User { command: Some(c), .. } if c == "/goal the build is green"
    )));
}

/// A goal under way is named on the card, and replaced only on `[Start]`.
#[test]
fn an_offered_goal_names_the_one_it_replaces() {
    let mut panel = panel(vec![]);
    let worker = offer_in_worker(&mut panel, SuggestionKind::Goal, "new aim", "");
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), SuggestionReply::Declined);
    assert!(panel.goal_task.is_none(), "dismissing starts nothing");

    panel.goal_task = Some(GoalTask {
        goal: "old aim".into(),
        iterations: 1,
        started: Instant::now(),
        used_tools: false,
        idle_turns: 0,
        judge_at: None,
        judging: false,
        judge_failures: 0,
        first_turn: None,
    });
    let worker = offer_in_worker(&mut panel, SuggestionKind::Goal, "new aim", "");
    let detail = panel.pending.as_ref().unwrap().form().detail().unwrap();
    assert!(
        detail.contains("replaces the current goal: old aim"),
        "{detail}"
    );
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), SuggestionReply::Declined);
    assert_eq!(
        panel.goal_task.as_ref().map(|task| task.goal.as_str()),
        Some("old aim"),
        "the goal under way stays"
    );
}

/// A loop's card says how often it runs; `[Edit first]` puts its `/loop`
/// line in the input as a message, not a shell command.
#[test]
fn an_offered_loop_can_be_edited_first() {
    let mut panel = panel(vec![]);
    let worker = offer_in_worker(
        &mut panel,
        SuggestionKind::Loop {
            interval: Some(Duration::from_secs(300)),
        },
        "check CI",
        "",
    );
    let form = panel.pending.as_ref().unwrap().form();
    assert_eq!(form.title(), "Start this loop?");
    let detail = form.detail().unwrap();
    assert!(detail.contains("runs every 5m"), "{detail}");
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), SuggestionReply::Edit);
    assert!(!panel.shell_mode);
    assert_eq!(panel.input_text(), "/loop 5m check CI");
    assert!(panel.loop_task.is_none(), "editing starts nothing");
}

/// `[Start]` on a loop starts it, as the typed `/loop` line would.
#[test]
fn an_offered_loop_starts_on_start() {
    let mut panel = panel(vec![reply("a")]);
    let worker = offer_in_worker(
        &mut panel,
        SuggestionKind::Loop {
            interval: Some(Duration::from_secs(300)),
        },
        "check CI",
        "",
    );
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), SuggestionReply::Run);
    let task = panel.loop_task.as_ref().expect("the loop started");
    assert_eq!(task.prompt, "check CI");
    assert_eq!(task.interval, Some(Duration::from_secs(300)));
    assert_eq!(task.iterations, 1, "its first run went at once");
}

#[test]
fn plan_mode_withholds_run_from_a_suggestion() {
    termide_ui::clipboard::use_fake_clipboard("");
    let mut panel = panel(vec![]);
    panel.mode.set(Mode::Plan);
    let worker = suggest_in_worker(&mut panel, "touch new.rs", "create it");
    assert_eq!(panel.pending.as_ref().unwrap().form().options(), ["Copy"]);
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), SuggestionReply::Copied);
}

#[test]
fn a_second_bang_while_one_runs_is_refused() {
    let (runner, seen) = blocking_runner();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        shell_run: Some(runner),
        ..setup(vec![])
    });
    type_text(&mut panel, "$sleep 100");
    panel.submit();
    wait_for_commands(&mut panel, &seen, 1);
    // Still running: a second command is refused rather than queued.
    type_text(&mut panel, "$ls");
    panel.submit();
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the second command did not run"
    );
    let notices: Vec<String> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|i| match i {
            Item::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(
        notices.iter().any(|n| n.contains("still")),
        "no notice about the running command: {notices:?}"
    );
    panel.cancel_user_command();
    wait_for_bang_idle(&mut panel);
}

#[test]
fn a_failing_command_shows_its_output_and_is_not_a_model_turn() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let recorder = Arc::clone(&seen);
    let runner = ShellRunner::new(move |command, _| {
        recorder.lock().unwrap().push(command.to_string());
        Ok(ShellOutput {
            text: "boom\n[exit code 1]".to_string(),
            failed: true,
        })
    });
    let provider = Arc::new(Scripted::new(vec![]));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        shell_run: Some(runner),
        ..setup_with(provider.clone())
    });
    type_text(&mut panel, "$false");
    panel.submit();
    wait_for_bang_output(&mut panel, "$false", "[exit code 1]");
    wait_for_bang_idle(&mut panel);
    assert_eq!(seen.lock().unwrap().as_slice(), ["false"]);
    assert!(provider.seen_models.lock().unwrap().is_empty());
    // The status says it failed, in red.
    assert!(panel
        .pending_events
        .iter()
        .any(|e| matches!(e, PanelEvent::SetStatusMessage { is_error: true, .. })));
}

/// A steered hand-run message — one arriving through `MessageEnd` rather
/// than the panel's own path — shows as the same shell call block, failure
/// and all, not as a bare block of its output.
#[test]
fn a_steered_hand_run_message_shows_as_a_shell_call() {
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::MessageEnd(Message::User(
        UserMessage::text("fatal: bad\n")
            .with_ran("git branch")
            .with_ran_failed(true),
    )));
    let items = panel.transcript().items();
    assert_eq!(items.len(), 1, "{items:?}");
    let (command, output, failed, _) = user_command(&items[0]).expect("a shell call block");
    assert_eq!(command, "git branch");
    assert!(output.contains("fatal: bad"));
    assert!(failed);
}

/// The block reads like the agent's own shell call, `$` and all, and says
/// the user ran it; the input history gives back the command as typed.
#[test]
fn a_hand_run_command_shows_as_a_shell_call_block() {
    let (mut panel, seen, _) = panel_with_runner(vec![]);
    panel.transcript.set_fold(FoldMode::Never);
    type_text(&mut panel, "$git status");
    panel.submit();
    wait_for_commands(&mut panel, &seen, 1);
    wait_for_bang_idle(&mut panel);
    let blocks: Vec<_> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(user_command)
        .collect();
    assert_eq!(blocks.len(), 1, "one block, not two");
    let (_, output, failed, duration) = &blocks[0];
    assert_eq!(output, "out of git status");
    assert!(!failed);
    assert!(duration.is_some(), "the block carries its 🕒");
    assert!(
        !panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::User { .. })),
        "no user block besides"
    );
    let rows = render_text(&mut panel, 80, 20);
    assert!(
        rows.iter()
            .any(|r| r.starts_with("$ ") && r.contains("git status")),
        "{rows:?}"
    );
    let note = termide_i18n::t().agent_perm_note_user_ran();
    assert!(rows.iter().any(|r| r.contains(note)), "{rows:?}");
    assert!(panel.history().is_empty(), "a command is not a message");
    panel.shell_mode = true;
    assert_eq!(panel.history(), ["git status"]);
}

/// A session reopened after a hand-run command reads with it: the log carries
/// the message, and the restored transcript heads it with the `!cmd`.
#[test]
fn a_hand_run_command_survives_reopening_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let (runner, seen) = recording_runner();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        shell_run: Some(runner.clone()),
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "$git status");
    panel.submit();
    wait_for_commands(&mut panel, &seen, 1);
    wait_for_bang_idle(&mut panel);
    let path = panel.session_path().unwrap().to_path_buf();

    // The log holds the message with its `ran`, not only its output.
    let reopened = Session::open(&path).unwrap();
    let logged: Vec<&Message> = reopened
        .entries()
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Message { message, .. } => Some(message),
            _ => None,
        })
        .collect();
    assert!(
        logged.iter().any(|m| matches!(
            m,
            Message::User(u) if u.ran.as_deref() == Some("git status")
                && u.plain_text().contains("out of git status")
        )),
        "the hand-run command is not in the log"
    );

    // Reopening the panel restores the shell call block, as it showed.
    let again = AgentPanel::new(AgentPanelSetup {
        shell_run: Some(runner.clone()),
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(reopened),
        ..setup(vec![])
    });
    let blocks: Vec<_> = again
        .transcript()
        .items()
        .iter()
        .filter_map(user_command)
        .collect();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0].0, "git status");
    assert!(blocks[0].3.is_some(), "the logged timing restores its 🕒");
}

#[test]
fn dollar_into_an_empty_input_switches_to_shell_mode() {
    let (mut panel, _, _) = panel_with_runner(vec![]);
    let marker = |panel: &mut AgentPanel| {
        render_text(panel, 60, 12)
            .last()
            .cloned()
            .unwrap_or_default()
    };
    assert!(
        marker(&mut panel).starts_with("› "),
        "{:?}",
        marker(&mut panel)
    );

    // The `$` becomes the prompt marker instead of text.
    type_text(&mut panel, "$");
    assert!(panel.shell_mode);
    assert_eq!(panel.input_text(), "");
    assert!(
        marker(&mut panel).starts_with("$ "),
        "{:?}",
        marker(&mut panel)
    );

    // A `$` past the first character is the command's own.
    type_text(&mut panel, "a$");
    assert_eq!(panel.input_text(), "a$");

    // Backspace in the emptied input leaves the mode, and a `$` typed into
    // a message that has text is just text.
    panel.handle_key(chord(KeyCode::Backspace, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(panel.shell_mode, "the mode holds while text remains");
    panel.handle_key(chord(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(!panel.shell_mode);
    assert!(
        marker(&mut panel).starts_with("› "),
        "{:?}",
        marker(&mut panel)
    );
    type_text(&mut panel, "hi$");
    assert!(!panel.shell_mode);
    assert_eq!(panel.input_text(), "hi$");
}

#[test]
fn esc_leaves_shell_mode_with_the_command_typed() {
    let (mut panel, seen, _) = panel_with_runner(vec![]);
    type_text(&mut panel, "$git st");
    assert!(panel.captures_escape(), "Esc belongs to the mode");
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!panel.shell_mode);
    assert_eq!(panel.input_text(), "");
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn running_a_command_leaves_shell_mode() {
    let (mut panel, seen, _) = panel_with_runner(vec![]);
    type_text(&mut panel, "$git status");
    panel.submit();
    assert!(!panel.shell_mode, "each command switches the mode on anew");
    wait_for_commands(&mut panel, &seen, 1);
    wait_for_bang_idle(&mut panel);
}
