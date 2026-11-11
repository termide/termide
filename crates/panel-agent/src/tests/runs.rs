//! Runs: the bell, turns, the model and its context window, the live footer, compaction, scrolling and the reasoning chip.

use super::*;

#[test]
fn a_finished_run_asks_for_attention_until_the_panel_is_shown_focused() {
    let mut panel = AgentPanel::new(setup(vec![]));
    assert!(!panel.needs_attention());
    panel.apply(AgentEvent::AgentStart);
    assert!(!panel.needs_attention());
    panel.apply(AgentEvent::AgentEnd);
    assert!(panel.needs_attention());
    // Rendered in an inactive group it keeps waiting.
    render_buf_focused(&mut panel, 40, 12, false);
    assert!(panel.needs_attention());
    render_buf(&mut panel, 40, 12);
    assert!(!panel.needs_attention());
}

#[test]
fn only_a_long_run_rings_when_it_ends() {
    let mut panel = AgentPanel::new(setup(vec![]));
    end_run_of(&mut panel, 1);
    assert!(panel.needs_attention());
    assert!(!rings(&panel.tick()));
    end_run_of(&mut panel, 60);
    assert!(rings(&panel.tick()));
}

#[test]
fn a_run_the_user_stopped_rings_no_bell() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.run_start = Some(Instant::now() - Duration::from_secs(60));
    panel.stop_requested = Some(Instant::now());
    panel.apply(AgentEvent::AgentEnd);
    assert!(!rings(&panel.tick()));
}

#[test]
fn the_bell_rings_once_until_the_user_sees_the_panel() {
    let mut panel = AgentPanel::new(setup(vec![]));
    end_run_of(&mut panel, 60);
    assert!(rings(&panel.tick()));
    // Unseen, the next wait does not ring again.
    end_run_of(&mut panel, 60);
    assert!(!rings(&panel.tick()));
    // Shown focused in an unfocused window still counts as unseen.
    panel.handle_command(PanelCommand::SetHostFocus { focused: false });
    render_buf(&mut panel, 40, 12);
    end_run_of(&mut panel, 60);
    assert!(!rings(&panel.tick()));
    // Seen: the next wait rings.
    panel.handle_command(PanelCommand::SetHostFocus { focused: true });
    render_buf(&mut panel, 40, 12);
    end_run_of(&mut panel, 60);
    assert!(rings(&panel.tick()));
}

#[test]
fn typing_enter_runs_a_turn_and_renders_it() {
    let mut panel = panel(vec![reply("Hello from the model")]);
    type_text(&mut panel, "hi there");
    assert_eq!(panel.input_text(), "hi there");
    assert!(panel.captures_escape(), "non-empty input keeps Esc");

    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
    assert!(panel.input_text().is_empty());
    settle(&mut panel);

    let items = panel.transcript().items();
    assert!(matches!(&items[0], Item::User { text, .. } if text == "hi there"));
    assert!(items.iter().any(|item| matches!(
        item,
        Item::Assistant { text, streaming: false, .. } if text == "Hello from the model"
    )));
    let rows = render_text(&mut panel, 40, 16);
    assert!(rows.iter().any(|r| r.contains("› hi there")));
    assert!(rows.iter().any(|r| r.contains("Hello from the model")));
    assert!(
        rows.last().unwrap().starts_with("›"),
        "input box at the bottom"
    );
    assert!(
        rows[rows.len() - 2].starts_with("─"),
        "separator above the input"
    );

    // The knobs on the left, the figures flush right after the spacer.
    let segments = panel.status_segments();
    let split = segments
        .iter()
        .position(|s| s.kind == SegmentKind::Spacer)
        .expect("a spacer");
    let text = |segs: &[StatusSegment]| segs.iter().map(|s| s.text.as_str()).collect::<String>();
    assert_eq!(
            text(&segments[..split]),
            " Agent: default │ Permissions: configured │ Reasoning: off │ Tools: 0/0 │ Connection: local │ Model: m"
        );
    assert_eq!(text(&segments[split + 1..]), "↑100 ↓20 120/1k ▰▱▱▱▱▱▱▱ ");
}

