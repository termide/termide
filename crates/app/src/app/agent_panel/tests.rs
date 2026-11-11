use super::*;
use termide_agent_core::{AGENT_FILE, DEFAULT_AGENT_FILE, SEED_TOOLS};

/// What a project brings that runs programs waits for the user: its
/// MCP server is listed but not started, its hook does not run, and
/// `/trust` approves each as it is configured now, outside the project.
#[test]
fn a_projects_servers_and_hooks_wait_for_approval() {
    use termide_agent_core::McpStatus;
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    let global = tmp.path().join("ai");
    std::fs::create_dir_all(project.join(".termide/ai")).unwrap();
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(
        project.join(".mcp.json"),
        r#"{"mcpServers":{"ext":{"command":"/nonexistent/termide-ext","args":["-x"]}}}"#,
    )
    .unwrap();
    std::fs::write(
        project.join(".termide/ai/hooks.toml"),
        "[check]\nevent = \"before_tool_call\"\ncommand = \"./check.sh\"\n",
    )
    .unwrap();
    let store = tmp.path().join("data/trust/approved.toml");
    let approvals = termide_agent_core::approvals::Approvals::new(Some(store.clone()));
    let catalog = FsCatalog::with_global(&project, &project, Some(global), None, approvals);

    let status = catalog.mcp_status();
    assert_eq!(status[0].status, McpStatus::NeedsApproval);
    assert_eq!(status[0].launch, "/nonexistent/termide-ext -x");
    let hooks = catalog.held_hooks();
    assert_eq!(hooks.len(), 1);
    assert_eq!(
        (hooks[0].name.as_str(), hooks[0].launch.as_str()),
        ("check", "./check.sh")
    );

    let Trusted::Approved(Some(report)) = catalog.trust("ext", &status[0].fingerprint).unwrap()
    else {
        panic!("the server shown is approved");
    };
    assert_eq!(report.started, ["ext"]);
    assert_ne!(catalog.mcp_status()[0].status, McpStatus::NeedsApproval);
    assert_eq!(
        catalog.trust("check", &hooks[0].fingerprint).unwrap(),
        Trusted::Approved(None)
    );
    assert!(catalog.held_hooks().is_empty());
    assert!(store.is_file());
    assert!(catalog.trust("nope", "").is_err());
    // Nothing was written into the project.
    assert!(!project.join(".termide/approved.toml").exists());
}

