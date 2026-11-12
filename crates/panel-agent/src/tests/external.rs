//! External agents and the goal check against them.

use super::*;

/// An external agent still starting — a reopen, then `/continue` on the
/// goal at once — is asked again shortly; the goal stays, with no warning.
#[test]
fn a_goal_check_refused_as_busy_is_asked_again_soon() {
    let panel = goal_check_refused(PromptError::Busy);
    let task = panel.goal_task.as_ref().expect("the goal is kept");
    assert!(!task.judging);
    assert!(task.judge_at.is_some_and(|at| at > Instant::now()));
    assert_eq!(task.judge_failures, 0);
}

/// A gone agent fails the check like a failed call: retried after a wait,
/// the goal kept rather than dropped.
#[test]
fn a_goal_check_on_a_stopped_agent_keeps_the_goal() {
    let panel = goal_check_refused(PromptError::Stopped);
    let task = panel.goal_task.as_ref().expect("the goal is kept");
    assert!(!task.judging);
    assert_eq!(task.judge_failures, 1);
    assert!(task.judge_at.is_some());
}

#[test]
fn an_external_agent_replaces_the_loop_and_hides_its_knobs() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("native")])
    });
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    assert!(panel.switch_agent("outside"));
    assert_eq!(chip(&panel, AGENT_ACTION), "outside");
    let texts: Vec<String> = panel
        .status_segments()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert!(!texts.iter().any(|t| t.starts_with("Mode")), "{texts:?}");
    assert!(!texts.iter().any(|t| t.starts_with("Model")), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains("acp")), "{texts:?}");
    // The earlier conversation is shown, and handed to the agent, which has
    // no session of its own to resume yet.
    let items = panel.transcript().items();
    assert!(matches!(&items[0], Item::User { text, .. } if text == "hello"));
    assert_eq!(
        EXTERNAL_START.with(|start| start.borrow_mut().take()),
        Some((None, vec!["user:hello".into(), "assistant:native".into()]))
    );

    // Model and mode are not ours any more.
    let events = panel.handle_status_action(MODE_ACTION);
    assert!(!events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(panel.mode.get(), Mode::Configured);

    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Assistant { text, .. } if text == "from outside")));
    // It reports no tokens and times no prefill: no made-up indicators.
    assert!(panel.transcript().items().iter().all(
        |i| !matches!(i, Item::Assistant { text, cost: Some(_), .. } if text == "from outside")
    ));
    // The log records the switch, the session the agent opened and its
    // answer.
    let session = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(session.current_agent().as_deref(), Some("outside"));
    assert_eq!(session.context_messages().len(), 4);
    assert_eq!(
        session.external_session().map(|r| r.session_id),
        Some("ext-1".into())
    );
    // Reopened, the conversation goes on in the agent's own session.
    let session = panel.session.take();
    panel.switch_session(session);
    assert_eq!(
        EXTERNAL_START
            .with(|start| start.borrow_mut().take())
            .and_then(|(resume, _)| resume),
        Some("outside:ext-1".into())
    );

    // Back to the built-in loop.
    assert!(panel.switch_agent("default"));
    assert!(!panel.external);
    assert_eq!(chip(&panel, MODE_ACTION), "configured");
}

