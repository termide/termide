//! The welcome banner, its session list and search, saving the chat and the session totals.

use super::*;

#[test]
fn an_empty_session_shows_a_welcome_banner() {
    let mut panel = panel(vec![]);
    let rows = render_text(&mut panel, 60, 16);
    // The banner sits at the top, below one blank row, leaving the space
    // under it to the list of recent sessions. Its title is the agent, and
    // the directory comes first among the fields.
    assert!(rows[1].contains("default"), "{rows:#?}");
    // The logo starts on the title's row.
    assert!(rows[1].contains('╭') && !rows[0].contains('╭'), "{rows:#?}");
    let t = termide_i18n::t();
    let row_of = |label: &str| {
        rows.iter()
            .position(|r| r.contains(label))
            .unwrap_or_else(|| panic!("missing {label}: {rows:#?}"))
    };
    let cwd = row_of(t.agent_banner_cwd());
    assert!(cwd < row_of(t.agent_banner_connection()), "{rows:#?}");
    assert!(cwd < row_of(t.agent_banner_model()), "{rows:#?}");
    assert!(
        panel
            .banner_hits
            .iter()
            .any(|(rect, hit)| rect.y == 1 && *hit == BannerHit::Action(AGENT_ACTION)),
        "the title picks the agent"
    );
    // An agent's description goes under its name.
    assert!(panel.switch_agent("review"));
    let rows = render_text(&mut panel, 60, 16);
    assert!(rows[1].contains("review"), "{rows:#?}");
    assert!(rows[2].contains("Reviews diffs"), "{rows:#?}");
    // The banner is the empty-state: once a turn runs, real content shows.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let all = render_text(&mut panel, 60, 16).join("\n");
    assert!(
        !all.contains(termide_i18n::t().agent_banner_cwd()),
        "banner gone once used: {all}"
    );
    // Once the banner is gone it leaves no clickable fields behind.
    assert!(panel.banner_hits.is_empty());
}

#[test]
fn clicking_the_banner_directory_asks_to_move_the_fresh_session() {
    let mut panel = panel(vec![]);
    render_text(&mut panel, 60, 16);
    let rect = panel
        .banner_hits
        .iter()
        .find_map(|(rect, hit)| (*hit == BannerHit::Action(CWD_ACTION)).then_some(*rect))
        .expect("the directory is clickable in a fresh session");
    let events = panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, 60, 16),
    );
    let Some(PanelEvent::ChangeAgentCwd { session, cwd }) = events.into_iter().next() else {
        panic!("no directory request");
    };
    assert_eq!(cwd, panel.cwd);
    assert_eq!(session.as_deref(), panel.session_path());

    // Once something was sent the session stays where it was made.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(panel.handle_status_action(CWD_ACTION).is_empty());
}

#[test]
fn notices_before_the_first_request_go_under_the_banner() {
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        ..setup(vec![])
    });
    tx.send(LateTools::Ready {
        source: "github".into(),
        tools: vec![Arc::new(Late("github__search"))],
    })
    .unwrap();
    panel.tick();
    let rows = render_text(&mut panel, 60, 20);
    let all = rows.join("\n");
    // The banner stays up, the notice sits under it past a dashed rule.
    assert!(
        all.contains(termide_i18n::t().agent_banner_cwd()),
        "banner stays: {all}"
    );
    let rule = rows.iter().position(|r| r.contains("╌╌╌")).expect("a rule");
    let notice = rows
        .iter()
        .position(|r| r.contains("mcp github: 1 tools connected"))
        .expect("the notice");
    let cwd = rows
        .iter()
        .position(|r| r.contains(termide_i18n::t().agent_banner_cwd()))
        .unwrap();
    assert!(cwd < rule && rule < notice, "{rows:#?}");
    assert!(!panel.banner_hits.is_empty());

    // On a short panel the notices scroll with the banner: the fields come
    // first, and the wheel brings the newest notice in.
    for i in 0..10 {
        panel.notice(format!("note {i}"), NoticeKind::Info);
    }
    let rows = render_text(&mut panel, 60, 16);
    let all = rows.join("\n");
    let cwd = termide_i18n::t().agent_banner_cwd();
    assert!(all.contains(cwd) && !all.contains("note 9"), "{rows:#?}");
    panel.handle_scroll(50, Rect::new(0, 0, 60, 16));
    let all = render_text(&mut panel, 60, 16).join("\n");
    assert!(all.contains("note 9") && !all.contains(cwd), "{all}");

    // The first request takes the banner away; the notices stay above it.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let all = render_text(&mut panel, 60, 40).join("\n");
    assert!(
        !all.contains(termide_i18n::t().agent_banner_cwd()),
        "banner gone: {all}"
    );
    assert!(all.contains("note 9"), "{all}");
    assert!(panel.banner_hits.is_empty());
}

