use super::*;
use std::io::pipe;
use termide_agent_core::{
    permission_channel, PermissionAnswer, PermissionEnvelope, PermissionRules,
};

/// `fs/read_text_file` serves a range of a file without loading it whole
/// and refuses a whole read past its cap.
#[test]
fn read_text_file_pages_ranges_and_caps_whole_reads() {
    let dir = tempfile::tempdir().unwrap();
    let body: String = (1..=1000).map(|n| format!("l{n}\r\n")).collect();
    std::fs::write(dir.path().join("a.txt"), &body).unwrap();
    let range = |params: Value| read_text_file(dir.path(), &params).unwrap()["content"].clone();
    assert_eq!(
        range(json!({ "path": "a.txt", "line": 500, "limit": 3 })),
        "l500\nl501\nl502"
    );
    assert_eq!(
        range(json!({ "path": "a.txt", "line": 999 })),
        "l999\nl1000"
    );
    assert_eq!(range(json!({ "path": "a.txt", "limit": 1 })), "l1");
    assert_eq!(range(json!({ "path": "a.txt" })), body.as_str());

    let big = std::fs::File::create(dir.path().join("big.txt")).unwrap();
    big.set_len(READ_TEXT_MAX_BYTES as u64 + 1).unwrap();
    let error = read_text_file(dir.path(), &json!({ "path": "big.txt" })).unwrap_err();
    assert!(error.contains("in parts"), "{error}");
}

/// `fs/write_text_file` replaces a file whole, through a symlink to its
/// target, keeping the target's permissions.
#[cfg(unix)]
#[test]
fn write_text_file_is_atomic_and_follows_symlinks() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("tool.sh");
    std::fs::write(&real, "old").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::os::unix::fs::symlink("tool.sh", dir.path().join("link.sh")).unwrap();
    write_text_file(dir.path(), &json!({ "path": "link.sh", "content": "new" })).unwrap();
    assert_eq!(std::fs::read_to_string(&real).unwrap(), "new");
    assert!(std::fs::symlink_metadata(dir.path().join("link.sh"))
        .unwrap()
        .file_type()
        .is_symlink());
    let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
}

/// Only meaningful when the test itself runs with a controlling
/// terminal; without one both answers are `NO_TTY`.
#[cfg(unix)]
#[test]
fn an_agent_cannot_open_termides_terminal() {
    let config = AcpConfig {
        command: "sh".into(),
        args: vec![
            "-c".into(),
            "if (exec 3</dev/tty) 2>/dev/null; then echo TTY; else echo NO_TTY; fi".into(),
        ],
        env: Default::default(),
        timeout_secs: 5,
        flavor: AcpFlavor::default(),
        context_limit: None,
    };
    let out = agent_command(&config, Path::new("/"))
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "NO_TTY");
}

/// An agent on the other end of two pipes, scripted for one prompt: it
/// thinks, speaks, edits a file through our fs methods after asking for
/// permission, speaks again and ends the turn; a second prompt is
/// cancelled.
fn fake_agent(dir: PathBuf) -> (AcpRuntime, Receiver<PermissionEnvelope>) {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    let agent_dir = dir.clone();
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let mut reader = BufReader::new(to_agent_rx).lines();
        let mut next_id = 100;
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            let id = message["id"].clone();
            match message["method"].as_str() {
                Some("initialize") => {
                    assert_eq!(
                        message["params"]["clientCapabilities"]["fs"]["writeTextFile"],
                        true
                    );
                    send(
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": 1, "agentCapabilities": {} } }),
                    );
                }
                Some("session/new") => {
                    assert_eq!(message["params"]["cwd"], json!(agent_dir));
                    send(json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": "s1" } }));
                }
                Some("session/prompt") => {
                    let text = message["params"]["prompt"][0]["text"]
                        .as_str()
                        .unwrap()
                        .to_string();
                    if text == "second" {
                        // Wait for the cancel notification, then stop.
                        loop {
                            let Some(Ok(line)) = reader.next() else {
                                return;
                            };
                            let m: Value = serde_json::from_str(&line).unwrap();
                            if m["method"] == "session/cancel" {
                                break;
                            }
                        }
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "cancelled" } }),
                        );
                        continue;
                    }
                    let update = |u: Value| json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s1", "update": u } });
                    send(update(
                        json!({ "sessionUpdate": "agent_thought_chunk", "content": { "type": "text", "text": "hmm" } }),
                    ));
                    send(update(
                        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Editing " } }),
                    ));
                    send(update(
                        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "now." } }),
                    ));
                    // Ask for permission and wait for the answer.
                    next_id += 1;
                    send(
                        json!({ "jsonrpc": "2.0", "id": next_id, "method": "session/request_permission", "params": {
                            "sessionId": "s1",
                            "toolCall": { "toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "rawInput": { "path": "notes.md" } },
                            "options": [
                                { "optionId": "o-once", "name": "Allow", "kind": "allow_once" },
                                { "optionId": "o-always", "name": "Always", "kind": "allow_always" },
                                { "optionId": "o-no", "name": "Reject", "kind": "reject_once" }
                            ] } }),
                    );
                    let answer: Value = loop {
                        let Some(Ok(line)) = reader.next() else {
                            return;
                        };
                        let m: Value = serde_json::from_str(&line).unwrap();
                        if m["id"] == json!(next_id) {
                            break m;
                        }
                    };
                    assert_eq!(answer["result"]["outcome"]["optionId"], "o-once");
                    send(update(
                        json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "status": "in_progress", "rawInput": { "path": "notes.md" } }),
                    ));
                    // Write through the client and read it back.
                    next_id += 1;
                    send(
                        json!({ "jsonrpc": "2.0", "id": next_id, "method": "fs/write_text_file", "params": { "sessionId": "s1", "path": "notes.md", "content": "one\ntwo\nthree\n" } }),
                    );
                    loop {
                        let Some(Ok(line)) = reader.next() else {
                            return;
                        };
                        let m: Value = serde_json::from_str(&line).unwrap();
                        if m["id"] == json!(next_id) {
                            assert!(m.get("error").is_none(), "{m}");
                            break;
                        }
                    }
                    next_id += 1;
                    send(
                        json!({ "jsonrpc": "2.0", "id": next_id, "method": "fs/read_text_file", "params": { "sessionId": "s1", "path": "notes.md", "line": 2, "limit": 1 } }),
                    );
                    loop {
                        let Some(Ok(line)) = reader.next() else {
                            return;
                        };
                        let m: Value = serde_json::from_str(&line).unwrap();
                        if m["id"] == json!(next_id) {
                            assert_eq!(m["result"]["content"], "two");
                            break;
                        }
                    }
                    next_id += 1;
                    send(
                        json!({ "jsonrpc": "2.0", "id": next_id, "method": "terminal/create", "params": {} }),
                    );
                    loop {
                        let Some(Ok(line)) = reader.next() else {
                            return;
                        };
                        let m: Value = serde_json::from_str(&line).unwrap();
                        if m["id"] == json!(next_id) {
                            assert_eq!(m["error"]["code"], -32601);
                            break;
                        }
                    }
                    send(update(
                        json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "status": "completed",
                            "content": [{ "type": "diff", "path": "notes.md", "oldText": "", "newText": "one\ntwo\nthree\n" }],
                            "locations": [{ "path": "notes.md" }] }),
                    ));
                    send(update(
                        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Done." } }),
                    ));
                    send(
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                    );
                }
                _ => {}
            }
        }
    });
    let cancel = CancelToken::new();
    let (prompter, permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        5,
        AcpFlavor::Generic,
    );
    (runtime, permissions)
}

fn drain_until_end(
    runtime: &AcpRuntime,
    permissions: &Receiver<PermissionEnvelope>,
    answer: PermissionAnswer,
) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    loop {
        events.extend(runtime.drain());
        if let Ok(envelope) = permissions.try_recv() {
            assert_eq!(envelope.request.tool, "edit");
            // The subject is derived from the call, as for the built-in
            // agent: the project-relative path, not the ACP title.
            assert_eq!(envelope.request.subject, "notes.md");
            envelope.reply.send(answer.clone()).unwrap();
        }
        if events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
            return events;
        }
        assert!(Instant::now() < deadline, "no AgentEnd: {events:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// An agent that advertises two models at `session/new` and accepts a
/// `session/set_model`.
fn models_agent(dir: PathBuf) -> AcpRuntime {
    models_agent_with(
        dir,
        json!({
            "models": {
                "availableModels": [
                    { "modelId": "m-fast", "name": "Fast" },
                    { "modelId": "m-slow", "name": "Slow" }
                ],
                "currentModelId": "m-fast"
            }
        }),
    )
}

/// An agent that advertises the same two models as a `model` config
/// option, one of them in a group, and switches through
/// `session/set_config_option`.
fn config_option_agent(dir: PathBuf) -> AcpRuntime {
    models_agent_with(
        dir,
        json!({
            "configOptions": [
                { "id": "mode", "category": "mode", "type": "select",
                  "currentValue": "ask", "options": [{ "value": "ask", "name": "Ask" }] },
                { "id": "model", "category": "model", "type": "select",
                  "currentValue": "m-fast",
                  "options": [
                      { "value": "m-fast", "name": "Fast" },
                      { "group": "older", "name": "Older",
                        "options": [{ "value": "m-slow", "name": "Slow" }] }
                  ] }
            ]
        }),
    )
}

/// An agent that records every message it gets and answers the
/// handshake and `session/set_config_option`; it takes MCP servers over
/// HTTP.
fn recording_agent(
    dir: PathBuf,
    flavor: AcpFlavor,
    host_tools: Option<HostTools>,
    mode: ModeHandle,
) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
    recording_agent_with(dir, flavor, host_tools, mode, true)
}

/// [`recording_agent`], saying whether it takes MCP servers over HTTP.
fn recording_agent_with(
    dir: PathBuf,
    flavor: AcpFlavor,
    host_tools: Option<HostTools>,
    mode: ModeHandle,
    http: bool,
) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let mut reader = BufReader::new(to_agent_rx).lines();
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            record.lock().unwrap().push(message.clone());
            let id = message["id"].clone();
            let result = match message["method"].as_str() {
                Some("initialize") => json!({
                    "protocolVersion": 1,
                    "agentCapabilities": { "mcpCapabilities": { "http": http } },
                }),
                Some("session/new") => json!({ "sessionId": "s1" }),
                Some(_) => json!({}),
                None => continue,
            };
            send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    });
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode,
            system_prompt: "termide's prompt".into(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools,
            resume: None,
            history: Vec::new(),
        },
        5,
        flavor,
    );
    (runtime, seen)
}

/// Wait until the agent has seen a message matching `wanted`.
fn seen_where(seen: &Arc<Mutex<Vec<Value>>>, wanted: impl Fn(&Value) -> bool) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found: Vec<Value> = seen
            .lock()
            .unwrap()
            .iter()
            .filter(|m| wanted(m))
            .cloned()
            .collect();
        if !found.is_empty() {
            return found;
        }
        assert!(
            Instant::now() < deadline,
            "never seen: {:?}",
            seen.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct Echo;

impl termide_agent_core::Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Say it back"
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }
    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _on_update: &mut dyn FnMut(termide_agent_core::ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        let result = ToolResultMessage::text(call, "echoed")
            .with_details(json!({ "said": "echoed", "call": call.id }));
        // As `read` returns an image file.
        if call.arguments["text"] == "picture" {
            result.with_images(vec![("image/png".into(), "AAAA".into())])
        } else {
            result
        }
    }
}

/// The names of the tools termide's MCP server at `url` lists.
fn served_tools(url: &str, auth: &str) -> Vec<String> {
    let reply = post_mcp(
        url,
        auth,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
    );
    reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect()
}

/// Send `body` to termide's MCP server at `url` and return its reply.
fn post_mcp(url: &str, auth: &str, body: &Value) -> Value {
    use std::io::Read;
    let addr = url.trim_start_matches("http://").trim_end_matches("/mcp");
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    let body = body.to_string();
    write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: {auth}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (_, body) = response.split_once("\r\n\r\n").unwrap();
    serde_json::from_str(body).unwrap()
}