#[test]
fn an_external_agent_on_termides_tools_lists_gets_and_refuses_them() {
    let served = Arc::new(Mutex::new(Vec::new()));
    let external = |on_ours: bool| {
        let (tx, rx) = mpsc::channel();
        let served = Arc::clone(&served);
        let mut panel = AgentPanel::new(AgentPanelSetup {
            backend: Some(Arc::new(move |setup: BackendSetup| {
                let backend = if on_ours {
                    External::on_termides_tools(setup, Arc::clone(&served))
                } else {
                    External::new(setup)
                };
                Ok(Box::new(backend) as Box<dyn Backend>)
            })),
            late_tools: Some(rx),
            ..setup(vec![])
        });
        panel.offered_tools = vec!["read".into(), "shell".into()];
        // An MCP server of termide's connects after the agent started.
        tx.send(LateTools::Ready {
            source: "db".into(),
            tools: vec![Arc::new(Late("db__query")) as Arc<dyn termide_agent_core::Tool>],
        })
        .unwrap();
        panel.tick();
        panel
    };

    // On its own tools there is nothing of termide's to show or serve.
    let mut own = external(false);
    assert!(own.external);
    assert!(own.handle_status_action(TOOLSET_ACTION).is_empty());
    assert!(!own
        .status_segments()
        .iter()
        .any(|s| s.action == Some(TOOLSET_ACTION)));
    assert!(served.lock().unwrap().is_empty());

    // On termide's tools it is served the MCP server's tools as they come,
    // and the list and the chip are there, with them.
    let mut panel = external(true);
    assert!(panel.external && panel.is_fresh());
    assert!(
        served.lock().unwrap().iter().any(|n| n == "db__query"),
        "{served:?}"
    );
    assert!(panel.waiting_tools.is_empty());
    assert_eq!(chip(&panel, TOOLSET_ACTION), "3/3");
    let events = panel.handle_status_action(TOOLSET_ACTION);
    let Some(PanelEvent::ShowChecklist { items, .. }) = events.first() else {
        panic!("the toolset list opens: {events:?}");
    };
    let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
    assert_eq!(keys, ["read", "shell", "db__query"]);

    // After the first request a tool switched off is refused rather than
    // taken out, as with the built-in loop.
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.apply_toolset(&["read".to_string(), "db__query".to_string()]);
    assert!(panel.toolset_off.contains("shell"));
    assert!(panel.context_off.is_empty());
    assert!(panel.blocked.read().unwrap().contains("shell"));
    assert_eq!(chip(&panel, TOOLSET_ACTION), "2/3");
    let items = panel.toolset_items();
    let bash = items.iter().find(|i| i.key == "shell").unwrap();
    assert!(
        bash.enabled && !bash.checked && !bash.note.is_empty(),
        "{bash:?}"
    );
}

#[test]
fn the_tools_chip_counts_only_what_the_list_offers() {
    // A resumed session switched off the tools of an MCP server that has not
    // connected (yet): they are not in the list, so not in the count either.
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.offered_tools = vec!["read".into(), "shell".into()];
    panel.toolset_off = ["gone__a", "gone__b", "gone__c"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(chip(&panel, TOOLSET_ACTION), "2/2");
    panel.toolset_off.insert("shell".into());
    assert_eq!(chip(&panel, TOOLSET_ACTION), "1/2");
}

#[test]
fn an_external_agent_on_its_own_tools_gets_and_lists_the_mcp_servers_alone() {
    let served = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = mpsc::channel();
    let for_backend = Arc::clone(&served);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        backend: Some(Arc::new(move |setup: BackendSetup| {
            Ok(
                Box::new(External::taking_mcp_tools(setup, Arc::clone(&for_backend)))
                    as Box<dyn Backend>,
            )
        })),
        late_tools: Some(rx),
        ..setup(vec![])
    });
    panel.offered_tools = vec!["read".into(), "shell".into()];
    // No MCP tools yet: nothing of termide's to show.
    assert!(!panel
        .status_segments()
        .iter()
        .any(|s| s.action == Some(TOOLSET_ACTION)));

    tx.send(LateTools::Ready {
        source: "db".into(),
        tools: vec![
            Arc::new(Late("db__query")) as Arc<dyn termide_agent_core::Tool>,
            Arc::new(Late("db__drop")) as Arc<dyn termide_agent_core::Tool>,
        ],
    })
    .unwrap();
    panel.tick();
    // Served the MCP server's tools, none of termide's own beside its own.
    assert_eq!(*served.lock().unwrap(), ["db__query", "db__drop"]);
    assert_eq!(chip(&panel, TOOLSET_ACTION), "2/2");
    let events = panel.handle_status_action(TOOLSET_ACTION);
    let Some(PanelEvent::ShowChecklist { items, .. }) = events.first() else {
        panic!("the toolset list opens: {events:?}");
    };
    let keys: Vec<&str> = items.iter().map(|i| i.key.as_str()).collect();
    assert_eq!(keys, ["db__query", "db__drop"]);

    // Switched off after the first request, one is refused.
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.apply_toolset(&["db__query".to_string()]);
    assert!(panel.blocked.read().unwrap().contains("db__drop"));
    assert_eq!(chip(&panel, TOOLSET_ACTION), "1/2");
}

