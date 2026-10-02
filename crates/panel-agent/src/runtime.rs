//! Building the agent behind the panel: the session it opens or resumes,
//! the agent profile and model it runs with, and the runtime that drives it.

use std::collections::BTreeSet;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use termide_agent_core::{
    permission_channel, question_channel, suggestion_channel, Agent, AgentRuntime, Backend,
    BackendSetup, CancelToken, ChainedHooks, CheckpointHooks, CheckpointStore, CompactionPolicy,
    CompactionPrompts, GoalPrompt, HandoffPrompt, Hooks, HostTools, LoggedMessage, Message, Mode,
    ModeHandle, ModelInfo, ModelSpec, PermissionEnvelope, PermissionHooks, PermissionRules,
    PersistRule, PlanGuard, PlanPrompt, Provider, QuestionEnvelope, Refusals, ReviewerSetup,
    Session, ShellRunner, SuggestionEnvelope, Timing, ToolRegistry,
};

use crate::toolset::{Blocked, ToolsetGuard};
use crate::{
    transcript, AgentCatalog, AgentProfile, BackendFactory, FoldMode, HooksFactory, Item,
    NoticeKind, PersistFn, Transcript,
};

/// Start a background `list_models` call, returning the receiver to poll from
/// `tick()`. Used both by the model picker and the silent context-window probe.
pub(crate) fn spawn_model_list(
    provider: Arc<dyn Provider>,
) -> Receiver<Result<Vec<ModelInfo>, String>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(provider.list_models());
    });
    rx
}

/// The checkpoint store of `session`, under the session directory.
pub(crate) fn checkpoint_store(
    session_dir: Option<&std::path::Path>,
    session: Option<&Session>,
) -> Option<Arc<Mutex<CheckpointStore>>> {
    let dir = session_dir?;
    let session = session?;
    Some(Arc::new(Mutex::new(CheckpointStore::for_session(
        dir,
        session.id(),
    ))))
}

/// Create a session log in `dir` and record the model it starts on, so a
/// later resume comes back on the same model.
pub(crate) fn start_session(
    dir: Option<&std::path::Path>,
    cwd: &std::path::Path,
    connection: &str,
    provider: &str,
    model: &ModelSpec,
    agent: &str,
) -> Option<Session> {
    let mut session = match Session::create_exclusive(dir?, cwd) {
        Ok(session) => session,
        Err(error) => {
            log::warn!("cannot start an agent session log: {error}");
            return None;
        }
    };
    // The connection first: a reopened session goes back to it, and the
    // model after it is that connection's.
    if !connection.is_empty() {
        if let Err(error) = session.append_connection_change(connection) {
            log::warn!("agent session write failed: {error}");
        }
    }
    if let Err(error) = session.append_model_change(provider, &model.id, Some(model.context_window))
    {
        log::warn!("agent session write failed: {error}");
    }
    if let Err(error) = session.append_agent_change(agent) {
        log::warn!("agent session write failed: {error}");
    }
    Some(session)
}

/// What `session` switched off, and the profile to run it with when that
/// differs from what is running: another agent than `current`, or another
/// set switched off than `built_without` (what the running profile was built
/// without). `None` keeps the running profile.
pub(crate) fn session_agent(
    catalog: &dyn AgentCatalog,
    current: &str,
    built_without: &BTreeSet<String>,
    session: Option<&Session>,
) -> (BTreeSet<String>, Option<(String, AgentProfile)>) {
    let off: BTreeSet<String> = session
        .and_then(Session::current_toolset)
        .unwrap_or_default()
        .into_iter()
        .collect();
    let recorded = session
        .and_then(Session::current_agent)
        .filter(|name| name != current);
    if recorded.is_none() && off == *built_without {
        return (off, None);
    }
    let name = recorded.unwrap_or_else(|| current.to_string());
    match catalog.resolve_without(&name, &off) {
        Some(profile) => (off, Some((name, profile))),
        None => {
            log::warn!("session ran as agent {name}, which no longer exists; using {current}");
            (off, None)
        }
    }
}

