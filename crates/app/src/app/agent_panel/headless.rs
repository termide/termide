//! The headless agent: `termide --prompt`, one task run without the UI, its
//! answer on stdout.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use termide_agent_acp::AcpRuntime;
use termide_agent_core::{
    build_system_prompt, discover_context_files, ensure_global_layout, permission_channel,
    subject_of, Agent, AgentDirs, AgentEvent, AutoDenyPrompter, Backend, BackendSetup, CancelToken,
    HostTools, IntentLog, Message, Mode, ModeHandle, ModelSpec, PermissionAnswer,
    PermissionEnvelope, PermissionHooks, PromptOptions, SessionView, StopReason, StreamEvent,
    ToolContext, Usage, UserMessage, DEFAULT_AGENT, GLOBAL_AGENT_DIR,
};
use termide_agent_tools::SkillTool;
use termide_config::AiSettings;

use super::{
    base_tools, cli_acp_config, connection_provider, recall_tool, resolve_model, restrict_tools,
    reviewer_setup, shared_web, usable_connection,
};

/// How a headless run reports its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessOutput {
    /// The answer streamed to stdout, tool activity to stderr.
    Text,
    /// One JSON object printed at the end: answer, usage, tool calls, status.
    Json,
    /// One JSON object per event (NDJSON): a `tool_use`/`tool_result` per
    /// tool, a `message` per assistant turn, a final `result`.
    StreamJson,
}

/// The most of a tool's output a `tool_result` event carries: enough to see
/// why a call failed or what it found, not a whole file read into the log.
const STREAM_OUTPUT_CHARS: usize = 2000;

/// How much reasoning a `thinking` event gathers before it is written: the
/// reasoning reaches the log while the model still writes it, so a call cut
/// off from outside (a timeout that kills the process) leaves it behind.
const THINKING_CHUNK_CHARS: usize = 4000;

/// Gathers streamed text into events of whole lines: the reasoning, or,
/// `raw`, a call's JSON arguments, one line cut anywhere.
#[derive(Default)]
struct ThinkingChunks {
    pending: String,
    raw: bool,
}

impl ThinkingChunks {
    /// Add `delta`; the lines gathered so far once they reach
    /// [`THINKING_CHUNK_CHARS`] (all of it, cut mid-line, once a line runs
    /// four times that long).
    fn push(&mut self, delta: &str) -> Option<String> {
        self.pending.push_str(delta);
        if self.pending.len() < THINKING_CHUNK_CHARS {
            return None;
        }
        let end = match self.pending.rfind('\n') {
            _ if self.raw => self.pending.len(),
            Some(at) => at + 1,
            None if self.pending.len() >= 4 * THINKING_CHUNK_CHARS => self.pending.len(),
            None => return None,
        };
        let rest = self.pending.split_off(end);
        Some(std::mem::replace(&mut self.pending, rest))
    }

    /// What is left, at the end of a call.
    fn finish(&mut self) -> Option<String> {
        let rest = std::mem::take(&mut self.pending);
        (!rest.is_empty()).then_some(rest)
    }
}

/// `text` cut to `max` characters, with how much was left out.
fn capped(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        None => text.to_string(),
        Some((cut, _)) => format!(
            "{}… [{} more characters]",
            &text[..cut],
            text[cut..].chars().count()
        ),
    }
}

/// The token usage of every assistant turn of the run, added up.
fn total_usage(messages: &[Message]) -> Usage {
    let mut total = Usage::default();
    for message in messages {
        if let Message::Assistant(assistant) = message {
            total.add(&assistant.usage);
        }
    }
    total
}

fn usage_json(usage: &Usage) -> serde_json::Value {
    serde_json::json!({
        "input": usage.input,
        "output": usage.output,
        "cache_read": usage.cache_read,
        "cache_write": usage.cache_write,
    })
}

