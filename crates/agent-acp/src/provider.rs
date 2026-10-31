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
use termide_agent_core::{permission_channel, ModelInfo, PermissionRules, Request};

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
    /// What a call that names no model runs on: the agent's own, or its
    /// cheapest offered (see [`AcpProvider::cheapest`]).
    unnamed: Unnamed,
}

/// The model a call that names none runs on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Unnamed {
    /// The one the agent picks.
    Own,
    /// The first offered of the flavor's cheapest class, see [`cheap_hints`].
    Cheapest,
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
            unnamed: Unnamed::Own,
        }
    }

    /// The same, but a call that names no model runs on the agent's cheapest
    /// class of model it offers (Claude Code's Haiku, Codex's smallest), when
    /// the flavor has one and the agent offers it; on its own otherwise. For
    /// side calls such as the `auto` reviewer, which need no frontier model.
    #[must_use]
    pub fn cheapest(mut self) -> Self {
        self.unnamed = Unnamed::Cheapest;
        self
    }

    /// A provider whose connections `connect` makes, in tests.
    #[cfg(test)]
    fn with_connector(flavor: AcpFlavor, timeout: Duration, connect: Connector) -> Self {
        Self {
            flavor,
            timeout,
            connect,
            runtime: Mutex::new(None),
            unnamed: Unnamed::Own,
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
                self.unnamed,
            )
        });
        match answer {
            Ok(SideAnswer {
                answer: Ok(text),
                usage,
                model,
            }) if !text.trim().is_empty() => {
                on_event(StreamEvent::TextDelta(text.clone()));
                AssistantMessage {
                    content: vec![AssistantContent::Text { text }],
                    stop_reason: StopReason::Stop,
                    usage,
                    provider: ACP_PROVIDER.into(),
                    // The model that answered, which the request may have
                    // left to the agent to pick.
                    model: model.unwrap_or_else(|| request.model.id.clone()),
                    error_message: None,
                    failure: None,
                    timestamp: now_millis(),
                }
            }
            // Spent all the same, on the model it went to.
            Ok(SideAnswer {
                answer,
                usage,
                model,
            }) => AssistantMessage {
                usage,
                model: model.unwrap_or_else(|| request.model.id.clone()),
                ..failed(
                    answer
                        .err()
                        .unwrap_or_else(|| "the agent answered nothing".to_string()),
                )
            },
            Err(error) => failed(error),
        }
    }

    fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        let models = self.shared()?.list_models(self.flavor)?;
        Ok(models
            .into_iter()
            .map(|model| ModelInfo {
                id: model.id,
                context_window: None,
            })
            .collect())
    }
}

