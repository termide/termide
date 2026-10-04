//! Headless smoke run against an OpenAI-compatible server.
//!
//! ```text
//! cargo run -p termide-agent-providers --example smoke -- \
//!     [--url http://127.0.0.1:10000/v1] [--model <id>] "prompt"
//! ```
//! Defaults target the local omlx server and its Flash model.
//! Prints text deltas as they arrive and one line per tool call and result.

use std::sync::Arc;

use termide_agent_core::{
    build_system_prompt, discover_context_files, Agent, AgentEvent, CancelToken, CompactionPolicy,
    Message, ModelSpec, NoHooks, PromptOptions, Session, StreamEvent, UserMessage,
};
use termide_agent_providers::OpenAiCompatProvider;
use termide_agent_tools::builtin_tools;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut url = "http://127.0.0.1:10000/v1".to_string();
    let mut model_id = "Qwen3.8-Flash-Next-oQ4e-mtp".to_string();
    let mut prompt = String::new();
    let mut compaction = CompactionPolicy::default();
    let mut context_window: u64 = 32_000;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => url = args.next().expect("--url value"),
            "--model" => model_id = args.next().expect("--model value"),
            // Shrink the window to force a compaction early, for testing.
            "--window" => {
                context_window = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--window <tokens>");
            }
            // Raise the reserve to force a compaction early, for testing.
            "--reserve" => {
                compaction.reserve_tokens = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--reserve <tokens>");
            }
            other => prompt = other.to_string(),
        }
    }
    if prompt.is_empty() {
        eprintln!("usage: smoke [--model <id>] [--url <base>] \"prompt\"");
        std::process::exit(2);
    }

    let cwd = std::env::current_dir().expect("cwd");
    let model = ModelSpec {
        provider: "local".into(),
        id: model_id,
        context_window,
        max_tokens: Some(2048),
        thinking: termide_agent_core::ThinkingLevel::High,
    };
    let provider = Arc::new(
        OpenAiCompatProvider::new("local", url).with_api_key(std::env::var("OPENAI_API_KEY").ok()),
    );
    let tools = builtin_tools(None);
    let context_files = discover_context_files(&cwd, None, None);
    let system_prompt = build_system_prompt(&PromptOptions::new(&cwd, &tools, &context_files));
    let session_dir = std::env::temp_dir().join("termide-agent-smoke");
    let mut session = Session::create(&session_dir, &cwd).expect("create session");
    session
        .append_model_change("local", &model.id, Some(model.context_window))
        .expect("record model");
    let mut agent = Agent::new(provider, tools, model, cwd)
        .with_system_prompt(system_prompt)
        .with_compaction(compaction);

    agent.run(
        UserMessage::text(prompt),
        &mut NoHooks,
        &CancelToken::new(),
        &mut |event| match event {
            AgentEvent::MessageUpdate(StreamEvent::TextDelta(text)) => print!("{text}"),
            AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(_)) => {}
            AgentEvent::MessageUpdate(StreamEvent::Retry {
                error, delay_ms, ..
            }) => {
                eprintln!("\n[retry in {delay_ms} ms: {error}]");
            }
            AgentEvent::ToolExecutionStart { call } => {
                println!("\n>> {}({})", call.name, call.arguments);
            }
            AgentEvent::ToolExecutionEnd { result } => {
                let text = result.plain_text();
                let preview: String = text.chars().take(300).collect();
                println!(
                    "<< {}{}{}",
                    if result.is_error { "ERROR " } else { "" },
                    preview,
                    if text.len() > 300 { " …" } else { "" }
                );
            }
            AgentEvent::MessageEnd(message) => {
                if let Message::Assistant(assistant) = &message {
                    if let Some(error) = &assistant.error_message {
                        eprintln!("\n[model error: {error}]");
                    }
                }
                if let Err(error) = session.append_message(&message) {
                    eprintln!("\n[session write failed: {error}]");
                }
            }
            AgentEvent::CompactionStart { reason, .. } => eprintln!("\n[compacting: {reason:?}]"),
            AgentEvent::Compacted {
                summary,
                kept,
                tokens_before,
                ..
            } => {
                eprintln!("[compacted {tokens_before} tokens, kept {kept} messages]");
                if let Err(error) = session.append_compaction(&summary, tokens_before, kept) {
                    eprintln!("[session write failed: {error}]");
                }
            }
            AgentEvent::CompactionFailed { error } => eprintln!("\n[compaction failed: {error}]"),
            AgentEvent::AgentEnd => println!("\n[done] session: {}", session.path().display()),
            _ => {}
        },
    );
}
