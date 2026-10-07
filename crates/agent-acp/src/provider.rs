//! An external agent as a [`Provider`] for side calls — the `auto` reviewer,
//! the recall solver — so a CLI agent's subscription answers them as an
//! endpoint's model would.
//!
//! The agent's process is started on the first call and kept; each call opens
//! a session of its own, without tools or the agent's own settings where the
//! adapter allows (Claude Code takes the call's system prompt in place of its
//! own), picks the model asked for among those the agent offers, sends the
//! request and closes the session. The answer reaches neither a panel nor a
//! log, and a permission request from the session is refused.

use super::*;
use termide_agent_core::{permission_channel, PermissionRules, Request};

/// How long a side call's turn may take.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(120);

/// What starts the service connection: the agent's process, or streams in
/// tests.
type Connector = Box<dyn Fn() -> Result<AcpRuntime, String> + Send + Sync>;

/// An external agent answering one-shot requests, see the module.
pub struct AcpProvider {
    flavor: AcpFlavor,
    timeout: Duration,
    connect: Connector,
    runtime: Mutex<Option<AcpRuntime>>,
}

impl AcpProvider {
    /// The agent `config` starts, named `name` in logs, working in `cwd`.
    #[must_use]
    pub fn new(name: &str, config: AcpConfig, cwd: PathBuf) -> Self {
        let name = name.to_string();
        let (flavor, timeout) = (config.flavor, Duration::from_secs(config.timeout_secs));
        Self {
            flavor,
            timeout,
            connect: Box::new(move || {
                AcpRuntime::start_service(&name, &config, service_setup(cwd.clone()))
            }),
            runtime: Mutex::new(None),
        }
    }

    /// A provider whose connections `connect` makes, in tests.
    #[cfg(test)]
    fn with_connector(flavor: AcpFlavor, timeout: Duration, connect: Connector) -> Self {
        Self {
            flavor,
            timeout,
            connect,
            runtime: Mutex::new(None),
        }
    }

    /// The live connection, started (or started again, after the agent
    /// failed or exited) on demand, once its handshake is done.
    fn shared(&self) -> Result<Arc<Shared>, String> {
        let shared = {
            let mut runtime = self.runtime.lock().unwrap_or_else(PoisonError::into_inner);
            let failed = runtime.as_ref().is_none_or(|runtime| {
                matches!(
                    *runtime
                        .shared
                        .conn
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner),
                    Conn::Failed(_)
                )
            });
            if failed {
                *runtime = Some((self.connect)()?);
            }
            Arc::clone(&runtime.as_ref().ok_or("no connection")?.shared)
        };
        let deadline = Instant::now() + self.timeout;
        loop {
            match &*shared.conn.lock().unwrap_or_else(PoisonError::into_inner) {
                Conn::Ready { .. } => break,
                Conn::Failed(error) => return Err(error.clone()),
                Conn::Starting => {}
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "the agent did not start within {} s",
                    self.timeout.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(shared)
    }
}

impl Provider for AcpProvider {
    fn name(&self) -> &str {
        ACP_PROVIDER
    }

    fn stream(
        &self,
        request: &Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> AssistantMessage {
        let failed = |error: String| {
            AssistantMessage::failed(ACP_PROVIDER, &request.model.id, StopReason::Error, error)
        };
        if cancel.is_cancelled() {
            return AssistantMessage::failed(
                ACP_PROVIDER,
                &request.model.id,
                StopReason::Aborted,
                "aborted",
            );
        }
        let user: Vec<String> = request
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.model_text()),
                _ => None,
            })
            .collect();
        let answer = self.shared().and_then(|shared| {
            shared.service_ask(
                self.flavor,
                request.system_prompt,
                &user.join("\n\n"),
                &request.model.id,
            )
        });
        match answer {
            Ok(text) if !text.trim().is_empty() => {
                on_event(StreamEvent::TextDelta(text.clone()));
                AssistantMessage {
                    content: vec![AssistantContent::Text { text }],
                    stop_reason: StopReason::Stop,
                    usage: Usage::default(),
                    provider: ACP_PROVIDER.into(),
                    model: request.model.id.clone(),
                    error_message: None,
                    timestamp: now_millis(),
                }
            }
            Ok(_) => failed("the agent answered nothing".to_string()),
            Err(error) => failed(error),
        }
    }
}