#[test]
fn a_call_of_termides_tool_ends_with_the_details_its_reply_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        Some(echo_host()),
        ModeHandle::new(Mode::default()),
    );
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    let server = &new["params"]["mcpServers"][0];
    let url = server["url"].as_str().unwrap();
    let auth = server["headers"][0]["value"].as_str().unwrap();
    // As Claude Code goes about it: the call is announced, its arguments
    // follow, the server runs it, and the end carries the reply's text.
    let shared = &runtime.shared;
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "other",
        "status": "pending", "title": "mcp__termide__echo", "rawInput": {},
    }));
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1",
        "rawInput": { "text": "hi" },
    }));
    let reply = post_mcp(
        url,
        auth,
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                     "params": { "name": "echo", "arguments": { "text": "hi" } } }),
    );
    assert!(reply["result"].get("details").is_none());
    assert!(!reply.to_string().contains("said"), "not for the model");
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
        "content": [{ "type": "content", "content": { "type": "text", "text": "echoed" } }],
    }));
    let ended = runtime.drain().into_iter().find_map(|e| match e {
        AgentEvent::ToolExecutionEnd { result } => Some(result),
        _ => None,
    });
    let ended = ended.expect("the call ends");
    assert_eq!(ended.tool_name, "echo");
    // The server ran it as the call the agent announced, by its id.
    assert_eq!(
        ended.details,
        Some(json!({ "said": "echoed", "call": "t1" }))
    );
}

/// Claude Code reports the picture termide's tool returned as a line of
/// text naming the file it saved it to: the block shows the picture all
/// the same, from what the server kept.
#[test]
fn a_call_of_termides_tool_ends_with_the_pictures_its_report_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        Some(echo_host()),
        ModeHandle::new(Mode::default()),
    );
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    let server = &new["params"]["mcpServers"][0];
    let url = server["url"].as_str().unwrap();
    let auth = server["headers"][0]["value"].as_str().unwrap();
    let shared = &runtime.shared;
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "other",
        "status": "pending", "title": "mcp__termide__echo",
        "rawInput": { "text": "picture" },
    }));
    let reply = post_mcp(
        url,
        auth,
        &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                     "params": { "name": "echo", "arguments": { "text": "picture" } } }),
    );
    assert_eq!(reply["result"]["content"][1]["type"], "image");
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
        "content": [{ "type": "content", "content": { "type": "text",
                      "text": "echoed\n[Image: source: /tmp/blob.png]" } }],
    }));
    let ended = runtime.drain().into_iter().find_map(|e| match e {
        AgentEvent::ToolExecutionEnd { result } => Some(result),
        _ => None,
    });
    let ended = ended.expect("the call ends");
    assert_eq!(ended.images().collect::<Vec<_>>(), [("image/png", "AAAA")]);
    assert_eq!(ended.details.unwrap()["said"], "echoed");
}

/// A picture the agent's own tool returns, sent as ACP image content.
#[test]
fn an_agents_own_call_ends_with_the_pictures_it_reports() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    let shared = &runtime.shared;
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "read",
        "status": "pending", "title": "Read a.png", "rawInput": { "file_path": "a.png" },
    }));
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
        "content": [
            { "type": "content", "content": { "type": "image",
              "mimeType": "image/png", "data": "BBBB" } },
        ],
    }));
    let ended = runtime.drain().into_iter().find_map(|e| match e {
        AgentEvent::ToolExecutionEnd { result } => Some(result),
        _ => None,
    });
    let ended = ended.expect("the call ends");
    assert_eq!(ended.images().collect::<Vec<_>>(), [("image/png", "BBBB")]);
}

#[test]
fn the_agents_own_compaction_reaches_the_panel_as_a_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    // Asked for, so the agent does not stand a tool call in for it.
    let init = seen_where(&seen, |m| m["method"] == "initialize").remove(0);
    assert_eq!(
        init["params"]["clientCapabilities"]["session"]["compaction"],
        json!({})
    );
    let shared = &runtime.shared;
    runtime.drain();
    shared.on_update(&json!({ "sessionUpdate": "usage_update", "used": 230_000, "size": 262_144 }));
    shared.on_update(&json!({
        "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "Reading." },
    }));
    // As Claude Code goes about it: the start, the summary streamed,
    // the end with the summary whole, and the end again.
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": "c1", "status": "in_progress",
    }));
    shared.on_update(&json!({
        "sessionUpdate": "compaction_summary_chunk", "compactionId": "c1",
        "content": { "type": "text", "text": "## Kept" },
    }));
    let summary = "## Kept\nThe user wants the parser fixed.";
    let end = json!({
        "sessionUpdate": "compaction_update", "compactionId": "c1", "status": "completed",
        "summary": [{ "type": "text", "text": summary }],
    });
    shared.on_update(&end);
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": "c1", "status": "completed",
    }));
    let events = runtime.drain();
    let at = |pick: &dyn Fn(&AgentEvent) -> bool| events.iter().position(pick);
    let text_end = at(&|e| matches!(e, AgentEvent::MessageEnd(_))).expect("text closed");
    let start = at(&|e| {
        matches!(
            e,
            AgentEvent::CompactionStart {
                reason: CompactionReason::Threshold,
                prompt_tokens: 230_000
            }
        )
    })
    .expect("compaction starts");
    assert!(text_end < start, "{events:?}");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionUpdate(StreamEvent::TextDelta(text)) if text == "## Kept"
    )));
    let ended: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compacted {
                summary,
                kept,
                tokens_before,
                ..
            } => Some((summary.as_str(), *kept, *tokens_before)),
            _ => None,
        })
        .collect();
    assert_eq!(ended, [(summary, 0, 230_000)], "{events:?}");
    // No tool call stands for it.
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })));
}

#[test]
fn an_interrupted_compaction_of_the_agent_ends_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    let shared = &runtime.shared;
    runtime.drain();
    // Reported only at its end, as a runtime with no start may.
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": "c2", "status": "cancelled",
    }));
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": "c3", "status": "failed",
        "error": "aborted",
    }));
    let failed: Vec<String> = runtime
        .drain()
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::CompactionFailed { error, .. } => Some(error),
            AgentEvent::Compacted { .. } => panic!("nothing was compacted"),
            _ => None,
        })
        .collect();
    assert_eq!(failed, ["cancelled", "aborted"]);
}

#[test]
fn an_adapters_made_up_compact_call_reaches_the_panel_as_a_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    let shared = &runtime.shared;
    runtime.drain();
    shared.on_update(&json!({ "sessionUpdate": "usage_update", "used": 150_000, "size": 200_000 }));
    // As an adapter without the compaction capability reports it: a
    // call of kind think, its start, a heartbeat and its end.
    let meta = json!({ "claudeCode": { "toolName": "compact" } });
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "c1", "title": "Compact conversation",
        "kind": "think", "status": "in_progress", "_meta": meta,
    }));
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress",
        "_meta": meta,
    }));
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
        "rawOutput": { "trigger": "automatic", "preTokens": 160_000, "postTokens": 9_000 },
        "_meta": meta,
    }));
    // The same call failing, reported only at its end.
    shared.on_update(&json!({
            "sessionUpdate": "tool_call", "toolCallId": "c2", "title": "Compact conversation",
            "kind": "think", "status": "failed",
            "content": [{ "type": "content", "content": { "type": "text", "text": "Compaction failed: aborted" } }],
            "_meta": meta,
        }));
    let events = runtime.drain();
    assert!(
        events.iter().any(|e| matches!(
            e,
            AgentEvent::CompactionStart {
                prompt_tokens: 150_000,
                ..
            }
        )),
        "{events:?}"
    );
    // No summary: the panel logs no compaction in place of the history.
    let ended: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Compacted {
                summary,
                tokens_before,
                ..
            } => Some((summary.as_str(), *tokens_before)),
            _ => None,
        })
        .collect();
    assert_eq!(ended, [("", 160_000)], "{events:?}");
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::CompactionFailed { error, .. } if error == "aborted"
    )));
    assert!(
        !events.iter().any(|e| matches!(
            e,
            AgentEvent::ToolExecutionStart { .. } | AgentEvent::ToolExecutionEnd { .. }
        )),
        "{events:?}"
    );
    // A call that merely thinks is still a call.
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Plan", "kind": "think",
        "status": "in_progress", "rawInput": { "thought": "x" },
    }));
    assert!(runtime
        .drain()
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })));
}

#[test]
fn a_message_typed_while_the_agent_compacts_joins_the_turn_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    let shared = &runtime.shared;
    let deadline = Instant::now() + Duration::from_secs(5);
    while shared.session_id().is_err() {
        assert!(Instant::now() < deadline, "no session");
        std::thread::sleep(Duration::from_millis(5));
    }
    // A turn of an agent that steers is running.
    shared.steering.store(true, Ordering::Release);
    shared.turn_running.store(true, Ordering::Release);
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": "c1", "status": "in_progress",
    }));
    let message = UserMessage::text("more");
    runtime.steer(message.clone());
    // Steered now, it would abort the compaction: it waits.
    assert_eq!(*shared.queue.lock().unwrap(), [message]);
    assert!(shared.steering_out.lock().unwrap().is_empty());
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": "c1", "status": "failed",
        "error": "aborted",
    }));
    let steering = loop {
        let steering = seen_where(&seen, |m| m["method"] == "_session/steering");
        if !steering.is_empty() {
            break steering;
        }
        assert!(Instant::now() < deadline, "the message never went");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(steering[0]["params"]["prompt"][0]["text"], "more");
}

#[test]
fn a_call_whose_raw_input_streams_shows_and_logs_it_in_full() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    // Claude Code sends the raw input as it parses: the first key alone,
    // then the rest. The call starts on the first and fills in after.
    let shared = &runtime.shared;
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "other",
        "status": "pending", "title": "mcp__termide__task", "rawInput": {},
    }));
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1",
        "rawInput": { "agent": "default" },
    }));
    let full = json!({ "agent": "default", "prompt": "check the diff" });
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "rawInput": full,
    }));
    // The same input again changes nothing, so it says nothing.
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "rawInput": full,
    }));
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
        "content": [{ "type": "content", "content": { "type": "text", "text": "done" } }],
    }));
    let events = runtime.drain();
    let updates: Vec<&ToolCall> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolCallUpdate { call } => Some(call),
            _ => None,
        })
        .collect();
    assert_eq!(updates.len(), 1, "{events:?}");
    assert_eq!(updates[0].name, "task");
    assert_eq!(updates[0].arguments, full);
    let logged = events.iter().find_map(|e| match e {
        AgentEvent::MessageEnd(Message::Assistant(message)) => match &message.content[..] {
            [AssistantContent::ToolCall(call)] => Some(call.arguments.clone()),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(logged, Some(full), "{events:?}");
}

#[test]
fn claude_code_gets_termides_prompt_and_tools_and_none_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let mut tools = termide_agent_core::ToolRegistry::new();
    tools.insert(Arc::new(Echo));
    let host = HostTools {
        tools,
        hooks: Box::new(termide_agent_core::NoHooks),
        context: ToolContext::new(PathBuf::new()),
        skills: Vec::new(),
        session_id: None,
    };
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        Some(host),
        ModeHandle::new(Mode::default()),
    );
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    let params = &new["params"];
    assert_eq!(params["_meta"]["systemPrompt"], "termide's prompt");
    let options = &params["_meta"]["claudeCode"]["options"];
    assert_eq!(options["tools"], json!([]));
    assert_eq!(options["settingSources"], json!([]));
    assert_eq!(options["promptSuggestions"], false);
    assert_eq!(options["allowedTools"], json!(["mcp__termide"]));
    let server = &params["mcpServers"][0];
    assert_eq!(server["type"], "http");
    assert_eq!(server["name"], "termide");
    assert!(server["url"]
        .as_str()
        .unwrap()
        .starts_with("http://127.0.0.1:"));
    assert!(server["headers"][0]["value"]
        .as_str()
        .unwrap()
        .starts_with("Bearer "));
    assert!(runtime.follows_mode());
    assert!(runtime.runs_host_tools());
    // A change of the set reaches the running server.
    let url = server["url"].as_str().unwrap();
    let auth = server["headers"][0]["value"].as_str().unwrap();
    assert_eq!(served_tools(url, auth), ["echo"]);
    runtime
        .update_host_tools(termide_agent_core::ToolRegistry::new())
        .unwrap();
    assert!(served_tools(url, auth).is_empty());
    // Its calls of termide's tools show under their own names.
    let call = tool_call_of(&json!({
        "toolCallId": "t1", "title": "mcp__termide__shell", "kind": "other",
        "rawInput": { "command": "ls" }
    }));
    assert_eq!(
        (call.name.as_str(), &call.arguments),
        ("shell", &json!({ "command": "ls" }))
    );
    // A tool the model made up comes as kind `other`: the block takes
    // the name it called, not the kind.
    let call = tool_call_of(&json!({
        "toolCallId": "t2", "title": "shell", "kind": "other",
        "rawInput": { "command": "ls" }
    }));
    assert_eq!(
        (call.name.as_str(), &call.arguments),
        ("shell", &json!({ "command": "ls" }))
    );
    // A kind that names an action keeps it, the title beside.
    let call = tool_call_of(&json!({
        "toolCallId": "t3", "title": "Reading x.rs", "kind": "read",
        "rawInput": { "path": "x.rs" }
    }));
    assert_eq!(call.name, "read");
    assert_eq!(call.arguments["title"], "Reading x.rs");
    // Codex's own calls: a command by its raw input or its title, the
    // file of a read in its locations, of an edit in its diff.
    let call = tool_call_of(&json!({
        "toolCallId": "t4", "title": "cargo test", "kind": "execute",
        "rawInput": { "command": ["cargo", "test"], "cwd": "/p" }
    }));
    assert_eq!(
        (call.name.as_str(), &call.arguments["command"]),
        ("shell", &json!("cargo test"))
    );
    let call = tool_call_of(&json!({
        "toolCallId": "t5", "title": "ls -la", "kind": "execute"
    }));
    assert_eq!(call.arguments["command"], "ls -la");
    let call = tool_call_of(&json!({
        "toolCallId": "t6", "title": "Read file 'src/x.rs'", "kind": "read",
        "locations": [{ "path": "src/x.rs" }]
    }));
    assert_eq!(
        (call.name.as_str(), &call.arguments["path"]),
        ("read", &json!("src/x.rs"))
    );
    let call = tool_call_of(&json!({
        "toolCallId": "t7", "title": "Editing files", "kind": "edit",
        "content": [{ "type": "diff", "path": "/p/src/x.rs", "oldText": "a", "newText": "b" }]
    }));
    assert_eq!(
        (call.name.as_str(), &call.arguments["path"]),
        ("edit", &json!("/p/src/x.rs"))
    );
}