/// A `.mcp.json` or hook changed between the notice and the approval is
/// not approved: the approval is of what was shown, and the launch the
/// file holds now comes back to be shown instead.
#[test]
fn a_launch_changed_after_it_was_shown_is_not_approved() {
    use termide_agent_core::McpStatus;
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    let global = tmp.path().join("ai");
    std::fs::create_dir_all(project.join(".termide/ai")).unwrap();
    std::fs::create_dir_all(&global).unwrap();
    let mcp = |args: &str| {
        std::fs::write(
                project.join(".mcp.json"),
                format!(r#"{{"mcpServers":{{"ext":{{"command":"/nonexistent/termide-ext","args":[{args}]}}}}}}"#),
            )
            .unwrap();
    };
    let hook = |command: &str| {
        std::fs::write(
            project.join(".termide/ai/hooks.toml"),
            format!("[check]\nevent = \"before_tool_call\"\ncommand = \"{command}\"\n"),
        )
        .unwrap();
    };
    mcp(r#""-x""#);
    hook("./check.sh");
    let store = tmp.path().join("data/trust/approved.toml");
    let approvals = termide_agent_core::approvals::Approvals::new(Some(store));
    let catalog = FsCatalog::with_global(&project, &project, Some(global), None, approvals);
    let shown = catalog.mcp_status()[0].fingerprint.clone();
    let shown_hook = catalog.held_hooks()[0].fingerprint.clone();

    // Changed in between: refused, and the new launch comes back.
    mcp(r#""-x","--evil""#);
    hook("./evil.sh");
    let Trusted::Changed {
        hook: false,
        launch,
        fingerprint,
    } = catalog.trust("ext", &shown).unwrap()
    else {
        panic!("a changed server is not approved");
    };
    assert_eq!(launch, "/nonexistent/termide-ext -x --evil");
    assert_ne!(fingerprint, shown);
    assert_eq!(catalog.mcp_status()[0].status, McpStatus::NeedsApproval);
    let Trusted::Changed {
        hook: true,
        launch,
        fingerprint: new_hook,
    } = catalog.trust("check", &shown_hook).unwrap()
    else {
        panic!("a changed hook is not approved");
    };
    assert_eq!(launch, "./evil.sh");
    assert_eq!(catalog.held_hooks().len(), 1);

    // What came back, shown in turn, is what an approval then takes.
    assert!(matches!(
        catalog.trust("ext", &fingerprint).unwrap(),
        Trusted::Approved(Some(_))
    ));
    assert_ne!(catalog.mcp_status()[0].status, McpStatus::NeedsApproval);
    assert_eq!(
        catalog.trust("check", &new_hook).unwrap(),
        Trusted::Approved(None)
    );
    assert!(catalog.held_hooks().is_empty());
}

#[test]
fn a_side_call_runs_on_a_connection_a_subscription_or_the_sessions_model() {
    let mut settings = AiSettings::default();
    for (name, provider, model) in [
        ("local", "openai_compatible", "qwen"),
        ("claude", "claude_code", ""),
    ] {
        settings.connections.insert(
            name.to_string(),
            termide_config::Connection {
                provider: provider.to_string(),
                model: model.to_string(),
                ..termide_config::Connection::default()
            },
        );
    }
    let pick = |connection: &str, model: &str| {
        side_model(
            &settings,
            &termide_config::SideModel {
                connection: connection.into(),
                model: model.into(),
            },
            "test",
        )
    };
    // An endpoint's model, or another of it.
    let own = |choice: ModelChoice| {
        let (provider, model) = choice.own.expect("a model of its own");
        (provider.name().to_string(), model.id)
    };
    assert_eq!(own(pick("local", "")).1, "qwen");
    assert_eq!(own(pick("local", "big")).1, "big");
    // A CLI agent through its subscription.
    assert_eq!(own(pick("claude", "haiku")), ("acp".into(), "haiku".into()));
    // None named: the session's, under another model when one is.
    let session = pick("", "haiku");
    assert!(session.own.is_none());
    assert_eq!(session.session_model.as_deref(), Some("haiku"));
    assert!(pick("", "").session_model.is_none());
    // A name that matches nothing: the session's.
    assert!(pick("gone", "").own.is_none());
}

#[test]
fn a_subscription_agent_with_no_reviewer_named_reviews_on_its_cheapest_model() {
    let mut settings = AiSettings::default();
    for (name, provider) in [
        ("local", "openai_compatible"),
        ("claude", "claude_code"),
        ("codex", "codex"),
        ("gemini", "gemini_cli"),
    ] {
        settings.connections.insert(
            name.to_string(),
            termide_config::Connection {
                provider: provider.to_string(),
                ..termide_config::Connection::default()
            },
        );
    }
    let cheap = |settings: &AiSettings, session: &str| {
        cheap_reviewer(settings, &settings.reviewer_of(session), session).map(|choice| {
            let (provider, model) = choice.own.expect("a model of its own");
            (provider.name().to_string(), model.id)
        })
    };
    // The agent's subscription, the model picked from what it offers.
    assert_eq!(
        cheap(&settings, "claude"),
        Some(("acp".into(), String::new()))
    );
    assert_eq!(
        cheap(&settings, "codex"),
        Some(("acp".into(), String::new()))
    );
    // An endpoint, or an agent with no cheap class: the session's model.
    assert_eq!(cheap(&settings, "local"), None);
    assert_eq!(cheap(&settings, "gemini"), None);
    // A reviewer named, by the connection or `[ai.auto_reviewer]`, wins.
    settings.auto_reviewer.model = "sonnet".into();
    assert_eq!(cheap(&settings, "claude"), None);
    settings.auto_reviewer.model.clear();
    settings.connections.get_mut("claude").unwrap().reviewer = "local".into();
    assert_eq!(cheap(&settings, "claude"), None);
}

/// Every built-in tool a panel can offer has a shipped text, and every
/// shipped text a tool: a tool whose `ToolText::seed` name is wrong, or
/// one added without its `assets/tools/<name>.md`, would otherwise reach
/// the model with no description and stay off the prompt's tool list.
#[test]
fn every_built_in_tool_has_its_shipped_text() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = AgentDirs::new(tmp.path(), Some(tmp.path()), None);
    let web = termide_agent_web::Web::new(WebConfig {
        backend: termide_agent_web::Backend::Http,
        engine: None,
        chrome_path: None,
        display: termide_agent_web::Display::Headless,
        profile: tmp.path().join("browser"),
    });
    let recall = recall_tool(&AiSettings::default(), &dirs, tmp.path(), tmp.path());
    let mut tools = base_tools(&dirs, Some(&web), Some(&recall));
    // Offered only with a search engine, which this test leaves out.
    tools.insert(Arc::new(termide_agent_web::WebSearchTool::new(web)));
    tools.insert(Arc::new(QuestionTool));
    tools.insert(Arc::new(SuggestCommandTool));
    tools.insert(Arc::new(SuggestGoalTool));
    tools.insert(Arc::new(SuggestLoopTool));
    let run: SubagentRun = Arc::new(|_, _, _, _| unreachable!());
    tools.insert(Arc::new(TaskTool::new(
        vec![("review".into(), "Reviews".into())],
        run,
    )));
    tools.insert(Arc::new(SkillTool::new(Vec::new())));
    for tool in peers::peer_tools(DEFAULT_AGENT, tmp.path()) {
        tools.insert(tool);
    }
    tools.insert(peers::fork_tool(DEFAULT_AGENT, tmp.path()));
    tools.insert(debug_tool::debug_tool(tmp.path()));
    tools.insert(Arc::new(JobsTool::new(Arc::new(PanelTasks))));

    let mut names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    names.sort_unstable();
    let mut shipped: Vec<&str> = SEED_TOOLS.iter().map(|(name, _)| *name).collect();
    shipped.sort_unstable();
    assert_eq!(names, shipped);
    for tool in tools.iter() {
        assert!(!tool.description().trim().is_empty(), "{}", tool.name());
        assert!(
            tool.prompt_snippet().is_some_and(|s| !s.trim().is_empty()),
            "{}",
            tool.name()
        );
    }
}

/// A local endpoint (the one new sessions start on) and a hosted one.
fn with_cloud() -> AiSettings {
    let mut settings = AiSettings {
        connection: "local".into(),
        ..AiSettings::default()
    };
    settings.connections.insert(
        "local".into(),
        Connection {
            model: "local-model".into(),
            ..Connection::default()
        },
    );
    settings.connections.insert(
        "cloud".into(),
        Connection {
            provider: "anthropic_compatible".into(),
            model: "claude-x".into(),
            context_window_fallback: Some(200_000),
            ..Connection::default()
        },
    );
    settings
}

/// What a panel on connection `name` of `settings` runs on.
fn active_on(settings: &AiSettings, name: &str) -> Active {
    let connection = &settings.connections[name];
    Active {
        connection: name.to_string(),
        kind: connection.provider.clone(),
        provider: build_provider(connection, None),
        model: connection.model.clone(),
        context_window: connection.effective_context_window(),
    }
}

/// The address decides nothing but the hosted APIs, which are known
/// services: llama.cpp is as likely on a rented machine with a domain of
/// its own (`llm.mbk.lan` through a VPN, a public name) as at home.
#[test]
fn hosted_apis_take_reasoning_effort_and_every_other_server_the_template() {
    let at = |base_url: &str| {
        let connection = Connection {
            base_url: base_url.into(),
            ..Connection::default()
        };
        (
            reasoning_param(&connection),
            sends_reasoning(&connection),
            prefill_progress(&connection),
        )
    };
    for hosted in [
        "https://api.openai.com/v1",
        "https://openrouter.ai/api/v1",
        "https://generativelanguage.googleapis.com/v1beta/openai",
    ] {
        assert_eq!(
            at(hosted),
            (ReasoningParam::Effort, false, false),
            "{hosted}"
        );
    }
    for server in [
        "http://127.0.0.1:10000/v1",
        "http://localhost:8080/v1",
        "http://llm.mbk.lan:8000/v1",
        "https://llm.example.com/v1",
        "http://8.8.8.8:8080/v1",
        "http://[fd00::5]:8080/v1",
        "https://api.openai.com.evil.example/v1",
    ] {
        assert_eq!(
            at(server),
            (ReasoningParam::EnableThinking, true, true),
            "{server}"
        );
    }
}

#[test]
fn only_openrouter_is_asked_to_cache_the_prompt() {
    let asks = |base_url: &str| {
        asks_for_prompt_cache(&Connection {
            base_url: base_url.into(),
            ..Connection::default()
        })
    };
    assert!(asks("https://openrouter.ai/api/v1"));
    assert!(asks("https://eu.openrouter.ai/api/v1"));
    assert!(!asks("https://openrouter.ai.example/api/v1"));
    assert!(!asks("https://api.openai.com/v1"));
    assert!(!asks("http://127.0.0.1:8080/v1"));
}

/// The applied `[ai]` settings are one per process: the tests that read
/// or publish them take turns, and a publisher clears them as it ends.
static APPLIED_TURN: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn applied_turn() -> std::sync::MutexGuard<'static, ()> {
    APPLIED_TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Clears the applied settings when dropped, a failed assertion included.
struct ClearApplied;

impl Drop for ClearApplied {
    fn drop(&mut self) {
        *APPLIED_AI.write().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

#[test]
fn connections_build_and_hand_their_provider_to_delegated_tasks() {
    let _turn = applied_turn();
    let mut settings = with_cloud();
    // `cloud`'s calls are reviewed by `local`'s model; `local` leaves it
    // to `[ai.auto_reviewer]`, which says the session's model.
    settings.connections.get_mut("cloud").unwrap().reviewer = "local".into();
    let active: ActiveSlot = Arc::new(std::sync::RwLock::new(active_on(&settings, "local")));
    let connections = AiConnections {
        settings,
        dirs: AgentDirs::new(Path::new("/tmp"), None, None),
        active: Arc::clone(&active),
    };
    let names: Vec<String> = connections.list().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["cloud", "local"]);
    let cloud = connections.build("cloud", DEFAULT_AGENT).unwrap();
    assert_eq!(cloud.kind, "anthropic_compatible");
    assert_eq!(
        (cloud.model.as_str(), cloud.context_window),
        ("claude-x", 200_000)
    );
    assert!(cloud.backend.is_none(), "an endpoint, not a CLI agent");
    let reviewer = |choice: &ConnectionChoice| {
        choice
            .reviewer
            .model
            .own
            .as_ref()
            .map(|(_, model)| model.id.clone())
    };
    assert_eq!(reviewer(&cloud).as_deref(), Some("local-model"));
    let local = connections.build("local", DEFAULT_AGENT).unwrap();
    assert_eq!(reviewer(&local), None);
    assert!(connections.build("missing", DEFAULT_AGENT).is_none());
    // Switching to it moves what a delegated task runs on.
    connections.activate(&cloud);
    let now = active.read().unwrap().clone();
    assert_eq!(
        (now.model.as_str(), now.context_window),
        ("claude-x", 200_000)
    );
    assert_eq!(
        (now.connection.as_str(), now.kind.as_str()),
        ("cloud", "anthropic_compatible")
    );
}

/// A delegated task runs on the session's connection, or on the one its
/// `subagents` field names: termide's loop on a model connection, a copy
/// of Claude Code on its; Codex and Gemini CLI run none.
#[test]
fn subagents_run_where_the_session_connection_says() {
    let mut settings = with_cloud();
    for (name, provider) in [("claude", "claude_code"), ("codex", "codex")] {
        settings.connections.insert(
            name.into(),
            Connection {
                provider: provider.into(),
                ..Connection::default()
            },
        );
    }
    let target = |settings: &AiSettings, session: &str| {
        subagent_target(settings, &active_on(settings, session)).map(|target| match target {
            SubagentTarget::Session(None) => "session".to_string(),
            SubagentTarget::Session(Some(model)) => format!("session {model}"),
            // Named only when it is not the connection's own.
            SubagentTarget::Model(name, connection)
                if connection.model == settings.connections[&name].model =>
            {
                format!("model {name}")
            }
            SubagentTarget::Model(name, connection) => format!("model {name} {}", connection.model),
            SubagentTarget::Cli(name, _) => format!("cli {name}"),
        })
    };
    // Nothing named: the session's own — termide's loop, or a copy of
    // Claude Code.
    assert_eq!(target(&settings, "local").as_deref(), Ok("session"));
    assert_eq!(target(&settings, "claude").as_deref(), Ok("cli claude"));
    // Codex has nowhere to run them, and says where to choose one.
    let error = target(&settings, "codex").unwrap_err();
    assert!(error.contains("Subagents"), "{error}");

    let set = |settings: &mut AiSettings, on: &str, to: &str| {
        settings.connections.get_mut(on).unwrap().subagents = to.into();
    };
    set(&mut settings, "codex", "local");
    assert_eq!(target(&settings, "codex").as_deref(), Ok("model local"));
    set(&mut settings, "local", "claude");
    assert_eq!(target(&settings, "local").as_deref(), Ok("cli claude"));
    set(&mut settings, "claude", "cloud");
    assert_eq!(target(&settings, "claude").as_deref(), Ok("model cloud"));
    // Itself, Codex or a name of no connection: the session's own.
    for named in ["local", "codex", "gone"] {
        set(&mut settings, "local", named);
        assert_eq!(
            target(&settings, "local").as_deref(),
            Ok("session"),
            "{named}"
        );
    }
    set(&mut settings, "codex", "gone");
    assert!(target(&settings, "codex").is_err());

    // With nothing on the connection, `[ai.subagents]` says, a model of the
    // connection included; the connection's own still comes first.
    set(&mut settings, "local", "");
    set(&mut settings, "codex", "");
    settings.subagents = termide_config::SideModel {
        connection: "cloud".into(),
        model: "small".into(),
    };
    assert_eq!(
        target(&settings, "local").as_deref(),
        Ok("model cloud small")
    );
    assert_eq!(
        target(&settings, "codex").as_deref(),
        Ok("model cloud small")
    );
    set(&mut settings, "codex", "local");
    assert_eq!(target(&settings, "codex").as_deref(), Ok("model local"));
    settings.subagents = termide_config::SideModel {
        connection: String::new(),
        model: "small".into(),
    };
    assert_eq!(target(&settings, "local").as_deref(), Ok("session small"));
}

/// A CLI agent standing in for Claude Code: it offers models, answers a
/// task with two messages and ends, or ends when stopped.
#[derive(Default)]
struct FakeCli {
    picked: std::sync::Mutex<Option<String>>,
    prompted: std::sync::Mutex<Vec<String>>,
    events: std::sync::Mutex<std::collections::VecDeque<AgentEvent>>,
    busy: std::sync::atomic::AtomicBool,
    /// Its first turn stops after its first words, going on until it is
    /// cut short, as a turn under way when its panel pauses.
    long_first_turn: bool,
}

impl Backend for FakeCli {
    fn prompt(&self, message: UserMessage) -> Result<(), termide_agent_core::PromptError> {
        use termide_agent_core::StopReason;
        self.prompted.lock().unwrap().push(message.model_text());
        self.busy.store(true, std::sync::atomic::Ordering::SeqCst);
        let said = |text: &str| {
            AgentEvent::MessageEnd(Message::Assistant(AssistantMessage {
                content: vec![termide_agent_core::AssistantContent::Text { text: text.into() }],
                stop_reason: StopReason::Stop,
                usage: Usage {
                    input: 100,
                    output: 10,
                    ..Usage::default()
                },
                provider: "acp".into(),
                model: "m".into(),
                error_message: None,
                failure: None,
                timestamp: 0,
            }))
        };
        let call = |id: &str, name: &str, arguments: serde_json::Value| ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
            extra_content: None,
        };
        let read = call(
            "c1",
            "read",
            serde_json::json!({ "path": "/proj/src/main.rs" }),
        );
        let test = call(
            "c2",
            "shell",
            serde_json::json!({ "command": "cargo test\n--quiet" }),
        );
        if self.long_first_turn && self.prompted.lock().unwrap().len() == 1 {
            self.events.lock().unwrap().extend([
                AgentEvent::ExternalSession {
                    agent: "claude".into(),
                    session_id: "ext-1".into(),
                },
                AgentEvent::AgentStart,
                AgentEvent::MessageEnd(Message::User(message)),
                said("working"),
            ]);
            return Ok(());
        }
        // As an ACP agent's turn does, the task is logged first.
        self.events.lock().unwrap().extend([
            AgentEvent::AgentStart,
            AgentEvent::MessageEnd(Message::User(message)),
            said("working"),
            AgentEvent::ToolExecutionStart { call: read.clone() },
            AgentEvent::ToolExecutionStart { call: test.clone() },
            AgentEvent::ToolExecutionEnd {
                result: termide_agent_core::ToolResultMessage::text(&read, "fn main"),
            },
            AgentEvent::ToolExecutionEnd {
                result: termide_agent_core::ToolResultMessage::error(&test, "failed"),
            },
            said("the answer"),
            AgentEvent::AgentEnd,
        ]);
        Ok(())
    }
    fn steer(&self, _message: UserMessage) {}
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {
        self.events.lock().unwrap().push_back(AgentEvent::AgentEnd);
    }
    fn is_busy(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn drain(&self) -> Vec<AgentEvent> {
        let events: Vec<_> = self.events.lock().unwrap().drain(..).collect();
        if events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
            self.busy.store(false, std::sync::atomic::Ordering::SeqCst);
        }
        events
    }
    fn update(
        &self,
        _update: Box<dyn FnOnce(&mut Agent) + Send>,
    ) -> Result<(), termide_agent_core::PromptError> {
        Err(termide_agent_core::PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), termide_agent_core::PromptError> {
        Err(termide_agent_core::PromptError::Unsupported)
    }
    fn available_models(&self) -> Vec<termide_agent_core::BackendModel> {
        ["claude-opus-5", "claude-haiku-5"]
            .map(|id| termide_agent_core::BackendModel {
                id: id.into(),
                name: id.into(),
            })
            .to_vec()
    }
    fn select_model(&self, model_id: String) -> Result<(), String> {
        *self.picked.lock().unwrap() = Some(model_id);
        Ok(())
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

/// A subagent on Claude Code runs on the connection's model, reports
/// what it says as it goes and answers with its last words; stopped
/// before it starts, it is not handed the task.
/// What a task's reviewer spent goes into the task's own log, by model,
/// so its session shows who spent what; the sum joins its outcome.
#[test]
fn a_tasks_reviews_are_logged_with_the_task() {
    let logs = tempfile::tempdir().unwrap();
    let mut log = DelegateLog::start(
        Some(logs.path()),
        Path::new("/proj"),
        SessionParent {
            session: Some("p1".into()),
            agent: "lead".into(),
            call: "call-7".into(),
        },
        &DelegateRun {
            connection: "claude",
            provider: "claude_code",
            model: "haiku",
            context_window: None,
            agent: "search",
        },
    );
    let reviews = termide_agent_core::SpentMeter::default();
    let spent = Usage {
        input: 30,
        output: 2,
        ..Default::default()
    };
    reviews.add_on("tiny", &spent);
    assert_eq!(log.note_reviews(&reviews), spent);
    // Taken once.
    assert_eq!(log.note_reviews(&reviews), Usage::default());
    let session = Session::open(&log.path().unwrap()).unwrap();
    let spending = session.spending();
    assert_eq!(spending.by.get(&Spender::Reviewer), Some(&spent));
    assert_eq!(spending.models.get("tiny"), Some(&spent));
}

#[test]
fn a_subagent_on_a_cli_agent_runs_its_task_to_the_end() {
    let (_prompter, permission_rx) = permission_channel(CancelToken::new());
    let (budget, cancel) = (CancelToken::new(), CancelToken::new());
    let control = termide_panel_agent::DelegateControl::new();
    let drive = CliDrive {
        control: &control,
        name: "claude:search",
        connection: "claude-code",
        model: "haiku",
        permission_rx: Some(&permission_rx),
        refusal: "nobody to ask",
        max_turns: SUBAGENT_MAX_TURNS,
        cwd: Path::new("/proj"),
        budget: &budget,
        cancel: &cancel,
        held_at_end: std::cell::Cell::new(false),
    };
    let cli = FakeCli::default();
    let mut progress = Vec::new();
    let logs = tempfile::tempdir().unwrap();
    let parent = SessionParent {
        session: Some("p1".into()),
        agent: "lead".into(),
        call: "call-7".into(),
    };
    let mut log = DelegateLog::start(
        Some(logs.path()),
        Path::new("/proj"),
        parent.clone(),
        &DelegateRun {
            connection: "claude",
            provider: "claude_code",
            model: "haiku",
            context_window: None,
            agent: "search",
        },
    );
    let task = UserMessage::text("find it").with_origin(MessageOrigin {
        session: "p1".into(),
        agent: "lead".into(),
        kind: OriginKind::Task,
        panel: None,
        hops: 0,
    });
    let answer = drive.run(&cli, &|| false, task.clone(), &mut log, &mut |update| {
        let ToolUpdate::Output(text) = update;
        progress.push(text);
    });
    let outcome = log.into_outcome(answer.unwrap());
    assert_eq!(outcome.answer.as_deref(), Ok("the answer"));
    // Every reply's tokens count, not the last one's alone.
    assert_eq!((outcome.spent.input, outcome.spent.output), (200, 20));
    assert_eq!(
        cli.picked.lock().unwrap().as_deref(),
        Some("claude-haiku-5")
    );
    // The task goes as the delegating agent's words.
    assert_eq!(*cli.prompted.lock().unwrap(), [task.model_text()]);
    assert!(task.model_text().contains("from=\"lead\""));
    // Its session is logged, naming where it came from: the task, as
    // the agent's, and what the delegate said.
    let path = outcome.session.expect("a log");
    let logged = Session::open(&path).unwrap();
    assert_eq!(logged.header().parent.as_ref(), Some(&parent));
    let summary = SessionSummary::from(&logged);
    assert_eq!(summary.display_label(), "& find it");
    let messages: Vec<Message> = logged
        .entries()
        .iter()
        .filter_map(|entry| match &entry.kind {
            termide_agent_core::EntryKind::Message { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(messages.len(), 3);
    assert!(matches!(&messages[0], Message::User(user) if user.origin.is_some()));
    assert!(matches!(&messages[2], Message::Assistant(_)));
    // Its calls show one line each among what it said, relative to its
    // directory, a failed one marked; a command shows its first line.
    assert_eq!(
        progress.last().unwrap(),
        "working\n\n· read src/main.rs\n· shell cargo test ✗\n\nthe answer"
    );

    cancel.cancel();
    let stopped = FakeCli::default();
    let answer = drive.run(
        &stopped,
        &|| false,
        UserMessage::text("find it"),
        &mut DelegateLog(None),
        &mut |_| {},
    );
    assert!(answer.is_err());
    assert!(stopped.prompted.lock().unwrap().is_empty());
}

/// Held at its panel's pause, a subagent on a CLI agent has its turn cut
/// short and, once the pause is lifted, is told to carry on in the same
/// session; the session the agent opened is in its log, for a task
/// carried on later.
#[test]
fn a_paused_subagent_on_a_cli_agent_carries_on_in_its_session() {
    let (budget, cancel) = (CancelToken::new(), CancelToken::new());
    let control = termide_panel_agent::DelegateControl::new();
    let drive = CliDrive {
        control: &control,
        name: "claude:search",
        connection: "claude-code",
        model: "",
        permission_rx: None,
        refusal: "nobody to ask",
        max_turns: SUBAGENT_MAX_TURNS,
        cwd: Path::new("/proj"),
        budget: &budget,
        cancel: &cancel,
        held_at_end: std::cell::Cell::new(false),
    };
    let cli = FakeCli {
        long_first_turn: true,
        ..FakeCli::default()
    };
    let logs = tempfile::tempdir().unwrap();
    let mut log = DelegateLog::start(
        Some(logs.path()),
        Path::new("/proj"),
        SessionParent {
            session: Some("p1".into()),
            agent: "lead".into(),
            call: "call-8".into(),
        },
        &DelegateRun {
            connection: "claude",
            provider: termide_agent_core::ACP_PROVIDER,
            model: "",
            context_window: None,
            agent: "search",
        },
    );
    control.pause();
    let lift = {
        let control = control.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            control.resume();
        })
    };
    let answer = drive.run(
        &cli,
        &|| false,
        UserMessage::text("find it"),
        &mut log,
        &mut |_| {},
    );
    lift.join().unwrap();
    assert_eq!(answer.unwrap().answer.as_deref(), Ok("the answer"));
    let prompted = cli.prompted.lock().unwrap().clone();
    assert_eq!(prompted.len(), 2, "{prompted:?}");
    assert!(prompted[1].contains("paused, not stopped"), "{prompted:?}");
    assert!(!drive.held_at_end.get());
    assert_eq!(
        log.external_resume().0.map(|session| session.session_id),
        Some("ext-1".to_string())
    );
}

#[test]
fn a_failed_subagent_run_says_why() {
    use termide_agent_core::{AssistantMessage, StopReason};
    let failed = |error: Option<&str>| {
        let mut reply = AssistantMessage::failed("agent", "m", StopReason::Error, "");
        reply.error_message = error.map(str::to_string);
        Message::Assistant(reply)
    };
    let refused = failed(Some("connection refused"));
    assert_eq!(
        run_error(std::slice::from_ref(&refused)).as_deref(),
        Some("connection refused")
    );
    // Only the last reply counts: a later one without an error has none.
    assert_eq!(run_error(&[refused, failed(None)]), None);
    assert_eq!(run_error(&[]), None);
}

#[test]
fn an_open_panel_offers_the_connections_applied_since() {
    let _turn = applied_turn();
    let _clear = ClearApplied;
    let mut built_with = with_cloud();
    built_with.connections.remove("cloud");
    let connections = AiConnections {
        active: Arc::new(std::sync::RwLock::new(active_on(&built_with, "local"))),
        settings: built_with,
        dirs: AgentDirs::new(Path::new("/tmp"), None, None),
    };
    // Applying the settings modal adds `cloud`; the panel offers it at
    // once.
    publish_ai_settings(&with_cloud());
    let names: Vec<String> = connections.list().into_iter().map(|e| e.name).collect();
    assert_eq!(names, ["cloud", "local"]);
    assert!(connections.build("cloud", DEFAULT_AGENT).is_some());
}

#[test]
fn a_session_reopens_on_the_connection_it_recorded_while_it_exists() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    let mut settings = with_cloud();
    let name = |settings: &AiSettings, session: &Session| {
        session_connection(settings, Some(session)).map(|(name, _)| name)
    };
    // Nothing recorded: the one new sessions start on.
    assert_eq!(name(&settings, &session).as_deref(), Some("local"));
    session.append_connection_change("cloud").unwrap();
    assert_eq!(name(&settings, &session).as_deref(), Some("cloud"));
    // A connection since removed falls back to the one new sessions use.
    settings.connections.remove("cloud");
    assert_eq!(name(&settings, &session).as_deref(), Some("local"));
    assert_eq!(name(&AiSettings::default(), &session), None);
}

/// A log from before connections were recorded: its model ran on Claude
/// Code, so it reopens on the Claude Code connection, not on the default
/// endpoint with Claude's model id.
#[test]
fn a_log_without_a_connection_reopens_on_one_of_its_provider() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session
        .append_model_change("claude_code", "opus[1m]", None)
        .unwrap();
    let mut settings = with_cloud();
    settings.connections.insert(
        "claude".into(),
        Connection {
            provider: "claude_code".into(),
            ..Connection::default()
        },
    );
    let reopened = session_connection(&settings, Some(&session)).map(|(name, _)| name);
    assert_eq!(reopened.as_deref(), Some("claude"));
    // No connection of that provider left: the one new sessions start on.
    settings.connections.remove("claude");
    let reopened = session_connection(&settings, Some(&session)).map(|(name, _)| name);
    assert_eq!(reopened.as_deref(), Some("local"));
}

#[test]
fn a_fork_is_named_after_its_source_with_a_counter_that_stays_unique() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    // Nothing sent yet: naming the copy would keep an empty log alive, so
    // the copy stays unnamed and is discarded with its panel.
    assert_eq!(fork_name(&session, None), None);

    session.set_name("Refactor").unwrap();
    assert_eq!(fork_name(&session, None).as_deref(), Some("Refactor (2)"));

    // Forking counts on from the names already in the directory: with the
    // first copy listed under `(2)`, the next fork takes `(3)`.
    let first_copy = session
        .fork(dir.path(), fork_name(&session, None).as_deref())
        .unwrap();
    let forked = Session::open_exclusive(&first_copy).unwrap();
    assert_eq!(forked.name(), Some("Refactor (2)"));
    assert_eq!(fork_name(&session, None).as_deref(), Some("Refactor (3)"));
    // Forking the copy counts on too, from the name it carries.
    assert_eq!(fork_name(&forked, None).as_deref(), Some("Refactor (3)"));

    // A source the user never named is labelled by its first prompt, so
    // the copy's suffix is what tells the two rows apart.
    let mut plain = Session::create(dir.path(), dir.path()).unwrap();
    plain
        .append_message(&Message::User(UserMessage::text("fix the bug")))
        .unwrap();
    assert_eq!(fork_name(&plain, None).as_deref(), Some("fix the bug (2)"));
}

#[test]
fn a_fork_the_agent_asked_for_answers_its_call_in_the_copy_alone() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = Session::create(dir.path(), dir.path()).unwrap();
    let call = |id: &str, name: &str| ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: serde_json::json!({}),
        extra_content: None,
    };
    let reply = AssistantMessage {
        content: vec![
            termide_agent_core::AssistantContent::ToolCall(call("f1", "fork")),
            termide_agent_core::AssistantContent::ToolCall(call("r1", "read")),
        ],
        stop_reason: termide_agent_core::StopReason::ToolUse,
        usage: Usage::default(),
        provider: "scripted".into(),
        model: "m".into(),
        error_message: None,
        failure: None,
        timestamp: 0,
    };
    source
        .append_message(&Message::User(UserMessage::text("do both")))
        .unwrap();
    source.append_run_start().unwrap();
    source.append_message(&Message::Assistant(reply)).unwrap();
    let copy = source.fork(dir.path(), None).unwrap();

    settle_fork(&copy, "f1").unwrap();
    let forked = Session::open(&copy).unwrap();
    let results: Vec<(String, bool, String)> = forked
        .context_messages()
        .into_iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some((
                result.tool_call_id.clone(),
                result.is_error,
                result.plain_text(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0],
        ("f1".to_string(), false, FORKED_RESULT.to_string())
    );
    assert_eq!((results[1].0.as_str(), results[1].1), ("r1", true));
    assert_eq!(forked.last_run(), termide_agent_core::LastRun::Ended);
    // The session it was copied from goes on running its calls.
    assert_eq!(source.last_run(), termide_agent_core::LastRun::CutOff);
}

#[test]
fn a_fork_name_is_the_lowest_free_counter_in_the_directory() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session.set_name("Refactor").unwrap();
    let copy = session.fork(dir.path(), Some("Refactor (2)")).unwrap();
    assert_eq!(fork_name(&session, None).as_deref(), Some("Refactor (3)"));

    // The counter counts the names in use, not the forks made: rename the
    // copy away and `(2)` is free again, so no number runs up for nothing.
    let mut forked = Session::open(&copy).unwrap();
    forked.set_name("Refactor (7)").unwrap();
    assert_eq!(fork_name(&session, None).as_deref(), Some("Refactor (2)"));
    // A fork of the renamed copy takes the same free number: whichever
    // forks first writes it, and the other then counts on from there.
    assert_eq!(fork_name(&forked, None).as_deref(), Some("Refactor (2)"));
    drop(forked);
    std::fs::remove_file(&copy).unwrap();
    assert_eq!(fork_name(&session, None).as_deref(), Some("Refactor (2)"));
}

#[test]
fn a_fork_the_agent_named_takes_its_title_with_a_counter_only_when_taken() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session.set_name("Refactor").unwrap();
    assert_eq!(fork_name(&session, Some(" Docs ")).as_deref(), Some("Docs"));
    session.fork(dir.path(), Some("Docs")).unwrap();
    assert_eq!(
        fork_name(&session, Some("Docs")).as_deref(),
        Some("Docs (2)")
    );
    // A blank title is none: the copy is named after its source.
    assert_eq!(
        fork_name(&session, Some(" ")).as_deref(),
        Some("Refactor (2)")
    );
}