impl Shared {
    /// Ask `system` and `user` in a session of their own, on `model` when it
    /// names one the agent offers; see the module.
    fn service_ask(
        &self,
        flavor: AcpFlavor,
        system: &str,
        user: &str,
        model: &str,
    ) -> Result<String, String> {
        let mut params = json!({ "cwd": self.cwd, "mcpServers": [] });
        let text = if flavor == AcpFlavor::ClaudeCode {
            params["_meta"] = json!({
                "systemPrompt": system,
                "claudeCode": { "options": {
                    "settingSources": [], "strictMcpConfig": true, "tools": []
                } },
            });
            user.to_string()
        } else {
            // The others keep their own prompt: the instructions lead the
            // request instead.
            format!("{system}\n\n{user}")
        };
        let opened = self.request("session/new", params, Duration::from_secs(60))?;
        let session_id = opened["sessionId"]
            .as_str()
            .ok_or("session/new returned no sessionId")?
            .to_string();
        if !model.trim().is_empty() {
            self.pick_model(&session_id, &opened, model);
        }
        if flavor == AcpFlavor::Codex {
            // It asks before anything but reading, and is refused.
            let _ = self.request(
                "session/set_config_option",
                json!({ "sessionId": session_id, "configId": "mode", "value": "read-only" }),
                Duration::from_secs(30),
            );
        }
        let answer = self.side_prompt(&session_id, &text, ANSWER_TIMEOUT);
        if self.can_close.load(Ordering::Acquire) {
            let _ = self.request(
                "session/close",
                json!({ "sessionId": session_id }),
                Duration::from_secs(10),
            );
        }
        answer
    }

    /// Switch session `session_id` to the model `wanted` names among those
    /// `opened` (its `session/new` result) offers; a name that matches none,
    /// or more than one, leaves the agent's own.
    fn pick_model(&self, session_id: &str, opened: &Value, wanted: &str) {
        let option = opened["configOptions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(option_of)
            .find(|option| option.is("model") || option.id == "model");
        let (offered, config_id) = match option {
            Some(option) => (option.values, Some(option.id)),
            None => {
                let list = opened["models"]["availableModels"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        let id = m["modelId"].as_str()?.to_string();
                        let name = m["name"].as_str().unwrap_or(&id).to_string();
                        Some(BackendModel { id, name })
                    })
                    .collect();
                (list, None)
            }
        };
        let Some(id) = match_model(wanted, &offered) else {
            log::warn!(
                "acp {}: no model {wanted:?} among {:?}; the agent's own answers",
                self.name,
                offered.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()
            );
            return;
        };
        let set = match config_id {
            Some(config_id) => self.request(
                "session/set_config_option",
                json!({ "sessionId": session_id, "configId": config_id, "value": id }),
                Duration::from_secs(30),
            ),
            None => self.request(
                "session/set_model",
                json!({ "sessionId": session_id, "modelId": id }),
                Duration::from_secs(30),
            ),
        };
        if let Err(error) = set {
            log::warn!("acp {}: cannot switch to {id}: {error}", self.name);
        }
    }
}

/// The model of `offered` that `wanted` names: by its id, else by its id or
/// name ignoring case, else the only one whose id or name contains it
/// (`haiku`).
fn match_model(wanted: &str, offered: &[BackendModel]) -> Option<String> {
    let wanted = wanted.trim();
    if let Some(model) = offered.iter().find(|m| m.id == wanted) {
        return Some(model.id.clone());
    }
    let lower = wanted.to_lowercase();
    if let Some(model) = offered
        .iter()
        .find(|m| m.id.to_lowercase() == lower || m.name.to_lowercase() == lower)
    {
        return Some(model.id.clone());
    }
    let mut hits = offered
        .iter()
        .filter(|m| m.id.to_lowercase().contains(&lower) || m.name.to_lowercase().contains(&lower));
    match (hits.next(), hits.next()) {
        (Some(model), None) => Some(model.id.clone()),
        _ => None,
    }
}