#[test]
fn ctrl_s_saves_the_chat_and_the_menu_offers_it() {
    let mut panel = panel(vec![reply_thinking("Hi there.", "they greet")]);
    assert!(panel
        .context_menu_items()
        .iter()
        .any(|(_, action)| *action == SAVE_CHAT_ACTION));
    // Nothing said yet: nothing to save, and the panel says so.
    let events = panel.handle_key(chord(KeyCode::Char('s'), KeyModifiers::CONTROL));
    assert!(!events
        .iter()
        .any(|e| matches!(e, PanelEvent::SaveContentAs { .. })));
    assert!(notices(&panel).last().unwrap().contains("Nothing to save"));

    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let events = panel.handle_key(chord(KeyCode::Char('s'), KeyModifiers::CONTROL));
    let Some(PanelEvent::SaveContentAs {
        content,
        default_name,
    }) = events.into_iter().next()
    else {
        panic!("Ctrl+S asks where to save");
    };
    assert!(default_name.ends_with(".md"), "{default_name}");
    assert!(content.starts_with("# "), "{content}");
    assert!(content.contains("### 🧑 You · ") && content.contains("\nhello\n"));
    assert!(content.contains("### 🤖 Agent · ") && content.contains("Hi there."));
    // The reasoning and the panel's notices stay out.
    assert!(!content.contains("they greet") && !content.contains("Nothing to save"));
}

/// What the `auto` mode reviewers spend joins the session's totals at the
/// next look, once; the context fill stays.
#[test]
fn the_reviewers_tokens_join_the_session_totals() {
    let mut panel = panel(vec![]);
    panel.context_tokens = 500;
    panel.reviewer.spent.add(&termide_agent_core::Usage {
        input: 300,
        output: 4,
        cache_read: 1000,
        cache_write: 0,
        ..Default::default()
    });
    panel.tick();
    assert_eq!(
        (
            panel.session_input,
            panel.session_cached,
            panel.session_output
        ),
        (300, 1000, 4)
    );
    assert_eq!(panel.context_tokens, 500);
    assert!(!panel.take_reviewer_spent());
}

/// What the reviewer spent is logged by model, so a panel reopening the
/// session starts its totals where they were, and its info tells who spent
/// what on which model.
#[test]
fn spending_outlives_a_reopen_and_shows_by_whom() {
    use termide_agent_core::Usage;
    let dir = tempfile::tempdir().unwrap();
    let mut answer = reply("ok");
    answer.usage = Usage {
        input: 1_000,
        output: 50,
        ..Default::default()
    };
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![answer])
    });
    type_text(&mut panel, "do something");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.reviewer.spent.add_on(
        "small",
        &Usage {
            input: 300,
            output: 4,
            cache_read: 1_000,
            ..Default::default()
        },
    );
    panel.tick();
    let totals = (
        panel.session_input,
        panel.session_cached,
        panel.session_output,
    );
    assert_eq!(totals, (1_300, 1_000, 54));
    let path = panel.session.as_ref().unwrap().path().to_path_buf();
    drop(panel);

    let mut reopened = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    assert!(reopened.switch_session(Some(Session::open(&path).unwrap())));
    assert_eq!(
        (
            reopened.session_input,
            reopened.session_cached,
            reopened.session_output,
        ),
        totals
    );
    // The context stands where the last call left it, not at nothing.
    assert_eq!(reopened.context_tokens, 1_050);
    let Some(PanelEvent::ShowSessionUsage { rows }) = reopened.session_summary().into_iter().next()
    else {
        panic!("the session info shows");
    };
    let row = |key: &str| {
        rows.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    // The counts and figures first, then the tokens right before the
    // context, then the breakdowns.
    let order = [
        "Messages",
        "Compactions",
        "Cache hits",
        "Tokens",
        "Context",
        "Context holds",
        "Spent by",
        "By model",
    ];
    let at: Vec<Option<usize>> = order
        .iter()
        .map(|key| rows.iter().position(|(k, _)| k == key))
        .collect();
    assert!(
        at.iter().all(Option::is_some) && at.windows(2).all(|w| w[0] < w[1]),
        "{rows:?}"
    );
    let by = row("Spent by");
    assert!(
        by.contains("Conversation") && by.contains("Reviewer"),
        "{by}"
    );
    assert!(row("By model").contains("small"), "{rows:?}");
    // 1000 of the 2300 prompt tokens came from the cache.
    assert_eq!(row("Cache hits"), "43%");
    // What the context holds now: the request and the answer.
    let parts = row("Context holds");
    assert!(parts.starts_with("Conversation"), "{parts}");
    // The bars of every breakdown stand in one column.
    let columns: Vec<usize> = [by, row("By model"), parts]
        .iter()
        .flat_map(|cell| cell.lines().map(str::to_string).collect::<Vec<_>>())
        .map(|line| line.chars().position(|c| c == '█' || c == '░').unwrap())
        .collect();
    assert!(columns.windows(2).all(|w| w[0] == w[1]), "{rows:?}");
}