/// Run one agent task without the UI and stream the answer to stdout, for
/// scripting and CI: `termide --prompt "..."`. Text goes to stdout, tool
/// activity and errors to stderr. There is no one to answer a permission
/// prompt, so it runs under the configured rules and mode with everything
/// else refused (as a subagent does); set `mode = "auto"` to have the
/// reviewer decide, `mode = "all"` or allow rules for unattended use.
///
/// On a CLI-agent connection (`claude_code`, `codex`, `gemini_cli`), or for
/// an agent whose `AGENT.md` names a `command`, the task runs in that
/// agent's own loop over ACP, served termide's prompt and tools as in a
/// panel, and reported the same way.
/// Returns the process exit code.
pub fn run_agent_headless(
    settings: &AiSettings,
    shell: Option<&str>,
    cwd: &Path,
    project_root: &Path,
    agent_name: Option<&str>,
    prompt: &str,
    output: HeadlessOutput,
) -> i32 {
    let Some((connection_name, connection)) = usable_connection(settings) else {
        eprintln!("termide: AI is not configured (add an [ai.connections] entry)");
        return 1;
    };

    let global = termide_config::get_config_dir()
        .ok()
        .map(|dir| dir.join(GLOBAL_AGENT_DIR));
    if let Some(global) = &global {
        let _ = ensure_global_layout(global);
    }
    let dirs = AgentDirs::new(cwd, Some(project_root), global.as_deref())
        .with_home(dirs::home_dir().as_deref());
    let name = agent_name.unwrap_or(DEFAULT_AGENT);
    if agent_name.is_some_and(|n| n != DEFAULT_AGENT && !dirs.agents().iter().any(|a| a == n)) {
        eprintln!("termide: no agent named {name}");
        return 2;
    }
    let definition = dirs.agent(name);
    // An agent of its own `command` runs whatever the connection, on its
    // `model` as in a panel; else a CLI-agent connection runs its adapter.
    let external = definition
        .spec
        .acp
        .clone()
        .map(|config| {
            let model = definition.spec.model.clone().unwrap_or_default();
            (name.to_string(), config, model)
        })
        .or_else(|| {
            cli_acp_config(connection).map(|config| {
                let model = definition
                    .spec
                    .model
                    .clone()
                    .unwrap_or_else(|| connection.model.trim().to_string());
                (format!("{connection_name}:{name}"), config, model)
            })
        });

    super::publish_shell(shell);
    let web = shared_web(&settings.web, &dirs);
    let recall = recall_tool(settings, &dirs, project_root, cwd);
    let mut tools = base_tools(&dirs, Some(&web), Some(&recall), false);
    restrict_tools(&mut tools, &definition.spec.tools, name);
    let skills = dirs.model_skills();
    if !skills.is_empty() {
        tools.insert(Arc::new(SkillTool::new(skills.clone())));
    }
    termide_agent_core::apply_tool_texts(&mut tools, &dirs.tool_texts());
    let context_files = discover_context_files(cwd, Some(project_root), None);
    let mut options = PromptOptions::new(cwd, &tools, &context_files);
    options.skills = &skills;
    options.soul = definition.soul.as_deref();
    let system_prompt = build_system_prompt(&options);

    let mut rules = settings.permissions.clone();
    if let Some(mode) = definition.spec.mode {
        rules.mode = mode;
    }
    // Plan mode is a UI affordance (it waits for a card); headless has no
    // one to accept a plan, so the default mode decides instead.
    if rules.mode == Mode::Plan {
        eprintln!("termide: plan mode has no meaning without the panel; using auto");
        rules.mode = Mode::Auto;
    }

    let cancel = CancelToken::new();
    let refusals = dirs.refusals();
    let reviewer = reviewer_setup(settings, &dirs, name);
    let permissions = PermissionHooks::new(
        rules.clone(),
        Box::new(AutoDenyPrompter::new(refusals.unattended_headless.clone())),
    )
    .with_classifier(Box::new(reviewer.classifier(cancel.clone())))
    .with_refusals(refusals.clone());
    // The approved command hooks run around the calls as in a panel; a
    // project's hook never approved there does not run here either.
    let approvals = termide_agent_core::approvals::Approvals::new(
        termide_project::approvals_path(project_root).ok(),
    );
    let mut chain: Vec<Box<dyn termide_agent_core::Hooks>> = Vec::new();
    if let Some(factory) = super::command_hooks(&dirs, &approvals, cwd) {
        chain.push(factory());
    }
    chain.push(Box::new(permissions));
    let mut hooks = termide_agent_core::ChainedHooks::new(chain);

    let mut reporter = Reporter::new(output, cwd);
    if let Some((agent, config, model)) = external {
        let setup = BackendSetup {
            cwd: cwd.to_path_buf(),
            // The agent's own permission requests the rules leave open come
            // on this channel, and are refused: nobody watches.
            prompter: {
                let (prompter, rx) = permission_channel(cancel.clone());
                reporter.refused = Some((rx, refusals.unattended_headless.clone()));
                prompter
            },
            cancel: cancel.clone(),
            mode: ModeHandle::new(rules.mode),
            rules,
            persist: None,
            system_prompt,
            plan: dirs.plan_prompt(),
            plan_seen: false,
            goal: dirs.goal_prompt(),
            handoff: dirs.handoff_prompt(),
            reviewer,
            host_tools: Some(HostTools {
                tools,
                hooks: Box::new(hooks),
                context: ToolContext::new(cwd.to_path_buf()),
                skills,
                session_id: None,
            }),
            resume: None,
            fork: None,
            history: Vec::new(),
        };
        let runtime = match AcpRuntime::start(&agent, &config, setup) {
            Ok(runtime) => runtime,
            Err(error) => {
                eprintln!("termide: {error}");
                return 1;
            }
        };
        let messages = drive_external(&runtime, &model, prompt, &mut reporter);
        return reporter.finish(&messages);
    }

    let provider = connection_provider(connection_name, connection);
    let requested = definition
        .spec
        .model
        .clone()
        .unwrap_or_else(|| connection.model.clone());
    let Some(id) = resolve_model(provider.as_ref(), &requested) else {
        eprintln!("termide: the provider lists no model; name one in the connection");
        return 1;
    };
    let model = ModelSpec {
        provider: "agent".to_string(),
        id,
        context_window: connection.effective_context_window(),
        max_tokens: settings.output_limit(),
        thinking: settings.reasoning,
        reasoning_budget: connection.reasoning_budget(),
    };
    let mut agent = Agent::new(Arc::clone(&provider), tools, model, cwd.to_path_buf())
        .with_system_prompt(system_prompt)
        .with_compaction(settings.compaction)
        .with_compaction_prompts(dirs.compaction_prompts());
    agent.run(
        UserMessage::text(prompt),
        &mut hooks,
        &cancel,
        &mut |event| reporter.event(event),
    );
    reporter.finish(agent.messages())
}