/// Prompt `text` and wait for the turn to end; the text blocks the agent
/// got for it.
fn prompt_blocks(runtime: &AcpRuntime, seen: &Arc<Mutex<Vec<Value>>>, text: &str) -> Vec<String> {
    runtime.prompt(UserMessage::text(text)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !runtime
        .drain()
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd))
    {
        assert!(Instant::now() < deadline, "the turn never ended");
        std::thread::sleep(Duration::from_millis(5));
    }
    let sent = seen_where(seen, |m| {
        m["method"] == "session/prompt"
            && m["params"]["prompt"]
                .as_array()
                .and_then(|blocks| blocks.last())
                .is_some_and(|block| block["text"] == text)
    })
    .remove(0);
    sent["params"]["prompt"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["text"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn claude_code_is_told_of_a_plan_mode_switch_before_the_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    let mode = ModeHandle::new(Mode::Plan);
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        mode.clone(),
    );
    let plan = PlanPrompt::default();
    // Started in plan mode: its system prompt says nothing of it, the first
    // turn opens with the instructions.
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    assert_eq!(new["params"]["_meta"]["systemPrompt"], "termide's prompt");
    let one = prompt_blocks(&runtime, &seen, "one");
    assert!(one[0].contains(&plan.instructions), "{one:?}");
    assert_eq!(one[1], "one");
    assert_eq!(prompt_blocks(&runtime, &seen, "one more"), ["one more"]);
    mode.set(Mode::Edit);
    let two = prompt_blocks(&runtime, &seen, "two");
    assert_eq!(two.len(), 2, "{two:?}");
    assert!(two[0].starts_with("<system-reminder>") && two[0].contains(&plan.leave));
    assert_eq!(prompt_blocks(&runtime, &seen, "three"), ["three"]);
    mode.set(Mode::Plan);
    let four = prompt_blocks(&runtime, &seen, "four");
    assert!(four[0].contains(&plan.instructions), "{four:?}");
    assert_eq!(four[1], "four");

    // Codex is put in its own plan mode instead: no note.
    let mode = ModeHandle::new(Mode::Plan);
    let (codex, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::Codex,
        None,
        mode.clone(),
    );
    mode.set(Mode::Edit);
    assert_eq!(prompt_blocks(&codex, &seen, "go"), ["go"]);
}

/// The agent compacting its context, as Claude Code reports it.
fn compact(runtime: &AcpRuntime, id: &str) {
    let shared = &runtime.shared;
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": id, "status": "in_progress",
    }));
    shared.on_update(&json!({
        "sessionUpdate": "compaction_update", "compactionId": id, "status": "completed",
        "summary": [{ "type": "text", "text": "## Kept" }],
    }));
    runtime.drain();
}

#[test]
fn claude_code_is_told_the_plan_mode_again_after_it_compacts() {
    let dir = tempfile::tempdir().unwrap();
    let mode = ModeHandle::new(Mode::default());
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        mode.clone(),
    );
    let plan = PlanPrompt::default();
    // Never in plan mode: a compaction leaves nothing to tell.
    assert_eq!(prompt_blocks(&runtime, &seen, "one"), ["one"]);
    compact(&runtime, "c1");
    assert_eq!(prompt_blocks(&runtime, &seen, "two"), ["two"]);

    mode.set(Mode::Plan);
    let three = prompt_blocks(&runtime, &seen, "three");
    assert!(three[0].contains(&plan.instructions), "{three:?}");
    // The instructions went with the history: told again.
    compact(&runtime, "c2");
    let four = prompt_blocks(&runtime, &seen, "four");
    assert!(four[0].contains(&plan.instructions), "{four:?}");
    assert_eq!(prompt_blocks(&runtime, &seen, "five"), ["five"]);

    mode.set(Mode::Edit);
    let six = prompt_blocks(&runtime, &seen, "six");
    assert!(six[0].contains(&plan.leave), "{six:?}");
    // The summary may still speak of plan mode: `leave` again.
    compact(&runtime, "c3");
    let seven = prompt_blocks(&runtime, &seen, "seven");
    assert!(seven[0].contains(&plan.leave), "{seven:?}");
    assert_eq!(prompt_blocks(&runtime, &seen, "eight"), ["eight"]);
}

#[test]
fn a_resumed_claude_code_session_from_a_log_with_plan_mode_is_told_it_is_off() {
    let plan = PlanPrompt::default();
    let caps = json!({ "sessionCapabilities": { "resume": {} } });
    for (plan_seen, forgotten, told) in [
        (true, false, true),
        // A log without plan mode, or a session gone and opened anew with a
        // recap: nothing to take back.
        (false, false, false),
        (true, true, false),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = session_agent_as(
            dir.path().to_path_buf(),
            caps.clone(),
            forgotten,
            old_session("fake"),
            earlier(),
            AcpFlavor::ClaudeCode,
            plan_seen,
        );
        let (_, prompt) = run_prompt(&runtime, &seen, "next");
        let leaves = prompt
            .iter()
            .any(|block| block["text"].as_str().unwrap().contains(&plan.leave));
        assert_eq!(leaves, told, "{plan_seen} {forgotten}: {prompt:?}");
    }
}

#[test]
fn claude_code_is_told_of_a_change_of_its_tools_before_the_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        Some(echo_host()),
        ModeHandle::new(Mode::Configured),
    );
    assert_eq!(prompt_blocks(&runtime, &seen, "one"), ["one"]);
    // `recall` switched off, an MCP server's tool come.
    let mut tools = termide_agent_core::ToolRegistry::new();
    tools.insert(Arc::new(Echo));
    tools.insert(Arc::new(Named("skill")));
    tools.insert(Arc::new(Named("db__query")));
    runtime.update_host_tools(tools).unwrap();
    let two = prompt_blocks(&runtime, &seen, "two");
    assert_eq!(two.len(), 2, "{two:?}");
    assert!(
        two[0].contains("Now available: mcp__termide__db__query.")
            && two[0].contains("do not call them: mcp__termide__recall."),
        "{two:?}"
    );
    // Told once.
    assert_eq!(prompt_blocks(&runtime, &seen, "three"), ["three"]);
}

#[test]
fn a_call_announced_without_arguments_shows_once_they_arrive() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    let shared = &runtime.shared;
    shared.on_update(&json!({ "sessionUpdate": "tool_call", "toolCallId": "t1",
            "title": "mcp__termide__shell", "kind": "other", "rawInput": {}, "status": "pending" }));
    let starts = |events: &[AgentEvent]| -> Vec<ToolCall> {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolExecutionStart { call } => Some(call.clone()),
                _ => None,
            })
            .collect()
    };
    assert!(
        starts(&runtime.drain()).is_empty(),
        "not before its arguments"
    );
    shared.on_update(
        &json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "title": "mcp__termide__shell", "rawInput": { "command": "ls" } }),
    );
    shared.on_update(
        &json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1",
            "status": "completed", "content": [] }),
    );
    let events = runtime.drain();
    let started = starts(&events);
    assert_eq!(started.len(), 1, "{events:?}");
    assert_eq!(started[0].name, "shell");
    assert_eq!(started[0].arguments, json!({ "command": "ls" }));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. })));
}

/// The agent's calls reach the session log as the built-in loop's do:
/// calls made together in one message ahead of their results, and a call
/// the turn left unfinished answered with an error, never left open.
#[test]
fn calls_are_logged_with_their_results_like_the_native_loops() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    let shared = &runtime.shared;
    let call = |id: &str, command: &str| {
        json!({ "sessionUpdate": "tool_call", "toolCallId": id,
                "title": "mcp__termide__shell", "kind": "other",
                "rawInput": { "command": command }, "status": "pending" })
    };
    let done = |id: &str, text: &str| {
        json!({ "sessionUpdate": "tool_call_update", "toolCallId": id, "status": "completed",
                "content": [{ "type": "content", "content": { "type": "text", "text": text } }] })
    };
    shared.on_update(&call("t1", "ls"));
    shared.on_update(&call("t2", "pwd"));
    shared.on_update(&done("t2", "/p"));
    shared.on_update(&done("t1", "a.rs"));
    shared.on_update(&call("t3", "sleep 9"));
    shared.close_unfinished_calls();

    let logged: Vec<String> = runtime
        .drain()
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::MessageEnd(Message::Assistant(a)) => Some(format!(
                "calls:{}",
                a.tool_calls()
                    .map(|c| format!("{}={}", c.id, c.arguments["command"].as_str().unwrap_or("")))
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            AgentEvent::MessageEnd(Message::ToolResult(r)) => Some(format!(
                "result:{}:{}:{}",
                r.tool_call_id,
                r.is_error,
                r.plain_text()
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        logged,
        [
            "calls:t1=ls,t2=pwd",
            "result:t2:false:/p",
            "result:t1:false:a.rs",
            "calls:t3=sleep 9",
            "result:t3:true:The turn ended before this call finished.",
        ]
    );
}

#[test]
fn the_reported_context_fill_and_size_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::Codex,
        None,
        ModeHandle::new(Mode::default()),
    );
    assert_eq!(runtime.context_usage(), None);
    runtime
        .shared
        .on_update(&json!({ "sessionUpdate": "usage_update", "used": 975, "size": 1_000_000 }));
    assert_eq!(runtime.context_usage(), Some((975, 1_000_000)));
    // Limited by the user, the window is shown no larger than the limit.
    runtime
        .shared
        .context_limit
        .store(200_000, Ordering::Release);
    runtime
        .shared
        .on_update(&json!({ "sessionUpdate": "usage_update", "used": 975, "size": 1_000_000 }));
    assert_eq!(runtime.context_usage(), Some((975, 200_000)));
}