/// A subagent's tokens join the session's totals when its `task` ends; its
/// context was its own, so the context fill stays.
#[test]
fn a_subagents_tokens_join_the_session_totals() {
    use termide_agent_core::{ToolCall, Usage};
    let mut panel = panel(vec![]);
    panel.context_tokens = 500;
    let call = ToolCall {
        id: "t1".into(),
        name: "task".into(),
        arguments: serde_json::json!({ "agent": "search", "prompt": "find it" }),
        extra_content: None,
    };
    panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&call, "found").with_spent(Usage {
            input: 40,
            output: 7,
            cache_read: 900,
            cache_write: 2,
            ..Default::default()
        }),
    });
    assert_eq!(
        (
            panel.session_input,
            panel.session_cached,
            panel.session_output
        ),
        (42, 900, 7)
    );
    assert_eq!(panel.context_tokens, 500);
}

/// What cleaning took out of `shell` output is read from the results in the
/// log, so a reopened session shows it, right after the compactions.
#[test]
fn the_summary_reports_output_cleaning_savings() {
    use termide_agent_core::{Message, ToolCall};
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
    let call = ToolCall {
        id: "b1".into(),
        name: "shell".into(),
        arguments: serde_json::json!({ "command": "ls" }),
        extra_content: None,
    };
    session
        .append_message(&Message::ToolResult(
            ToolResultMessage::text(&call, "out").with_details(
                serde_json::json!({ "raw_bytes": 100_000_u64, "cleaned_bytes": 25_000_u64 }),
            ),
        ))
        .unwrap();
    // A newer result counts characters, which its bytes would double.
    session
        .append_message(&Message::ToolResult(
            ToolResultMessage::text(&call, "out").with_details(serde_json::json!({
                "raw_bytes": 8_000_u64, "cleaned_bytes": 4_000_u64,
                "raw_chars": 4_000_u64, "cleaned_chars": 2_000_u64,
            })),
        ))
        .unwrap();
    let path = session.path().to_path_buf();
    drop(session);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    assert!(panel.switch_session(Some(Session::open(&path).unwrap())));
    let events = panel.session_summary();
    let Some(PanelEvent::ShowSessionUsage { rows }) = events.first() else {
        panic!("expected a summary modal, got {events:?}");
    };
    let at = rows
        .iter()
        .position(|(key, _)| key == "Cleaned shell")
        .unwrap_or_else(|| panic!("no cleaning row: {rows:?}"));
    assert_eq!(rows[at + 1].0, "Tokens");
    assert_eq!(rows[at + 2].0, "Context");
    // 77 000 of 104 000 taken out: 74%, about 19k tokens.
    assert_eq!(rows[at].1, "74%  ≈19k");
}

#[test]
fn slash_usage_opens_the_session_info_modal() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/usage");
    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PanelEvent::ShowSessionUsage { .. })),
        "expected an info modal, got {events:?}"
    );
}

#[test]
fn slash_prompt_opens_the_system_prompt() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/prompt");
    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        events.iter().any(|e| matches!(e, PanelEvent::ViewFile(_))),
        "expected the prompt file to open, got {events:?}"
    );
}