/// How often the run of an external agent is looked at.
const EXTERNAL_POLL: Duration = Duration::from_millis(50);

/// Pick `model` (empty for the agent's own), hand the external agent the
/// task and report what it does until its turn ends; the messages of the
/// run, as the built-in loop's transcript holds them.
fn drive_external(
    backend: &AcpRuntime,
    model: &str,
    prompt: &str,
    reporter: &mut Reporter,
) -> Vec<Message> {
    // The model goes before the task does, so the whole task runs on it.
    if !model.is_empty() {
        while backend.is_starting() {
            std::thread::sleep(EXTERNAL_POLL);
        }
        let models = backend.available_models();
        match termide_agent_acp::match_model(model, &models) {
            Some(id) => {
                if let Err(error) = backend.select_model(id) {
                    eprintln!("termide: cannot pick {model}: {error:?}; the agent's default runs");
                }
            }
            None if !models.is_empty() => {
                eprintln!("termide: the agent has no model {model}; its default runs");
            }
            None => {}
        }
    }
    let mut messages = vec![Message::User(UserMessage::text(prompt))];
    if let Err(error) = backend.prompt(UserMessage::text(prompt)) {
        eprintln!("termide: the agent did not start: {error:?}");
        return messages;
    }
    // An agent that went away without saying its turn ended is over too.
    let mut idle = 0;
    loop {
        reporter.refuse_requests();
        let mut ended = false;
        for event in backend.drain() {
            if let AgentEvent::MessageEnd(message) = &event {
                messages.push(message.clone());
            }
            ended |= matches!(event, AgentEvent::AgentEnd);
            reporter.event(event);
        }
        idle = if backend.is_busy() { 0 } else { idle + 1 };
        if ended || idle > 1 {
            return messages;
        }
        std::thread::sleep(EXTERNAL_POLL);
    }
}