/// Claude Code's rows by model count more than its turn's `usage`: what
/// they hold beyond it is told apart by model, the turn taken out of the
/// session's model's row; Codex's single row is the usage itself.
#[test]
fn what_a_turn_spent_beyond_its_usage_is_read_by_model() {
    let row = |model: &str, input: u64, output: u64, read: u64, write: u64| {
        json!({ "model": model, "token_count": {
                "inputTokens": input, "outputTokens": output,
                "cachedInputTokens": read, "cachedWriteTokens": write,
                "reasoningOutputTokens": 0,
            } })
    };
    let usage = Usage {
        input: 10,
        output: 20,
        cache_read: 300,
        cache_write: 40,
        thinking: 0,
    };
    let claude = json!({ "stopReason": "end_turn", "_meta": { "quota": { "model_usage": [
            row("claude-haiku-4-5", 5, 1, 0, 0),
            row("claude-opus-5[1m]", 15, 30, 300, 40),
        ] } } });
    assert_eq!(
        usage_beyond(&claude, &usage, "opus-5"),
        [
            (
                "claude-haiku-4-5".to_string(),
                Usage {
                    input: 5,
                    output: 1,
                    ..Usage::default()
                }
            ),
            (
                "claude-opus-5".to_string(),
                Usage {
                    input: 5,
                    output: 10,
                    ..Usage::default()
                }
            ),
        ]
    );
    let codex = json!({ "stopReason": "end_turn", "_meta": { "quota": { "model_usage": [{
            "model": "gpt-5.5", "token_count": {
                "inputTokens": 10, "outputTokens": 20, "cachedInputTokens": 300,
                "cacheWriteInputTokens": 40, "reasoningOutputTokens": 0,
            } }] } } });
    assert!(usage_beyond(&codex, &usage, "gpt-5.5").is_empty());
    assert!(usage_beyond(&json!({ "stopReason": "end_turn" }), &usage, "x").is_empty());
}

#[test]
fn a_turns_reported_usage_is_read() {
    let usage = usage_of(
        &json!({ "stopReason": "end_turn", "usage": {
            "inputTokens": 4, "outputTokens": 63, "cachedReadTokens": 919,
            "cachedWriteTokens": 1009, "totalTokens": 1995 } }),
        AcpFlavor::ClaudeCode,
    );
    assert_eq!(
        (
            usage.input,
            usage.output,
            usage.cache_read,
            usage.cache_write
        ),
        (4, 63, 919, 1009)
    );
    assert_eq!(
        usage_of(&json!({ "stopReason": "end_turn" }), AcpFlavor::Codex),
        Usage::default()
    );
}

/// Gemini CLI reports the Gemini API's counts: its prompt holds the
/// cached part, and its output leaves the thoughts out.
#[test]
fn gemini_clis_usage_is_read_as_the_others() {
    let usage = usage_of(
        &json!({ "stopReason": "end_turn", "usage": {
                "inputTokens": 56_200, "outputTokens": 1_870,
                "cachedReadTokens": 41_300, "thoughtTokens": 980,
                "totalTokens": 58_070 } }),
        AcpFlavor::GeminiCli,
    );
    assert_eq!(
        (usage.input, usage.output, usage.cache_read, usage.thinking),
        (14_900, 2_850, 41_300, 980)
    );
}

#[test]
fn codex_is_put_in_the_modes_that_match_the_panels() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::Codex,
        None,
        ModeHandle::new(Mode::Configured),
    );
    let option = |config: &'static str, value: &'static str| {
        move |m: &Value| {
            m["method"] == "session/set_config_option"
                && m["params"]["configId"] == config
                && m["params"]["value"] == value
        }
    };
    // At start: everything asks, so termide decides.
    seen_where(&seen, option("mode", "read-only"));
    seen_where(&seen, option("collaboration_mode", "default"));
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    assert!(
        new["params"].get("_meta").is_none(),
        "Codex keeps its prompt"
    );
    runtime.set_mode(Mode::Edit);
    seen_where(&seen, option("mode", "workspace-write"));
    runtime.set_mode(Mode::All);
    seen_where(&seen, option("mode", "agent-full-access"));
    runtime.set_mode(Mode::Plan);
    seen_where(&seen, option("collaboration_mode", "plan"));
}

/// A tool of termide's under `0`'s name, which does nothing.
struct Named(&'static str);

impl termide_agent_core::Tool for Named {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "Do it"
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }
    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _on_update: &mut dyn FnMut(termide_agent_core::ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        ToolResultMessage::text(call, "done")
    }
}

#[test]
fn the_calls_name_the_panels_session_log_so_recall_leaves_its_context_out() {
    let dir = tempfile::tempdir().unwrap();
    let host = HostTools {
        session_id: Some("log-7".into()),
        ..echo_host()
    };
    let (runtime, _seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        Some(host),
        ModeHandle::new(Mode::default()),
    );
    assert_eq!(runtime.shared.session_view().id.as_deref(), Some("log-7"));
    let (bare, _seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    assert_eq!(bare.shared.session_view().id, None);
}

/// termide's tools, as the panel offers them to every agent.
fn echo_host() -> HostTools {
    let mut tools = termide_agent_core::ToolRegistry::new();
    tools.insert(Arc::new(Echo));
    tools.insert(Arc::new(Named("recall")));
    tools.insert(Arc::new(Named("skill")));
    HostTools {
        tools,
        hooks: Box::new(termide_agent_core::NoHooks),
        context: ToolContext::new(PathBuf::new()),
        skills: vec![termide_agent_core::SkillInfo {
            name: "review".into(),
            description: "Review a change".into(),
            argument_hint: "<path>".into(),
            model_invocable: true,
            path: PathBuf::new(),
        }],
        session_id: None,
    }
}

#[test]
fn codex_and_gemini_keep_their_tools_and_are_served_the_mcp_servers() {
    for flavor in [AcpFlavor::Codex, AcpFlavor::GeminiCli] {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = recording_agent(
            dir.path().to_path_buf(),
            flavor,
            Some(echo_host()),
            ModeHandle::new(Mode::Configured),
        );
        // It lists its tools once, so its session waits until termide's
        // MCP servers have answered.
        std::thread::sleep(Duration::from_millis(100));
        assert!(!seen
            .lock()
            .unwrap()
            .iter()
            .any(|m| m["method"] == "session/new"));
        runtime.host_tools_settled();
        let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
        let params = &new["params"];
        assert!(params.get("_meta").is_none(), "it keeps its own prompt");
        let server = &params["mcpServers"][0];
        assert_eq!(server["name"], "termide");
        assert!(!runtime.runs_host_tools());
        assert!(runtime.takes_mcp_tools());
        // Of termide's own tools, only those it has no counterpart of:
        // `echo` would stand beside its own.
        let url = server["url"].as_str().unwrap();
        let auth = server["headers"][0]["value"].as_str().unwrap();
        assert_eq!(served_tools(url, auth), ["recall", "skill"]);
        // The skills are listed where its own prompt does not.
        let listed = post_mcp(
            url,
            auth,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        );
        let skill = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "skill")
            .unwrap()
            .clone();
        assert!(
            skill["description"]
                .as_str()
                .unwrap()
                .ends_with("- review <path>: Review a change"),
            "{skill}"
        );
        // An MCP server of termide's connected: its tools are served.
        let mut mcp = termide_agent_core::ToolRegistry::new();
        mcp.insert(Arc::new(Echo));
        runtime.update_host_tools(mcp).unwrap();
        assert_eq!(served_tools(url, auth), ["echo"]);
    }
}

#[test]
fn a_codex_call_of_termides_tool_shows_as_it_and_is_not_asked_twice() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _seen) = recording_agent_with(
        dir.path().to_path_buf(),
        AcpFlavor::Codex,
        None,
        ModeHandle::new(Mode::Configured),
        false,
    );
    let shared = &runtime.shared;
    // As Codex sends them: the call names the server and the tool, the
    // permission request names the call alone, the end carries the MCP
    // result.
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "c1", "kind": "execute",
        "status": "in_progress", "title": "mcp.termide.vault__secret_word",
        "rawInput": { "server": "termide", "tool": "vault__secret_word", "arguments": { "a": 1 } },
        "_meta": { "is_mcp_tool_call": true },
    }));
    let answer = shared.ask_permission(&json!({
        "toolCall": { "toolCallId": "c1", "kind": "execute", "status": "pending" },
        "options": [
            { "kind": "allow_once", "optionId": "allow_once" },
            { "kind": "reject_once", "optionId": "cancel" },
        ],
        "_meta": { "is_mcp_tool_approval": true },
    }));
    assert_eq!(answer["outcome"]["optionId"], "allow_once");
    shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "completed",
        "rawOutput": { "result": { "content": [{ "type": "text", "text": "pineapple" }] } },
    }));
    let events = runtime.drain();
    let started = events.iter().find_map(|e| match e {
        AgentEvent::ToolExecutionStart { call } => Some(call.clone()),
        _ => None,
    });
    let started = started.expect("the call starts");
    assert_eq!(
        (started.name.as_str(), &started.arguments),
        ("vault__secret_word", &json!({ "a": 1 }))
    );
    let ended = events.iter().find_map(|e| match e {
        AgentEvent::ToolExecutionEnd { result } => Some(result.clone()),
        _ => None,
    });
    let ended = ended.expect("the call ends");
    assert_eq!(
        (ended.tool_name.as_str(), ended.plain_text().as_str()),
        ("vault__secret_word", "pineapple")
    );
    // Ended, it is a call like any other again.
    assert!(shared.host_calls.lock().unwrap().is_empty());
}

#[test]
fn codex_asking_about_an_announced_edit_by_its_id_is_judged_as_that_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("report.py");
    let (runtime, _seen) = recording_agent_with(
        dir.path().to_path_buf(),
        AcpFlavor::Codex,
        None,
        ModeHandle::new(Mode::Edit),
        false,
    );
    let shared = &runtime.shared;
    // As Codex sends them: the file change is announced with its diff and
    // no raw input, and the request that follows names its id and paths.
    shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "p1", "kind": "edit",
        "status": "pending", "title": "Editing files",
        "content": [{ "type": "diff", "path": path, "oldText": "a\n", "newText": "b\n" }],
    }));
    let answer = shared.ask_permission(&json!({
            "toolCall": { "toolCallId": "p1", "title": "Editing files", "locations": [{ "path": path }] },
            "options": [
                { "kind": "allow_once", "optionId": "approved" },
                { "kind": "reject_once", "optionId": "abort" },
            ],
        }));
    // An edit inside the project passes in `edit` mode without a card.
    assert_eq!(answer["outcome"]["optionId"], "approved");
}

#[test]
fn an_agent_without_http_servers_is_served_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent_with(
        dir.path().to_path_buf(),
        AcpFlavor::Codex,
        Some(echo_host()),
        ModeHandle::new(Mode::Configured),
        false,
    );
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    assert_eq!(new["params"]["mcpServers"], json!([]));
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.takes_mcp_tools() {
        assert!(Instant::now() < deadline, "it still claims the tools");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(runtime
        .update_host_tools(termide_agent_core::ToolRegistry::new())
        .is_err());
}

#[test]
fn gemini_cli_is_put_in_the_mode_that_matches_the_panels() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::GeminiCli,
        None,
        ModeHandle::new(Mode::Configured),
    );
    let mode = |value: &'static str| {
        move |m: &Value| m["method"] == "session/set_mode" && m["params"]["modeId"] == value
    };
    // At start: it asks, so termide decides.
    seen_where(&seen, mode("default"));
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    assert!(
        new["params"].get("_meta").is_none(),
        "Gemini CLI keeps its prompt"
    );
    runtime.set_mode(Mode::Edit);
    seen_where(&seen, mode("autoEdit"));
    runtime.set_mode(Mode::All);
    seen_where(&seen, mode("yolo"));
    runtime.set_mode(Mode::Plan);
    seen_where(&seen, mode("plan"));
}

#[test]
fn another_agent_is_left_to_its_own_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::Generic,
        None,
        ModeHandle::new(Mode::default()),
    );
    let new = seen_where(&seen, |m| m["method"] == "session/new").remove(0);
    assert_eq!(
        new["params"],
        json!({ "cwd": dir.path(), "mcpServers": [] })
    );
    assert!(!runtime.follows_mode());
}

fn models_agent_with(dir: PathBuf, advertised: Value) -> AcpRuntime {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let mut reader = BufReader::new(to_agent_rx).lines();
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            let id = message["id"].clone();
            match message["method"].as_str() {
                Some("initialize") => send(
                    json!({ "jsonrpc": "2.0", "id": id, "result": { "protocolVersion": 1, "agentCapabilities": {} } }),
                ),
                Some("session/new") => {
                    let mut result = advertised.clone();
                    result["sessionId"] = json!("s1");
                    send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                }
                Some("session/set_model") => {
                    assert_eq!(message["params"]["modelId"], "m-slow");
                    send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                }
                Some("session/set_config_option") => {
                    assert_eq!(message["params"]["configId"], "model");
                    assert_eq!(message["params"]["value"], "m-slow");
                    send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                }
                _ => {}
            }
        }
    });
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        5,
        AcpFlavor::Generic,
    )
}