#[test]
fn handoff_offers_to_save_or_start_a_new_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.path().to_path_buf(),
        ..setup(vec![reply("# Handoff\n\n## Remaining\nFinish the parser.")])
    });
    type_text(&mut panel, "/handoff");
    panel.submit();

    // The brief is produced off-thread; a card offers what to do with it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline, "no handoff card");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(panel.pending, Some(Pending::Handoff { .. })));

    // Choosing "Save to HANDOFF.md" writes the brief to the panel's dir.
    panel.apply_form_action(ChoiceAction::Chosen(0));
    let written = std::fs::read_to_string(dir.path().join("HANDOFF.md")).unwrap();
    assert!(written.contains("Finish the parser."), "{written}");
    assert!(panel.pending.is_none());
}

#[test]
fn a_model_switch_in_a_fresh_session_updates_the_banner_not_the_transcript() {
    let mut panel = panel(vec![]);
    assert!(render_text(&mut panel, 60, 16)
        .join("\n")
        .contains(termide_i18n::t().agent_banner_cwd()));

    // No prompt yet: switching the model updates the banner in place and
    // pushes no notice, so the banner stays.
    assert!(panel.switch_model("gpt-5-brand-new", None));
    assert!(panel.is_fresh());
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { .. })));
    let all = render_text(&mut panel, 60, 16).join("\n");
    assert!(
        all.contains(termide_i18n::t().agent_banner_cwd()),
        "banner stays: {all}"
    );
    assert!(all.contains("gpt-5-brand-new"), "banner shows it: {all}");

    // Once the conversation has begun, a switch is announced as before.
    type_text(&mut panel, "hi");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(!panel.is_fresh());
    assert!(panel.switch_model("gpt-6-next", None));
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("gpt-6-next"))));
}