/// The names `served` holds, sorted.
fn served_names(served: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    let mut names = served.lock().unwrap().clone();
    names.sort();
    names
}

/// Before the first request an external agent's toolset works as the
/// built-in loop's: what is switched off leaves its context and what is
/// switched on comes back, the agent started again on the new set.
#[test]
fn switching_off_before_the_first_request_starts_an_external_agent_again_without_it() {
    let served = Arc::new(Mutex::new(Vec::new()));
    let mut panel = external_with_server(External::on_termides_tools, &served, None);
    assert!(panel.external && panel.is_fresh());
    assert_eq!(
        served_names(&served),
        ["db__drop", "db__query", "read", "shell"]
    );

    panel.apply_toolset(&["read".to_string(), "db__query".to_string()]);
    panel.tick();
    assert!(panel.external);
    assert_eq!(served_names(&served), ["db__query", "read"]);
    let off: BTreeSet<String> = ["db__drop".to_string(), "shell".to_string()].into();
    assert_eq!(panel.toolset_off, off);
    assert_eq!(panel.context_off, off);
    assert!(panel.blocked.read().unwrap().is_empty());
    assert_eq!(chip(&panel, TOOLSET_ACTION), "2/4");
    assert!(panel.toolset_items().iter().all(|item| item.enabled));

    // Ticked on again, they come back.
    let all: Vec<String> = ["read", "shell", "db__query", "db__drop"]
        .map(String::from)
        .into();
    panel.apply_toolset(&all);
    panel.tick();
    assert_eq!(
        served_names(&served),
        ["db__drop", "db__query", "read", "shell"]
    );
    assert!(panel.toolset_off.is_empty() && panel.context_off.is_empty());
    assert_eq!(chip(&panel, TOOLSET_ACTION), "4/4");
}

/// A new session keeps what the last one switched off, out of the agent's
/// context, and before its first request it can be switched on again: the
/// agent opens a session of its own anew rather than resuming the empty one.
#[test]
fn an_external_agent_s_new_session_can_switch_on_what_the_last_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    let served = Arc::new(Mutex::new(Vec::new()));
    let mut panel = external_with_server(
        External::taking_mcp_tools,
        &served,
        Some(dir.path().to_path_buf()),
    );
    assert_eq!(served_names(&served), ["db__drop", "db__query"]);
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.apply_toolset(&["db__query".to_string()]);
    assert!(panel.blocked.read().unwrap().contains("db__drop"));

    assert!(panel.switch_session(None));
    panel.tick();
    assert_eq!(served_names(&served), ["db__query"]);
    assert!(panel.context_off.contains("db__drop"));
    let items = panel.toolset_items();
    let drop = items.iter().find(|i| i.key == "db__drop").unwrap();
    assert!(drop.enabled && !drop.checked, "{drop:?}");

    panel.apply_toolset(&["db__query".to_string(), "db__drop".to_string()]);
    panel.tick();
    assert!(panel.external);
    assert_eq!(served_names(&served), ["db__drop", "db__query"]);
    assert!(panel.toolset_off.is_empty() && panel.context_off.is_empty());
    assert!(panel.blocked.read().unwrap().is_empty());
    assert_eq!(
        EXTERNAL_START
            .with(|start| start.borrow_mut().take())
            .and_then(|(resume, _)| resume),
        None
    );
}