#[test]
fn models_are_read_from_the_session_and_switched_over_acp() {
    for agent in [models_agent, config_option_agent] {
        let dir = tempfile::tempdir().unwrap();
        let runtime = agent(dir.path().to_path_buf());
        switches_between_fast_and_slow(&runtime);
    }
}

/// The config options as an agent states them, effort at `effort`.
fn stated_options(effort: &str) -> Value {
    json!([
        { "id": "model", "category": "model", "type": "select", "currentValue": "m-fast",
          "options": [{ "value": "m-fast", "name": "Fast" }] },
        { "id": "effort", "name": "Effort", "category": "thought_level", "type": "select",
          "currentValue": effort,
          "options": [{ "value": "low", "name": "Low" }, { "value": "high", "name": "High" }] },
        { "id": "brave", "name": "Brave", "type": "boolean", "currentValue": true }
    ])
}

#[test]
fn options_are_listed_and_set_while_a_turn_runs() {
    let dir = tempfile::tempdir().unwrap();
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let mut reader = BufReader::new(to_agent_rx).lines();
        let mut prompt_id = None;
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            let id = message["id"].clone();
            let result = match message["method"].as_str() {
                Some("initialize") => json!({ "protocolVersion": 1, "agentCapabilities": {} }),
                Some("session/new") => {
                    send(
                        json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": "s1", "update": {
                                "sessionUpdate": "available_commands_update",
                                "availableCommands": [
                                    { "name": "compact", "description": "Compact the context" },
                                    { "name": "review", "description": "Review", "input": { "hint": "<path>" } }
                                ] } } }),
                    );
                    json!({ "sessionId": "s1", "configOptions": stated_options("low") })
                }
                // The turn stays open until the option is set.
                Some("session/prompt") => {
                    prompt_id = Some(id);
                    continue;
                }
                Some("session/set_config_option") => {
                    assert_eq!(message["params"]["configId"], "effort");
                    let value = message["params"]["value"].as_str().unwrap();
                    send(json!({ "jsonrpc": "2.0", "id": id,
                            "result": { "configOptions": stated_options(value) } }));
                    if let Some(prompt) = prompt_id.take() {
                        send(json!({ "jsonrpc": "2.0", "id": prompt,
                                "result": { "stopReason": "end_turn" } }));
                    }
                    continue;
                }
                _ => continue,
            };
            send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    });
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir.path().to_path_buf(),
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        5,
        AcpFlavor::Generic,
    );
    runtime.prompt(UserMessage::text("go")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.config_options().is_empty() {
        assert!(Instant::now() < deadline, "options never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The model is listed as models, a boolean is not understood: the
    // effort alone remains.
    let options = runtime.config_options();
    assert_eq!(options.len(), 1, "{options:?}");
    assert!(options[0].is("thought_level"));
    assert_eq!(options[0].current_name(), "Low");
    assert_eq!(runtime.current_model(), Some("m-fast".to_string()));
    // Its own commands are listed.
    let commands = runtime.agent_commands();
    assert_eq!(
        commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        ["compact", "review"]
    );
    assert_eq!(commands[1].hint, "<path>");

    assert!(runtime.is_busy());
    runtime
        .set_option("effort".to_string(), "high".to_string())
        .unwrap();
    assert_eq!(runtime.config_options()[0].current, "high");
}

/// An agent that steers: its turn for "first" streams a word, then waits
/// for a `_session/steering` request, answers it `outcome`, streams
/// another word and ends. Every prompt's text is recorded. With
/// `"injected, call dropped"` it starts a call after the word and, the
/// message taken, goes on without ever answering it, as Claude Code
/// does with a call it was streaming.
fn steering_agent(dir: PathBuf, outcome: &'static str) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let update = |u: Value| json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s1", "update": u } });
        let chunk = |text: &str| {
            update(
                json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } }),
            )
        };
        let mut reader = BufReader::new(to_agent_rx).lines();
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            record.lock().unwrap().push(message.clone());
            let id = message["id"].clone();
            match message["method"].as_str() {
                Some("initialize") => send(json!({ "jsonrpc": "2.0", "id": id, "result": {
                        "protocolVersion": 1, "agentCapabilities": {},
                        "_meta": { "steering": { "supported": true } } } })),
                Some("session/new") => {
                    send(json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": "s1" } }));
                }
                Some("session/prompt") => {
                    let text = message["params"]["prompt"][0]["text"]
                        .as_str()
                        .unwrap_or("");
                    if text != "first" {
                        send(chunk("later"));
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                        );
                        continue;
                    }
                    send(chunk("working"));
                    if outcome == "injected, call dropped" {
                        send(update(
                            json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "comment", "kind": "other", "status": "in_progress", "rawInput": { "issue": "X-1" } }),
                        ));
                    }
                    let steer = loop {
                        let Some(Ok(line)) = reader.next() else {
                            return;
                        };
                        let m: Value = serde_json::from_str(&line).unwrap();
                        record.lock().unwrap().push(m.clone());
                        if m["method"] == "_session/steering" {
                            break m;
                        }
                    };
                    if outcome.starts_with("injected") {
                        send(
                            json!({ "jsonrpc": "2.0", "id": steer["id"], "result": { "outcome": "injected" } }),
                        );
                        send(chunk("noted"));
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                        );
                    } else {
                        // The turn is over before the message could join it.
                        send(
                            json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                        );
                        send(
                            json!({ "jsonrpc": "2.0", "id": steer["id"], "result": { "outcome": outcome, "reason": "noRunningTurn" } }),
                        );
                    }
                }
                _ => {}
            }
        }
    });
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        5,
        AcpFlavor::Generic,
    );
    (runtime, seen)
}

/// Run "first", steer "more" once the turn streams, and collect the
/// events until the run ends.
fn steer_into_first(runtime: &AcpRuntime) -> Vec<AgentEvent> {
    runtime.prompt(UserMessage::text("first")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    let mut steered = false;
    loop {
        events.extend(runtime.drain());
        let streaming = events.iter().any(
            |e| matches!(e, AgentEvent::MessageUpdate(StreamEvent::TextDelta(t)) if t == "working"),
        );
        if streaming && !steered {
            runtime.steer(UserMessage::text("more"));
            steered = true;
        }
        if steered
            && events
                .iter()
                .filter(|e| matches!(e, AgentEvent::AgentEnd))
                .count()
                >= 1
        {
            return events;
        }
        assert!(Instant::now() < deadline, "no end: {events:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The messages logged, in order: `user:` or `assistant:` and the text.
fn logged(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageEnd(Message::User(u)) => Some(format!("user:{}", u.plain_text())),
            AgentEvent::MessageEnd(Message::Assistant(a)) => {
                Some(format!("assistant:{}", a.plain_text()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_message_typed_during_a_turn_joins_it_when_the_agent_steers() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = steering_agent(dir.path().to_path_buf(), "injected");
    let events = steer_into_first(&runtime);
    assert_eq!(
        logged(&events),
        [
            "user:first",
            "assistant:working",
            "user:more",
            "assistant:noted"
        ]
    );
    let steering = seen_where(&seen, |m| m["method"] == "_session/steering");
    assert_eq!(steering[0]["params"]["prompt"][0]["text"], "more");
    assert_eq!(
        steering[0]["params"]["_meta"]["steering"]["idleBehavior"],
        "promptRequired"
    );
    // It went into the turn, not as a turn of its own.
    let prompts = seen_where(&seen, |m| m["method"] == "session/prompt");
    assert_eq!(prompts.len(), 1);
    assert_eq!(runtime.queue_lens(), (0, 0));
}

/// A call the agent drops when the message comes in, never answered,
/// does not hold the message back to the turn's end: it is logged
/// before the answer to it, and stays in the queue strip until then.
#[test]
fn a_message_taken_past_a_dropped_call_is_logged_before_its_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _seen) = steering_agent(dir.path().to_path_buf(), "injected, call dropped");
    let events = steer_into_first(&runtime);
    let logged: Vec<String> = logged(&events)
        .into_iter()
        .filter(|m| m != "assistant:")
        .collect();
    assert_eq!(
        logged,
        [
            "user:first",
            "assistant:working",
            "user:more",
            "assistant:noted"
        ]
    );
    let dropped = events.iter().find_map(|e| match e {
        AgentEvent::MessageEnd(Message::ToolResult(r)) if r.tool_call_id == "t1" => Some(r),
        _ => None,
    });
    assert!(dropped.is_some_and(|r| r.is_error), "{events:?}");
    // The strip never empties while "more" is out of sight.
    let shown = events
        .iter()
        .position(
            |e| matches!(e, AgentEvent::MessageEnd(Message::User(u)) if u.plain_text() == "more"),
        )
        .unwrap();
    let first_steer = events
        .iter()
        .position(|e| matches!(e, AgentEvent::QueueUpdate { steering: 1, .. }))
        .unwrap();
    assert!(
        !events[first_steer..shown]
            .iter()
            .any(|e| matches!(e, AgentEvent::QueueUpdate { steering: 0, .. })),
        "{events:?}"
    );
    assert_eq!(runtime.queue_lens(), (0, 0));
}

#[test]
fn a_message_the_turn_could_not_take_goes_as_the_next_turn() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = steering_agent(dir.path().to_path_buf(), "promptRequired");
    let mut events = steer_into_first(&runtime);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !logged(&events).contains(&"assistant:later".to_string()) {
        assert!(Instant::now() < deadline, "no second turn: {events:?}");
        std::thread::sleep(Duration::from_millis(5));
        events.extend(runtime.drain());
    }
    assert_eq!(
        logged(&events),
        [
            "user:first",
            "assistant:working",
            "user:more",
            "assistant:later"
        ]
    );
    let prompts = seen_where(&seen, |m| {
        m["method"] == "session/prompt" && m["params"]["prompt"][0]["text"] == "more"
    });
    assert_eq!(prompts.len(), 1);
}

/// An agent that forks and closes sessions when `forks` says so, and
/// answers every prompt with `answer`, streamed in the prompt's session.
fn side_agent(
    dir: PathBuf,
    forks: bool,
    answer: &'static str,
) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let mut reader = BufReader::new(to_agent_rx).lines();
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            record.lock().unwrap().push(message.clone());
            let id = message["id"].clone();
            let result = match message["method"].as_str() {
                Some("initialize") => {
                    let sessions = if forks {
                        json!({ "fork": {}, "close": {} })
                    } else {
                        json!({})
                    };
                    json!({ "protocolVersion": 1,
                            "agentCapabilities": { "sessionCapabilities": sessions } })
                }
                Some("session/new") => json!({ "sessionId": "s1" }),
                Some("session/fork") => json!({ "sessionId": "f1" }),
                Some("session/prompt") => {
                    let session = message["params"]["sessionId"].clone();
                    send(
                        json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": session, "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": answer } } } }),
                    );
                    json!({ "stopReason": "end_turn" })
                }
                Some(_) => json!({}),
                None => continue,
            };
            send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    });
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        5,
        AcpFlavor::Generic,
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.shared.session_id().is_err() {
        assert!(Instant::now() < deadline, "the session never opened");
        std::thread::sleep(Duration::from_millis(5));
    }
    (runtime, seen)
}

/// The events until one `wanted` arrives.
fn events_until(runtime: &AcpRuntime, wanted: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while !events.iter().any(&wanted) {
        assert!(Instant::now() < deadline, "never came: {events:?}");
        std::thread::sleep(Duration::from_millis(5));
        events.extend(runtime.drain());
    }
    events
}

#[test]
fn the_goal_judge_asks_a_fork_of_the_session_and_closes_it() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = side_agent(dir.path().to_path_buf(), true, "DONE\nthe tests pass");
    runtime.judge("make the tests pass".into()).unwrap();
    let events = events_until(&runtime, |e| matches!(e, AgentEvent::GoalJudged { .. }));
    assert!(events.contains(&AgentEvent::GoalJudged {
        outcome: termide_agent_core::GoalOutcome::Done,
        reason: "the tests pass".into(),
        reply: "DONE\nthe tests pass".into(),
    }));
    // Nothing of it reached the transcript.
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::MessageEnd(_))));
    let prompt = seen_where(&seen, |m| m["method"] == "session/prompt").remove(0);
    assert_eq!(prompt["params"]["sessionId"], "f1");
    let text = prompt["params"]["prompt"][0]["text"].as_str().unwrap();
    assert!(text.contains("make the tests pass"), "{text}");
    let close = seen_where(&seen, |m| m["method"] == "session/close").remove(0);
    assert_eq!(close["params"]["sessionId"], "f1");
    assert!(!runtime.is_busy());
}

