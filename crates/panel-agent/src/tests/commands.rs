//! Completions, prompt templates, skills, definitions and MCP servers.

use super::*;

#[test]
fn file_completions_list_paths_and_at_mentions_insert_them() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("README.md"), "hi").unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git/HEAD"), "ref").unwrap();

    // The walker skips .git and offers files and directories.
    let all = file_completions(dir.path(), "");
    let values: Vec<&str> = all.iter().map(|i| i.value.as_str()).collect();
    assert!(values.contains(&"README.md"), "{values:?}");
    assert!(values.contains(&"src/"), "{values:?}");
    assert!(values.contains(&"src/main.rs"), "{values:?}");
    assert!(!values.iter().any(|v| v.contains(".git")), "{values:?}");
    // A prefix filters, name matches rank first.
    let main = file_completions(dir.path(), "main");
    assert_eq!(main.first().map(|i| i.value.as_str()), Some("src/main.rs"));
    // What git ignores is not offered; the match is fuzzy on the path.
    std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    std::fs::write(dir.path().join("target/debug/main"), "").unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    let values: Vec<String> = file_completions(dir.path(), "srmain")
        .into_iter()
        .map(|i| i.value)
        .collect();
    assert_eq!(values, ["src/main.rs"]);

    // Typing @ opens the file popup; selecting a file replaces the token.
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.path().to_path_buf(),
        ..setup(vec![])
    });
    type_text(&mut panel, "look at @READ");
    assert!(panel.completion.is_some(), "no @ completion popup");
    assert!(panel.completion_span.is_some());
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "README.md"));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "look at README.md ");
    assert!(panel.completion.is_none());

    // A directory keeps the popup open for its contents.
    type_text(&mut panel, "@src");
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "look at README.md @src/");
    assert!(panel.completion.is_some(), "dir did not reopen the popup");
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "src/main.rs"));
}

/// `@selection` and `@term` lead the `@` list; picking one takes back what
/// was typed and asks the app for the text, which comes back held under a
/// placeholder on a line of its own and goes out with a header, fenced.
#[test]
fn a_snippet_mention_asks_for_the_text_and_holds_it_under_a_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.path().to_path_buf(),
        ..setup(vec![])
    });
    type_text(&mut panel, "why @");
    let values: Vec<String> = panel
        .completion
        .as_ref()
        .expect("an @ popup")
        .items()
        .iter()
        .map(|i| i.value.clone())
        .collect();
    assert_eq!(values[..2], ["@selection", "@term"]);
    type_text(&mut panel, "te");
    let events = panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(events.iter().any(|event| matches!(
        event,
        PanelEvent::WantSnippet { panel: id, wanted: SnippetWanted::Terminal } if *id == panel.panel_id()
    )));
    assert_eq!(panel.input_text(), "why ");

    panel.attach_snippet(AgentSnippet {
        source: SnippetSource::Terminal {
            title: "me@host/src (zsh)".into(),
            name: "zsh".into(),
            selection: false,
        },
        text: "$ cargo test\nerror: ```x``` failed\n".into(),
    });
    type_text(&mut panel, "fails");
    assert_eq!(
        panel.input_text(),
        "why \n[#1 terminal zsh, 2 lines]\nfails"
    );
    assert_eq!(
        panel.expand_pastes(&panel.input_text()),
        "why \nThe last 2 lines of the terminal \"me@host/src (zsh)\":\n````\n$ cargo test\n\
         error: ```x``` failed\n````\n\nfails"
    );
}

#[test]
fn an_editor_selection_is_named_by_its_path_from_the_panel_and_its_lines() {
    let cwd = std::path::Path::new("/work");
    let selection = |path: Option<&str>, first, last| AgentSnippet {
        source: SnippetSource::Editor {
            path: path.map(std::path::PathBuf::from),
            first,
            last,
        },
        text: "fn main() {}".into(),
    };
    let (label, text) = snippet_text(&selection(Some("/work/src/main.rs"), 10, 24), cwd);
    assert_eq!(label, "selection src/main.rs:10-24");
    assert_eq!(
        text,
        "Selected in src/main.rs, lines 10-24:\n```\nfn main() {}\n```\n"
    );
    let (label, text) = snippet_text(&selection(Some("/elsewhere/a.rs"), 3, 3), cwd);
    assert_eq!(label, "selection /elsewhere/a.rs:3");
    assert!(
        text.starts_with("Selected in /elsewhere/a.rs, line 3:\n"),
        "{text}"
    );
    let (label, text) = snippet_text(&selection(None, 1, 2), cwd);
    assert_eq!(label, "selection 1-2");
    assert!(text.starts_with("Selected in an unsaved editor buffer, lines 1-2:"));
}