#[test]
fn a_fork_without_a_title_is_named_after_the_first_line_of_its_request() {
    assert_eq!(fork_title(Some("  Log   migration "), "x"), "Log migration");
    assert_eq!(
        fork_title(None, "\n  Migrate the   logs  \nthen the docs"),
        "Migrate the logs"
    );
    assert_eq!(fork_title(Some(" "), "the docs"), "the docs");
    // A long line is cut at the last word that fits.
    let long = "word ".repeat(20);
    let title = fork_title(None, &long);
    assert!(title.ends_with("word…"), "{title}");
    assert!(title.chars().count() <= FORK_TITLE_CHARS, "{title}");
    let unbroken = "x".repeat(60);
    assert_eq!(
        fork_title(None, &unbroken).chars().count(),
        FORK_TITLE_CHARS
    );
}

#[test]
fn cli_providers_get_an_acp_backend() {
    // A CLI-adapter provider drives an external agent over ACP; a
    // wire-protocol provider uses the built-in loop (no backend override).
    let on = |provider: &str| Connection {
        provider: provider.into(),
        ..Connection::default()
    };
    assert!(cli_provider_backend(&on("claude_code"), "default").is_some());
    assert!(cli_provider_backend(&on("codex"), "default").is_some());
    assert!(cli_provider_backend(&on("gemini_cli"), "default").is_some());
    assert!(cli_provider_backend(&on("openai_compatible"), "default").is_none());
    assert!(cli_provider_backend(&on("anthropic_compatible"), "default").is_none());
}