#[test]
fn a_handoff_without_a_fork_is_a_turn_kept_from_the_panel() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = side_agent(dir.path().to_path_buf(), false, "The brief.");
    runtime.handoff().unwrap();
    let events = events_until(&runtime, |e| matches!(e, AgentEvent::Handoff { .. }));
    assert!(events.contains(&AgentEvent::Handoff {
        brief: Ok("The brief.".into())
    }));
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::MessageEnd(_))));
    let prompt = seen_where(&seen, |m| m["method"] == "session/prompt").remove(0);
    assert_eq!(prompt["params"]["sessionId"], "s1");
    assert!(prompt["params"]["prompt"][0]["text"]
        .as_str()
        .unwrap()
        .ends_with(&HandoffPrompt::default().request));
    assert!(!seen
        .lock()
        .unwrap()
        .iter()
        .any(|m| m["method"] == "session/fork"));
    // The session is free again for the next request.
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.is_busy() {
        assert!(Instant::now() < deadline, "still busy");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A reviewer that answers `verdict` and keeps what it was shown.
struct Reviewer {
    verdict: &'static str,
    shown: Arc<Mutex<Vec<String>>>,
}

impl Provider for Reviewer {
    fn name(&self) -> &str {
        "reviewer"
    }
    fn stream(
        &self,
        request: &termide_agent_core::Request<'_>,
        _on_event: &mut dyn FnMut(StreamEvent),
        _cancel: &CancelToken,
    ) -> AssistantMessage {
        for message in request.messages {
            if let Message::User(user) = message {
                self.shown.lock().unwrap().push(user.plain_text());
            }
        }
        AssistantMessage {
            content: vec![AssistantContent::Text {
                text: self.verdict.into(),
            }],
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            provider: "reviewer".into(),
            model: request.model.id.clone(),
            error_message: None,
            failure: None,
            timestamp: 0,
        }
    }
}

/// An agent whose turn asks to run `cargo build` and reports the option
/// it was answered with.
fn asking_agent(
    dir: PathBuf,
    mode: Mode,
    reviewer: ReviewerSetup,
) -> (AcpRuntime, Receiver<PermissionEnvelope>) {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let mut reader = BufReader::new(to_agent_rx).lines();
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            let id = message["id"].clone();
            match message["method"].as_str() {
                Some("initialize") => send(json!({ "jsonrpc": "2.0", "id": id,
                        "result": { "protocolVersion": 1, "agentCapabilities": {} } })),
                Some("session/new") => send(json!({ "jsonrpc": "2.0", "id": id,
                        "result": { "sessionId": "s1" } })),
                Some("session/prompt") => {
                    send(
                        json!({ "jsonrpc": "2.0", "id": 900, "method": "session/request_permission",
                            "params": { "sessionId": "s1",
                                "toolCall": { "toolCallId": "t1", "title": "cargo build", "kind": "execute",
                                    "rawInput": { "command": "cargo build" } },
                                "options": [
                                    { "optionId": "yes", "name": "Allow", "kind": "allow_once" },
                                    { "optionId": "no", "name": "Reject", "kind": "reject_once" } ] } }),
                    );
                    let answer: Value = loop {
                        let Some(Ok(line)) = reader.next() else {
                            return;
                        };
                        let m: Value = serde_json::from_str(&line).unwrap();
                        if m["id"] == 900 {
                            break m;
                        }
                    };
                    let chosen = answer["result"]["outcome"]["optionId"]
                        .as_str()
                        .unwrap_or("cancelled")
                        .to_string();
                    send(
                        json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": "s1", "update": { "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": chosen } } } }),
                    );
                    send(
                        json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } }),
                    );
                }
                Some(_) if !id.is_null() => {
                    send(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
                }
                _ => {}
            }
        }
    });
    let cancel = CancelToken::new();
    let (prompter, permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(mode),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer,
            host_tools: None,
            resume: None,
            history: vec![Message::User(UserMessage::text("set the project up"))],
        },
        5,
        AcpFlavor::Codex,
    );
    (runtime, permissions)
}

#[test]
fn in_auto_mode_the_reviewer_answers_an_external_agents_request() {
    for (verdict, chosen) in [
        ("ALLOW it builds the project", "yes"),
        ("BLOCK not asked for", "no"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let shown = Arc::new(Mutex::new(Vec::new()));
        let reviewer = ReviewerSetup {
            model: termide_agent_core::ModelChoice::own(
                Arc::new(Reviewer {
                    verdict,
                    shown: Arc::clone(&shown),
                }),
                ModelSpec {
                    provider: "reviewer".into(),
                    id: "small".into(),
                    context_window: 0,
                    max_tokens: None,
                    thinking: ThinkingLevel::Off,
                },
            ),
            ..ReviewerSetup::default()
        };
        let (runtime, permissions) = asking_agent(dir.path().to_path_buf(), Mode::Auto, reviewer);
        runtime.prompt(UserMessage::text("build it")).unwrap();
        let events = events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
        // Nobody was asked; the reviewer decided.
        assert!(permissions.try_recv().is_err());
        assert!(
            logged(&events).contains(&format!("assistant:{chosen}")),
            "{events:?}"
        );
        // It judged against what the user asked, earlier and now.
        let shown = shown.lock().unwrap().join("\n");
        assert!(
            shown.contains("set the project up") && shown.contains("build it"),
            "{shown}"
        );
    }
}

#[test]
fn without_a_model_to_review_with_the_user_is_asked() {
    let dir = tempfile::tempdir().unwrap();
    // Started from streams: no subscription is "the session's model".
    let (runtime, permissions) = asking_agent(
        dir.path().to_path_buf(),
        Mode::Auto,
        ReviewerSetup::default(),
    );
    runtime.prompt(UserMessage::text("build it")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let envelope = loop {
        if let Ok(envelope) = permissions.try_recv() {
            break envelope;
        }
        assert!(Instant::now() < deadline, "nobody was asked");
        std::thread::sleep(Duration::from_millis(5));
    };
    envelope.reply.send(PermissionAnswer::AllowOnce).unwrap();
    let events = events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
    assert!(logged(&events).contains(&"assistant:yes".to_string()));
}

fn switches_between_fast_and_slow(runtime: &AcpRuntime) {
    // The handshake runs on a thread; wait for the advertised models.
    let deadline = Instant::now() + Duration::from_secs(5);
    while runtime.available_models().is_empty() {
        assert!(Instant::now() < deadline, "models never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        runtime.available_models(),
        vec![
            BackendModel {
                id: "m-fast".into(),
                name: "Fast".into()
            },
            BackendModel {
                id: "m-slow".into(),
                name: "Slow".into()
            },
        ]
    );
    assert_eq!(runtime.current_model(), Some("m-fast".to_string()));

    runtime.select_model("m-slow".to_string()).unwrap();
    assert_eq!(runtime.current_model(), Some("m-slow".to_string()));
}

#[test]
fn a_turn_streams_asks_edits_and_ends_like_the_native_loop() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, permissions) = fake_agent(dir.path().to_path_buf());
    // Sent before the handshake finished: queued, then delivered.
    runtime.prompt(UserMessage::text("first")).unwrap();
    assert!(runtime.is_busy());
    assert_eq!(
        runtime.prompt(UserMessage::text("x")),
        Err(PromptError::Busy)
    );
    let events = drain_until_end(&runtime, &permissions, PermissionAnswer::AllowSession);
    assert!(!runtime.is_busy());
    // The reasoning is kept with the text it led to, for the log.
    let first = events.iter().find_map(|e| match e {
        AgentEvent::MessageEnd(Message::Assistant(a)) => Some(a),
        _ => None,
    });
    assert_eq!(
        first.map(AssistantMessage::thinking_text).as_deref(),
        Some("hmm")
    );

    let mut kinds: Vec<String> = events
        .iter()
        .map(|e| match e {
            AgentEvent::AgentStart => "start".into(),
            AgentEvent::TurnStart => "turn".into(),
            AgentEvent::MessageStart { .. } => "msg-start".into(),
            AgentEvent::MessageUpdate(StreamEvent::TextDelta(t)) => format!("text:{t}"),
            AgentEvent::MessageUpdate(StreamEvent::ThinkingDelta(_)) => "think".into(),
            AgentEvent::MessageUpdate(_) => "update".into(),
            AgentEvent::MessageEnd(Message::User(u)) => format!("user:{}", u.plain_text()),
            AgentEvent::MessageEnd(Message::Assistant(a)) => {
                let calls: Vec<&str> = a.tool_calls().map(|c| c.name.as_str()).collect();
                if calls.is_empty() {
                    format!("assistant:{}:{:?}", a.plain_text(), a.stop_reason)
                } else {
                    format!("calls:{}", calls.join(","))
                }
            }
            AgentEvent::MessageEnd(Message::ToolResult(r)) => format!("result:{}", r.tool_name),
            AgentEvent::ToolExecutionStart { call } => {
                format!("tool-start:{}:{}", call.name, call.arguments["title"])
            }
            AgentEvent::ToolExecutionUpdate { .. } => "tool-update".into(),
            AgentEvent::ToolExecutionEnd { result } => format!(
                "tool-end:{}:{}",
                result.tool_name,
                result
                    .details
                    .as_ref()
                    .map(|d| d["path"].as_str().unwrap_or("").ends_with("notes.md"))
                    .unwrap_or(false)
            ),
            AgentEvent::TurnEnd => "turn-end".into(),
            AgentEvent::QueueUpdate { .. } => "queue".into(),
            AgentEvent::AgentEnd => "end".into(),
            AgentEvent::ExternalSession { session_id, .. } => format!("session:{session_id}"),
            _ => "other".into(),
        })
        .collect();
    // The handshake runs on its own thread, so where the session it
    // opened is reported among the turn's first events is not fixed.
    let opened = kinds.iter().position(|k| k == "session:s1");
    assert!(opened.is_some(), "the new session is reported: {kinds:?}");
    kinds.remove(opened.unwrap());
    assert_eq!(
        kinds,
        [
            "start",
            "turn",
            "user:first",
            "queue",
            "msg-start",
            "think",
            "text:Editing ",
            "text:now.",
            "assistant:Editing now.:ToolUse",
            "tool-start:edit:\"Write notes.md\"",
            "tool-start:write:null",
            "tool-end:write:true",
            "tool-end:edit:true",
            // The agent's call reaches the log as the built-in loop's
            // would; the client-side write it made is not a call of its.
            "calls:edit",
            "result:edit",
            "msg-start",
            "text:Done.",
            "assistant:Done.:Stop",
            "turn-end",
            "queue",
            "end"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("notes.md")).unwrap(),
        "one\ntwo\nthree\n"
    );
    assert_eq!(
        runtime.update(Box::new(|_| {})),
        Err(PromptError::Unsupported)
    );

    // The second prompt is cancelled through session/cancel.
    runtime.prompt(UserMessage::text("second")).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    runtime.steer(UserMessage::text("later"));
    assert_eq!(runtime.queue_lens(), (1, 0));
    runtime.abort();
    let events = drain_until_end(&runtime, &permissions, PermissionAnswer::Deny);
    assert!(events.iter().any(|e| matches!(e, AgentEvent::MessageEnd(Message::Assistant(a)) if a.stop_reason == StopReason::Aborted)));
    // An abort drops what was steered.
    assert_eq!(runtime.queue_lens(), (0, 0));
}

/// A message typed while a turn runs leaves the queue strip as soon as
/// the next turn takes it, not when that turn ends.
#[test]
fn a_message_taken_by_the_next_turn_leaves_the_queue_at_once() {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let (prompted_tx, prompted_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut prompts = 0;
        for line in BufReader::new(to_agent_rx).lines() {
            let Ok(line) = line else { break };
            let message: Value = serde_json::from_str(&line).unwrap();
            let result = match message["method"].as_str() {
                Some("initialize") => json!({ "protocolVersion": 1 }),
                Some("session/new") => json!({ "sessionId": "s1" }),
                Some("session/prompt") => {
                    prompts += 1;
                    if prompts == 1 {
                        // The first turn runs until the test lets it end.
                        prompted_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                    }
                    json!({ "stopReason": "end_turn" })
                }
                Some(_) => json!({}),
                None => continue,
            };
            let reply = json!({ "jsonrpc": "2.0", "id": message["id"], "result": result });
            writeln!(out, "{reply}").unwrap();
        }
    });
    let cancel = CancelToken::new();
    let (prompter, permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: PathBuf::from("/tmp"),
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        1,
        AcpFlavor::Generic,
    );
    runtime.prompt(UserMessage::text("first")).unwrap();
    prompted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    runtime.steer(UserMessage::text("later"));
    release_tx.send(()).unwrap();
    let events = drain_until_end(&runtime, &permissions, PermissionAnswer::Deny);

    let taken = events
        .iter()
        .position(
            |e| matches!(e, AgentEvent::MessageEnd(Message::User(u)) if u.plain_text() == "later"),
        )
        .expect("the queued message runs as the next turn");
    let after: Vec<&AgentEvent> = events[taken..]
        .iter()
        .filter(|e| matches!(e, AgentEvent::QueueUpdate { .. } | AgentEvent::TurnEnd))
        .collect();
    assert!(
        matches!(
            after.first(),
            Some(AgentEvent::QueueUpdate {
                steering: 0,
                follow_up: 0
            })
        ),
        "the queue empties before the turn ends: {after:?}"
    );
}