/// A mistyped name lists the built-ins the `/` list offers, no more and
/// no fewer.
#[test]
fn a_mistyped_command_lists_the_built_ins_on_offer() {
    let mut panel = panel(vec![reply("ok")]);
    type_text(&mut panel, "/");
    let offered: Vec<String> = panel
        .completion
        .as_ref()
        .expect("a completion popup")
        .items()
        .iter()
        .map(|i| i.value.clone())
        .filter(|name| BUILTINS.iter().any(|b| b.name == name))
        .collect();
    assert!(offered.iter().any(|n| n == "undo"), "{offered:?}");
    assert!(!offered.iter().any(|n| n == "pause"), "{offered:?}");
    panel.clear_input();
    type_text(&mut panel, "/nope");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let notice = panel
        .transcript()
        .items()
        .iter()
        .find_map(|item| match item {
            Item::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .expect("a notice");
    let listed: Vec<&str> = notice
        .rsplit_once(": ")
        .expect("a list of names")
        .1
        .split(", ")
        .filter(|name| BUILTINS.iter().any(|b| b.name == *name))
        .collect();
    let mut listed: Vec<String> = listed.into_iter().map(String::from).collect();
    let mut offered = offered;
    listed.sort();
    offered.sort();
    assert_eq!(listed, offered);
}

#[test]
fn slash_commands_expand_prompt_templates() {
    let mut expanding = panel(vec![reply("done")]);
    type_text(&mut expanding, "/review src/x.rs");
    expanding.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut expanding);
    let items = expanding.transcript().items();
    assert!(
        matches!(&items[0], Item::User { text, .. } if text == "Review src/x.rs carefully."),
        "{items:?}"
    );

    // An unknown command is refused with the names on offer; a path is text.
    let mut panel = panel(vec![reply("ok")]);
    type_text(&mut panel, "/nope");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.transcript().items().iter().any(
        |item| matches!(item, Item::Notice { text, .. } if text.contains("available: review"))
    ));
    assert_eq!(panel.input_text(), "/nope", "the input is kept for editing");
    assert_eq!(slash_command("/usr/bin/ls -la"), None);
    assert_eq!(slash_command("/review a b"), Some(("review", "a b")));
    assert_eq!(slash_command("/"), None);

    // The picker puts the command into the input, ready for arguments.
    let events = panel.handle_status_action(PROMPTS_ACTION);
    let picker = events.first().expect("picker");
    let PanelEvent::ShowSelect { options, .. } = picker else {
        panic!("expected a picker, got {picker:?}");
    };
    assert_eq!(options, &["/review <path> · Review a file"]);
    select(&mut panel, picker, 0);
    assert_eq!(panel.input_text(), "/review ");
}

#[test]
fn slash_runs_a_skill_after_templates_and_by_its_prefix() {
    let mut panel = skilled_panel(None);
    assert_eq!(send(&mut panel, "/deploy v1"), "Ship v1.");
    assert_eq!(send(&mut panel, "/review a.rs"), "Review a.rs carefully.");
    assert_eq!(
        send(&mut panel, "/skill:review a.rs"),
        "Skill review of a.rs."
    );
    assert_eq!(
        slash_command("/skill:review a"),
        Some(("skill:review", "a"))
    );
    assert_eq!(slash_command("/skill:"), None);
    assert_eq!(slash_command("/other:review"), None);
}

#[test]
fn an_expanded_command_keeps_what_was_typed() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Skilled::new()),
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    send(&mut panel, "/deploy v1");
    assert!(
        panel.transcript().items().iter().any(|item| matches!(
            item,
            Item::User { text, command: Some(command), .. }
                if text == "Ship v1." && command == "/deploy v1"
        )),
        "{:?}",
        panel.transcript().items()
    );
    // The input history recalls the command, not its expansion.
    assert_eq!(panel.history(), ["/deploy v1"]);

    // The log keeps it, so a reopened session shows the same headline.
    let path = panel
        .session
        .as_ref()
        .expect("a session")
        .path()
        .to_path_buf();
    let reopened = Session::open(&path).unwrap();
    let mut transcript = Transcript::default();
    for logged in &reopened.context_messages_with_times(&CompactionPrompts::default()) {
        push_history(&mut transcript, logged);
    }
    assert!(transcript.items().iter().any(|item| matches!(
        item,
        Item::User { command: Some(command), .. } if command == "/deploy v1"
    )));
}