#[test]
fn title_follows_the_first_prompt() {
    let mut fresh = panel(vec![reply("ok")]);
    // Empty conversation: the working directory.
    assert_eq!(fresh.title(), "Agent: /tmp");

    type_text(&mut fresh, "  make the   timeout configurable  ");
    fresh.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut fresh);
    assert_eq!(fresh.title(), "Agent: make the timeout configurable");

    // A long prompt is kept whole: the header cuts its end to the panel's
    // width, so a wide panel shows as much of it as fits.
    let mut wordy = panel(vec![reply("ok")]);
    type_text(&mut wordy, &"word ".repeat(30));
    wordy.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut wordy);
    assert_eq!(
        wordy.title(),
        format!("Agent: {}", "word ".repeat(30).trim_end())
    );
    assert_eq!(wordy.title_cut(), TitleCut::End);
}

#[test]
fn the_context_window_is_learned_from_the_provider() {
    let mut panel = panel(vec![reply("ok")]);
    // The configured window is only a fallback until the provider is known.
    assert_eq!(panel.model.context_window, 1000);
    // The provider reports the active model's real window; the panel always
    // adopts it, overriding the fallback.
    let models = vec![ModelInfo {
        id: panel.model.id.clone(),
        name: None,
        context_window: Some(48_000),
    }];
    assert!(panel.adopt_listed_models(&models));
    assert_eq!(panel.model.context_window, 48_000);
    // A second identical report is a no-op.
    assert!(!panel.adopt_listed_models(&models));
}

#[test]
fn a_model_left_to_the_provider_is_its_first_listed() {
    let mut base = setup(vec![reply("ok")]);
    base.model.id = String::new();
    let mut panel = AgentPanel::new(base);
    assert_eq!(panel.model_display(), "auto");
    // Before the list arrives there is nothing to send to: the text stays.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "hello");
    assert!(panel.transcript.items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text == termide_i18n::t().agent_notice_model_pending()
    )));
    let models = vec![
        ModelInfo {
            id: "first".into(),
            name: None,
            context_window: Some(64_000),
        },
        ModelInfo {
            id: "second".into(),
            name: None,
            context_window: None,
        },
    ];
    assert!(panel.adopt_listed_models(&models));
    assert_eq!(panel.model.id, "first");
    assert_eq!(panel.model.context_window, 64_000);
    // A new session in the panel starts on it too.
    assert_eq!(panel.configured_model.id, "first");
}

#[test]
fn a_model_without_a_reported_window_keeps_the_fallback() {
    // When the provider reports no window for the active model, the
    // configured fallback stays.
    let mut panel = panel(vec![reply("ok")]);
    let models = vec![ModelInfo {
        id: panel.model.id.clone(),
        name: None,
        context_window: None,
    }];
    assert!(!panel.adopt_listed_models(&models));
    assert_eq!(panel.model.context_window, 1000);
}

#[test]
fn an_empty_session_is_discarded_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        let path = panel.session.as_ref().unwrap().path().to_path_buf();
        assert!(path.exists());
        path
    };
    assert!(!path.exists(), "an unused session is removed on close");
}

#[test]
fn a_session_with_messages_survives_close() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        let path = panel.session.as_ref().unwrap().path().to_path_buf();
        type_text(&mut panel, "do something");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert!(path.exists());
        path
    };
    assert!(path.exists(), "a session with a conversation is kept");
}

#[test]
fn switching_away_from_an_empty_session_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    let empty = panel.session.as_ref().unwrap().path().to_path_buf();
    assert!(empty.exists());
    // A new session: the empty one we leave is discarded, not listed.
    assert!(panel.switch_session(None));
    assert!(!empty.exists(), "the empty session is removed on switch");
    let fresh = panel.session.as_ref().unwrap().path().to_path_buf();
    assert!(fresh.exists());
    assert_ne!(empty, fresh);
}