/// The configured model with the id and context window `session` last ran
/// on, when it recorded them: a resumed conversation continues on its own
/// model.
pub(crate) fn session_model(configured: &ModelSpec, session: Option<&Session>) -> ModelSpec {
    let mut model = match session.and_then(Session::current_model) {
        Some(recorded) if !recorded.id.is_empty() => ModelSpec {
            id: recorded.id,
            context_window: recorded.context_window.unwrap_or(configured.context_window),
            ..configured.clone()
        },
        _ => configured.clone(),
    };
    // A reasoning level picked in this session (the status-bar chip)
    // outlives a resume, overriding the configured default.
    if let Some(level) = session.and_then(Session::current_thinking) {
        model.thinking = level;
    }
    model
}

/// What [`spawn_runtime`] hands back.
pub(crate) struct Spawned {
    pub(crate) runtime: Box<dyn Backend>,
    pub(crate) permission_rx: Receiver<PermissionEnvelope>,
    pub(crate) question_rx: Receiver<QuestionEnvelope>,
    /// Commands the `suggest_command` tool offers, awaiting the user's card.
    pub(crate) suggestion_rx: Receiver<SuggestionEnvelope>,
    pub(crate) transcript: Transcript,
    pub(crate) mode: ModeHandle,
    pub(crate) external: bool,
}