#[test]
fn slash_completion_offers_a_hidden_skill_by_its_prefix() {
    let mut panel = skilled_panel(None);
    type_text(&mut panel, "/");
    let list = panel.completion.as_ref().expect("a completion popup");
    let names: Vec<&str> = list.items().iter().map(|i| i.value.as_str()).collect();
    assert!(names.contains(&"deploy"), "{names:?}");
    assert!(names.contains(&"review") && names.contains(&"skill:review"));
}

#[test]
fn shadowed_names_are_reported_when_the_panel_opens() {
    // A fresh session says so under its banner, which stays up.
    let fresh = skilled_panel(None);
    assert!(fresh.banner_shown());
    assert!(fresh.transcript().items().iter().any(|item| matches!(
        item,
        Item::Notice { text, kind: NoticeKind::Warn } if text.contains("/skill:review")
    )));

    // A resumed session has no banner; the warning is a notice all the same.
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session
        .append_timed_message(&Message::User(UserMessage::text("hi")), None)
        .unwrap();
    let resumed = skilled_panel(Some(Session::open(session.path()).unwrap()));
    assert!(!resumed.banner_shown());
    assert!(resumed.transcript().items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text.starts_with("/review")
    )));
}

#[test]
fn definitions_with_what_nothing_reads_are_reported_when_the_panel_opens() {
    let panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Misdefined),
        ..setup(vec![])
    });
    let warnings: Vec<&str> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|item| match item {
            Item::Notice {
                text,
                kind: NoticeKind::Warn,
            } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("/ai/agents/review/AGENT.md") && warnings[0].contains("descripton")
    );
    assert!(warnings[1].contains("/ai/tools/web-search.md"));
}

#[test]
fn late_tools_join_the_registry_and_failures_are_reported() {
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
    tx.send(LateTools::Failed {
        source: "ghost".into(),
        error: "cannot start ghost-server".into(),
    })
    .unwrap();
    let events = panel.tick();
    assert!(!events.is_empty());
    assert!(panel.tools.get("github__search").is_some());
    let notices: Vec<String> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|item| match item {
            Item::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "mcp github: 1 tools connected",
            "mcp ghost: cannot start ghost-server"
        ]
    );
    // The worker got them too: the next run sees the tool in its registry.
    let runtime = std::mem::replace(&mut panel.runtime, Box::new(Idle));
    let agent = runtime.into_agent().expect("agent");
    assert!(agent.tools().get("github__search").is_some());
}

#[test]
fn a_server_s_new_set_replaces_its_old_one_and_a_removed_server_takes_its_tools() {
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        ..setup(vec![])
    });
    let ready = |names: &[&'static str]| LateTools::Ready {
        source: "github".into(),
        tools: names
            .iter()
            .map(|name| Arc::new(Late(name)) as Arc<dyn termide_agent_core::Tool>)
            .collect(),
    };
    tx.send(ready(&["github__a", "github__b"])).unwrap();
    panel.tick();
    // The server changed its list: `a` leaves, `c` arrives, `b` stays.
    tx.send(ready(&["github__b", "github__c"])).unwrap();
    panel.tick();
    assert!(panel.tools.get("github__a").is_none());
    assert!(panel.tools.get("github__b").is_some());
    assert!(panel.tools.get("github__c").is_some());
    assert_eq!(panel.mcp_arrived.len(), 2);
    // Under the banner the server's one line says where it stands now.
    assert_eq!(notices(&panel), ["mcp github: 2 tools connected"]);

    // Dropped from the configuration: every tool of it goes, the worker's too.
    tx.send(LateTools::Gone {
        source: "github".into(),
    })
    .unwrap();
    panel.tick();
    assert!(panel.tools.get("github__b").is_none() && panel.mcp_arrived.is_empty());
    assert_eq!(
        notices(&panel).last().unwrap(),
        "mcp github: removed from the configuration"
    );
    let runtime = std::mem::replace(&mut panel.runtime, Box::new(Idle));
    let agent = runtime.into_agent().expect("agent");
    assert!(agent.tools().get("github__b").is_none());
    assert!(agent.tools().get("github__c").is_none());
}