/// Reports a run as it goes, in the form `--output` asked for, and its
/// result at the end: the same for termide's loop and an external agent.
struct Reporter {
    quiet: bool,
    stream: bool,
    cwd: std::path::PathBuf,
    wrote_text: bool,
    /// Tool calls in call order: (id, name, subject, is_error), for the
    /// JSON report and, in text mode, the stderr activity lines.
    tools: Vec<(String, String, String, bool)>,
    /// Every stream event carries the time since the run began, so a slow
    /// turn or a long tool call shows in the log.
    started: std::time::Instant,
    thinking: ThinkingChunks,
    drafts: std::collections::HashMap<String, (String, ThinkingChunks)>,
    /// An external agent's permission requests, refused with the reason.
    refused: Option<(Receiver<PermissionEnvelope>, String)>,
}

impl Reporter {
    fn new(output: HeadlessOutput, cwd: &Path) -> Self {
        Self {
            // Both JSON forms suppress the plain text/stderr chatter.
            quiet: output != HeadlessOutput::Text,
            stream: output == HeadlessOutput::StreamJson,
            cwd: cwd.to_path_buf(),
            wrote_text: false,
            tools: Vec::new(),
            started: std::time::Instant::now(),
            thinking: ThinkingChunks::default(),
            drafts: std::collections::HashMap::new(),
            refused: None,
        }
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn line(&self, mut value: serde_json::Value) {
        use std::io::Write;
        value["elapsed_ms"] = serde_json::json!(self.elapsed_ms());
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    }

    fn flush_thinking(&mut self) {
        if let Some(text) = self.thinking.finish() {
            self.line(serde_json::json!({ "type": "thinking", "text": text }));
        }
    }

    fn refuse_requests(&self) {
        if let Some((rx, reason)) = &self.refused {
            for envelope in rx.try_iter() {
                let _ = envelope
                    .reply
                    .send(PermissionAnswer::DenyWithReason(reason.clone()).into());
            }
        }
    }

    fn event(&mut self, event: AgentEvent) {
        let stream = self.stream;
        match event {
            AgentEvent::MessageUpdate(StreamEvent::TextDelta(text)) => {
                if !self.quiet {
                    use std::io::Write;
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(text.as_bytes());
                    let _ = out.flush();
                    self.wrote_text = true;
                }
            }
            AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(delta)) if stream => {
                if let Some(text) = self.thinking.push(&delta) {
                    self.line(serde_json::json!({ "type": "thinking", "text": text }));
                }
            }
            // A call's arguments show only when they run long — a big file
            // written, or a model that does not stop — while they stream;
            // the end of a short one is dropped, its subject says enough.
            AgentEvent::MessageUpdate(StreamEvent::ToolCallStart { id, name }) if stream => {
                let chunks = ThinkingChunks {
                    raw: true,
                    ..ThinkingChunks::default()
                };
                self.drafts.insert(id, (name, chunks));
            }
            AgentEvent::MessageUpdate(StreamEvent::ToolCallDelta { id, arguments }) if stream => {
                let pushed = self
                    .drafts
                    .get_mut(&id)
                    .and_then(|(name, chunks)| Some((name.clone(), chunks.push(&arguments)?)));
                if let Some((name, text)) = pushed {
                    self.line(
                        serde_json::json!({ "type": "tool_input", "name": name, "text": text }),
                    );
                }
            }
            AgentEvent::MessageUpdate(StreamEvent::ToolCallEnd { id }) if stream => {
                self.drafts.remove(&id);
            }
            AgentEvent::ThinkingLoop { report, retrying } if stream => {
                self.flush_thinking();
                self.line(serde_json::json!({
                    "type": "loop",
                    "retrying": retrying,
                    "repeats": report.repeats,
                    "thoughts": report.thoughts,
                }));
            }
            AgentEvent::EmptyCutOff { retrying } if stream => {
                self.line(serde_json::json!({ "type": "cut_off", "retrying": retrying }));
            }
            AgentEvent::MalformedToolCall if stream => {
                self.flush_thinking();
                self.line(serde_json::json!({ "type": "malformed_call" }));
            }
            AgentEvent::ReasoningBudget { budget, retrying } if stream => {
                self.flush_thinking();
                self.line(serde_json::json!({
                    "type": "reasoning_budget",
                    "budget": budget,
                    "retrying": retrying,
                }));
            }
            AgentEvent::MessageEnd(Message::Assistant(message)) if stream => {
                self.flush_thinking();
                let text = message.plain_text();
                if !text.trim().is_empty() {
                    self.line(serde_json::json!({ "type": "message", "text": text }));
                }
            }
            AgentEvent::ToolExecutionStart { call } => {
                let subject = subject_of(&call, &ToolContext::new(self.cwd.clone()));
                if !self.quiet {
                    if subject.is_empty() {
                        eprintln!("· {}", call.name);
                    } else {
                        eprintln!("· {} {subject}", call.name);
                    }
                }
                if stream {
                    self.line(serde_json::json!({
                        "type": "tool_use",
                        "name": call.name,
                        "subject": subject,
                    }));
                }
                self.tools
                    .push((call.id.clone(), call.name.clone(), subject, false));
            }
            AgentEvent::ToolExecutionEnd { result } => {
                let name = self
                    .tools
                    .iter_mut()
                    .find(|t| t.0 == result.tool_call_id)
                    .map(|entry| {
                        entry.3 = result.is_error;
                        entry.1.clone()
                    })
                    .unwrap_or_default();
                if !self.quiet && result.is_error {
                    eprintln!("  ! {}", result.plain_text());
                }
                if stream {
                    self.line(serde_json::json!({
                        "type": "tool_result",
                        "name": name,
                        "error": result.is_error,
                        "output": capped(&result.plain_text(), STREAM_OUTPUT_CHARS),
                    }));
                }
            }
            _ => {}
        }
    }