/// A side call's answer, or why it gave none, what it spent and the model
/// it was switched to (`None`: the agent's own, not known here).
struct SideAnswer {
    answer: Result<String, String>,
    usage: Usage,
    model: Option<String>,
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
        unnamed: Unnamed,
    ) -> Result<SideAnswer, String> {
        let mut params = json!({ "cwd": self.cwd, "mcpServers": [] });
        let text = if flavor == AcpFlavor::ClaudeCode {
            // A side call's request is unique and never read again, and the
            // calls come in bursts: the five-minute cache writes it for less
            // than the hour a subscription gives a session's conversation.
            params["_meta"] = json!({
                "systemPrompt": system,
                "claudeCode": { "options": {
                    "settingSources": [], "strictMcpConfig": true, "tools": [],
                    "promptSuggestions": false,
                    "env": { "CLAUDE_CODE_PROMPT_CACHE_TTL": "5m" },
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
        let picked = if !model.trim().is_empty() {
            self.pick_model(&session_id, &opened, model)
        } else if unnamed == Unnamed::Cheapest {
            self.pick_cheapest(&session_id, &opened, flavor)
        } else {
            None
        };
        if flavor == AcpFlavor::Codex {
            // It asks before anything but reading, and is refused.
            let _ = self.request(
                "session/set_config_option",
                json!({ "sessionId": session_id, "configId": "mode", "value": "read-only" }),
                Duration::from_secs(30),
            );
        }
        let (answer, usage) = self.side_prompt(&session_id, &text, ANSWER_TIMEOUT);
        if self.can_close.load(Ordering::Acquire) {
            let _ = self.request(
                "session/close",
                json!({ "sessionId": session_id }),
                Duration::from_secs(10),
            );
        }
        Ok(SideAnswer {
            answer,
            usage,
            model: picked,
        })
    }

    /// The models the agent offers on its login: a session is opened, as
    /// for a side call, read and closed.
    fn list_models(&self, flavor: AcpFlavor) -> Result<Vec<BackendModel>, String> {
        let mut params = json!({ "cwd": self.cwd, "mcpServers": [] });
        if flavor == AcpFlavor::ClaudeCode {
            params["_meta"] = json!({
                "claudeCode": { "options": {
                    "settingSources": [], "strictMcpConfig": true, "tools": []
                } },
            });
        }
        let opened = self.request("session/new", params, Duration::from_secs(60))?;
        if self.can_close.load(Ordering::Acquire) {
            if let Some(session_id) = opened["sessionId"].as_str() {
                let _ = self.request(
                    "session/close",
                    json!({ "sessionId": session_id }),
                    Duration::from_secs(10),
                );
            }
        }
        Ok(offered_models(&opened).0)
    }

    /// Switch session `session_id` to the model `wanted` names among those
    /// `opened` (its `session/new` result) offers; a name that matches none,
    /// or more than one, leaves the agent's own.
    fn pick_model(&self, session_id: &str, opened: &Value, wanted: &str) -> Option<String> {
        let (offered, config_id) = offered_models(opened);
        let Some(id) = match_model(wanted, &offered) else {
            log::warn!(
                "acp {}: no model {wanted:?} among {:?}; the agent's own answers",
                self.name,
                offered.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()
            );
            return None;
        };
        self.set_model(session_id, config_id.as_deref(), &id)
            .then_some(id)
    }

    /// Switch session `session_id` to the first model `opened` offers of
    /// `flavor`'s cheapest class; none such leaves the agent's own.
    fn pick_cheapest(&self, session_id: &str, opened: &Value, flavor: AcpFlavor) -> Option<String> {
        let (offered, config_id) = offered_models(opened);
        match cheapest_model(flavor, &offered) {
            Some(id) => self
                .set_model(session_id, config_id.as_deref(), &id)
                .then_some(id),
            None => {
                log::warn!(
                    "acp {}: no cheap model among {:?}; the agent's own answers",
                    self.name,
                    offered.iter().map(|m| m.id.as_str()).collect::<Vec<_>>()
                );
                None
            }
        }
    }

    /// Switch session `session_id` to model `id`, through the config option
    /// `config_id` when the agent switches models with one; whether it did.
    fn set_model(&self, session_id: &str, config_id: Option<&str>, id: &str) -> bool {
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
        match set {
            Ok(_) => true,
            Err(error) => {
                log::warn!("acp {}: cannot switch to {id}: {error}", self.name);
                false
            }
        }
    }
}

/// The models a `session/new` result `opened` offers, and the id of the
/// config option that switches them; `None` when the agent lists them in
/// `models` and switches them with `session/set_model`.
fn offered_models(opened: &Value) -> (Vec<BackendModel>, Option<String>) {
    let option = opened["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(option_of)
        .find(|option| option.is("model") || option.id == "model");
    match option {
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
    }
}

/// What the names of a flavor's cheapest class of model contain, the first
/// preferred: Claude Code's Haiku; Codex's small tier, `luna` now and `mini`
/// before it. None for the others.
fn cheap_hints(flavor: AcpFlavor) -> &'static [&'static str] {
    match flavor {
        AcpFlavor::ClaudeCode => &["haiku"],
        AcpFlavor::Codex => &["luna", "mini"],
        AcpFlavor::Generic | AcpFlavor::GeminiCli => &[],
    }
}

/// The first model of `offered` whose id or name contains one of `flavor`'s
/// [`cheap_hints`], tried in order. An agent lists its newest models first,
/// and Codex one entry per reasoning level, so the first match is taken
/// rather than refused as ambiguous as [`match_model`] does.
fn cheapest_model(flavor: AcpFlavor, offered: &[BackendModel]) -> Option<String> {
    cheap_hints(flavor).iter().find_map(|hint| {
        offered
            .iter()
            .find(|m| m.id.to_lowercase().contains(hint) || m.name.to_lowercase().contains(hint))
            .map(|m| m.id.clone())
    })
}

/// The model of `offered` that `wanted` names: by its id, else by its id or
/// name ignoring case, else the only one whose id or name contains it
/// (`haiku`).
#[must_use]
pub fn match_model(wanted: &str, offered: &[BackendModel]) -> Option<String> {
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

    #[test]
    fn the_cheapest_model_is_the_first_offered_of_the_flavors_small_class() {
        let offered = |ids: &[&str]| -> Vec<BackendModel> {
            ids.iter()
                .map(|id| BackendModel {
                    id: (*id).into(),
                    name: (*id).into(),
                })
                .collect()
        };
        let claude = offered(&["default", "opus", "claude-haiku-5-5", "sonnet"]);
        assert_eq!(
            cheapest_model(AcpFlavor::ClaudeCode, &claude).as_deref(),
            Some("claude-haiku-5-5")
        );
        // One entry per reasoning level, newest first: the first is taken.
        let codex = offered(&[
            "gpt-6.1-sol/medium",
            "gpt-6-luna/low",
            "gpt-6-luna/high",
            "gpt-5.6-luna/low",
        ]);
        assert_eq!(
            cheapest_model(AcpFlavor::Codex, &codex).as_deref(),
            Some("gpt-6-luna/low")
        );
        let older = offered(&["gpt-5-codex/low", "gpt-5-codex-mini/low"]);
        assert_eq!(
            cheapest_model(AcpFlavor::Codex, &older).as_deref(),
            Some("gpt-5-codex-mini/low")
        );
        // None of the class, or a flavor without one: the agent's own.
        assert_eq!(cheapest_model(AcpFlavor::ClaudeCode, &codex), None);
        assert_eq!(cheapest_model(AcpFlavor::GeminiCli, &claude), None);
    }

    #[test]
    fn a_call_naming_no_model_runs_on_the_cheapest_when_asked_to() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let for_connect = Arc::clone(&seen);
        let provider = AcpProvider::with_connector(
            AcpFlavor::ClaudeCode,
            Duration::from_secs(5),
            Box::new(move || Ok(reviewer_agent(&for_connect))),
        )
        .cheapest();
        let reply = ask(&provider, "");
        assert_eq!(reply.plain_text(), "ALLOW r1");
        // Credited to the model picked, and with what it spent: the
        // reviewer's tokens are counted, on the model they went to.
        assert_eq!(reply.model, "claude-haiku-4-5");
        assert_eq!(
            reply.usage,
            Usage {
                input: 3,
                output: 9,
                cache_read: 40,
                cache_write: 7,
                thinking: 4,
            }
        );
        let seen = seen.lock().unwrap();
        let picked: Vec<&Value> = seen
            .iter()
            .filter(|m| m["method"] == "session/set_config_option")
            .collect();
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0]["params"]["value"], "claude-haiku-4-5");
    }

    /// A call the agent stops still spent its tokens, on the model picked.
    #[test]
    fn a_stopped_call_still_counts_what_it_spent() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let for_connect = Arc::clone(&seen);
        let provider = AcpProvider::with_connector(
            AcpFlavor::ClaudeCode,
            Duration::from_secs(5),
            Box::new(move || Ok(reviewer_agent(&for_connect))),
        )
        .cheapest();
        let reply = ask_text(&provider, "", "refuse this call");
        assert_eq!(reply.stop_reason, StopReason::Error);
        assert!(
            reply
                .error_message
                .as_deref()
                .is_some_and(|e| e.contains("refusal")),
            "{reply:?}"
        );
        assert_eq!(reply.model, "claude-haiku-4-5");
        assert_eq!((reply.usage.input, reply.usage.output), (3, 9));
    }

    /// An agent that records what it is sent and answers every prompt with
    /// `ALLOW <the session id>` after a pause, so calls overlap, reporting
    /// what the prompt spent; a prompt saying `refuse` it refuses.
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
                        let stop = if message["params"]["prompt"][0]["text"]
                            .as_str()
                            .is_some_and(|text| text.contains("refuse"))
                        {
                            "refusal"
                        } else {
                            "end_turn"
                        };
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
                                "result": { "stopReason": stop, "usage": {
                                    "inputTokens": 3, "outputTokens": 9,
                                    "cachedReadTokens": 40, "cachedWriteTokens": 7,
                                    "thoughtTokens": 4, "totalTokens": 59 } } }),
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
            None,
        )
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_that_hangs_up_at_once_says_why_with_its_last_stderr_line() {
        // As npx does when the package it unpacked lost its executable bit.
        let config = AcpConfig {
            command: "sh".into(),
            args: vec![
                "-c".into(),
                "echo starting >&2; echo 'sh: claude-agent-acp: Permission denied' >&2; exit 126"
                    .into(),
            ],
            env: std::collections::BTreeMap::new(),
            timeout_secs: 5,
            flavor: AcpFlavor::Generic,
            context_limit: None,
        };
        let provider = AcpProvider::new("broken", config, std::env::temp_dir());
        let error = provider.list_models().unwrap_err();
        assert!(
            error.contains("closed the connection: sh: claude-agent-acp: Permission denied"),
            "{error}"
        );
    }

    #[test]
    fn the_models_of_the_agents_login_are_listed_from_a_session_of_their_own() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let for_connect = Arc::clone(&seen);
        let provider = AcpProvider::with_connector(
            AcpFlavor::ClaudeCode,
            Duration::from_secs(5),
            Box::new(move || Ok(reviewer_agent(&for_connect))),
        );
        let models = provider.list_models().unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["default", "claude-haiku-4-5"]);
        let seen = seen.lock().unwrap();
        let methods: Vec<&str> = seen.iter().filter_map(|m| m["method"].as_str()).collect();
        // Opened without tools or settings, read, closed; nothing is asked.
        assert_eq!(methods, ["initialize", "session/new", "session/close"]);
        assert_eq!(
            seen[1]["params"]["_meta"]["claudeCode"]["options"]["tools"],
            json!([])
        );
    }

    fn ask(provider: &AcpProvider, model: &str) -> AssistantMessage {
        ask_text(provider, model, "judge this call")
    }

    fn ask_text(provider: &AcpProvider, model: &str, text: &str) -> AssistantMessage {
        let spec = ModelSpec {
            provider: "agent".into(),
            id: model.into(),
            context_window: 0,
            max_tokens: None,
            thinking: ThinkingLevel::Off,
        };
        let messages = [Message::User(UserMessage::text(text))];
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
        // Its unique request goes to the five-minute cache, with no
        // follow-up suggestion generated after it.
        let options = &new["params"]["_meta"]["claudeCode"]["options"];
        assert_eq!(options["env"]["CLAUDE_CODE_PROMPT_CACHE_TTL"], "5m");
        assert_eq!(options["promptSuggestions"], false);
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