#[test]
fn a_cli_agents_window_limit_reaches_it_in_its_environment() {
    let on = |provider: &str, limit: u64| Connection {
        provider: provider.into(),
        context_window_limit: Some(limit),
        ..Connection::default()
    };
    // Claude Code, raised to the least it takes.
    let claude = cli_acp_config(&on("claude_code", 50_000)).unwrap();
    assert_eq!(claude.env["CLAUDE_CODE_AUTO_COMPACT_WINDOW"], "100000");
    assert_eq!(claude.context_limit, Some(100_000));
    // Codex, compacting at nine tenths of it.
    let codex = cli_acp_config(&on("codex", 64_000)).unwrap();
    let config: serde_json::Value = serde_json::from_str(&codex.env["CODEX_CONFIG"]).unwrap();
    assert_eq!(config["model_context_window"], 64_000);
    assert_eq!(config["model_auto_compact_token_limit"], 57_600);
    assert_eq!(codex.context_limit, Some(64_000));
    // Gemini CLI has no setting for it; none set, none sent.
    let gemini = cli_acp_config(&on("gemini_cli", 64_000)).unwrap();
    assert!(gemini.env.is_empty());
    assert_eq!(gemini.context_limit, None);
    let unset = Connection {
        context_window_limit: None,
        ..on("codex", 1)
    };
    let own = cli_acp_config(&unset).unwrap();
    assert!(own.env.is_empty());
    assert_eq!(own.context_limit, None);
}