#[test]
fn a_re_pickable_banner_value_is_bold_not_underlined() {
    let mut panel = panel(vec![]);
    let buf = render_buf(&mut panel, 60, 16);
    let (x, y) = (0..16u16)
        .find_map(|y| {
            let row: String = (0..60u16).map(|x| buf[(x, y)].symbol()).collect();
            row.find("default")
                .map(|at| (row[..at].chars().count() as u16, y))
        })
        .expect("the agent's name is on the banner");
    let cell = &buf[(x, y)];
    assert!(cell.modifier.contains(Modifier::BOLD));
    assert!(!cell.modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn clicking_a_banner_field_reopens_its_picker() {
    let mut panel = panel(vec![]);
    // Rendering the empty-state banner records its clickable fields.
    let _ = render_text(&mut panel, 60, 16);
    assert_eq!(
        panel.banner_hits.len(),
        4,
        "the directory, the model, the agent and the tools are re-pickable"
    );
    let (rect, _) = panel
        .banner_hits
        .iter()
        .find(|(_, hit)| *hit == BannerHit::Action(AGENT_ACTION))
        .copied()
        .expect("the agent field is clickable");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    let events = panel.handle_mouse(click, Rect::new(0, 0, 60, 16));
    assert!(
        events.iter().any(|e| matches!(
            e,
            PanelEvent::ShowSelect { on_select: SelectAction::Custom(a), .. } if a == AGENT_ACTION
        )),
        "clicking the agent field opens the agent picker, got {events:?}"
    );
}

#[test]
fn a_fresh_banner_offers_recent_sessions_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    // The very first session has nothing else to offer.
    let all = render_text(&mut panel, 80, 24).join("\n");
    let sessions = termide_i18n::t().agent_banner_sessions();
    assert!(!all.contains(sessions), "{all}");
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // A new session lists the used one, but not itself (it is empty).
    panel.handle_status_action(NEW_SESSION_ACTION);
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(all.contains(sessions), "{all}");
    // Each session is dated to the minute of its last change, local time.
    let stamp = crate::runtime::local_minute(panel.recent_sessions[0].modified);
    assert_eq!(stamp.len(), "YYYY-MM-DD HH:MM".len(), "{stamp}");
    assert_eq!(&stamp[13..14], ":", "{stamp}");
    assert!(all.contains(&format!("{stamp}  first task")), "{all}");
    let (rect, _) = panel
        .banner_hits
        .iter()
        .find(|(_, hit)| *hit == BannerHit::Session(0))
        .copied()
        .expect("the recent session is clickable");
    assert_eq!(
        panel
            .banner_hits
            .iter()
            .filter(|(_, hit)| matches!(hit, BannerHit::Session(_)))
            .count(),
        1
    );
    // The list starts in the logo's column, left of the fields beside it,
    // so the titles get that width too.
    let (title, _) = panel
        .banner_hits
        .iter()
        .find(|(_, hit)| *hit == BannerHit::Action(AGENT_ACTION))
        .copied()
        .expect("the agent's name is clickable");
    assert!(rect.x < title.x, "{rect:?} {title:?}");
    assert!(rect.width > title.width, "{rect:?} {title:?}");
    let row = &render_text(&mut panel, 80, 24)[rect.y as usize];
    let logo_row = &render_text(&mut panel, 80, 24)[title.y as usize];
    assert_eq!(row.find(&stamp), logo_row.find('╭'), "{row:?} {logo_row:?}");

    // A first click only selects it, showing the list's cursor.
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(click, Rect::new(0, 0, 80, 24));
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert!(panel.chat_focus);
    assert_eq!(panel.recent_selected, 0);
    // A click on the session shown selected opens it in place of the fresh
    // one, however long after the first.
    render_text(&mut panel, 80, 24);
    panel.session_clicks.reset();
    panel.handle_mouse(click, Rect::new(0, 0, 80, 24));
    assert_eq!(panel.session_path().unwrap(), first_path);
    assert!(matches!(
        &panel.transcript().items()[0],
        Item::User { text, .. } if text == "first task"
    ));
    // With a conversation on screen, the banner and its list are gone.
    assert!(panel.recent_sessions.is_empty());
}

#[test]
fn a_click_selects_a_banner_session_and_a_double_click_opens_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    let mut first_path = None;
    for prompt in ["first task", "second task"] {
        first_path.get_or_insert_with(|| panel.session_path().unwrap().to_path_buf());
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    let first_path = first_path.unwrap();
    render_text(&mut panel, 80, 24);
    // The oldest session is listed second, below the cursor's default row.
    let (rect, _) = panel
        .banner_hits
        .iter()
        .find(|(_, hit)| *hit == BannerHit::Session(1))
        .copied()
        .expect("the older session is clickable");
    let area = Rect::new(0, 0, 80, 24);
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(click, area);
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert_eq!(panel.recent_selected, 1);
    // The second click comes before any render has drawn the selection.
    panel.handle_mouse(click, area);
    assert_eq!(panel.session_path().unwrap(), first_path);
}

#[test]
fn tab_walks_the_banner_sessions_and_enter_opens_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two"), reply("three")])
    });
    let mut first_path = None;
    for prompt in ["first task", "second task", "third task"] {
        first_path.get_or_insert_with(|| panel.session_path().unwrap().to_path_buf());
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    assert_eq!(panel.recent_sessions.len(), 3);

    // Tall enough, all are listed, newest first.
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(
        all.find("third task") < all.find("second task")
            && all.find("second task") < all.find("first task"),
        "{all}"
    );

    // A short panel shows one row; Tab takes the keyboard into the list,
    // the cursor starts on the newest, and walking down scrolls.
    let all = render_text(&mut panel, 80, 12).join("\n");
    assert!(
        all.contains("third task") && !all.contains("first task"),
        "{all}"
    );
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(panel.chat_focus);
    // The cursor shows only while the panel is focused.
    let colors = ThemeColors::default();
    let inverted = |buf: &Buffer| (0..12).any(|y| (0..80).any(|x| buf[(x, y)].bg == colors.fg));
    assert!(inverted(&render_buf(&mut panel, 80, 12)));
    assert!(!inverted(&render_buf_focused(&mut panel, 80, 12, false)));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.recent_selected, 2, "the cursor stops at the end");
    // The whole banner scrolls to keep the cursor in view.
    let all = render_text(&mut panel, 80, 12).join("\n");
    assert!(all.contains("first task"), "{all}");
    assert!(panel.banner_top > 0);
    // Back on the first session the banner returns to its top.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.recent_selected, 0);
    assert_eq!(panel.banner_top, 0);
    panel.handle_key(chord(KeyCode::End, KeyModifiers::NONE));
    // Typing while the list has the keyboard hands it back to the prompt,
    // and the character lands there; Esc hands it back too.
    type_text(&mut panel, "x");
    assert!(!panel.chat_focus);
    assert_eq!(panel.input_text(), "x");
    panel.clear_input();
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!panel.chat_focus);

    // Back in the list the cursor is where it was; Home goes to the top,
    // End to the bottom, and Enter opens the session under it and returns
    // the keyboard to the prompt.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(panel.recent_selected, 0);
    panel.handle_key(chord(KeyCode::End, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.session_path(), first_path.as_deref());
    assert!(!panel.chat_focus);
}