#[test]
fn an_agent_that_lists_its_tools_once_is_told_when_the_mcp_servers_answered() {
    use termide_agent_core::{McpServerState, McpSignIn, McpStatus};
    let state = |status| McpServerState {
        name: "db".into(),
        status,
        sign_in: McpSignIn::None,
        launch: String::new(),
        fingerprint: String::new(),
    };
    let statuses = Arc::new(Mutex::new(vec![state(McpStatus::Connecting)]));
    let served = Arc::new(Mutex::new(Vec::new()));
    let settled: Arc<Mutex<Option<Vec<String>>>> = Arc::default();
    let (tx, rx) = mpsc::channel();
    let (for_backend, settled_for_backend) = (Arc::clone(&served), Arc::clone(&settled));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        backend: Some(Arc::new(move |setup: BackendSetup| {
            let mut backend = External::taking_mcp_tools(setup, Arc::clone(&for_backend));
            backend.settled = Arc::clone(&settled_for_backend);
            Ok(Box::new(backend) as Box<dyn Backend>)
        })),
        catalog: Arc::new(Statuses(Arc::clone(&statuses))),
        late_tools: Some(rx),
        ..setup(vec![])
    });
    // Still connecting: not yet.
    panel.tick();
    assert!(settled.lock().unwrap().is_none());
    // Reported connected, its tools not here yet: not yet either.
    *statuses.lock().unwrap() = vec![state(McpStatus::Ready { tools: 1 })];
    panel.tick();
    assert!(settled.lock().unwrap().is_none());
    // Its tools came: handed over first, then told.
    tx.send(LateTools::Ready {
        source: "db".into(),
        tools: vec![Arc::new(Late("db__query")) as Arc<dyn termide_agent_core::Tool>],
    })
    .unwrap();
    panel.tick();
    assert_eq!(
        settled.lock().unwrap().as_deref(),
        Some(&["db__query".to_string()][..])
    );
}

#[test]
fn an_external_agent_that_follows_the_mode_gets_termides_prompt_tools_and_mode() {
    let seen: Arc<Mutex<Option<ModeBackend>>> = Arc::new(Mutex::new(None));
    let modes = Arc::new(Mutex::new(Vec::new()));
    let (for_factory, modes_for_factory) = (Arc::clone(&seen), Arc::clone(&modes));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        backend: Some(Arc::new(move |setup: BackendSetup| {
            let backend = |modes| ModeBackend {
                modes,
                setup_mode: setup.mode.get(),
                prompt: setup.system_prompt.clone(),
                tools: setup
                    .host_tools
                    .as_ref()
                    .map(|host| host.tools.names().iter().map(|n| n.to_string()).collect())
                    .unwrap_or_default(),
            };
            *for_factory.lock().unwrap() = Some(backend(Arc::new(Mutex::new(Vec::new()))));
            Ok(Box::new(backend(Arc::clone(&modes_for_factory))) as Box<dyn Backend>)
        })),
        ..setup(vec![])
    });
    assert!(panel.external);
    {
        let seen = seen.lock().unwrap();
        let handed = seen.as_ref().unwrap();
        assert_eq!(handed.setup_mode, Mode::Configured);
        assert_eq!(handed.prompt, panel.system_prompt);
        assert_eq!(
            handed.tools,
            panel
                .tools
                .names()
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
        );
    }
    // The context's fill and size come from the agent's report.
    panel.tick();
    assert_eq!(
        (panel.context_tokens, panel.model.context_window),
        (975, 1_000_000)
    );
    // Its own loop cannot pause between steps: a run shows stop alone.
    panel.busy = true;
    assert_eq!(panel.run_buttons(), vec![RunButton::Stop]);
    assert!(!panel.request_pause());
    panel.busy = false;
    // The Mode chip is there, and a switch reaches the agent.
    assert_eq!(chip(&panel, MODE_ACTION), "configured");
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(*modes.lock().unwrap(), vec![Mode::Auto]);
}

#[test]
fn token_totals_keep_the_cache_apart_from_what_is_billed_in_full() {
    let mut billed = panel(vec![]);
    billed.apply(AgentEvent::AgentStart);
    billed.apply(AgentEvent::MessageEnd(Message::Assistant(
        AssistantMessage {
            usage: Usage {
                input: 4,
                output: 63,
                cache_read: 919,
                cache_write: 1009,
                ..Default::default()
            },
            ..reply("ok")
        },
    )));
    // Written to the cache is billed in full; read from it is apart.
    assert_eq!(billed.token_totals(), "↑1k ↻919 ↓63");
    let texts: Vec<String> = billed
        .status_segments()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert!(texts.iter().any(|t| t.contains("↻919")), "{texts:?}");
    // No cache, no cache figure.
    let mut plain = panel(vec![]);
    plain.session_input = 12;
    plain.session_output = 3;
    assert_eq!(plain.token_totals(), "↑12 ↓3");
}