#[test]
fn codex_keeps_the_users_own_config_under_the_limit() {
    let merged: serde_json::Value =
        serde_json::from_str(&codex_config(Some(r#"{"model":"x"}"#), 1000)).unwrap();
    assert_eq!(merged["model"], "x");
    assert_eq!(merged["model_context_window"], 1000);
    let fresh: serde_json::Value =
        serde_json::from_str(&codex_config(Some("not json"), 1000)).unwrap();
    assert_eq!(fresh["model_auto_compact_token_limit"], 900);
}

#[test]
fn a_rule_is_merged_under_ai_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[ai]\nmodel = \"m\"  # keep me\n").unwrap();

    merge_permission_rule(&path, "shell", "cat *", Decision::Allow).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    // Written under [ai.permissions.shell] (not the old [agent.…]), and the
    // rest of the file is preserved.
    assert!(text.contains("[ai.permissions.shell]"), "{text}");
    assert!(text.contains("\"cat *\" = \"allow\""), "{text}");
    assert!(text.contains("# keep me"), "{text}");
    assert!(!text.contains("[agent"), "{text}");

    // The document is valid and the config loader reads the rule back.
    let config = termide_config::Config::load_from(&path).unwrap();
    assert_eq!(
        config.ai.permissions.evaluate("shell", "cat notes.txt"),
        Some(Decision::Allow)
    );

    // A second grant updates in place — no duplicate table, still valid.
    merge_permission_rule(&path, "shell", "python3 *", Decision::Allow).unwrap();
    merge_permission_rule(&path, "shell", "cat *", Decision::Allow).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text.matches("[ai.permissions.shell]").count(), 1, "{text}");
    let config = termide_config::Config::load_from(&path).unwrap();
    assert_eq!(
        config.ai.permissions.evaluate("shell", "python3 x.py"),
        Some(Decision::Allow)
    );
}

#[test]
fn a_broken_config_is_left_for_a_hand_fix() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // Two tables of the same name: invalid TOML.
    let broken = "[ai.permissions.bash]\n\"cat *\" = \"allow\"\n[ai.permissions.bash]\n\"ls *\" = \"allow\"\n";
    std::fs::write(&path, broken).unwrap();
    assert!(merge_permission_rule(&path, "shell", "python3 *", Decision::Allow).is_err());
    // The file is untouched, not appended to.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
}
#[test]
fn restore_is_skipped_without_a_connection_to_run_on() {
    let mut settings = AiSettings::default();
    assert!(restore_agent_panel(
        &settings,
        PathBuf::from("/tmp"),
        None,
        None,
        AgentSetupState::default()
    )
    .is_none());
    // A connection with no model named leaves the model to the provider.
    settings
        .connections
        .insert("local".into(), Connection::default());
    assert!(usable_connection(&settings).is_some());
}

#[test]
fn a_saved_setup_seeds_the_fresh_session_of_a_restore() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = Path::new("/work");
    let mut settings = AiSettings::default();
    for (name, provider) in [("local", "openai_compatible"), ("cloud", "anthropic")] {
        let connection = Connection {
            provider: provider.into(),
            model: format!("{name}-model"),
            ..Connection::default()
        };
        settings.connections.insert(name.into(), connection);
    }
    let seed = |setup: &AgentSetupState| {
        seeded_session(&settings, Some(dir.path()), cwd, DEFAULT_AGENT, setup)
    };
    assert!(seed(&AgentSetupState::default()).is_none());

    let session = seed(&AgentSetupState {
        connection: Some("cloud".into()),
        model: Some("picked".into()),
        thinking: Some("high".into()),
        options: vec![("mode".into(), "plan".into())],
    })
    .unwrap();
    assert_eq!(session.current_connection().as_deref(), Some("cloud"));
    let model = session.current_model().unwrap();
    assert_eq!(
        (model.provider.as_str(), model.id.as_str()),
        ("anthropic", "picked")
    );
    assert_eq!(session.current_thinking(), Some(ThinkingLevel::High));
    assert_eq!(session.agent_options(), [("mode".into(), "plan".into())]);
    // Still empty: discarded like any unused session once it closes.
    assert!(session.is_empty());

    // A connection gone from the config leaves its model behind too.
    let session = seed(&AgentSetupState {
        connection: Some("gone".into()),
        model: Some("picked".into()),
        ..AgentSetupState::default()
    })
    .unwrap();
    let fallback = session.current_connection().unwrap();
    let model = session.current_model().unwrap().id;
    assert_eq!(model, format!("{fallback}-model"));
}

/// A provider listing `ids`.
struct Lists(Vec<&'static str>);

impl Provider for Lists {
    fn name(&self) -> &str {
        "lists"
    }

    fn stream(
        &self,
        _: &termide_agent_core::Request<'_>,
        _: &mut dyn FnMut(termide_agent_core::StreamEvent),
        _: &CancelToken,
    ) -> termide_agent_core::AssistantMessage {
        unreachable!("only asked for its models")
    }

    fn list_models(&self) -> Result<Vec<termide_agent_core::ModelInfo>, String> {
        Ok(self
            .0
            .iter()
            .map(|id| termide_agent_core::ModelInfo {
                id: (*id).to_string(),
                name: None,
                context_window: None,
            })
            .collect())
    }
}

/// A provider that answers with the model it was asked for, counting how
/// often it listed its models.
struct Echoes(std::sync::atomic::AtomicUsize);

impl Provider for Echoes {
    fn name(&self) -> &str {
        "echoes"
    }

    fn stream(
        &self,
        request: &termide_agent_core::Request<'_>,
        _: &mut dyn FnMut(termide_agent_core::StreamEvent),
        _: &CancelToken,
    ) -> termide_agent_core::AssistantMessage {
        termide_agent_core::AssistantMessage::failed(
            "echoes",
            request.model.id.clone(),
            termide_agent_core::StopReason::Error,
            "echo",
        )
    }

    fn list_models(&self) -> Result<Vec<termide_agent_core::ModelInfo>, String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(vec![termide_agent_core::ModelInfo {
            id: "first".into(),
            name: None,
            context_window: None,
        }])
    }
}