#[test]
fn the_context_bar_fills_with_the_percentage() {
    assert_eq!(context_bar(0), "▱▱▱▱▱▱▱▱");
    assert_eq!(context_bar(12), "▰▱▱▱▱▱▱▱");
    assert_eq!(context_bar(50), "▰▰▰▰▱▱▱▱");
    // Cells round to the nearest: a cell begun is not a cell filled.
    assert_eq!(context_bar(1), "▱▱▱▱▱▱▱▱");
    assert_eq!(context_bar(13), "▰▱▱▱▱▱▱▱");
    assert_eq!(context_bar(51), "▰▰▰▰▱▱▱▱");
    assert_eq!(context_bar(87), "▰▰▰▰▰▰▰▱");
    assert_eq!(context_bar(94), "▰▰▰▰▰▰▰▰");
    assert_eq!(context_bar(100), "▰▰▰▰▰▰▰▰");
    assert_eq!(context_bar(200), "▰▰▰▰▰▰▰▰");
}

#[test]
fn activity_follows_the_events_and_totals_accumulate() {
    let mut panel = panel(vec![reply("hi")]);
    assert!(panel.activity.is_none());
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Prefill));
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
        "hello".into(),
    )));
    assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Generating));
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(reply("hello"))));
    // reply()'s usage is input 100 / output 20.
    assert_eq!((panel.session_input, panel.session_output), (100, 20));
    panel.apply(AgentEvent::ToolExecutionStart {
        call: termide_agent_core::ToolCall {
            id: "1".into(),
            name: "shell".into(),
            arguments: serde_json::json!({}),
            extra_content: None,
        },
    });
    assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Tool));
    panel.apply(AgentEvent::AgentEnd);
    assert!(panel.activity.is_none());
}

#[test]
fn a_call_update_fills_in_the_shown_calls_arguments() {
    let mut panel = panel(vec![]);
    let call = |arguments: serde_json::Value| termide_agent_core::ToolCall {
        id: "t1".into(),
        name: "task".into(),
        arguments,
        extra_content: None,
    };
    panel.apply(AgentEvent::ToolExecutionStart {
        call: call(serde_json::json!({ "agent": "default" })),
    });
    let full = serde_json::json!({ "agent": "default", "prompt": "check the diff" });
    panel.apply(AgentEvent::ToolCallUpdate {
        call: call(full.clone()),
    });
    let shown = panel
        .transcript()
        .items()
        .iter()
        .find_map(|item| match item {
            Item::Tool { call, .. } => Some(call.arguments.clone()),
            _ => None,
        });
    assert_eq!(shown, Some(full));
    assert_eq!(panel.transcript().items().len(), 1);
}

#[test]
fn a_stop_ticks_on_the_live_line_and_the_answer_says_cancelled() {
    let footer = |panel: &AgentPanel| -> Vec<String> {
        panel
            .live_footer_lines(60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    };
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    panel.abort();
    // The stop ticks in place of the run clock, and leaves no notice behind.
    let stopping = termide_i18n::t().agent_notice_stopping();
    let lines = footer(&panel);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].trim_start().starts_with(stopping), "{lines:?}");
    assert!(
        termide_config::constants::TICK_FRAMES
            .iter()
            .any(|f| lines[0].contains(&format!(" {f} "))),
        "{lines:?}"
    );
    assert!(!panel
        .transcript
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { .. })));
    // The stopped answer says it was cancelled, not the provider's `aborted`.
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(
        AssistantMessage::failed("p", "m", StopReason::Aborted, "aborted"),
    )));
    panel.apply(AgentEvent::AgentEnd);
    assert!(footer(&panel).is_empty());
    let cancelled = termide_i18n::t().agent_run_cancelled();
    assert!(panel.transcript.items().iter().any(|i| matches!(
        i,
        Item::Assistant { error: Some(e), .. } if e == cancelled
    )));
}