/// The setup of a service connection: no one to ask (a permission request
/// is refused), no tools of termide's, no conversation.
fn service_setup(cwd: PathBuf) -> BackendSetup {
    let cancel = CancelToken::new();
    let (prompter, _nobody) = permission_channel(cancel.clone());
    BackendSetup {
        cwd,
        prompter,
        cancel,
        rules: PermissionRules::default(),
        persist: None,
        mode: ModeHandle::new(Mode::Ask),
        system_prompt: String::new(),
        plan: PlanPrompt::default(),
        goal: GoalPrompt::default(),
        handoff: HandoffPrompt::default(),
        reviewer: ReviewerSetup::default(),
        host_tools: None,
        resume: None,
        history: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::pipe;

    #[test]
    fn a_model_is_found_by_its_id_its_name_or_a_part_of_either() {
        let offered = [
            BackendModel {
                id: "default".into(),
                name: "Default (recommended)".into(),
            },
            BackendModel {
                id: "claude-haiku-4-5".into(),
                name: "Haiku".into(),
            },
            BackendModel {
                id: "claude-sonnet-4-5".into(),
                name: "Sonnet".into(),
            },
        ];
        assert_eq!(match_model("default", &offered).as_deref(), Some("default"));
        assert_eq!(
            match_model("HAIKU", &offered).as_deref(),
            Some("claude-haiku-4-5")
        );
        assert_eq!(
            match_model("sonnet-4", &offered).as_deref(),
            Some("claude-sonnet-4-5")
        );
        // Ambiguous or unknown: the agent's own.
        assert_eq!(match_model("claude", &offered), None);
        assert_eq!(match_model("opus", &offered), None);
    }

    /// An agent that records what it is sent and answers every prompt with
    /// `ALLOW <the session id>` after a pause, so calls overlap.
    fn reviewer_agent(seen: &Arc<Mutex<Vec<Value>>>) -> AcpRuntime {
        let (to_agent_rx, to_agent_tx) = pipe().unwrap();
        let (from_agent_rx, from_agent_tx) = pipe().unwrap();
        let record = Arc::clone(seen);
        let out = Arc::new(Mutex::new(from_agent_tx));
        std::thread::spawn(move || {
            let send = |out: &Arc<Mutex<std::io::PipeWriter>>, value: Value| {
                writeln!(out.lock().unwrap(), "{value}").unwrap();
            };
            let mut reader = BufReader::new(to_agent_rx).lines();
            let mut next = 0;
            while let Some(Ok(line)) = reader.next() {
                let message: Value = serde_json::from_str(&line).unwrap();
                record.lock().unwrap().push(message.clone());
                let id = message["id"].clone();
                let result = match message["method"].as_str() {
                    Some("initialize") => json!({ "protocolVersion": 1,
                        "agentCapabilities": { "sessionCapabilities": { "close": {} } } }),
                    Some("session/new") => {
                        next += 1;
                        json!({ "sessionId": format!("r{next}"), "configOptions": [
                            { "id": "model", "category": "model", "type": "select",
                              "currentValue": "default", "options": [
                                { "value": "default", "name": "Default" },
                                { "value": "claude-haiku-4-5", "name": "Haiku" } ] } ] })
                    }
                    Some("session/prompt") => {
                        // Answered from a thread of its own, after a pause.
                        let out = Arc::clone(&out);
                        let session = message["params"]["sessionId"].clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(50));
                            send(
                                &out,
                                json!({ "jsonrpc": "2.0", "method": "session/update",
                                "params": { "sessionId": session, "update": {
                                    "sessionUpdate": "agent_message_chunk",
                                    "content": { "type": "text",
                                        "text": format!("ALLOW {}", session.as_str().unwrap()) } } } }),
                            );
                            send(
                                &out,
                                json!({ "jsonrpc": "2.0", "id": id,
                                "result": { "stopReason": "end_turn" } }),
                            );
                        });
                        continue;
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
        AcpRuntime::connect(
            "reviewer",
            from_agent_rx,
            to_agent_tx,
            service_setup(PathBuf::from("/tmp")),
            5,
            AcpFlavor::ClaudeCode,
            true,
        )
    }

    fn ask(provider: &AcpProvider, model: &str) -> AssistantMessage {
        let spec = ModelSpec {
            provider: "agent".into(),
            id: model.into(),
            context_window: 0,
            max_tokens: None,
            thinking: ThinkingLevel::Off,
        };
        let messages = [Message::User(UserMessage::text("judge this call"))];
        let request = Request {
            model: &spec,
            system_prompt: "You review calls.",
            messages: &messages,
            tools: &[],
            thinking: ThinkingLevel::Off,
        };
        provider.stream(&request, &mut |_| {}, &CancelToken::new())
    }

    #[test]
    fn each_call_is_a_session_of_its_own_on_the_model_asked_for() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(AtomicU64::new(0));
        let (for_connect, count) = (Arc::clone(&seen), Arc::clone(&started));
        let provider = Arc::new(AcpProvider::with_connector(
            AcpFlavor::ClaudeCode,
            Duration::from_secs(5),
            Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
                Ok(reviewer_agent(&for_connect))
            }),
        ));
        // Two calls at once, each answered in its own session.
        let other = Arc::clone(&provider);
        let parallel = std::thread::spawn(move || ask(&other, "haiku"));
        let reply = ask(&provider, "haiku");
        let parallel = parallel.join().unwrap();
        let mut answers = [reply.plain_text(), parallel.plain_text()];
        answers.sort();
        assert_eq!(answers, ["ALLOW r1", "ALLOW r2"]);
        // One process serves them both.
        assert_eq!(started.load(Ordering::SeqCst), 1);
        let seen = seen.lock().unwrap();
        let new = seen.iter().find(|m| m["method"] == "session/new").unwrap();
        // Claude Code runs on the call's prompt, without tools or settings.
        assert_eq!(new["params"]["_meta"]["systemPrompt"], "You review calls.");
        assert_eq!(
            new["params"]["_meta"]["claudeCode"]["options"]["tools"],
            json!([])
        );
        let picked: Vec<&Value> = seen
            .iter()
            .filter(|m| m["method"] == "session/set_config_option")
            .collect();
        assert_eq!(picked.len(), 2);
        assert!(picked
            .iter()
            .all(|m| m["params"]["value"] == "claude-haiku-4-5"));
        let prompt = seen
            .iter()
            .find(|m| m["method"] == "session/prompt")
            .unwrap();
        assert_eq!(prompt["params"]["prompt"][0]["text"], "judge this call");
        assert_eq!(
            seen.iter()
                .filter(|m| m["method"] == "session/close")
                .count(),
            2
        );
    }
}