#[test]
fn a_side_call_with_no_model_named_asks_the_first_listed_once() {
    let inner = Arc::new(Echoes(std::sync::atomic::AtomicUsize::new(0)));
    let provider = FirstListedModel {
        inner: Arc::clone(&inner) as Arc<dyn Provider>,
        model: std::sync::OnceLock::new(),
    };
    let ask = |id: &str| {
        let model = ModelSpec {
            provider: "agent".into(),
            id: id.into(),
            context_window: 0,
            max_tokens: None,
            thinking: ThinkingLevel::Off,
        };
        let request = termide_agent_core::Request {
            model: &model,
            system_prompt: "",
            messages: &[],
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        provider
            .stream(&request, &mut |_| {}, &CancelToken::new())
            .model
    };
    assert_eq!(ask(""), "first");
    assert_eq!(ask(""), "first");
    assert_eq!(inner.0.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(ask("named"), "named");
}

#[test]
fn a_model_left_to_the_provider_is_its_first() {
    let provider = Lists(vec!["first", "second"]);
    assert_eq!(resolve_model(&provider, "").as_deref(), Some("first"));
    assert_eq!(resolve_model(&provider, "named").as_deref(), Some("named"));
    assert_eq!(resolve_model(&Lists(vec![]), ""), None);
}

#[test]
fn plan_mode_holds_a_subagent_whatever_its_own_mode() {
    assert_eq!(subagent_mode(Mode::Plan, Some(Mode::All)), Mode::Plan);
    assert_eq!(subagent_mode(Mode::Plan, None), Mode::Plan);
    assert_eq!(subagent_mode(Mode::Edit, Some(Mode::Ask)), Mode::Ask);
    assert_eq!(subagent_mode(Mode::Auto, None), Mode::Auto);
}

/// With subagents wired, every agent gets `task`, unless its `tools`
/// list leaves it out.
/// A provider that says nothing until its request is stopped, as a long
/// prefill does.
struct Silent;

impl Provider for Silent {
    fn name(&self) -> &str {
        "silent"
    }
    fn stream(
        &self,
        request: &termide_agent_core::Request<'_>,
        _on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage {
        while !cancel.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        AssistantMessage::failed(
            "silent",
            &request.model.id,
            termide_agent_core::StopReason::Aborted,
            "aborted",
        )
    }
}

/// The parent's stop ends a subagent at once, even while its model says
/// nothing.
#[test]
fn a_stopped_parent_stops_its_subagent_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = AgentDirs::new(tmp.path(), Some(tmp.path()), None);
    let settings = with_cloud();
    let mut active = active_on(&settings, "local");
    active.provider = Arc::new(Silent);
    let subagents = Subagents {
        active: Arc::new(std::sync::RwLock::new(active)),
        settings: settings.clone(),
        recall: recall_tool(&settings, &dirs, tmp.path(), tmp.path()),
        web: shared_web(&settings.web, &dirs),
        dirs,
        mcp: Connections::new(std::collections::BTreeMap::new()),
        cwd: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
        session_dir: Some(tmp.path().join("sessions")),
        rules: settings.permissions.clone(),
        mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
        max_tokens: settings.output_limit(),
        reasoning: ThinkingLevel::Off,
        compaction: settings.compaction,
        refusals: Refusals::default(),
    };
    let cancel = CancelToken::new();
    let stopper = {
        let cancel = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            cancel.cancel();
        })
    };
    let started = std::time::Instant::now();
    let result = subagents.run(
        "lead",
        &SubagentTask {
            agent: DEFAULT_AGENT,
            prompt: "look around",
            call: "c1",
            background: false,
        },
        &ToolContext::new(tmp.path().to_path_buf()),
        &cancel,
        &mut |_| {},
    );
    stopper.join().unwrap();
    assert_eq!(result.answer, Err("the subagent was stopped".to_string()));
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    // A stopped task leaves its log too, the task in it as the
    // delegating agent's.
    let logged = Session::open(&result.session.expect("a log")).unwrap();
    let parent = logged.header().parent.clone().unwrap();
    assert_eq!(
        (parent.agent.as_str(), parent.call.as_str()),
        ("lead", "c1")
    );
    assert!(logged.entries().iter().any(|entry| matches!(
        &entry.kind,
        termide_agent_core::EntryKind::Message {
            message: Message::User(user),
            ..
        } if user.plain_text() == "look around"
            && user.origin.as_ref().is_some_and(|o| o.kind == OriginKind::Task)
    )));
    // The task's run is recorded in its own log, so a task cut off by
    // termide quitting reads as unfinished there and offers to carry on.
    let kinds: Vec<_> = logged
        .entries()
        .iter()
        .map(|entry| match entry.kind {
            termide_agent_core::EntryKind::RunStart => "start",
            termide_agent_core::EntryKind::RunEnd { paused: true } => "paused",
            termide_agent_core::EntryKind::RunEnd { paused: false } => "end",
            _ => "",
        })
        .filter(|kind| !kind.is_empty())
        .collect();
    assert_eq!(kinds, vec!["start", "end"]);
}

/// Subagents whose model says nothing until stopped.
fn silent_subagents(root: &Path) -> Arc<Subagents> {
    let dirs = AgentDirs::new(root, Some(root), None);
    let settings = with_cloud();
    let mut active = active_on(&settings, "local");
    active.provider = Arc::new(Silent);
    Arc::new(Subagents {
        active: Arc::new(std::sync::RwLock::new(active)),
        settings: settings.clone(),
        recall: recall_tool(&settings, &dirs, root, root),
        web: shared_web(&settings.web, &dirs),
        dirs,
        mcp: Connections::new(std::collections::BTreeMap::new()),
        cwd: root.to_path_buf(),
        project_root: root.to_path_buf(),
        session_dir: Some(root.join("sessions")),
        rules: settings.permissions.clone(),
        mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
        max_tokens: settings.output_limit(),
        reasoning: ThinkingLevel::Off,
        compaction: settings.compaction,
        refusals: Refusals::default(),
    })
}

/// The user writing while the agent waits on a task moves the task to
/// the background: the call returns, the task runs on in the strip and
/// answers as a message — unless the agents have passed too many
/// messages already, when the call keeps waiting.
#[test]
fn a_waited_task_moves_to_the_background_when_the_user_writes() {
    use termide_panel_agent::live;
    let tmp = tempfile::tempdir().unwrap();
    let subagents = silent_subagents(tmp.path());
    let ctx = ToolContext {
        panel: Some(7301),
        ..ToolContext::new(tmp.path().to_path_buf())
    };
    let waited = |call: &'static str, cancel: CancelToken| {
        let (subagents, ctx) = (Arc::clone(&subagents), ctx.clone());
        let handle = std::thread::spawn(move || {
            subagents.run_detachable(
                "lead",
                &SubagentTask {
                    agent: DEFAULT_AGENT,
                    prompt: "look around",
                    call,
                    background: false,
                },
                &ctx,
                &cancel,
                &mut |_| {},
            )
        });
        let started = std::time::Instant::now();
        while !live::is_running(7301, call) {
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        handle
    };

    // Past the limit of messages between agents: it stays waited for.
    let (reports_tx, reports) = std::sync::mpsc::channel();
    let serving = CancelToken::new();
    let server = peers::serve_for_test(peers::MAX_HOPS, reports_tx.clone(), serving.clone());
    let cancel = CancelToken::new();
    let task = waited("c-kept", cancel.clone());
    assert_eq!(live::ask_aside(7301), 1);
    std::thread::sleep(DETACH_POLL * 4);
    assert!(!task.is_finished(), "the call still waits");
    assert!(live::background_of(7301).is_empty());
    cancel.cancel();
    let outcome = task.join().unwrap();
    assert_eq!(outcome.answer, Err("the subagent was stopped".to_string()));
    serving.cancel();
    server.join().unwrap();

    let serving = CancelToken::new();
    let server = peers::serve_for_test(0, reports_tx, serving.clone());
    let cancel = CancelToken::new();
    let task = waited("c-aside", cancel.clone());
    assert_eq!(live::ask_aside(7301), 1);
    let outcome = task.join().unwrap();
    let words = outcome.answer.unwrap();
    assert!(words.contains("goes on out of sight"), "{words}");
    assert!(words.contains("c-aside"), "{words}");
    let moved: Vec<String> = live::background_of(7301)
        .into_iter()
        .map(|task| task.call)
        .collect();
    assert_eq!(moved, ["c-aside"]);
    // The run's stop no longer reaches it through the call...
    cancel.cancel();
    std::thread::sleep(DETACH_POLL * 2);
    assert!(live::is_running(7301, "c-aside"));
    // ...but a stop of the panel's run does, keeping its answer.
    live::stop_with_run(7301);
    let report = reports
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("its answer goes to the panel");
    assert!(report.contains("look around"), "{report}");
    serving.cancel();
    server.join().unwrap();
}

/// A delegated task stopped by a limit that passes answers nothing: it is
/// parked, with the connection it stopped on and the moment it goes again,
/// so a switch off that connection is what gets it done.
#[test]
fn a_delegate_stopped_by_a_limit_is_parked_rather_than_answered() {
    let panel = 7101;
    let task = SubagentTask {
        agent: "methodical",
        prompt: "cut the release",
        call: "call-1",
        background: false,
    };
    let quota = Failure::from_text("You've hit your session limit · resets 9:50pm");
    let outcome = park_or_answer(
        LimitPolicy::Ask,
        Some(panel),
        &task,
        SubagentOutcome {
            answer: Err("the subagent failed: session limit".into()),
            spent: Usage::default(),
            session: Some(PathBuf::from("/logs/d1.jsonl")),
            parked: false,
            failure: Some(quota),
        },
        "claude-code",
        Some(Path::new("/logs/d1.jsonl")),
    );
    assert!(outcome.parked, "the limit is not the task's answer");
    // What the delegating agent reads says where it waits and that its
    // answer comes later — not a limit to be repeated back.
    let words = outcome.answer.as_deref().unwrap_or_default();
    assert!(words.contains("claude-code"), "{words}");
    assert!(words.contains("Do not re-issue this task"), "{words}");

    let parked = termide_panel_agent::live::parked_of(panel);
    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].connection, "claude-code");
    assert_eq!(parked[0].agent, "methodical");
    assert_eq!(
        parked[0].session,
        Some(PathBuf::from("/logs/d1.jsonl")),
        "it goes on from its own log"
    );
    assert_eq!(parked[0].attempts, 1);
    assert!(parked[0].wake_at > unix_now(), "it waits, not runs");

    // Answered at last: nothing is parked any longer.
    let done = park_or_answer(
        LimitPolicy::Ask,
        Some(panel),
        &task,
        SubagentOutcome {
            answer: Ok("shipped".into()),
            spent: Usage::default(),
            session: Some(PathBuf::from("/logs/d1.jsonl")),
            parked: false,
            failure: None,
        },
        "omlx",
        Some(Path::new("/logs/d1.jsonl")),
    );
    assert_eq!(done.answer.as_deref(), Ok("shipped"));
    assert!(termide_panel_agent::live::parked_of(panel).is_empty());
}