/// A usage limit is classified from the error's `errorKind` and text, its
/// reset taken from the `_claude/rateLimit` Claude Code reported; a retry
/// sends the failed turn's blocks again and logs no new message.
#[test]
fn a_limited_turn_is_classified_and_retried_as_it_was() {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    let prompts = Arc::new(Mutex::new(Vec::<Value>::new()));
    let record = Arc::clone(&prompts);
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        for line in BufReader::new(to_agent_rx).lines() {
            let Ok(line) = line else { break };
            let message: Value = serde_json::from_str(&line).unwrap();
            let id = message["id"].clone();
            let reply = match message["method"].as_str() {
                Some("initialize") => json!({ "result": { "protocolVersion": 1 } }),
                Some("session/new") => json!({ "result": { "sessionId": "s1" } }),
                Some("session/prompt") => {
                    let mut seen = record.lock().unwrap();
                    seen.push(message["params"]["prompt"].clone());
                    if seen.len() == 1 {
                        let update = json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                                "sessionId": "s1",
                                "update": { "sessionUpdate": "usage_update", "used": 10, "size": 100,
                                    "_meta": { "_claude/rateLimit": { "status": "rejected", "resetsAt": 1_900_000_000u64 } } } } });
                        writeln!(out, "{update}").unwrap();
                        json!({ "error": { "code": -32603,
                                "message": "Internal error: You've hit your session limit · resets 9:50pm (Europe/Samara)",
                                "data": { "errorKind": "rate_limit" } } })
                    } else {
                        json!({ "result": { "stopReason": "end_turn" } })
                    }
                }
                Some(_) => json!({ "result": {} }),
                None => continue,
            };
            let mut reply = reply;
            reply["jsonrpc"] = json!("2.0");
            reply["id"] = id;
            writeln!(out, "{reply}").unwrap();
        }
    });
    let cancel = CancelToken::new();
    let (prompter, permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: PathBuf::from("/tmp"),
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        1,
        AcpFlavor::Generic,
    );
    runtime.prompt(UserMessage::text("do it")).unwrap();
    let events = drain_until_end(&runtime, &permissions, PermissionAnswer::Deny);
    let failure = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::MessageEnd(Message::Assistant(a)) => a.failure,
            _ => None,
        })
        .expect("the failure is classified");
    assert_eq!(failure.kind, FailureKind::QuotaExhausted);
    assert_eq!(failure.resets_at, Some(1_900_000_000));

    runtime.retry().unwrap();
    let events = drain_until_end(&runtime, &permissions, PermissionAnswer::Deny);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::MessageEnd(Message::User(_)))),
        "a retry logs no new message: {events:?}"
    );
    let prompts = prompts.lock().unwrap();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[0], prompts[1]);
}

#[test]
fn a_failed_handshake_is_reported_on_the_first_prompt() {
    let (_keep, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    drop(from_agent_tx);
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "dead",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: PathBuf::from("/tmp"),
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        1,
        AcpFlavor::Generic,
    );
    // The handshake fails as soon as the agent's closed stdout is read.
    // Racing that, the first prompt is either accepted (and the failure
    // arrives as an error event) or already refused because the runtime
    // has stopped; both report the dead handshake on the first prompt.
    if runtime.prompt(UserMessage::text("hello")).is_ok() {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while !events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
            events.extend(runtime.drain());
            assert!(Instant::now() < deadline, "{events:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(events.iter().any(|e| matches!(e, AgentEvent::MessageEnd(Message::Assistant(a)) if a.stop_reason == StopReason::Error && a.error_message.is_some())));
    }
    assert_eq!(
        runtime.prompt(UserMessage::text("again")),
        Err(PromptError::Stopped)
    );
}

#[test]
fn an_execute_request_maps_to_a_shell_call() {
    let call = permission_call(&json!({
        "toolCallId": "t1",
        "title": "Run cat",
        "kind": "execute",
        "rawInput": { "command": "cat notes.txt" }
    }));
    assert_eq!(call.name, "shell");
    assert_eq!(call.arguments["command"], "cat notes.txt");

    // With no raw command, the title stands in so a read-only check works.
    let from_title = permission_call(&json!({
        "toolCallId": "t2", "title": "ls -la", "kind": "execute"
    }));
    assert_eq!(from_title.name, "shell");
    assert_eq!(from_title.arguments["command"], "ls -la");
}

#[test]
fn a_read_only_execute_request_is_allowed_without_asking() {
    // Exactly what `ask_permission` does: map the ACP request, then let
    // the same hooks the built-in agent uses decide it.
    let call = permission_call(&json!({
        "toolCallId": "t1",
        "title": "Run cat",
        "kind": "execute",
        "rawInput": { "command": "cat notes.txt" }
    }));
    let cancel = CancelToken::new();
    let (prompter, rx) = permission_channel(cancel);
    let mut hooks = PermissionHooks::new(PermissionRules::default(), Box::new(prompter));
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    assert_eq!(hooks.before_tool_call(&call, &ctx), ToolDecision::Allow);
    assert!(
        rx.try_recv().is_err(),
        "a read-only command must not reach the user"
    );
}

/// An agent whose `initialize` advertises `capabilities` and which knows
/// session `old`: `session/resume` and `session/load` take it up (unless
/// `forgotten`), the load replaying an exchange first; `session/new`
/// opens `fresh`; a prompt is answered "ok".
fn session_agent(
    dir: PathBuf,
    capabilities: Value,
    forgotten: bool,
    resume: Option<ExternalSessionRef>,
    history: Vec<Message>,
) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
    session_agent_as(
        dir,
        capabilities,
        forgotten,
        resume,
        history,
        AcpFlavor::Generic,
        false,
    )
}

/// [`session_agent`] of `flavor`, its log recording plan mode or not.
fn session_agent_as(
    dir: PathBuf,
    capabilities: Value,
    forgotten: bool,
    resume: Option<ExternalSessionRef>,
    history: Vec<Message>,
    flavor: AcpFlavor,
    plan_seen: bool,
) -> (AcpRuntime, Arc<Mutex<Vec<Value>>>) {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut out = from_agent_tx;
        let mut send = |value: Value| writeln!(out, "{value}").unwrap();
        let mut reader = BufReader::new(to_agent_rx).lines();
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            record.lock().unwrap().push(message.clone());
            let id = message["id"].clone();
            let session = message["params"]["sessionId"].clone();
            let update = |u: Value| json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": session, "update": u } });
            let result = match message["method"].as_str() {
                Some("initialize") => {
                    json!({ "protocolVersion": 1, "agentCapabilities": capabilities })
                }
                Some("session/resume" | "session/load") if forgotten => {
                    send(
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32002, "message": "no such session" } }),
                    );
                    continue;
                }
                Some("session/load") => {
                    send(update(
                        json!({ "sessionUpdate": "user_message_chunk", "content": { "type": "text", "text": "old request" } }),
                    ));
                    send(update(
                        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "old answer" } }),
                    ));
                    send(update(
                        json!({ "sessionUpdate": "tool_call", "toolCallId": "old-call", "title": "Read", "kind": "read", "rawInput": {} }),
                    ));
                    json!({})
                }
                Some("session/resume") => json!({}),
                Some("session/new") => json!({ "sessionId": "fresh" }),
                Some("session/prompt") => {
                    send(update(
                        json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": "ok" } }),
                    ));
                    json!({ "stopReason": "end_turn" })
                }
                Some(_) => json!({}),
                None => continue,
            };
            send(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
        }
    });
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume,
            history,
        },
        5,
        flavor,
    );
    (runtime, seen)
}

fn old_session(agent: &str) -> Option<ExternalSessionRef> {
    Some(ExternalSessionRef {
        agent: agent.into(),
        session_id: "old".into(),
    })
}

fn earlier() -> Vec<Message> {
    vec![
        Message::User(UserMessage::text("earlier request")),
        Message::Assistant(AssistantMessage {
            content: vec![AssistantContent::Text {
                text: "earlier answer".into(),
            }],
            stop_reason: StopReason::Stop,
            usage: Usage::default(),
            provider: ACP_PROVIDER.into(),
            model: "fake".into(),
            error_message: None,
            failure: None,
            timestamp: 0,
        }),
    ]
}

/// Prompt `text`, wait for the turn to end and return every event since
/// the runtime started, with the prompt the agent got.
fn run_prompt(
    runtime: &AcpRuntime,
    seen: &Arc<Mutex<Vec<Value>>>,
    text: &str,
) -> (Vec<AgentEvent>, Vec<Value>) {
    runtime.prompt(UserMessage::text(text)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    while !events.iter().any(|e| matches!(e, AgentEvent::AgentEnd)) {
        assert!(
            Instant::now() < deadline,
            "the turn never ended: {events:?}"
        );
        events.extend(runtime.drain());
        std::thread::sleep(Duration::from_millis(5));
    }
    let sent = seen_where(seen, |m| m["method"] == "session/prompt").remove(0);
    (events, sent["params"]["prompt"].as_array().unwrap().clone())
}

fn methods(seen: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
    seen.lock()
        .unwrap()
        .iter()
        .filter_map(|m| m["method"].as_str().map(str::to_string))
        .collect()
}

fn opened(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ExternalSession { agent, session_id } => {
                Some(format!("{agent}:{session_id}"))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn an_agent_that_resumes_takes_its_own_session_up_again() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = session_agent(
        dir.path().to_path_buf(),
        json!({ "loadSession": true, "sessionCapabilities": { "resume": {} } }),
        false,
        old_session("fake"),
        earlier(),
    );
    let (events, prompt) = run_prompt(&runtime, &seen, "next");
    assert_eq!(
        methods(&seen),
        ["initialize", "session/resume", "session/prompt"]
    );
    let resumed = seen_where(&seen, |m| m["method"] == "session/resume").remove(0);
    assert_eq!(resumed["params"]["sessionId"], "old");
    assert_eq!(resumed["params"]["cwd"], json!(dir.path()));
    // The prompt goes to the resumed session, alone: the agent knows the rest.
    let sent = seen_where(&seen, |m| m["method"] == "session/prompt").remove(0);
    assert_eq!(sent["params"]["sessionId"], "old");
    assert_eq!(prompt, [json!({ "type": "text", "text": "next" })]);
    assert!(opened(&events).is_empty(), "nothing new to record");
}

#[test]
fn a_loaded_session_replays_nothing_into_the_panel() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) = session_agent(
        dir.path().to_path_buf(),
        json!({ "loadSession": true }),
        false,
        old_session("fake"),
        earlier(),
    );
    let (events, prompt) = run_prompt(&runtime, &seen, "next");
    assert_eq!(
        methods(&seen),
        ["initialize", "session/load", "session/prompt"]
    );
    assert_eq!(prompt.len(), 1);
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageUpdate(StreamEvent::TextDelta(delta)) => Some(delta.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        text, "ok",
        "the replay must not reach the panel: {events:?}"
    );
    assert!(!events
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })));
}