#[test]
fn an_external_agent_lists_and_switches_models() {
    let dir = tempfile::tempdir().unwrap();
    let picked = Arc::new(Mutex::new(None));
    let for_factory = Arc::clone(&picked);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        backend: Some(Arc::new(move |_setup: BackendSetup| {
            Ok(Box::new(ModelBackend {
                picked: Arc::clone(&for_factory),
                starting: Arc::default(),
            }) as Box<dyn Backend>)
        })),
        ..setup(vec![])
    });
    assert!(panel.external);
    // A tick adopts the advertised current model for the banner and chip.
    panel.tick();
    assert_eq!(panel.model.id, "m-fast");
    let texts: Vec<String> = panel
        .status_segments()
        .into_iter()
        .map(|s| s.text)
        .collect();
    // The chip shows the name the agent gives the model, not its id.
    assert!(texts.iter().any(|t| t == "Fast"), "{texts:?}");

    // The Model chip opens the agent's model list.
    let events = panel.handle_status_action(MODEL_ACTION);
    let Some(PanelEvent::ShowSelect { options, .. }) = events.first() else {
        panic!("expected a model picker, got {events:?}");
    };
    assert_eq!(options.len(), 2);
    assert!(options.iter().any(|o| o.contains("Slow")), "{options:?}");

    // Picking the second switches it over ACP and updates the chip.
    panel.handle_command(PanelCommand::SelectionMade {
        action: MODEL_ACTION.to_string(),
        index: 1,
    });
    assert_eq!(*picked.lock().unwrap(), Some("m-slow".to_string()));
    assert_eq!(panel.model.id, "m-slow");
}

#[test]
fn an_external_agent_spins_in_place_of_its_model_while_starting() {
    let starting = Arc::new(AtomicBool::new(true));
    let for_factory = Arc::clone(&starting);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        backend: Some(Arc::new(move |_setup: BackendSetup| {
            Ok(Box::new(ModelBackend {
                picked: Arc::default(),
                starting: Arc::clone(&for_factory),
            }) as Box<dyn Backend>)
        })),
        ..setup(vec![])
    });
    assert!(panel.external);
    // Starting: the chip is there, holding a spinner, and opens nothing.
    let spinner = termide_config::constants::SPINNER_FRAMES;
    let display = panel.model_display();
    assert!(spinner.contains(&display.as_str()), "{display}");
    let label = termide_i18n::t().agent_chip_model();
    assert!(panel.status_segments().iter().any(|s| s.text == label));
    assert!(panel.handle_status_action(MODEL_ACTION).is_empty());
    // Started: the model's name.
    starting.store(false, Ordering::SeqCst);
    panel.tick();
    assert_eq!(panel.model_display(), "Fast");
}