/// Spawn the agent — the built-in loop on a worker thread, or the external
/// agent `backend` makes — with its permission channel, a transcript
/// mirroring `session`'s history and the live mode handle. An external agent
/// that cannot start is reported in the transcript and the built-in loop
/// runs instead.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_runtime(
    provider: &Arc<dyn Provider>,
    tools: &ToolRegistry,
    model: &ModelSpec,
    cwd: &std::path::Path,
    system_prompt: &str,
    rules: PermissionRules,
    compaction: CompactionPolicy,
    compaction_prompts: &CompactionPrompts,
    plan_prompt: &PlanPrompt,
    goal_prompt: &GoalPrompt,
    handoff_prompt: &HandoffPrompt,
    reviewer: &ReviewerSetup,
    refusals: &Refusals,
    persist_rule: Option<PersistFn>,
    extra_hooks: Option<&HooksFactory>,
    backend: Option<&BackendFactory>,
    checkpoints: Option<Arc<Mutex<CheckpointStore>>>,
    fold: FoldMode,
    session: Option<&Session>,
    blocked: &Blocked,
    shell_run: Option<ShellRunner>,
) -> Spawned {
    let cancel = CancelToken::new();
    let (prompter, permission_rx) = permission_channel(cancel.clone());
    // The `question` tool asks through this; an external agent asks its own
    // way, so its asker is dropped and nothing ever arrives.
    let (asker, question_rx) = question_channel(cancel.clone());
    // The `suggest_command` tool offers commands through this, and waits for
    // the card; like the asker, an external agent has no use for it.
    let (suggester, suggestion_rx) = suggestion_channel(cancel.clone());
    let system_prompt = if rules.mode == Mode::Plan {
        plan_prompt.apply(system_prompt)
    } else {
        system_prompt.to_string()
    };
    let system_prompt = system_prompt.as_str();
    // The external backend, if one is used, is handed the same rules so its
    // permission requests get the built-in agent's treatment (read-only
    // commands and matching rules pass without a prompt).
    let backend_rules = rules.clone();
    let mut hooks = PermissionHooks::new(rules, Box::new(prompter))
        .with_classifier(Box::new(reviewer.classifier(cancel.clone())))
        .with_refusals(refusals.clone());
    let mode = hooks.mode_handle();
    if let Some(persist) = persist_rule {
        hooks = hooks.with_persist(Box::new(persist) as PersistRule);
    }
    // The chain every call of termide's tools runs through, before the
    // permission decision: what the session switched off goes first, refused
    // whatever else would allow it; then plan mode's guard, so nothing — not
    // even a hook's approval — changes a file while it is on; then the
    // checkpoint recorder, so no call that runs is missed; then the command
    // hooks, which may block or approve before anyone is asked, and whose
    // rewritten arguments are what the rules then judge.
    let guards = |checkpoints: Option<Arc<Mutex<CheckpointStore>>>| {
        let mut chain: Vec<Box<dyn Hooks>> = vec![
            Box::new(ToolsetGuard {
                blocked: Arc::clone(blocked),
            }),
            Box::new(PlanGuard::new(mode.clone()).with_refusals(refusals)),
        ];
        if let Some(store) = checkpoints {
            chain.push(Box::new(CheckpointHooks::new(store)));
        }
        if let Some(factory) = extra_hooks {
            chain.push(factory());
        }
        chain
    };

    let mut transcript = Transcript::default();
    transcript.set_fold(fold);
    let history = session
        .map(|s| s.context_messages_with_times(compaction_prompts))
        .unwrap_or_default();
    for logged in &history {
        push_history(&mut transcript, logged);
    }
    let messages: Vec<Message> = history.into_iter().map(|logged| logged.message).collect();

    if let Some(factory) = backend {
        // The external agent gets its own prompter on a channel of its own,
        // and the same rules, so it builds permission hooks that decide its
        // requests exactly as the built-in agent's do.
        let (external_prompter, external_rx) = permission_channel(cancel.clone());
        // termide's tools, for an agent that calls them in place of its own:
        // each call runs through the built-in loop's chain, asking on the
        // same channel under the same live mode.
        let mut host_permissions =
            PermissionHooks::new(backend_rules.clone(), Box::new(external_prompter.clone()))
                .with_mode_handle(mode.clone())
                .with_refusals(refusals.clone());
        if let Some(persist) = persist_rule {
            host_permissions = host_permissions.with_persist(Box::new(persist) as PersistRule);
        }
        let mut host_chain = guards(checkpoints.clone());
        host_chain.push(Box::new(host_permissions));
        match factory(BackendSetup {
            cwd: cwd.to_path_buf(),
            prompter: external_prompter,
            cancel: cancel.clone(),
            rules: backend_rules,
            persist: persist_rule.map(|f| Box::new(f) as PersistRule),
            mode: mode.clone(),
            system_prompt: system_prompt.to_string(),
            host_tools: Some(HostTools {
                tools: tools.clone(),
                hooks: Box::new(ChainedHooks::new(host_chain)),
            }),
        }) {
            Ok(runtime) => {
                if !messages.is_empty() {
                    transcript.push(Item::Notice {
                        text: termide_i18n::t().agent_notice_external_history().into(),
                        kind: NoticeKind::Info,
                    });
                }
                return Spawned {
                    runtime,
                    permission_rx: external_rx,
                    question_rx,
                    suggestion_rx,
                    transcript,
                    mode,
                    external: true,
                };
            }
            Err(error) => transcript.push(Item::Notice {
                text: termide_i18n::t().agent_notice_external_failed_fmt(&error.to_string()),
                kind: NoticeKind::Error,
            }),
        }
    }

    let mut agent = Agent::new(
        Arc::clone(provider),
        tools.clone(),
        model.clone(),
        cwd.to_path_buf(),
    )
    .with_system_prompt(system_prompt)
    .with_compaction(compaction)
    .with_compaction_prompts(compaction_prompts.clone())
    .with_goal_prompt(goal_prompt.clone())
    .with_handoff_prompt(handoff_prompt.clone())
    .with_messages(messages)
    .with_asker(asker)
    .with_suggester(suggester);
    if let Some(run) = shell_run {
        // A command the user confirms on the card runs through this, in the
        // middle of the call that asked.
        agent = agent.with_shell_run(run);
    }
    let mut chain = guards(checkpoints);
    chain.push(Box::new(hooks));
    let hooks: Box<dyn Hooks> = Box::new(ChainedHooks::new(chain));
    let runtime = AgentRuntime::spawn_with_cancel(agent, hooks, cancel);
    Spawned {
        runtime: Box::new(runtime),
        permission_rx,
        question_rx,
        suggestion_rx,
        transcript,
        mode,
        external: false,
    }
}