#[test]
fn a_new_session_is_recorded_and_told_a_recap_once() {
    let dir = tempfile::tempdir().unwrap();
    // No way to resume: the agent advertises none.
    let (runtime, seen) = session_agent(
        dir.path().to_path_buf(),
        json!({}),
        false,
        old_session("fake"),
        earlier(),
    );
    let (events, prompt) = run_prompt(&runtime, &seen, "next");
    assert_eq!(
        methods(&seen),
        ["initialize", "session/new", "session/prompt"]
    );
    assert_eq!(opened(&events), ["fake:fresh"]);
    assert_eq!(prompt.len(), 2);
    let recap = prompt[0]["text"].as_str().unwrap();
    assert!(recap.contains("earlier request") && recap.contains("earlier answer"));
    assert_eq!(prompt[1]["text"], "next");
    // The recap is not part of what the panel logs as the request.
    let logged = events.iter().find_map(|e| match e {
        AgentEvent::MessageEnd(Message::User(user)) => Some(user.plain_text()),
        _ => None,
    });
    assert_eq!(logged.as_deref(), Some("next"));

    seen.lock().unwrap().clear();
    let (_, prompt) = run_prompt(&runtime, &seen, "again");
    assert_eq!(prompt, [json!({ "type": "text", "text": "again" })]);
}

#[test]
fn a_session_the_agent_lost_or_never_had_starts_anew_with_the_recap() {
    for (forgotten, resume) in [(true, old_session("fake")), (false, old_session("other"))] {
        let dir = tempfile::tempdir().unwrap();
        let (runtime, seen) = session_agent(
            dir.path().to_path_buf(),
            json!({ "sessionCapabilities": { "resume": {} } }),
            forgotten,
            resume,
            earlier(),
        );
        let (events, prompt) = run_prompt(&runtime, &seen, "next");
        assert!(methods(&seen).contains(&"session/new".to_string()));
        assert_eq!(
            methods(&seen).contains(&"session/resume".to_string()),
            forgotten,
            "another agent's session is never asked for"
        );
        assert_eq!(opened(&events), ["fake:fresh"]);
        assert_eq!(prompt.len(), 2);
    }
}

#[test]
fn with_its_window_known_a_new_session_is_told_the_whole_record() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) =
        session_agent(dir.path().to_path_buf(), json!({}), false, None, earlier());
    // Remembered from an earlier run of the agent on this model.
    let windows = dir.path().join("acp-windows.json");
    windows::remember(
        &windows,
        &windows::key(AcpFlavor::Generic, "fake", "default"),
        200_000,
    );
    runtime.shared.windows.set(windows).unwrap();
    let (_, prompt) = run_prompt(&runtime, &seen, "next");
    let told = prompt[0]["text"].as_str().unwrap();
    assert!(told.contains("follows as a record"), "{told}");
    assert!(told.contains("<user>\nearlier request\n</user>"));
    assert!(told.contains("<assistant>\nearlier answer\n</assistant>"));
    assert_eq!(prompt[1]["text"], "next");
}

#[test]
fn a_window_the_agent_reports_is_remembered_for_its_next_run() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, _) = recording_agent(
        dir.path().to_path_buf(),
        AcpFlavor::ClaudeCode,
        None,
        ModeHandle::new(Mode::default()),
    );
    let windows = dir.path().join("acp-windows.json");
    runtime.shared.windows.set(windows.clone()).unwrap();
    *runtime.shared.current_model.lock().unwrap() = Some("opus".into());
    // Not reported yet: told from the model's name.
    assert_eq!(runtime.shared.known_window(), Some(200_000));
    runtime
        .shared
        .on_update(&json!({ "sessionUpdate": "usage_update", "used": 975, "size": 1_000_000 }));
    assert_eq!(
        windows::remembered(
            &windows,
            &windows::key(AcpFlavor::ClaudeCode, "any", "opus")
        ),
        Some(1_000_000)
    );
    // The user's limit applies to what is known.
    runtime
        .shared
        .context_limit
        .store(300_000, Ordering::Release);
    assert_eq!(runtime.shared.known_window(), Some(300_000));
}

#[test]
fn a_fresh_conversation_gets_no_recap() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, seen) =
        session_agent(dir.path().to_path_buf(), json!({}), false, None, Vec::new());
    let (events, prompt) = run_prompt(&runtime, &seen, "first");
    assert_eq!(opened(&events), ["fake:fresh"]);
    assert_eq!(prompt, [json!({ "type": "text", "text": "first" })]);
}

/// An agent that answers a prompt at once and then, like Claude Code
/// when a task it moved to the background finishes, goes on by itself:
/// `after` is streamed a moment after the turn's answer, as no turn's.
fn self_starting_agent(dir: PathBuf, after: &'static str) -> AcpRuntime {
    let (to_agent_rx, to_agent_tx) = pipe().unwrap();
    let (from_agent_rx, from_agent_tx) = pipe().unwrap();
    std::thread::spawn(move || {
        let out = Arc::new(Mutex::new(from_agent_tx));
        let send = |out: &Arc<Mutex<std::io::PipeWriter>>, value: Value| {
            writeln!(out.lock().unwrap(), "{value}").unwrap();
        };
        let mut reader = BufReader::new(to_agent_rx).lines();
        let mut first = true;
        while let Some(Ok(line)) = reader.next() {
            let message: Value = serde_json::from_str(&line).unwrap();
            let id = message["id"].clone();
            let result = match message["method"].as_str() {
                Some("initialize") => json!({ "protocolVersion": 1 }),
                Some("session/new") => json!({ "sessionId": "s1" }),
                Some("session/prompt") => {
                    let text = message["params"]["prompt"][0]["text"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    send(
                        &out,
                        json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": "s1", "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": format!("re:{text}") } } } }),
                    );
                    if std::mem::take(&mut first) {
                        let later = Arc::clone(&out);
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(150));
                            send(
                                &later,
                                json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                                    "sessionId": "s1", "update": {
                                        "sessionUpdate": "agent_message_chunk",
                                        "content": { "type": "text", "text": after } } } }),
                            );
                        });
                    }
                    json!({ "stopReason": "end_turn" })
                }
                Some(_) => json!({}),
                None => continue,
            };
            send(
                &out,
                json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            );
        }
    });
    let cancel = CancelToken::new();
    let (prompter, _permissions) = permission_channel(cancel.clone());
    let runtime = AcpRuntime::from_streams(
        "fake",
        from_agent_rx,
        to_agent_tx,
        BackendSetup {
            cwd: dir,
            prompter,
            cancel,
            rules: PermissionRules::default(),
            persist: None,
            mode: ModeHandle::new(Mode::default()),
            system_prompt: String::new(),
            plan: PlanPrompt::default(),
            plan_seen: false,
            goal: GoalPrompt::default(),
            handoff: HandoffPrompt::default(),
            reviewer: ReviewerSetup::default(),
            host_tools: None,
            resume: None,
            history: Vec::new(),
        },
        5,
        AcpFlavor::Generic,
    );
    runtime
        .shared
        .quiet_after_text_ms
        .store(300, Ordering::Release);
    runtime.shared.quiet_ms.store(300, Ordering::Release);
    runtime
}

#[test]
fn an_agent_going_on_after_its_turn_is_a_run_until_it_falls_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = self_starting_agent(dir.path().to_path_buf(), "background done");
    runtime.prompt(UserMessage::text("first")).unwrap();
    let mut events = events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
    // The agent's own output opens a run: the panel is busy meanwhile.
    events.extend(events_until(&runtime, |e| {
        matches!(e, AgentEvent::AgentStart)
    }));
    assert!(runtime.is_busy());
    events.extend(events_until(&runtime, |e| {
        matches!(e, AgentEvent::AgentEnd)
    }));
    let ends = |events: &[AgentEvent]| {
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::AgentEnd))
            .count()
    };
    // Two runs: the turn and the agent's own, each closed.
    assert_eq!(ends(&events), 2, "{events:?}");
    assert!(!runtime.is_busy());
    assert_eq!(
        logged(&events),
        [
            "user:first",
            "assistant:re:first",
            "assistant:background done"
        ]
    );
}

/// Runs until `release` is set or the call is cancelled.
struct Held(Arc<AtomicBool>);

impl termide_agent_core::Tool for Held {
    fn name(&self) -> &str {
        "held"
    }
    fn description(&self) -> &str {
        "Run until released"
    }
    fn parameters(&self) -> Value {
        json!({ "type": "object" })
    }
    fn execute(
        &self,
        call: &ToolCall,
        _ctx: &ToolContext,
        _on_update: &mut dyn FnMut(termide_agent_core::ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        while !self.0.load(Ordering::Acquire) && !cancel.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        ToolResultMessage::text(call, "done")
    }
}

/// Claude Code closes a call it moves to the background, but termide's
/// MCP server still runs it: the run lasts until it ends.
#[test]
fn a_call_termide_still_runs_keeps_the_agents_run_on() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = self_starting_agent(dir.path().to_path_buf(), "noted");
    let release = Arc::new(AtomicBool::new(false));
    let mut tools = termide_agent_core::ToolRegistry::new();
    tools.insert(Arc::new(Held(Arc::clone(&release))));
    let server = McpServer::start(
        tools,
        Box::new(termide_agent_core::NoHooks),
        ToolContext::new(PathBuf::new()),
    )
    .unwrap();
    let (url, auth) = (server.url(), format!("Bearer {}", server.token()));
    *runtime.shared.mcp_server.lock().unwrap() = Some(server);
    let call = std::thread::spawn(move || {
        post_mcp(
            &url,
            &auth,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                         "params": { "name": "held", "arguments": {} } }),
        )
    });
    while !runtime.shared.serving_call() {
        std::thread::sleep(Duration::from_millis(5));
    }
    runtime.prompt(UserMessage::text("first")).unwrap();
    events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
    // Well past the quiet that would end it, the call holds the run.
    std::thread::sleep(Duration::from_millis(1000));
    assert!(runtime.is_busy());
    assert!(!runtime
        .drain()
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd)));
    // Claude Code closes the call it moved to the background; termide
    // still runs it, so it is the panel's to show.
    runtime.shared.on_update(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "t1", "kind": "other",
        "status": "pending", "title": "mcp__termide__held", "rawInput": {},
    }));
    runtime.shared.on_update(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed",
        "content": [{ "type": "content", "content": { "type": "text", "text": "moved" } }],
    }));
    let background = runtime.background_calls();
    assert_eq!(background.len(), 1, "{background:?}");
    assert_eq!(background[0].call.id, "t1");
    assert_eq!(background[0].call.name, "held");
    // Its row's stop reaches it, by the block's id.
    assert!(!runtime.stop_background_call("t2"));
    assert!(runtime.stop_background_call("t1"));
    call.join().unwrap();
    assert!(!release.load(Ordering::Acquire), "stopped, not released");
    assert!(runtime.background_calls().is_empty());
    events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
    assert!(!runtime.is_busy());
}

#[test]
fn a_message_typed_while_the_agent_works_by_itself_goes_as_a_turn() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = self_starting_agent(dir.path().to_path_buf(), "working");
    runtime
        .shared
        .quiet_after_text_ms
        .store(60_000, Ordering::Release);
    runtime.prompt(UserMessage::text("first")).unwrap();
    let mut events = events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
    events.extend(events_until(
        &runtime,
        |e| matches!(e, AgentEvent::MessageUpdate(StreamEvent::TextDelta(t)) if t == "working"),
    ));
    assert!(runtime.is_busy());
    // The panel steers while busy; the agent's run gives way to a turn.
    runtime.steer(UserMessage::text("next"));
    events.extend(events_until(&runtime, |e| {
        matches!(e, AgentEvent::AgentEnd)
    }));
    assert_eq!(
        logged(&events),
        [
            "user:first",
            "assistant:re:first",
            "assistant:working",
            "user:next",
            "assistant:re:next"
        ]
    );
    assert!(!runtime.is_busy());
}

#[test]
fn stopping_the_agent_working_by_itself_ends_its_run() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = self_starting_agent(dir.path().to_path_buf(), "working");
    runtime
        .shared
        .quiet_after_text_ms
        .store(60_000, Ordering::Release);
    runtime.prompt(UserMessage::text("first")).unwrap();
    events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
    events_until(&runtime, |e| matches!(e, AgentEvent::AgentStart));
    runtime.abort();
    events_until(&runtime, |e| matches!(e, AgentEvent::AgentEnd));
    assert!(!runtime.is_busy());
}
