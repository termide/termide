//! The headless agent: `termide --prompt`, one task run without the UI, its
//! answer on stdout.

use std::path::Path;
use std::sync::Arc;

use termide_agent_core::{
    build_system_prompt, discover_context_files, ensure_global_layout, subject_of, Agent,
    AgentDirs, AgentEvent, AutoDenyPrompter, CancelToken, IntentLog, Message, Mode, ModelSpec,
    PermissionHooks, PromptOptions, SessionView, StopReason, StreamEvent, ToolContext, Usage,
    UserMessage, DEFAULT_AGENT, GLOBAL_AGENT_DIR,
};
use termide_agent_tools::SkillTool;
use termide_config::AiSettings;

use super::{
    base_tools, connection_provider, recall_tool, resolve_model, restrict_tools, reviewer_setup,
    shared_web, usable_connection,
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
    use std::io::Write;
    // Both JSON forms suppress the plain text/stderr chatter.
    let quiet = output != HeadlessOutput::Text;
    let stream = output == HeadlessOutput::StreamJson;

    let Some((name, connection)) = usable_connection(settings) else {
        eprintln!("termide: AI is not configured (add an [ai.connections] entry)");
        return 1;
    };
    let provider = connection_provider(name, connection);

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
    if definition.spec.acp.is_some() {
        eprintln!("termide: headless mode cannot drive an external (ACP) agent");
        return 2;
    }

    super::publish_shell(shell);
    let web = shared_web(&settings.web, &dirs);
    let recall = recall_tool(settings, &dirs, project_root, cwd);
    let mut tools = base_tools(&dirs, Some(&web), Some(&recall));
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
    };
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

    let mut agent = Agent::new(Arc::clone(&provider), tools, model, cwd.to_path_buf())
        .with_system_prompt(system_prompt)
        .with_compaction(settings.compaction)
        .with_compaction_prompts(dirs.compaction_prompts());
    let cancel = CancelToken::new();
    let refusals = dirs.refusals();
    let mut hooks = PermissionHooks::new(
        rules,
        Box::new(AutoDenyPrompter::new(refusals.unattended_headless.clone())),
    )
    .with_classifier(Box::new(
        reviewer_setup(settings, &dirs, name).classifier(cancel.clone()),
    ))
    .with_refusals(refusals);
    let stdout = std::io::stdout();
    let mut wrote_text = false;
    // Tool calls in call order: (id, name, subject, is_error), for the JSON
    // report and, in text mode, the stderr activity lines.
    let mut tools: Vec<(String, String, String, bool)> = Vec::new();
    // Every stream event carries the time since the run began, so a slow
    // turn or a long tool call shows in the log.
    let started = std::time::Instant::now();
    let mut thinking = ThinkingChunks::default();
    let mut drafts: std::collections::HashMap<String, (String, ThinkingChunks)> =
        std::collections::HashMap::new();
    let elapsed_ms = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    {
        let line = |value: serde_json::Value| {
            let mut value = value;
            value["elapsed_ms"] = serde_json::json!(elapsed_ms());
            let mut out = stdout.lock();
            let _ = writeln!(out, "{value}");
            let _ = out.flush();
        };
        let mut emit = |event: AgentEvent| match event {
            AgentEvent::MessageUpdate(StreamEvent::TextDelta(text)) => {
                if !quiet {
                    let mut out = stdout.lock();
                    let _ = out.write_all(text.as_bytes());
                    let _ = out.flush();
                    wrote_text = true;
                }
            }
            AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(delta)) if stream => {
                if let Some(text) = thinking.push(&delta) {
                    line(serde_json::json!({ "type": "thinking", "text": text }));
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
                drafts.insert(id, (name, chunks));
            }
            AgentEvent::MessageUpdate(StreamEvent::ToolCallDelta { id, arguments }) if stream => {
                if let Some((name, chunks)) = drafts.get_mut(&id) {
                    if let Some(text) = chunks.push(&arguments) {
                        line(
                            serde_json::json!({ "type": "tool_input", "name": name, "text": text }),
                        );
                    }
                }
            }
            AgentEvent::MessageUpdate(StreamEvent::ToolCallEnd { id }) if stream => {
                drafts.remove(&id);
            }
            AgentEvent::ThinkingLoop { report, retrying } if stream => {
                if let Some(text) = thinking.finish() {
                    line(serde_json::json!({ "type": "thinking", "text": text }));
                }
                line(serde_json::json!({
                    "type": "loop",
                    "retrying": retrying,
                    "repeats": report.repeats,
                    "thoughts": report.thoughts,
                }));
            }
            AgentEvent::EmptyCutOff { retrying } if stream => {
                line(serde_json::json!({ "type": "cut_off", "retrying": retrying }));
            }
            AgentEvent::MessageEnd(Message::Assistant(message)) if stream => {
                if let Some(text) = thinking.finish() {
                    line(serde_json::json!({ "type": "thinking", "text": text }));
                }
                let text = message.plain_text();
                if !text.trim().is_empty() {
                    line(serde_json::json!({ "type": "message", "text": text }));
                }
            }
            AgentEvent::ToolExecutionStart { call } => {
                let subject = subject_of(&call, &ToolContext::new(cwd.to_path_buf()));
                if !quiet {
                    if subject.is_empty() {
                        eprintln!("· {}", call.name);
                    } else {
                        eprintln!("· {} {subject}", call.name);
                    }
                }
                if stream {
                    line(serde_json::json!({
                        "type": "tool_use",
                        "name": call.name,
                        "subject": subject,
                    }));
                }
                tools.push((call.id.clone(), call.name.clone(), subject, false));
            }
            AgentEvent::ToolExecutionEnd { result } => {
                let name = tools
                    .iter_mut()
                    .find(|t| t.0 == result.tool_call_id)
                    .map(|entry| {
                        entry.3 = result.is_error;
                        entry.1.clone()
                    })
                    .unwrap_or_default();
                if !quiet && result.is_error {
                    eprintln!("  ! {}", result.plain_text());
                }
                if stream {
                    line(serde_json::json!({
                        "type": "tool_result",
                        "name": name,
                        "error": result.is_error,
                        "output": capped(&result.plain_text(), STREAM_OUTPUT_CHARS),
                    }));
                }
            }
            _ => {}
        };
        agent.run(UserMessage::text(prompt), &mut hooks, &cancel, &mut emit);
    }
    if wrote_text {
        println!();
    }

    let last = agent
        .messages()
        .iter()
        .rev()
        .find_map(|message| match message {
            Message::Assistant(assistant) => Some(assistant),
            _ => None,
        });
    let code = match last {
        Some(last) if last.error_message.is_some() => 1,
        Some(last) if last.stop_reason == StopReason::Aborted => 130,
        Some(_) => 0,
        None => 1,
    };
    if quiet {
        let mut report = serde_json::json!({
            "ok": code == 0,
            "answer": last.map(termide_agent_core::AssistantMessage::plain_text).unwrap_or_default(),
            "stop_reason": last.map(|m| stop_label(m.stop_reason)),
            "model": last.map(|m| m.model.clone()),
            "provider": last.map(|m| m.provider.clone()),
            "usage": last.map(|m| usage_json(&m.usage)),
            "usage_total": usage_json(&total_usage(agent.messages())),
            "elapsed_ms": elapsed_ms(),
            "tools": tools.iter().map(|(_, name, subject, is_error)| serde_json::json!({
                "name": name,
                "subject": subject,
                "error": is_error,
            })).collect::<Vec<_>>(),
            "error": last.and_then(|m| m.error_message.clone())
                .or_else(|| (last.is_none()).then(|| "the agent produced no answer".to_string())),
        });
        // In stream mode the report is the terminal event; tag it.
        if stream {
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