#[test]
fn ctrl_f_searches_the_banner_sessions_in_the_prompt_box() {
    let dir = tempfile::tempdir().unwrap();
    let (mut panel, paths) = panel_with_recent_sessions(
        dir.path(),
        &["fix login bug", "write docs", "refactor login form"],
    );
    let t = termide_i18n::t();
    type_text(&mut panel, "my draft");

    // Ctrl+F sets the draft aside: the prompt box holds the query, its
    // border says so and counts the matches, and the banner shows the list
    // alone.
    panel.handle_key(ctrl_f());
    assert!(panel.session_search_on());
    assert_eq!(panel.input_text(), "");
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(all.contains(t.agent_session_search_title()), "{all}");
    assert!(all.contains(" 3/3 "), "{all}");
    assert!(
        !panel
            .banner_hits
            .iter()
            .any(|(_, hit)| matches!(hit, BannerHit::Action(_))),
        "the fields make way for the list"
    );

    // Typing filters the list by title; the cursor shows from the query and
    // the arrows walk the matches.
    type_text(&mut panel, "login");
    assert_eq!(panel.recent_sessions.len(), 2);
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(all.contains(" 2/3 "), "{all}");
    assert!(!all.contains("write docs"), "{all}");
    let colors = ThemeColors::default();
    let buf = render_buf(&mut panel, 80, 24);
    assert!((0..24).any(|y| (0..80).any(|x| buf[(x, y)].bg == colors.fg)));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.recent_selected, 1);
    let picked = panel.recent_sessions[1].path.clone();
    assert!(picked == paths[0] || picked == paths[2]);

    // Enter opens it and the search ends; the prompt starts empty, as on any
    // switch of session.
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.session_path(), Some(picked.as_path()));
    assert!(!panel.session_search_on());
    assert_eq!(panel.input_text(), "");
}

#[test]
fn esc_ends_the_session_search_at_once_with_the_draft_back() {
    let dir = tempfile::tempdir().unwrap();
    let (mut panel, _) = panel_with_recent_sessions(dir.path(), &["alpha", "beta"]);
    let t = termide_i18n::t();
    type_text(&mut panel, "draft");
    panel.handle_key(ctrl_f());

    // Nothing matches: the banner says so and Enter opens nothing.
    type_text(&mut panel, "zzz");
    assert!(panel.recent_sessions.is_empty());
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(all.contains(t.agent_session_search_empty()), "{all}");
    let before = panel.session_path().map(Path::to_path_buf);
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.session_path().map(Path::to_path_buf), before);

    // Esc ends the search at once, query or not: the full list and the
    // draft come back.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!panel.session_search_on());
    assert_eq!(panel.recent_sessions.len(), 2);
    assert_eq!(panel.input_text(), "draft");
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(!all.contains(t.agent_session_search_title()), "{all}");

    // From the list, too; Tab moves between the query and the list, and a
    // character typed in the list lands in the query.
    panel.handle_key(ctrl_f());
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(panel.chat_focus);
    type_text(&mut panel, "b");
    assert!(!panel.chat_focus);
    assert_eq!(panel.input_text(), "b");
    assert_eq!(panel.recent_sessions.len(), 1);
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!panel.session_search_on());
    assert!(!panel.chat_focus);
    assert_eq!(panel.input_text(), "draft");
}

#[test]
fn the_session_query_is_not_a_request() {
    let dir = tempfile::tempdir().unwrap();
    let (mut panel, _) = panel_with_recent_sessions(dir.path(), &["alpha"]);
    panel.handle_key(ctrl_f());
    // `$` and `/` are letters of the query, not shell mode or a command.
    type_text(&mut panel, "$/");
    assert_eq!(panel.input_text(), "$/");
    assert!(!panel.shell_mode);
    assert!(panel.completion.is_none());
    // A paste lands in it on one line, and the list follows.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    panel.handle_key(ctrl_f());
    panel.handle_command(PanelCommand::PasteText {
        text: "al\nzz".into(),
    });
    assert_eq!(panel.input_text(), "al zz");
    assert!(panel.recent_sessions.is_empty());
}

#[test]
fn ctrl_f_without_sessions_to_search_does_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    type_text(&mut panel, "draft");
    panel.handle_key(ctrl_f());
    assert!(!panel.session_search_on());
    assert_eq!(panel.input_text(), "draft");
}