#[test]
fn a_server_s_line_tracks_the_toolset_under_the_banner_and_logs_it_after() {
    let (tx, rx) = mpsc::channel();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        catalog: Arc::new(Servers(Arc::clone(&asked))),
        ..setup(vec![])
    });
    let ready = || LateTools::Ready {
        source: "github".into(),
        tools: vec![
            Arc::new(Late("github__a")) as Arc<dyn termide_agent_core::Tool>,
            Arc::new(Late("github__b")),
        ],
    };
    // A built-in tool beside the server's, as an agent offers one.
    panel.offered_tools = vec!["shell".into()];
    tx.send(ready()).unwrap();
    panel.tick();
    let keys = |panel: &AgentPanel| -> Vec<String> {
        panel
            .toolset_items()
            .into_iter()
            .map(|item| item.key)
            .collect()
    };
    let all = keys(&panel);
    let without = |gone: &[&str]| -> Vec<String> {
        all.iter()
            .filter(|key| !gone.contains(&key.as_str()))
            .cloned()
            .collect()
    };
    let before = notices(&panel).len();

    // Under the banner the server keeps one line, rewritten in place.
    panel.apply_toolset(&without(&["github__a", "github__b"]));
    assert_eq!(notices(&panel).len(), before);
    assert!(notices(&panel).contains(&"mcp github: 0 of 2 tools on".to_string()));
    panel.apply_toolset(&all);
    assert!(notices(&panel).contains(&"mcp github: 2 tools connected".to_string()));
    panel.press_toolset_button("mcp-reload:github");
    assert!(notices(&panel).contains(&"mcp github: connecting".to_string()));
    tx.send(ready()).unwrap();
    panel.tick();
    assert_eq!(notices(&panel).len(), before);
    assert!(notices(&panel).contains(&"mcp github: 2 tools connected".to_string()));

    // In a conversation every change is a line of its own.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let builtin = all
        .iter()
        .find(|key| !key.starts_with("github__"))
        .cloned()
        .expect("a built-in tool");
    panel.apply_toolset(&without(&["github__a", builtin.as_str()]));
    let seen = notices(&panel);
    let tail = &seen[seen.len() - 2..];
    assert_eq!(tail[0], "mcp github: 1 of 2 tools on");
    assert_eq!(tail[1], format!("Tools switched off: {builtin}; on: —"));
    panel.press_toolset_button("mcp-reload:github");
    tx.send(ready()).unwrap();
    panel.tick();
    assert_eq!(
        notices(&panel).last().unwrap(),
        "mcp github: reconnected, 2 tools"
    );
    assert_eq!(
        *asked.lock().unwrap(),
        ["reconnect github", "reconnect github"]
    );
}

#[test]
fn a_sign_in_is_asked_for_and_its_address_shown() {
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        ..setup(vec![])
    });
    tx.send(LateTools::NeedsLogin {
        source: "plane".into(),
    })
    .unwrap();
    panel.tick();
    assert_eq!(
        notices(&panel),
        ["mcp plane: needs sign-in — /mcp login plane"]
    );
    // The sign-in under way takes the server's line, with the address.
    tx.send(LateTools::LoginStarted {
        source: "plane".into(),
        url: "https://as.example/authorize?x=1".into(),
    })
    .unwrap();
    panel.tick();
    let seen = notices(&panel);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].contains("https://as.example/authorize?x=1"),
        "{seen:?}"
    );
}

#[test]
fn mcp_lists_reloads_and_signs_in_and_out() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Servers(Arc::clone(&asked))),
        ..setup(vec![])
    });
    for command in [
        "/mcp",
        "/mcp reload",
        "/mcp login plane",
        "/mcp logout plane",
        "/mcp what",
    ] {
        type_text(&mut panel, command);
        panel.submit();
    }
    assert_eq!(
        *asked.lock().unwrap(),
        ["reload", "login plane", "logout plane"]
    );
    assert_eq!(
        notices(&panel),
        [
            "mcp db: 4 tools",
            "mcp plane: needs sign-in",
            "MCP configuration reloaded — connecting: new; removed: —; unchanged: db",
            "mcp plane: signed out",
            "Usage: /mcp [reload [<server>] | login <server> | logout <server>]",
        ]
    );
    // Nothing of it went to the model.
    assert!(!panel.is_busy());
}