/// `on_limit = "stop"` ends the task as it always did — but with the
/// connection and the reset named, so the agent knows what to do next.
#[test]
fn a_stopped_policy_ends_a_parked_task_with_words_that_point_forward() {
    let panel = 7102;
    let task = SubagentTask {
        agent: "methodical",
        prompt: "cut the release",
        call: "call-2",
        background: false,
    };
    let report = {
        let mut report = SubagentReport::new(4, "claude-code");
        let mut reply = AssistantMessage::failed(
            "m",
            "m",
            termide_agent_core::StopReason::Error,
            "You've hit your usage limit",
        );
        reply.failure = Some(Failure::from_text(
            "You've hit your usage limit · resets 9:50pm",
        ));
        report.said(&reply, &mut |_| {});
        report
    };
    let outcome = park_or_answer(
        LimitPolicy::Stop,
        Some(panel),
        &task,
        report.into_outcome(false),
        "claude-code",
        None,
    );
    assert!(!outcome.parked);
    let words = outcome.answer.unwrap_err();
    assert!(words.contains("claude-code"), "{words}");
    assert!(words.contains("resets"), "{words}");
    assert!(words.contains("switch the connection"), "{words}");
    assert!(termide_panel_agent::live::parked_of(panel).is_empty());
}

/// A failure waiting does not cure — a sign-in, a refused request — is
/// the task's own end: parking it would wait out a moment that never
/// comes.
#[test]
fn a_failure_that_waits_for_nothing_is_not_parked() {
    let panel = 7103;
    let task = SubagentTask {
        agent: "methodical",
        prompt: "cut the release",
        call: "call-3",
        background: false,
    };
    for kind in [FailureKind::Auth, FailureKind::BadRequest] {
        let outcome = park_or_answer(
            LimitPolicy::Wait,
            Some(panel),
            &task,
            SubagentOutcome {
                answer: Err("no".into()),
                spent: Usage::default(),
                session: None,
                parked: false,
                failure: Some(Failure::new(kind)),
            },
            "claude-code",
            None,
        );
        assert!(!outcome.parked, "{kind:?} waits for nothing");
    }
    // A task with no panel of its own has nowhere to wait and nobody to
    // answer later: the limit is all it can say.
    let outcome = park_or_answer(
        LimitPolicy::Wait,
        None,
        &task,
        SubagentOutcome {
            answer: Err("limit".into()),
            spent: Usage::default(),
            session: None,
            parked: false,
            failure: Some(Failure::new(FailureKind::QuotaExhausted)),
        },
        "claude-code",
        None,
    );
    assert_eq!(outcome.answer.unwrap_err(), "limit");
}

/// A task parked too many times is given up rather than left waiting: the
/// delegating agent hears the limit with its reset, and acts.
#[test]
fn a_task_parked_past_its_tries_is_given_up() {
    let panel = 7104;
    let task = SubagentTask {
        agent: "methodical",
        prompt: "cut the release",
        call: "call-4",
        background: false,
    };
    let quota = Failure::new(FailureKind::QuotaExhausted);
    // Each try of a parked task is its own run, and each ends the same way.
    let stopped = || SubagentOutcome {
        answer: Err("the subagent failed: usage limit".into()),
        spent: Usage::default(),
        session: Some(PathBuf::from("/logs/d4.jsonl")),
        parked: false,
        failure: Some(quota),
    };
    for attempt in 1..=MAX_PARK_ATTEMPTS {
        let outcome = park_or_answer(
            LimitPolicy::Wait,
            Some(panel),
            &task,
            stopped(),
            "claude-code",
            Some(Path::new("/logs/d4.jsonl")),
        );
        assert!(outcome.parked, "try {attempt} parks");
        let parked = termide_panel_agent::live::parked_of(panel);
        assert_eq!(parked[0].attempts, attempt);
        // The wait it sleeps by grows with the tries.
        assert!(parked[0].wake_at >= unix_now(), "try {attempt} waits");
    }
    let outcome = park_or_answer(
        LimitPolicy::Wait,
        Some(panel),
        &task,
        stopped(),
        "claude-code",
        Some(Path::new("/logs/d4.jsonl")),
    );
    assert!(!outcome.parked, "past its tries it ends");
    assert!(termide_panel_agent::live::parked_of(panel).is_empty());
}

/// A task held at its panel's pause when termide quits leaves its log
/// standing at a pause: opened again, the session asks to carry it on.
#[test]
fn a_held_task_is_left_paused_in_its_log() {
    let tmp = tempfile::tempdir().unwrap();
    let mut log = DelegateLog::start(
        Some(tmp.path()),
        tmp.path(),
        termide_agent_core::SessionParent {
            session: Some("p1".into()),
            agent: "lead".into(),
            call: "c1".into(),
        },
        &DelegateRun {
            connection: "default",
            provider: "openai",
            model: "gpt",
            context_window: None,
            agent: DEFAULT_AGENT,
        },
    );
    log.note_run_start();
    log.note(&Message::User(UserMessage::text("look around")));
    assert!(
        Session::open_read_only(&log.path().unwrap())
            .unwrap()
            .unfinished_run()
            .is_some(),
        "a run with no end is unfinished"
    );
    log.note_run_end(true);
    let reopened = Session::open_read_only(&log.path().unwrap()).unwrap();
    assert_eq!(reopened.last_run(), termide_agent_core::LastRun::Paused);
}

/// A subagent may take as many model calls as its limit says; the one
/// after is cut off, and its last words say so.
#[test]
fn a_subagent_is_cut_off_past_its_turn_limit() {
    let said = |text: &str| {
        let mut message =
            AssistantMessage::failed("m", "m", termide_agent_core::StopReason::Stop, "");
        message.error_message = None;
        message.content = vec![termide_agent_core::AssistantContent::Text { text: text.into() }];
        message
    };
    let mut report = SubagentReport::new(2, "local");
    assert!(!report.turn());
    assert!(!report.turn());
    report.said(&said("half done"), &mut |_| {});
    assert_eq!(
        report.into_outcome(false).answer.as_deref(),
        Ok("half done")
    );

    let mut report = SubagentReport::new(2, "local");
    for _ in 0..2 {
        report.turn();
    }
    assert!(report.turn());
    report.said(&said("half done"), &mut |_| {});
    assert_eq!(
        report.into_outcome(false).answer.as_deref(),
        Ok("half done\n\n(subagent stopped after 2 steps)")
    );
}

#[test]
fn a_waiting_subagent_says_so_under_what_it_said() {
    let t = termide_i18n::t();
    let mut report = SubagentReport::new(SUBAGENT_MAX_TURNS, "local");
    let mut shown = Vec::new();
    report.waiting(Some(2), &mut |ToolUpdate::Output(text)| shown.push(text));
    assert_eq!(
        shown.last().unwrap(),
        &format!("⏳ {}", t.agent_queued_ahead_fmt(2))
    );
    let said = AssistantMessage::failed("m", "m", termide_agent_core::StopReason::Stop, "");
    let mut said = said;
    said.error_message = None;
    said.content = vec![termide_agent_core::AssistantContent::Text {
        text: "looked".into(),
    }];
    report.said(&said, &mut |ToolUpdate::Output(text)| shown.push(text));
    report.waiting(Some(0), &mut |ToolUpdate::Output(text)| shown.push(text));
    assert_eq!(
        shown.last().unwrap(),
        &format!("looked\n\n⏳ {}", t.agent_queued())
    );
    report.waiting(None, &mut |ToolUpdate::Output(text)| shown.push(text));
    assert_eq!(shown.last().unwrap(), "looked");
}