/// An ACP adapter may go on with a prompt stopped before the agent answered
/// anything, hidden, and hold the next prompt behind it: the agent is started
/// anew. One stopped after it answered was interrupted for real.
#[test]
fn an_external_agent_stopped_before_its_first_reply_is_restarted() {
    let restarted = |panel: &AgentPanel| {
        let wanted = termide_i18n::t().agent_notice_restarted_after_stop();
        panel
            .transcript
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text == wanted))
    };
    let stopped = |panel: &mut AgentPanel| {
        panel.abort();
        panel.apply(AgentEvent::MessageEnd(Message::Assistant(
            AssistantMessage::failed("p", "m", StopReason::Aborted, "aborted"),
        )));
        panel.apply(AgentEvent::AgentEnd);
        panel.busy = false;
        panel.tick();
    };
    let mut panel = panel(vec![]);
    panel.external = true;
    panel.apply(AgentEvent::AgentStart);
    stopped(&mut panel);
    assert!(restarted(&panel));
    assert!(!panel.restart_due);

    let mut panel = self::panel(vec![]);
    panel.external = true;
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
        "on it".into(),
    )));
    stopped(&mut panel);
    assert!(!restarted(&panel));
}

#[test]
fn the_live_footer_shows_generation_meta_and_a_clock() {
    let text_of = |panel: &AgentPanel| -> Vec<String> {
        panel
            .live_footer_lines(40)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    };
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    // Prefill: the run clock alone — an animated glyph and the time since
    // the request, with no dividing rule and no generation line yet.
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        termide_config::constants::TICK_FRAMES
            .iter()
            .any(|f| lines[0].trim_start().starts_with(f)),
        "{lines:?}"
    );
    assert!(!lines[0].contains("🕒") && !lines[0].contains('╌'));

    // Once tokens stream, the `✍️` generation line joins the clock.
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
        "hello there".into(),
    )));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains('✍') && lines[0].contains('↓'));
    assert!(lines.iter().all(|l| !l.contains('⏫') && !l.contains('╌')));

    // A tool running after the reply: the clock alone, no generation.
    panel.apply(AgentEvent::ToolExecutionStart {
        call: termide_agent_core::ToolCall {
            id: "t1".into(),
            name: "shell".into(),
            arguments: serde_json::json!({ "command": "sleep 1" }),
            extra_content: None,
        },
    });
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(!lines[0].contains('✍'), "{lines:?}");
}

#[test]
fn a_compaction_between_runs_shows_its_progress_and_the_new_context() {
    let text_of = |panel: &AgentPanel| -> Vec<String> {
        panel
            .live_footer_lines(60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    };
    let mut panel = panel(vec![]);
    panel.context_tokens = 90_000;
    panel.apply(AgentEvent::CompactionStart {
        reason: termide_agent_core::CompactionReason::Manual,
        prompt_tokens: 80_000,
    });
    // The summary call reads the old transcript: a `⏫` line with its size,
    // and the clock keeps ticking although no run is in flight.
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("⏫") && lines[0].contains("↑~80k"),
        "{lines:?}"
    );
    panel.last_anim = Instant::now() - Duration::from_secs(1);
    assert!(panel
        .tick()
        .iter()
        .any(|e| matches!(e, PanelEvent::NeedsRedraw)));

    // Its summary streams: the `✍️` line, with nothing in the transcript.
    let items = panel.transcript.items().len();
    panel.apply(AgentEvent::CompactionUpdate(StreamEvent::TextDelta(
        "the summary so far".into(),
    )));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains('✍') && lines[0].contains('↓'),
        "{lines:?}"
    );
    assert_eq!(panel.transcript.items().len(), items);

    // Done: the context shows its new size at once, and nothing is left
    // ticking.
    panel.apply(AgentEvent::Compacted {
        summary: "the summary".into(),
        kept: 2,
        tokens_before: 90_000,
        tokens_after: 3_000,
        call: summary_call(),
    });
    assert_eq!(panel.context_tokens, 3_000);
    assert!(panel.activity.is_none());
    assert!(text_of(&panel).is_empty());
}