#[test]
fn the_toolset_list_heads_every_server_with_its_buttons() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Servers(Arc::clone(&asked))),
        ..setup(vec![])
    });
    let events = panel.handle_status_action(crate::toolset::TOOLSET_ACTION);
    let Some(PanelEvent::ShowChecklist { groups, prompt, .. }) = events.first() else {
        panic!("the toolset list opens");
    };
    assert!(prompt.contains("r reconnects"), "{prompt}");
    let summary: Vec<(String, String, Vec<String>)> = groups
        .iter()
        .map(|g| {
            (
                g.name.clone(),
                g.note.clone(),
                g.buttons
                    .iter()
                    .map(|b| format!("{}{}", b.key, b.id))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "MCP: db".into(),
                String::new(),
                vec!["rmcp-reload:db".into()]
            ),
            (
                "MCP: plane".into(),
                "needs sign-in".into(),
                vec!["lmcp-login:plane".into(), "rmcp-reload:plane".into()]
            ),
        ]
    );
    // A button does what it says and leaves the list open, brought up to
    // date.
    panel.pending_events.clear();
    panel.handle_command(PanelCommand::ChecklistButton {
        action: crate::toolset::TOOLSET_ACTION.into(),
        id: "mcp-login:plane".into(),
    });
    panel.handle_command(PanelCommand::ChecklistButton {
        action: crate::toolset::TOOLSET_ACTION.into(),
        id: "mcp-reload:db".into(),
    });
    assert!(panel.toolset_list_open);
    assert!(panel
        .pending_events
        .iter()
        .any(|e| matches!(e, PanelEvent::RefreshChecklist(_))));
    // The same one server from the input.
    type_text(&mut panel, "/mcp reload db");
    panel.submit();
    assert_eq!(
        *asked.lock().unwrap(),
        ["login plane", "reconnect db", "reconnect db"]
    );
}

#[test]
fn tools_that_arrive_while_the_list_is_open_refresh_it() {
    let (tx, rx) = mpsc::channel();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        catalog: Arc::new(Servers(Arc::clone(&asked))),
        ..setup(vec![])
    });
    // A tick with nothing to take asks for nothing, and a panel that raised
    // no list asks for nothing either.
    let events = panel.tick();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PanelEvent::RefreshChecklist(_))),
        "{events:?}"
    );
    tx.send(LateTools::Ready {
        source: "db".into(),
        tools: vec![Arc::new(Late("db__query")) as Arc<dyn termide_agent_core::Tool>],
    })
    .unwrap();
    let events = panel.tick();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PanelEvent::RefreshChecklist(_))),
        "{events:?}"
    );

    // The list stands open: what arrives now is brought to it, with the
    // headings as they now read — the connected one without its remark.
    panel.handle_status_action(crate::toolset::TOOLSET_ACTION);
    tx.send(LateTools::Ready {
        source: "db".into(),
        tools: vec![Arc::new(Late("db__other")) as Arc<dyn termide_agent_core::Tool>],
    })
    .unwrap();
    let events = panel.tick();
    let Some(PanelEvent::RefreshChecklist(refresh)) = events
        .iter()
        .find(|e| matches!(e, PanelEvent::RefreshChecklist(_)))
    else {
        panic!("the list is brought up to date: {events:?}");
    };
    assert_eq!(refresh.action, crate::toolset::TOOLSET_ACTION);
    // The server's new set is its own: the tool it had before has left with
    // the set that was replaced.
    let keys: Vec<&str> = refresh.items.iter().map(|i| i.key.as_str()).collect();
    assert_eq!(keys, ["db__other"], "{keys:?}");
    let heads: Vec<(&str, &str)> = refresh
        .groups
        .iter()
        .map(|g| (g.name.as_str(), g.note.as_str()))
        .collect();
    assert_eq!(
        heads,
        [("MCP: db", ""), ("MCP: plane", "needs sign-in")],
        "{heads:?}"
    );
    assert!(refresh.prompt.is_some());

    // Closed, the panel stops asking: there is no list left to keep up.
    panel.handle_command(PanelCommand::ChecklistDone {
        action: crate::toolset::TOOLSET_ACTION.into(),
        checked: Vec::new(),
    });
    tx.send(LateTools::Ready {
        source: "db".into(),
        tools: vec![Arc::new(Late("db__third")) as Arc<dyn termide_agent_core::Tool>],
    })
    .unwrap();
    let events = panel.tick();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, PanelEvent::RefreshChecklist(_))),
        "{events:?}"
    );
}