#[test]
fn f8_in_a_session_search_deletes_the_match_under_the_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let (mut panel, paths) = panel_with_recent_sessions(dir.path(), &["alpha", "beta"]);
    panel.handle_key(ctrl_f());
    type_text(&mut panel, "alp");
    let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    let [PanelEvent::ShowConfirm {
        message,
        on_confirm: ConfirmAction::Custom(action),
    }] = events.as_slice()
    else {
        panic!("expected a confirmation, got {events:?}");
    };
    assert!(message.contains("alpha"), "{message}");
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert!(!paths[0].exists());
    // The search goes on, now matching nothing.
    assert!(panel.session_search_on());
    assert!(panel.recent_sessions.is_empty());
    assert_eq!(panel.recent_all.len(), 1);
}

#[test]
fn the_banner_leaves_out_sessions_open_in_other_panels() {
    let dir = tempfile::tempdir().unwrap();
    let with_dir = |replies| AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(replies)
    };
    // One panel keeps its used session open; another's is used and closed.
    let mut busy = AgentPanel::new(with_dir(vec![reply("one")]));
    type_text(&mut busy, "kept open");
    busy.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut busy);
    let mut closed = AgentPanel::new(with_dir(vec![reply("two")]));
    type_text(&mut closed, "closed task");
    closed.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut closed);
    drop(closed);

    let mut fresh = AgentPanel::new(with_dir(vec![]));
    let all = render_text(&mut fresh, 80, 24).join("\n");
    assert!(all.contains("closed task"), "{all}");
    assert!(!all.contains("kept open"), "open elsewhere: {all}");

    // Closing the other panel releases its session, and the next tick lists
    // it.
    drop(busy);
    fresh.tick();
    let all = render_text(&mut fresh, 80, 24).join("\n");
    assert!(all.contains("kept open"), "{all}");
}

#[test]
fn f8_in_the_banner_list_deletes_the_picked_session_not_the_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    let mut first_path = None;
    for prompt in ["first task", "second task"] {
        first_path.get_or_insert_with(|| panel.session_path().unwrap().to_path_buf());
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    let first_path = first_path.unwrap();
    let fresh = panel.session_path().unwrap().to_path_buf();
    let confirm = |events: &[PanelEvent]| match events {
        [PanelEvent::ShowConfirm {
            message,
            on_confirm: ConfirmAction::Custom(action),
        }] => (message.clone(), action.clone()),
        other => panic!("expected a confirmation, got {other:?}"),
    };

    // From the prompt F8 still asks about the current session.
    let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    assert_eq!(confirm(&events).1, DELETE_SESSION_ACTION);

    // In the list it asks about the session under the cursor.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    let (message, action) = confirm(&events);
    assert!(message.contains("first task"), "{message}");
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert!(!first_path.exists());
    assert_eq!(panel.session_path(), Some(fresh.as_path()));
    assert_eq!(panel.recent_sessions.len(), 1);
    assert_eq!(panel.recent_selected, 0, "the cursor stays on the list");

    // Delete does the same; with the list empty the keyboard is back in the
    // prompt.
    let events = panel.handle_key(chord(KeyCode::Delete, KeyModifiers::NONE));
    let (message, action) = confirm(&events);
    assert!(message.contains("second task"), "{message}");
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert!(panel.recent_sessions.is_empty());
    assert!(!panel.chat_focus);
    assert!(fresh.exists(), "the fresh session is untouched");
}