#[test]
fn the_live_footer_shows_the_prefill_until_the_first_token() {
    let text_of = |panel: &AgentPanel| -> Vec<String> {
        panel
            .live_footer_lines(60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    };
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: Some(48_000),
    });
    // The model reads the prompt: a `⏫` line with the estimate, above the
    // run clock.
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("⏫") && lines[0].contains("↑~48k"),
        "{lines:?}"
    );

    // A server reporting its progress: a bar and the tokens read of the
    // total in place of the estimate.
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::PrefillProgress {
        processed: 24_000,
        total: 48_000,
        cached: 12_000,
    }));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("▰▰▰▰▱▱▱▱") && lines[0].contains("↑24k/48k"),
        "{lines:?}"
    );
    assert!(!lines[0].contains('~'), "{lines:?}");

    // The first token ends the prefill: the `✍️` line takes its place.
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
        "hello".into(),
    )));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains('✍'), "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains('⏫')), "{lines:?}");
}

#[test]
fn the_live_footer_shows_a_wait_for_a_slot_until_the_request_is_sent() {
    let text_of = |panel: &AgentPanel| -> Vec<String> {
        panel
            .live_footer_lines(80)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    };
    let t = termide_i18n::t();
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: Some(48_000),
    });
    // Waiting behind two others: a `⏳` line in place of the prefill's.
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::Queued { ahead: 2 }));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains('⏳') && lines[0].contains(&t.agent_queued_ahead_fmt(2)),
        "{lines:?}"
    );
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::Queued { ahead: 0 }));
    assert!(text_of(&panel)[0].contains(t.agent_queued()));

    // Sent: the model reads, and the prefill is timed from here.
    let waited_from = panel.activity.unwrap().msg_start;
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::Admitted));
    let activity = panel.activity.unwrap();
    assert!(activity.queued.is_none());
    assert!(activity.msg_start >= waited_from);
    let lines = text_of(&panel);
    assert!(
        lines[0].contains('⏫') && !lines[0].contains('⏳'),
        "{lines:?}"
    );
}

#[test]
fn a_tall_focused_block_scrolls_to_its_end() {
    // A reply taller than the viewport.
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut panel = panel(vec![reply(&long)]);
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // Render short so the answer cannot fit, then focus its block.
    let _ = render_text(&mut panel, 40, 10);
    panel.chat_focus = true;
    panel.selected = panel.transcript().items().len() - 1;

    // Scrolling to the bottom must reach it: the focus keeps the block in
    // view without snapping the viewport back to the block's first line.
    panel.scroll_by(1000);
    let _ = render_text(&mut panel, 40, 10);
    assert!(panel.max_top() > 0, "the block is taller than the viewport");
    assert_eq!(
        panel.top,
        panel.max_top(),
        "a tall focused block still scrolls to its end"
    );
}

#[test]
fn a_selected_block_does_not_trap_scrolling() {
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut panel = panel(vec![reply(&long)]);
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = render_text(&mut panel, 40, 10);
    // Select the first block (the user message at the very top).
    panel.chat_focus = true;
    panel.selected = 0;
    let _ = render_text(&mut panel, 40, 10);

    // The selection near the top does not stop scrolling to the bottom.
    panel.scroll_by(1000);
    let _ = render_text(&mut panel, 40, 10);
    assert!(
        panel.max_top() > 0,
        "the content is taller than the viewport"
    );
    assert_eq!(panel.top, panel.max_top(), "scrolling down stays free");

    // And scrolling back to the top is equally free.
    panel.scroll_by(-1000);
    let _ = render_text(&mut panel, 40, 10);
    assert_eq!(panel.top, 0, "scrolling up stays free");
}

#[test]
fn arrow_navigation_brings_the_selected_block_into_view() {
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut panel = panel(vec![reply(&long)]);
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = render_text(&mut panel, 40, 10);
    // Scroll to the top and focus the chat on the first block.
    panel.scroll_by(-1000);
    panel.chat_focus = true;
    panel.selected = 0;
    let _ = render_text(&mut panel, 40, 10);
    assert_eq!(panel.top, 0);

    // Arrowing down to the tall answer scrolls it into view.
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    let _ = render_text(&mut panel, 40, 10);
    assert!(panel.top > 0, "the answer below is scrolled into view");
}