#[test]
fn an_external_agents_settings_change_during_a_run_and_outlive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let busy = Arc::new(AtomicBool::new(true));
    let (for_factory, busy_for_factory) = (Arc::clone(&asked), Arc::clone(&busy));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        backend: Some(Arc::new(move |_setup: BackendSetup| {
            Ok(Box::new(OptionsBackend::new(&for_factory, &busy_for_factory)) as Box<dyn Backend>)
        })),
        ..setup(vec![])
    });
    panel.tick();
    // The effort is on the Reasoning chip, the fast mode on Options.
    assert_eq!(chip(&panel, REASONING_ACTION), "Low");
    assert_eq!(chip(&panel, OPTIONS_ACTION), "Off");
    // A run is in flight: the effort changes all the same.
    assert!(panel.runtime.is_busy());
    let events = panel.handle_status_action(REASONING_ACTION);
    let Some(PanelEvent::ShowSelect { options, .. }) = events.first() else {
        panic!("expected the efforts, got {events:?}");
    };
    assert_eq!(options.len(), 2);
    panel.handle_command(PanelCommand::SelectionMade {
        action: REASONING_ACTION.to_string(),
        index: 1,
    });
    assert_eq!(chip(&panel, REASONING_ACTION), "High");
    // The one other setting opens its values at once.
    panel.handle_status_action(OPTIONS_ACTION);
    panel.handle_command(PanelCommand::SelectionMade {
        action: OPTION_VALUE_ACTION.to_string(),
        index: 1,
    });
    panel.handle_status_action(MODEL_ACTION);
    panel.handle_command(PanelCommand::SelectionMade {
        action: MODEL_ACTION.to_string(),
        index: 1,
    });
    let picked = vec![
        ("effort".to_string(), "high".to_string()),
        ("fast".to_string(), "on".to_string()),
        (MODEL_OPTION.to_string(), "m-slow".to_string()),
    ];
    assert_eq!(*asked.lock().unwrap(), picked);
    assert_eq!(panel.session.as_ref().unwrap().agent_options(), picked);

    // Reopened, the agent starts on its defaults and is asked for the same.
    asked.lock().unwrap().clear();
    busy.store(false, Ordering::SeqCst);
    let path = panel.session.as_ref().unwrap().path().to_path_buf();
    panel.session = None;
    assert!(panel.switch_session(Some(Session::open(&path).unwrap())));
    panel.tick();
    let mut again = asked.lock().unwrap().clone();
    again.sort();
    let mut expected = picked;
    expected.sort();
    assert_eq!(again, expected);
    assert_eq!(chip(&panel, REASONING_ACTION), "High");
    assert_eq!(panel.model.id, "m-slow");
}

#[test]
fn an_external_agents_own_commands_are_offered_and_sent_as_typed() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let busy = Arc::new(AtomicBool::new(false));
    let (for_factory, busy_for_factory) = (Arc::clone(&asked), Arc::clone(&busy));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        backend: Some(Arc::new(move |_setup: BackendSetup| {
            Ok(Box::new(OptionsBackend::new(&for_factory, &busy_for_factory)) as Box<dyn Backend>)
        })),
        ..setup(vec![])
    });
    type_text(&mut panel, "/dep");
    let offered: Vec<String> = panel
        .completion
        .as_ref()
        .map(|list| list.items().iter().map(|i| i.value.clone()).collect())
        .unwrap_or_default();
    assert_eq!(offered, ["deploy"]);
    panel.clear_input();
    // The agent's `/compact` stands for termide's, and both go as typed (a
    // template of the same name would win, as over a skill).
    for typed in ["/deploy staging", "/compact the tests"] {
        type_text(&mut panel, typed);
        panel.submit();
        panel.tick();
    }
    let prompts: Vec<String> = asked
        .lock()
        .unwrap()
        .iter()
        .filter(|(kind, _)| kind == "prompt")
        .map(|(_, text)| text.clone())
        .collect();
    assert_eq!(prompts, ["/deploy staging", "/compact the tests"]);
}

#[test]
fn a_cli_provider_pre_selects_its_configured_model_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let picked = Arc::new(Mutex::new(None));
    let for_factory = Arc::clone(&picked);
    let mut base = setup(vec![]);
    // A CLI provider with a configured model to pre-select.
    base.provider_kind = "claude_code".into();
    base.model = ModelSpec {
        id: "m-slow".into(),
        ..base.model
    };
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        backend: Some(Arc::new(move |_setup: BackendSetup| {
            Ok(Box::new(ModelBackend {
                picked: Arc::clone(&for_factory),
                starting: Arc::default(),
            }) as Box<dyn Backend>)
        })),
        ..base
    });
    assert!(panel.external);
    // The agent starts on "m-fast"; a tick applies the configured "m-slow".
    panel.tick();
    assert_eq!(*picked.lock().unwrap(), Some("m-slow".to_string()));
    assert_eq!(panel.model.id, "m-slow");
}