#[test]
fn f2_in_the_banner_list_renames_the_picked_session_not_the_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    for prompt in ["first task", "second task"] {
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    let input = |events: &[PanelEvent]| match events {
        [PanelEvent::ShowInput {
            initial_value,
            on_submit: InputAction::Custom(action),
            ..
        }] => (initial_value.clone(), action.clone()),
        other => panic!("expected a rename prompt, got {other:?}"),
    };

    // From the prompt F2 still renames the current session.
    let events = panel.handle_key(chord(KeyCode::F(2), KeyModifiers::NONE));
    assert_eq!(input(&events).1, RENAME_ACTION);

    // In the list it renames the session under the cursor; the rename may
    // move it up the list, and the cursor follows it.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    let picked = panel.recent_sessions[1].path.clone();
    let events = panel.handle_key(chord(KeyCode::F(2), KeyModifiers::NONE));
    let (initial, action) = input(&events);
    assert_eq!(initial, "", "an unnamed session starts empty");
    panel.handle_command(PanelCommand::InputSubmitted {
        action,
        text: "renamed".to_string(),
    });
    let renamed = &panel.recent_sessions[panel.recent_selected];
    assert_eq!(renamed.path, picked, "the cursor follows the session");
    assert_eq!(renamed.name.as_deref(), Some("renamed"));
    assert_eq!(panel.recent_sessions.len(), 2);
    assert!(
        panel
            .recent_sessions
            .iter()
            .filter(|s| s.name.is_some())
            .count()
            == 1
    );
    assert_eq!(panel.session.as_ref().and_then(Session::name), None);

    // Asked again, the prompt offers the name it now has.
    let events = panel.handle_key(chord(KeyCode::F(2), KeyModifiers::NONE));
    let (initial, action) = input(&events);
    assert_eq!(initial, "renamed");

    // An empty answer clears the name: the row falls back to its first
    // prompt instead of going blank.
    panel.handle_command(PanelCommand::InputSubmitted {
        action,
        text: String::new(),
    });
    assert_eq!(panel.recent_sessions.len(), 2);
    assert!(panel.recent_sessions.iter().all(|s| s.name.is_none()));
}

#[test]
fn clearing_the_name_of_a_listed_session_with_no_messages_discards_it() {
    let dir = tempfile::tempdir().unwrap();
    let named = {
        let mut session = Session::create(dir.path(), Path::new("/work")).unwrap();
        session.set_name("only a name").unwrap();
        session.path().to_path_buf()
    };
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    assert_eq!(panel.recent_sessions.len(), 1);
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(panel.chat_focus);
    let events = panel.handle_key(chord(KeyCode::F(2), KeyModifiers::NONE));
    let [PanelEvent::ShowInput {
        on_submit: InputAction::Custom(action),
        ..
    }] = events.as_slice()
    else {
        panic!("expected a rename prompt, got {events:?}");
    };
    panel.handle_command(PanelCommand::InputSubmitted {
        action: action.clone(),
        text: String::new(),
    });
    assert!(panel.recent_sessions.is_empty());
    assert!(!named.exists(), "an untouched session is not left unlisted");
    assert!(!panel.chat_focus, "the keyboard goes back to the prompt");
}

#[test]
fn the_wheel_scrolls_the_whole_banner() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    for prompt in ["first task", "second task"] {
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    let area = Rect::new(0, 0, 80, 11);
    let all = render_text(&mut panel, 80, 11).join("\n");
    assert!(
        all.contains("default") && !all.contains("first task"),
        "{all}"
    );
    // The fields scroll away with the list, so the older session comes in
    // and the panel's own scrollbar tracks the banner.
    panel.handle_scroll(3, area);
    let all = render_text(&mut panel, 80, 11).join("\n");
    assert!(
        all.contains("first task") && !all.contains("default"),
        "{all}"
    );
    assert!(panel.scrollbars.vertical.is_some());
    // Only rows on screen take clicks.
    let view = panel.viewport_height() as u16;
    assert!(panel.banner_hits.iter().all(|(rect, _)| rect.y < view));
    assert!(!panel
        .banner_hits
        .iter()
        .any(|(_, hit)| *hit == BannerHit::Action(AGENT_ACTION)));
    // Past the end it stops, and back up the fields return.
    panel.handle_scroll(50, area);
    assert_eq!(panel.banner_top, panel.banner_max_top());
    panel.handle_scroll(-50, area);
    assert_eq!(panel.banner_top, 0);
    // The wheel leaves the list's cursor where it is.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_scroll(50, area);
    assert_eq!(panel.recent_selected, 0);
}

#[test]
fn a_notice_above_the_scrolled_banner_list_keeps_the_list_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    for prompt in ["first task", "second task"] {
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    let _ = render_text(&mut panel, 80, 11);
    panel.handle_scroll(50, Rect::new(0, 0, 80, 11));
    let before = render_text(&mut panel, 80, 11);
    let row_of = |rows: &[String], text: &str| rows.iter().position(|r| r.contains(text));
    panel.notice("mcp github: connecting".to_string(), NoticeKind::Info);
    let after = render_text(&mut panel, 80, 11);
    assert_eq!(
        row_of(&before, "first task"),
        row_of(&after, "first task"),
        "{after:#?}"
    );
}