#[test]
fn a_compaction_that_kept_nothing_says_so_without_a_count_of_zero() {
    let mut panel = AgentPanel::new(setup(vec![reply("ok")]));
    let t = termide_i18n::t();
    panel.apply(AgentEvent::Compacted {
        summary: "the gist".into(),
        kept: 2,
        tokens_before: 90_000,
        tokens_after: 3_000,
        call: summary_call(),
    });
    // An external agent keeps nothing verbatim: the summary stands for all.
    panel.apply(AgentEvent::Compacted {
        summary: String::new(),
        kept: 0,
        tokens_before: 92_000,
        tokens_after: 0,
        call: summary_call(),
    });
    let shown = notices(&panel);
    assert_eq!(
        shown[shown.len() - 2..],
        [
            t.agent_notice_compacted_fmt(90_000, 2),
            t.agent_notice_compacted_summary_fmt(92_000),
        ]
    );
}

#[test]
fn a_compaction_and_a_refused_summary_call_are_logged_with_how_the_call_went() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    panel.apply(AgentEvent::Compacted {
        summary: "the gist".into(),
        kept: 2,
        tokens_before: 90_000,
        tokens_after: 3_000,
        call: summary_call(),
    });
    // An external agent compacted without saying how: an empty summary
    // must not replace the history the log rebuilds the context from.
    panel.apply(AgentEvent::Compacted {
        summary: String::new(),
        kept: 0,
        tokens_before: 92_000,
        tokens_after: 0,
        call: summary_call(),
    });
    // The model went on with the work instead of summarising it.
    let refused = CompactionCall {
        stop_reason: StopReason::ToolUse,
        tool_calls: vec!["shell".into()],
        thinking_chars: 1_200,
        ..summary_call()
    };
    panel.apply(AgentEvent::CompactionFailed {
        error: "the model returned a degenerate summary".into(),
        tokens_before: 95_000,
        call: Some(refused.clone()),
        reply: "Looking at the loop:".into(),
    });
    // With no call made, there is nothing to look at afterwards.
    panel.apply(AgentEvent::CompactionFailed {
        error: "too few messages to compact".into(),
        tokens_before: 10,
        call: None,
        reply: String::new(),
    });
    let logged = Session::open(panel.session.as_ref().unwrap().path()).unwrap();
    let kinds: Vec<&EntryKind> = logged
        .branch()
        .into_iter()
        .map(|e| &e.kind)
        .filter(|k| {
            matches!(
                k,
                EntryKind::Compaction { .. } | EntryKind::CompactionFailed { .. }
            )
        })
        .collect();
    assert_eq!(
        kinds,
        vec![
            &EntryKind::Compaction {
                summary: "the gist".into(),
                tokens_before: 90_000,
                keep_last: 2,
                tokens_after: Some(3_000),
                call: Some(summary_call()),
            },
            &EntryKind::CompactionFailed {
                error: "the model returned a degenerate summary".into(),
                tokens_before: 95_000,
                call: refused,
                reply: "Looking at the loop:".into(),
            },
        ]
    );
    // A refused call changes nothing the context is rebuilt from.
    assert!(logged
        .context_messages()
        .first()
        .is_some_and(|m| matches!(m, Message::User(u) if u.plain_text().contains("the gist"))));
}

#[test]
fn a_context_cut_down_without_a_summary_is_logged_for_replay() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    panel.apply(AgentEvent::Fitted {
        reason: CompactionReason::Threshold,
        budget: 250_000,
        keep_tail: 10_000,
        steps: vec![
            (FitStep::Reasoning, 400_000),
            (FitStep::ReadOutputs, 90_000),
        ],
        tokens_before: 247_000,
        tokens_after: 98_000,
    });
    assert_eq!(panel.context_tokens, 98_000);
    let logged = Session::open(panel.session.as_ref().unwrap().path()).unwrap();
    assert!(logged.branch().iter().any(|e| e.kind
        == EntryKind::Fitted {
            budget: 250_000,
            keep_tail: 10_000,
            steps: vec![
                (FitStep::Reasoning, 400_000),
                (FitStep::ReadOutputs, 90_000)
            ],
            tokens_before: 247_000,
        }));
}