/// Mirror a session's message into transcript items when a session is
/// reopened. The log's timestamp restores when each block was written; its
/// recorded timing, with the turn's own token usage, restores the `⏫`/`✍️`
/// cost and a tool's `🕒` duration, as the live run showed them.
pub(crate) fn push_history(transcript: &mut Transcript, logged: &LoggedMessage) {
    let at = hms_from_millis(logged.timestamp);
    match &logged.message {
        // A command the user ran is the shell call block it was; a template
        // or skill expansion heads with its `/name`.
        Message::User(user) => {
            let duration_ms = match logged.timing {
                Some(Timing::Tool { duration_ms, .. }) => Some(duration_ms),
                _ => None,
            };
            let item =
                transcript::user_command_item(user, at.clone(), duration_ms).unwrap_or_else(|| {
                    Item::User {
                        text: user.plain_text(),
                        at,
                        command: user.command.clone(),
                    }
                });
            transcript.push(item);
        }
        Message::Assistant(assistant) => {
            let cost = match logged.timing {
                Some(Timing::Turn { prefill_ms, gen_ms }) => Some(transcript::Cost {
                    prefill_ms,
                    gen_ms,
                    input: assistant.usage.input,
                    output: assistant.usage.output,
                }),
                _ => None,
            };
            // Reasoning is restored as its own block above the tools and answer.
            // When it is present it carries the turn's cost, and the answer is
            // left with its time alone, matching a live turn.
            let thinking = assistant.thinking_text();
            let has_thinking = !thinking.trim().is_empty();
            if has_thinking {
                transcript.push(Item::Thinking {
                    text: thinking,
                    streaming: false,
                    at: at.clone(),
                    cost,
                });
            }
            for call in assistant.tool_calls() {
                transcript.push(Item::Tool {
                    call: call.clone(),
                    result: None,
                    live: None,
                    at: at.clone(),
                    duration_ms: None,
                    waited_ms: None,
                    waiting: false,
                });
            }
            // The answer keeps its wall-clock time; skip an empty answer block
            // when the reasoning already stands for the turn (a tool-only turn),
            // so no phantom block is left behind.
            let answer = assistant.plain_text();
            let error = assistant.error_message.clone();
            if !answer.trim().is_empty() || !has_thinking || error.is_some() {
                transcript.push(Item::Assistant {
                    text: answer,
                    streaming: false,
                    error,
                    at,
                    cost: if has_thinking { None } else { cost },
                    run_ms: None,
                });
            }
        }
        Message::ToolResult(result) => {
            let id = result.tool_call_id.clone();
            let (elapsed, wait) = match logged.timing {
                Some(Timing::Tool {
                    duration_ms,
                    waited_ms,
                }) => (Some(duration_ms), waited_ms),
                _ => (None, None),
            };
            transcript.with_tool(&id, |item| {
                if let Item::Tool {
                    result: slot,
                    at: tool_at,
                    duration_ms,
                    waited_ms,
                    ..
                } = item
                {
                    *slot = Some(result.clone());
                    *tool_at = at;
                    *duration_ms = elapsed;
                    *waited_ms = wait;
                }
            });
        }
    }
}

/// Format an epoch-millis timestamp as the local `YYYY-MM-DD HH:MM`, how a
/// list of sessions dates each one by its last change.
pub(crate) fn local_minute(ms: u64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms as i64) {
        chrono::offset::LocalResult::Single(dt) => dt.format("%Y-%m-%d %H:%M").to_string(),
        _ => termide_agent_core::civil_date(ms),
    }
}

/// Format an epoch-millis timestamp as the local `HH:MM:SS`, matching
/// [`now_hms`] so restored blocks read the same as live ones.
pub(crate) fn hms_from_millis(ms: u64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms as i64) {
        chrono::offset::LocalResult::Single(dt) => dt.format("%H:%M:%S").to_string(),
        _ => String::new(),
    }
}