/// A subagent gets the tools of the MCP servers connected when its task
/// starts, whatever its `tools` list says, and they stay out of its
/// prompt as they do of the panel's.
#[cfg(unix)]
#[test]
fn a_subagent_gets_the_connected_mcp_servers_tools() {
    let script = r#"while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"sh","version":"0"}}}\n' "$id";;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"lookup","description":"Look up","inputSchema":{"type":"object"}}]}}\n' "$id";;
  esac
done"#;
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("ai");
    let dir = global.join("agents").join("search");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(AGENT_FILE), "---\ntools: read\n---\n").unwrap();
    let catalog = FsCatalog::with_global(
        tmp.path(),
        tmp.path(),
        Some(global),
        None,
        Default::default(),
    );
    let servers = [(
        "docs".to_string(),
        termide_agent_core::McpServerConfig {
            command: Some("sh".into()),
            args: vec!["-c".into(), script.into()],
            timeout_secs: 5,
            ..Default::default()
        },
    )]
    .into();
    let mcp = Connections::new(servers);
    // The panel's subscription is what connects the servers.
    let late = mcp.subscribe();
    assert!(matches!(
        late.recv_timeout(std::time::Duration::from_secs(10))
            .unwrap(),
        termide_agent_core::LateTools::Ready { .. }
    ));
    let settings = with_cloud();
    let subagents = Subagents {
        active: Arc::new(std::sync::RwLock::new(active_on(&settings, "local"))),
        settings: settings.clone(),
        dirs: catalog.dirs.clone(),
        recall: recall_tool(&settings, &catalog.dirs, tmp.path(), tmp.path()),
        web: shared_web(&settings.web, &catalog.dirs),
        mcp,
        cwd: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
        session_dir: None,
        rules: settings.permissions.clone(),
        mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
        max_tokens: settings.output_limit(),
        reasoning: ThinkingLevel::Off,
        compaction: settings.compaction,
        refusals: Refusals::default(),
    };
    let definition = catalog.dirs.agent("search");
    let (tools, _, prompt) = subagents.toolset("search", &definition, false);
    assert_eq!(tools.names(), ["read", "docs__lookup"]);
    assert!(!prompt.contains("docs__lookup"));
    // A delegate asks its parent's user only when there is one, and
    // only if its `tools` list keeps `question`.
    let everything = catalog.dirs.agent(DEFAULT_AGENT);
    let asks = |asks| {
        subagents
            .toolset(DEFAULT_AGENT, &everything, asks)
            .0
            .get("question")
            .is_some()
    };
    assert!(asks(true) && !asks(false));
    assert!(subagents
        .toolset("search", &definition, true)
        .0
        .get("question")
        .is_none());
    // An external delegate drives its own tools: none of ours, nor the
    // MCP servers'.
    let outside = tmp.path().join("ai/agents/outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
        outside.join(AGENT_FILE),
        "---\ncommand: my-agent --acp\n---\n",
    )
    .unwrap();
    let external = catalog.dirs.agent("outside");
    let (tools, _, _) = subagents.toolset("outside", &external, true);
    assert!(tools.is_empty(), "{:?}", tools.names());
}

#[test]
fn an_agent_tools_list_governs_the_task_tool() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("ai");
    for (agent, tools) in [("search", "read"), ("lead", "read, task")] {
        let dir = global.join("agents").join(agent);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(AGENT_FILE), format!("---\ntools: {tools}\n---\n")).unwrap();
    }
    let mut catalog = FsCatalog::with_global(
        tmp.path(),
        tmp.path(),
        Some(global),
        None,
        Default::default(),
    );
    let settings = with_cloud();
    catalog.subagents = Some(Arc::new(Subagents {
        active: Arc::new(std::sync::RwLock::new(active_on(&settings, "local"))),
        settings: settings.clone(),
        dirs: catalog.dirs.clone(),
        recall: recall_tool(&settings, &catalog.dirs, tmp.path(), tmp.path()),
        web: shared_web(&settings.web, &catalog.dirs),
        mcp: Arc::clone(&catalog.mcp),
        cwd: tmp.path().to_path_buf(),
        project_root: tmp.path().to_path_buf(),
        session_dir: None,
        rules: settings.permissions.clone(),
        mode: termide_agent_core::ModeHandle::new(settings.permissions.mode),
        max_tokens: settings.output_limit(),
        reasoning: ThinkingLevel::Off,
        compaction: settings.compaction,
        refusals: Refusals::default(),
    }));

    assert_eq!(catalog.resolve("search").unwrap().tools.names(), ["read"]);
    assert_eq!(
        catalog.resolve("lead").unwrap().tools.names(),
        ["read", "task", "jobs"]
    );
    assert!(catalog
        .resolve(DEFAULT_AGENT)
        .unwrap()
        .tools
        .get("task")
        .is_some());
}

#[test]
fn a_background_answer_names_its_task_and_says_how_it_ended() {
    let done = background_report(
        "find the parser\nin src/",
        "search",
        &Ok("in lex.rs".into()).into(),
    );
    assert!(
        done.contains("handed to search") && done.contains("\"find the parser…\""),
        "{done}"
    );
    assert!(done.ends_with("in lex.rs"), "{done}");
    let failed = background_report(
        "x",
        "search",
        &Err("the user stopped the subagent".into()).into(),
    );
    assert!(failed.contains("\"x\"") && failed.contains("It failed: the user stopped"));
}

/// An agent may hand a task to a fresh copy of itself, so a lone
/// `default` agent has someone to delegate to.
#[test]
fn an_agent_can_delegate_to_a_copy_of_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("ai");
    let lone = FsCatalog::with_global(
        tmp.path(),
        tmp.path(),
        Some(global.clone()),
        None,
        Default::default(),
    );
    let names = |catalog: &FsCatalog| -> Vec<String> {
        catalog
            .delegatable()
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    };
    // The lone default agent delegates to a copy of itself.
    assert_eq!(names(&lone), [DEFAULT_AGENT]);

    let dir = global.join("agents").join("search");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(AGENT_FILE), "---\ntools: read\n---\n").unwrap();
    let two = FsCatalog::with_global(
        tmp.path(),
        tmp.path(),
        Some(global),
        None,
        Default::default(),
    );
    // `search` among them, for itself as for `default`.
    assert_eq!(names(&two), [DEFAULT_AGENT, "search"]);

    // An external agent is a delegate too.
    let dir = tmp.path().join("ai").join("agents").join("outside");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(AGENT_FILE), "---\ncommand: my-agent --acp\n---\n").unwrap();
    let three = FsCatalog::with_global(
        tmp.path(),
        tmp.path(),
        Some(tmp.path().join("ai")),
        None,
        Default::default(),
    );
    assert_eq!(names(&three), [DEFAULT_AGENT, "outside", "search"]);
}

/// `AGENT.md`'s front matter narrows the tools and names a model and a mode; a name no
/// root defines does not resolve, the default always does.
#[test]
fn the_catalog_turns_definitions_into_profiles() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("ai");
    let review = global.join("agents/review");
    std::fs::create_dir_all(&review).unwrap();
    std::fs::create_dir_all(global.join("agents/default")).unwrap();
    std::fs::write(
        global.join(DEFAULT_AGENT_FILE),
        "Root template.\n\n{{tools}}\n",
    )
    .unwrap();
    std::fs::write(
        review.join(AGENT_FILE),
        "---\ndescription: Reviews diffs\nmodel: big\nmode: auto\ntools: read, bash, nope\n---\n\
             You review.\n\n{{tools}}\n",
    )
    .unwrap();
    let catalog = FsCatalog::with_global(
        tmp.path(),
        tmp.path(),
        Some(global.clone()),
        None,
        Default::default(),
    );

    let entries = catalog.list();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["default", "review"]);
    // The default agent, defined nowhere, is described by the panel's label.
    assert_eq!(entries[0].description, termide_i18n::t().panel_agent());
    assert_eq!(entries[1].description, "Reviews diffs");
    let review = catalog.resolve("review").unwrap();
    assert!(review.system_prompt.starts_with("You review.\n\n- read:"));
    assert_eq!(review.tools.names(), ["read", "shell"]);
    assert_eq!(review.model.as_deref(), Some("big"));
    assert_eq!(review.mode, Some(termide_agent_core::Mode::Auto));

    let default = catalog.resolve(DEFAULT_AGENT).unwrap();
    // The built-in four and `terminals`, plus the two that speak to the
    // user watching the panel, `question` and the `suggest_*` cards, and
    // the two that speak to the agents of the other panels, `fork`, and
    // the debugger.
    assert_eq!(
        default.tools.names(),
        [
            "read",
            "edit",
            "write",
            "shell",
            "terminals",
            "question",
            "suggest_command",
            "suggest_goal",
            "suggest_loop",
            "agents",
            "send_message",
            "fork",
            "debug",
            "jobs"
        ]
    );
    assert!(default
        .system_prompt
        .starts_with("Root template.\n\n- read:"));
    assert!(default.model.is_none());
    assert!(catalog.resolve("missing").is_none());

    // A skill adds the `skill` tool, to every agent, and a prompt line.
    let skill = global.join("skills/deploy");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: deploy\ndescription: Ship it\n---\nSteps.\n",
    )
    .unwrap();
    std::fs::write(global.join(DEFAULT_AGENT_FILE), "{{skills}}\n").unwrap();
    let review = catalog.resolve("review").unwrap();
    assert_eq!(review.tools.names(), ["read", "shell", "skill"]);

    // A `command` makes an external agent: no tools of ours, a backend.
    let outside = global.join("agents/outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(
            outside.join(AGENT_FILE),
            "---\ndescription: Claude Code\ncommand: npx -y @agentclientprotocol/claude-agent-acp\n---\n",
        )
        .unwrap();
    let outside = catalog.resolve("outside").unwrap();
    assert!(outside.backend.is_some());
    assert!(outside.tools.is_empty());
    assert!(outside.late_tools.is_none());
    let default = catalog.resolve(DEFAULT_AGENT).unwrap();
    assert_eq!(default.system_prompt, "- deploy: Ship it\n");
}