#[test]
fn an_external_agents_turn_usage_does_not_fill_the_context_bar() {
    let mut panel = panel(vec![]);
    let mut own = reply("built-in");
    own.usage = Usage {
        input: 900,
        output: 100,
        cache_read: 0,
        cache_write: 0,
        ..Default::default()
    };
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(own)));
    assert_eq!(panel.context_tokens, 1_000);
    // What a whole Claude Code turn spent, over every call and cache read.
    let mut external = reply("external");
    external.provider = ACP_PROVIDER.into();
    external.usage = Usage {
        input: 62,
        output: 19_189,
        cache_read: 1_397_173,
        cache_write: 79_138,
        ..Default::default()
    };
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(external)));
    assert_eq!(panel.context_tokens, 1_000);
}

#[test]
fn the_session_records_the_provider_kind() {
    let dir = tempfile::tempdir().unwrap();
    let panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        provider_kind: "anthropic_compatible".into(),
        ..setup(vec![reply("ok")])
    });
    let recorded = panel.session.as_ref().unwrap().current_model().unwrap();
    assert_eq!(recorded.provider, "anthropic_compatible");
}

#[test]
fn the_reasoning_level_is_picked_from_the_chip_and_persists_in_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(Scripted::new(vec![reply("ok")]));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup_with(Arc::clone(&provider))
    });
    assert_eq!(chip(&panel, REASONING_ACTION), "off");
    let path = panel.session.as_ref().unwrap().path().to_path_buf();

    let events = panel.handle_status_action(REASONING_ACTION);
    let PanelEvent::ShowSelect { options, .. } = &events[0] else {
        panic!("a level picker: {events:?}");
    };
    assert_eq!(options, &["● off", "  low", "  medium", "  high"]);
    select(&mut panel, &events[0], 3);
    assert_eq!(chip(&panel, REASONING_ACTION), "high");

    // The next request asks for it.
    type_text(&mut panel, "hi");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(
        *provider.seen_thinking.lock().unwrap(),
        [ThinkingLevel::High]
    );

    // The choice is written to the session, so a resume brings it back.
    let reopened = Session::open(&path).unwrap();
    assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::High));
    assert_eq!(
        session_model(&panel.configured_model, Some(&reopened)).thinking,
        ThinkingLevel::High
    );
}

#[test]
fn a_level_the_model_lacks_shows_as_the_nearest_it_has() {
    let provider = Scripted {
        levels: vec![ThinkingLevel::Low, ThinkingLevel::High],
        ..Scripted::new(vec![])
    };
    let mut setup = setup_with(Arc::new(provider));
    setup.model.thinking = ThinkingLevel::Max;
    let panel = AgentPanel::new(setup);
    assert_eq!(chip(&panel, REASONING_ACTION), "high");
}

#[test]
fn an_on_off_model_flips_from_the_chip() {
    let provider = Scripted {
        levels: vec![ThinkingLevel::Off, ThinkingLevel::High],
        ..Scripted::new(vec![])
    };
    let mut panel = AgentPanel::new(setup_with(Arc::new(provider)));
    assert_eq!(chip(&panel, REASONING_ACTION), "off");
    panel.handle_status_action(REASONING_ACTION);
    assert_eq!(chip(&panel, REASONING_ACTION), "on");
    assert_eq!(panel.model.thinking, ThinkingLevel::High);
    panel.handle_status_action(REASONING_ACTION);
    assert_eq!(chip(&panel, REASONING_ACTION), "off");
}

#[test]
fn a_model_that_cannot_be_asked_to_reason_has_no_chip() {
    let provider = Scripted {
        levels: Vec::new(),
        ..Scripted::new(vec![])
    };
    let mut panel = AgentPanel::new(setup_with(Arc::new(provider)));
    assert!(!panel
        .status_segments()
        .iter()
        .any(|s| s.action == Some(REASONING_ACTION)));
    assert!(panel.handle_status_action(REASONING_ACTION).is_empty());
}