    /// Report the result of the run whose transcript is `messages`; the
    /// process exit code.
    fn finish(self, messages: &[Message]) -> i32 {
        if self.wrote_text {
            println!();
        }
        let last = messages.iter().rev().find_map(|message| match message {
            Message::Assistant(assistant) => Some(assistant),
            _ => None,
        });
        let code = match last {
            Some(last) if last.error_message.is_some() => 1,
            Some(last) if last.stop_reason == StopReason::Aborted => 130,
            Some(_) => 0,
            None => 1,
        };
        if self.quiet {
            let mut report = serde_json::json!({
                "ok": code == 0,
                "answer": last.map(termide_agent_core::AssistantMessage::plain_text).unwrap_or_default(),
                "stop_reason": last.map(|m| stop_label(m.stop_reason)),
                "model": last.map(|m| m.model.clone()),
                "provider": last.map(|m| m.provider.clone()),
                "usage": last.map(|m| usage_json(&m.usage)),
                "usage_total": usage_json(&total_usage(messages)),
                "elapsed_ms": self.elapsed_ms(),
                "tools": self.tools.iter().map(|(_, name, subject, is_error)| serde_json::json!({
                    "name": name,
                    "subject": subject,
                    "error": is_error,
                })).collect::<Vec<_>>(),
                "error": last.and_then(|m| m.error_message.clone())
                    .or_else(|| (last.is_none()).then(|| "the agent produced no answer".to_string())),
            });
            // In stream mode the report is the terminal event; tag it.
            if self.stream {
                report["type"] = serde_json::json!("result");
            }
            println!("{report}");
        } else {
            match last {
                Some(last) if last.error_message.is_some() => eprintln!(
                    "termide: {}",
                    last.error_message.as_deref().unwrap_or("the run failed")
                ),
                None => eprintln!("termide: the agent produced no answer"),
                _ => {}
            }
        }
        code
    }
}

/// The wire label for a stop reason, for the JSON report.
fn stop_label(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Stop => "stop",
        StopReason::Length => "length",
        StopReason::ToolUse => "tool_use",
        StopReason::Error => "error",
        StopReason::Aborted => "aborted",
    }
}

/// `termide --recall <query>`: run the agent's `recall` search for the
/// project at `project_root` and print the results — the text the model
/// would read, or with `json` one object with the hits (and the solver's
/// answer when `[ai.recall]` turns it on). There is no session, so the
/// solver answers with the default connection's model unless it names its
/// own. Returns the process exit code: 0 with results, 1 without.
pub fn run_recall(settings: &AiSettings, project_root: &Path, query: &str, json: bool) -> i32 {
    let global = termide_config::get_config_dir()
        .ok()
        .map(|dir| dir.join(GLOBAL_AGENT_DIR));
    let dirs = AgentDirs::new(project_root, Some(project_root), global.as_deref())
        .with_home(dirs::home_dir().as_deref());
    let tool = recall_tool(settings, &dirs, project_root, project_root);
    // The default connection answers in the session's place, as a panel's
    // model would: not one that drives a CLI agent, and with its first
    // listed model when it names none. Asked only when the solver is on.
    let session = usable_connection(settings)
        .filter(|_| settings.recall.solver)
        .filter(|(_, connection)| !termide_config::is_cli_provider(&connection.provider))
        .and_then(|(name, connection)| {
            let provider = connection_provider(name, connection);
            let model = resolve_model(provider.as_ref(), &connection.model)?;
            Some(SessionView {
                id: None,
                intent: IntentLog::new(),
                provider,
                model: ModelSpec {
                    provider: "agent".to_string(),
                    id: model,
                    context_window: connection.effective_context_window(),
                    max_tokens: None,
                    thinking: termide_agent_core::ThinkingLevel::Off,
                    reasoning_budget: None,
                },
            })
        });
    let outcome = tool.search(
        termide_agent_recall::SearchRequest::new(query),
        session.as_ref(),
        &CancelToken::new(),
    );
    if json {
        println!("{}", termide_agent_recall::to_json(&outcome));
    } else {
        println!("{}", termide_agent_recall::render(&outcome));
    }
    i32::from(outcome.hits.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_is_written_in_whole_lines_as_it_streams() {
        let mut chunks = ThinkingChunks::default();
        let line = format!("{}\n", "x".repeat(99));
        let mut written = String::new();
        for _ in 0..100 {
            // Split mid-line, as a stream does.
            for piece in [&line[..50], &line[50..]] {
                if let Some(text) = chunks.push(piece) {
                    assert!(text.ends_with('\n'), "whole lines only");
                    written.push_str(&text);
                }
            }
        }
        assert!(!written.is_empty(), "written before the call ends");
        written.push_str(&chunks.finish().unwrap_or_default());
        assert_eq!(written, line.repeat(100));
        assert_eq!(chunks.finish(), None);

        // A line that never ends is still written, once very long.
        let mut endless = ThinkingChunks::default();
        let flushed = (0..4 * THINKING_CHUNK_CHARS)
            .filter_map(|_| endless.push("y"))
            .collect::<String>();
        assert_eq!(flushed.len(), 4 * THINKING_CHUNK_CHARS);

        // JSON arguments are one line: cut at the size.
        let mut raw = ThinkingChunks {
            raw: true,
            ..ThinkingChunks::default()
        };
        let flushed = (0..THINKING_CHUNK_CHARS)
            .filter_map(|_| raw.push("z"))
            .collect::<String>();
        assert_eq!(flushed.len(), THINKING_CHUNK_CHARS);
    }

    #[test]
    fn a_tool_output_is_capped_by_characters_and_says_how_much_was_cut() {
        assert_eq!(capped("short", 10), "short");
        assert_eq!(capped("exactly10!", 10), "exactly10!");
        // Cut on a character, not a byte: no panic inside a multibyte one.
        assert_eq!(capped("привет мир", 6), "привет… [4 more characters]");
    }

    #[test]
    fn the_total_usage_adds_up_every_assistant_turn() {
        let turn = |input, output| {
            Message::Assistant(termide_agent_core::AssistantMessage {
                content: Vec::new(),
                stop_reason: StopReason::Stop,
                usage: Usage {
                    input,
                    output,
                    ..Usage::default()
                },
                provider: String::new(),
                model: String::new(),
                error_message: None,
                failure: None,
                timestamp: 0,
            })
        };
        let messages = [
            Message::User(UserMessage::text("go")),
            turn(100, 10),
            turn(250, 20),
        ];
        let total = total_usage(&messages);
        assert_eq!((total.input, total.output), (350, 30));
    }

    #[test]
    fn recall_from_the_command_line_exits_zero_only_with_results() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "the walrus plan\n").unwrap();
        let settings = AiSettings::default();
        assert_eq!(run_recall(&settings, tmp.path(), "walrus", false), 0);
        assert_eq!(run_recall(&settings, tmp.path(), "walrus", true), 0);
        assert_eq!(run_recall(&settings, tmp.path(), "zebra", false), 1);
    }
}
