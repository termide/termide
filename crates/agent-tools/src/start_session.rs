//! `start_session`: start a session of another agent, or of a copy of this
//! one, and hand it part of the work.
//!
//! Three things are chosen apart, as the comparison of coding agents in
//! `doc/en/agent-design.md` sets them out: what the new session knows
//! (`context`: nothing but the prompt, or this conversation), whether its
//! answer comes back (`reply`: the call waits for it, it comes later as a
//! message, or nothing comes back) and where the user sees it (`show`: out of
//! sight, followed from the panel's state strip, or in an agent panel of its
//! own). A session that already exists is written to with `send_message`.
//!
//! The tool holds no agent or panel machinery: the app supplies the closure
//! that starts the session, so this crate stays free of the provider, the
//! catalog and the panels.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{json, Value};
use termide_agent_core::{
    CancelToken, Failure, Tool, ToolCall, ToolContext, ToolResultMessage, ToolUpdate, Usage,
};
use termide_agent_core::{ToolText, SUBAGENT_SESSION_KEY};

/// The tool's name.
pub const START_SESSION: &str = "start_session";

/// What the new session knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionContext {
    /// Nothing but the prompt: a fresh session of the named agent.
    Fresh,
    /// This conversation: a copy of this session, compacted on `focus` when
    /// the call names one.
    Fork,
}

/// Whether, and how, the new session's answer comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionReply {
    /// The call waits for it and returns it.
    Wait,
    /// The call returns at once; the answer comes later as a message.
    Later,
    /// Nothing comes back.
    None,
}

/// Where the user sees the new session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionShow {
    /// Out of sight, followed from the delegating panel's state strip.
    Background,
    /// In an agent panel of its own beside the delegating one.
    Panel,
}

/// What a `start_session` call asks for, its arguments checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRequest<'a> {
    /// The agent of a fresh session; a fork runs on this session's own, and
    /// may name it only to say so.
    pub agent: Option<&'a str>,
    pub prompt: &'a str,
    /// The short name the session takes, if the call gave one.
    pub title: Option<&'a str>,
    pub context: SessionContext,
    /// What a fork's copy keeps in full when it is compacted first.
    pub focus: Option<&'a str>,
    pub reply: SessionReply,
    pub show: SessionShow,
    /// The id of the `start_session` call.
    pub call: &'a str,
}

impl SessionRequest<'_> {
    /// Whether the session runs as a delegate: on a thread of termide's,
    /// followed from the delegating panel and, while it runs, held to that
    /// panel's mode — everything but a fork opened in a panel, which is the
    /// user's session from its start.
    #[must_use]
    pub fn delegated(&self) -> bool {
        !(self.context == SessionContext::Fork && self.show == SessionShow::Panel)
    }
}

/// How a delegate's run ended: its final answer, or an error message, the
/// tokens its model calls spent either way, and the log its session went to,
/// when it had one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentOutcome {
    pub answer: Result<String, String>,
    pub spent: Usage,
    pub session: Option<PathBuf>,
    /// The task is parked at a limit that passes on its own: `answer` only
    /// says where it waits and when it goes again, so nothing is reported to
    /// the panel that delegated it — its real answer is still owed there.
    pub parked: bool,
    /// What stopped it, when its last reply says: a limit and the moment it
    /// resets, so the runner can wait for that rather than guess.
    pub failure: Option<Failure>,
}

impl From<Result<String, String>> for SubagentOutcome {
    /// An outcome that spent nothing and left no log.
    fn from(answer: Result<String, String>) -> Self {
        Self {
            answer,
            spent: Usage::default(),
            session: None,
            parked: false,
            failure: None,
        }
    }
}

/// A task run by a delegate: a fresh session of an agent, or a copy of the
/// delegating session working out of sight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentTask<'a> {
    /// The agent to run.
    pub agent: &'a str,
    pub prompt: &'a str,
    /// The id of the delegating `start_session` call.
    pub call: &'a str,
    /// Hand the task over and go on: the call returns once the task is
    /// placed, and the delegate's answer comes later as a message of its own.
    pub background: bool,
    /// Show the task in an agent panel of its own beside the others, which
    /// follows it while it runs and holds its session, for the user to go on
    /// with, once it is over. Always handed over in the background.
    pub panel: bool,
    /// Its answer goes back to the delegating session; without, nothing does.
    pub report: bool,
    /// It runs out of a copy of the delegating session (`ToolContext::resume`
    /// names it) rather than a fresh one, compacted on `focus` first when
    /// given.
    pub fork: Option<ForkedTask<'a>>,
}

/// What a delegate running out of a copy of the delegating session keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForkedTask<'a> {
    pub focus: Option<&'a str>,
}

/// Starts the session a call asks for. A delegate's progress is forwarded
/// through `on_update`; the context is the calling one's.
pub type StartSession = Arc<
    dyn Fn(
            &SessionRequest,
            &ToolContext,
            &CancelToken,
            &mut dyn FnMut(ToolUpdate),
        ) -> SubagentOutcome
        + Send
        + Sync,
>;

/// The `start_session` tool.
pub struct StartSessionTool {
    /// The agents a fresh session can run, as `(name, description)`.
    agents: Vec<(String, String)>,
    start: StartSession,
    description: String,
}

impl StartSessionTool {
    #[must_use]
    pub fn new(agents: Vec<(String, String)>, start: StartSession) -> Self {
        let mut tool = Self {
            agents,
            start,
            description: String::new(),
        };
        tool.description = tool.render_description(&ToolText::seed(START_SESSION).description);
        tool
    }

    /// The call's arguments as a request, or why they make none.
    fn request<'a>(&self, call: &'a ToolCall) -> Result<SessionRequest<'a>, String> {
        let text = |name: &str| -> Option<&'a str> {
            call.arguments
                .get(name)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        };
        let choice = |name: &str, values: &[&str], default: &'a str| -> Result<&'a str, String> {
            match text(name) {
                None => Ok(default),
                Some(value) if values.contains(&value) => Ok(value),
                Some(value) => Err(format!("`{name}` is {}, not {value}", values.join(" or "))),
            }
        };
        let prompt = text("prompt").ok_or("missing required argument `prompt`")?;
        let context = match choice("context", &["none", "fork"], "none")? {
            "fork" => SessionContext::Fork,
            _ => SessionContext::Fresh,
        };
        let show = match choice("show", &["background", "panel"], "background")? {
            "panel" => SessionShow::Panel,
            _ => SessionShow::Background,
        };
        // A session in a panel is the user's to follow: no call waits on it.
        let reply_default = match show {
            SessionShow::Panel => "later",
            SessionShow::Background => "wait",
        };
        let reply = match choice("reply", &["wait", "later", "none"], reply_default)? {
            "later" => SessionReply::Later,
            "none" => SessionReply::None,
            _ => SessionReply::Wait,
        };
        let agent = text("agent");
        let focus = text("focus");
        match context {
            SessionContext::Fresh => {
                let Some(agent) = agent else {
                    return Err(
                        "missing argument `agent`: a fresh session runs one of the agents \
                         listed; set `context` to `fork` for a copy of this session"
                            .into(),
                    );
                };
                if self.agents.is_empty() {
                    return Err(
                        "no agent runs a fresh session here; set `context` to `fork` for a copy \
                         of this session"
                            .into(),
                    );
                }
                if !self.agents.iter().any(|(name, _)| name == agent) {
                    let names: Vec<&str> =
                        self.agents.iter().map(|(name, _)| name.as_str()).collect();
                    return Err(format!(
                        "no agent named {agent}; available: {}",
                        names.join(", ")
                    ));
                }
                if focus.is_some() {
                    return Err(
                        "`focus` compacts a fork's copy of this conversation; a fresh session \
                         has none to compact"
                            .into(),
                    );
                }
            }
            SessionContext::Fork => {
                if show == SessionShow::Panel && reply != SessionReply::None {
                    return Err(
                        "a fork shown in a panel is the user's session from its start and \
                         reports nothing back: set `reply` to `none`, or leave `show` out to \
                         have its answer"
                            .into(),
                    );
                }
            }
        }
        if show == SessionShow::Panel && reply == SessionReply::Wait {
            return Err(
                "a session shown in a panel is the user's to follow, so no call waits on it: \
                 set `reply` to `later` or `none`"
                    .into(),
            );
        }
        Ok(SessionRequest {
            agent,
            prompt,
            title: text("title"),
            context,
            focus,
            reply,
            show,
            call: &call.id,
        })
    }
}

impl Tool for StartSessionTool {
    fn name(&self) -> &str {
        START_SESSION
    }

    fn description(&self) -> &str {
        &self.description
    }

    /// `{{agents}}` takes the agents a fresh session can run, one per line.
    fn render_description(&self, template: &str) -> String {
        let agents: Vec<String> = self
            .agents
            .iter()
            .map(|(name, desc)| {
                if desc.is_empty() {
                    format!("- {name}")
                } else {
                    format!("- {name}: {desc}")
                }
            })
            .collect();
        template.replace("{{agents}}", &agents.join("\n"))
    }

    fn parameters(&self) -> Value {
        let names: Vec<&str> = self.agents.iter().map(|(name, _)| name.as_str()).collect();
        let mut agent = json!({
            "type": "string",
            "description": "The agent a fresh session runs; leave it out for a fork, which runs this session's own"
        });
        // An empty `enum` matches nothing, which some providers refuse.
        if !names.is_empty() {
            agent["enum"] = json!(names);
        }
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The session's first request: its part of the work, complete in itself for a fresh session, and for a fork what stays with this session"
                },
                "agent": agent,
                "context": {
                    "type": "string",
                    "enum": ["none", "fork"],
                    "description": "What the session knows: `none` (the default), the prompt alone; `fork`, a copy of this conversation"
                },
                "focus": {
                    "type": "string",
                    "description": "A fork only: what its copy keeps in full when it is compacted first; leave it out to keep the whole conversation"
                },
                "reply": {
                    "type": "string",
                    "enum": ["wait", "later", "none"],
                    "description": "Its answer: `wait` returns it from this call (the default out of sight), `later` brings it as a message of its own (the default in a panel), `none` brings nothing back"
                },
                "show": {
                    "type": "string",
                    "enum": ["background", "panel"],
                    "description": "Where the user sees it: `background` (the default), followed from this panel; `panel`, in an agent panel of its own, the user's to go on with"
                },
                "title": {
                    "type": "string",
                    "description": "A short name for the session, a few words on its part of the work: its panel's title and its name among the sessions"
                }
            },
            "required": ["prompt"]
        })
    }

    fn prompt_snippet(&self) -> Option<&str> {
        ToolText::seed(START_SESSION).snippet.as_deref()
    }

    fn prompt_guidelines(&self) -> &[String] {
        &ToolText::seed(START_SESSION).guidelines
    }

    fn execute(
        &self,
        call: &ToolCall,
        ctx: &ToolContext,
        on_update: &mut dyn FnMut(ToolUpdate),
        cancel: &CancelToken,
    ) -> ToolResultMessage {
        let request = match self.request(call) {
            Ok(request) => request,
            Err(message) => return ToolResultMessage::error(call, message),
        };
        let outcome = (self.start)(&request, ctx, cancel, on_update);
        // What the delegate spent and where its session went ride in the
        // details, for the panel; the model reads the answer alone.
        let result = match outcome.answer {
            Ok(report) => ToolResultMessage::text(call, report),
            Err(message) => ToolResultMessage::error(call, message),
        };
        let result = match outcome.session {
            Some(path) => result.with_details(json!({
                SUBAGENT_SESSION_KEY: path.to_string_lossy(),
            })),
            None => result,
        };
        result.with_spent(outcome.spent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A request as the closure sees it, owned.
    #[derive(Debug, PartialEq)]
    struct Seen {
        agent: Option<String>,
        prompt: String,
        title: Option<String>,
        context: SessionContext,
        focus: Option<String>,
        reply: SessionReply,
        show: SessionShow,
    }

    fn tool(seen: &Arc<Mutex<Vec<Seen>>>) -> StartSessionTool {
        let log = Arc::clone(seen);
        let start: StartSession = Arc::new(move |request: &SessionRequest, _ctx, _c, _u| {
            log.lock().unwrap().push(Seen {
                agent: request.agent.map(str::to_string),
                prompt: request.prompt.to_string(),
                title: request.title.map(str::to_string),
                context: request.context,
                focus: request.focus.map(str::to_string),
                reply: request.reply,
                show: request.show,
            });
            if request.agent == Some("writer") {
                return SubagentOutcome {
                    answer: Err("the run failed".into()),
                    spent: Usage {
                        input: 7,
                        ..Usage::default()
                    },
                    session: None,
                    parked: false,
                    failure: None,
                };
            }
            SubagentOutcome {
                answer: Ok("looks fine".into()),
                spent: Usage {
                    output: 3,
                    ..Usage::default()
                },
                session: Some(PathBuf::from("/logs/s.jsonl")),
                parked: false,
                failure: None,
            }
        });
        StartSessionTool::new(
            vec![
                ("reviewer".into(), "Reviews diffs".into()),
                ("writer".into(), String::new()),
            ],
            start,
        )
    }

    fn run(tool: &StartSessionTool, args: Value) -> ToolResultMessage {
        tool.execute(
            &ToolCall {
                id: "s1".into(),
                name: START_SESSION.into(),
                arguments: args,
                extra_content: None,
            },
            &ToolContext::new(std::env::temp_dir()),
            &mut |_| {},
            &CancelToken::new(),
        )
    }

    #[test]
    fn it_lists_the_agents_and_reports_what_the_session_answered() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let tool = tool(&seen);
        assert!(tool.description().contains("reviewer: Reviews diffs"));
        assert_eq!(
            tool.parameters()["properties"]["agent"]["enum"],
            json!(["reviewer", "writer"])
        );
        let ok = run(
            &tool,
            json!({ "agent": "reviewer", "prompt": "check the diff" }),
        );
        assert!(!ok.is_error);
        assert_eq!(ok.plain_text(), "looks fine");
        assert_eq!(ok.spent().map(|spent| spent.output), Some(3));
        // The log beside what was spent.
        assert_eq!(
            ok.details.as_ref().unwrap()[SUBAGENT_SESSION_KEY],
            "/logs/s.jsonl"
        );
        let failed = run(&tool, json!({ "agent": "writer", "prompt": "x" }));
        assert!(failed.is_error);
        assert_eq!(failed.plain_text(), "the run failed");
        // A failed run spent tokens too.
        assert_eq!(failed.spent().map(|spent| spent.input), Some(7));
    }

    #[test]
    fn defaults_follow_where_the_session_is_shown() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let tool = tool(&seen);
        run(&tool, json!({ "agent": "reviewer", "prompt": " look " }));
        run(
            &tool,
            json!({ "agent": "reviewer", "prompt": "look", "show": "panel", "title": " Review " }),
        );
        run(
            &tool,
            json!({ "context": "fork", "prompt": "port it", "focus": " the port " }),
        );
        run(
            &tool,
            json!({ "context": "fork", "prompt": "port it", "show": "panel", "reply": "none" }),
        );
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen[0],
            Seen {
                agent: Some("reviewer".into()),
                prompt: "look".into(),
                title: None,
                context: SessionContext::Fresh,
                focus: None,
                reply: SessionReply::Wait,
                show: SessionShow::Background,
            }
        );
        // In a panel, the answer comes later unless the call says otherwise.
        assert_eq!(
            (seen[1].reply, seen[1].show, seen[1].title.as_deref()),
            (SessionReply::Later, SessionShow::Panel, Some("Review"))
        );
        assert_eq!(
            (seen[2].context, seen[2].focus.as_deref(), seen[2].reply),
            (SessionContext::Fork, Some("the port"), SessionReply::Wait)
        );
        assert_eq!(seen[3].agent, None);
        assert_eq!(
            (seen[3].reply, seen[3].show),
            (SessionReply::None, SessionShow::Panel)
        );
    }

    #[test]
    fn combinations_that_make_no_session_are_refused() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let tool = tool(&seen);
        let refused = |args: Value, words: &str| {
            let result = run(&tool, args);
            assert!(result.is_error, "{}", result.plain_text());
            assert!(
                result.plain_text().contains(words),
                "{}",
                result.plain_text()
            );
        };
        refused(json!({ "agent": "reviewer" }), "`prompt`");
        refused(json!({ "prompt": "x" }), "missing argument `agent`");
        refused(
            json!({ "agent": "ghost", "prompt": "x" }),
            "no agent named ghost",
        );
        refused(
            json!({ "agent": "reviewer", "prompt": "x", "focus": "y" }),
            "`focus`",
        );
        refused(
            json!({ "agent": "reviewer", "prompt": "x", "show": "panel", "reply": "wait" }),
            "no call waits",
        );
        refused(
            json!({ "context": "fork", "prompt": "x", "show": "panel" }),
            "set `reply` to `none`",
        );
        refused(
            json!({ "agent": "reviewer", "prompt": "x", "context": "copy" }),
            "`context` is none or fork",
        );
        refused(
            json!({ "agent": "reviewer", "prompt": "x", "reply": "soon" }),
            "`reply` is wait or later or none",
        );
        // None of them reached the closure.
        assert!(seen.lock().unwrap().is_empty());
    }

    #[test]
    fn only_a_fork_in_a_panel_is_not_a_delegate() {
        let request = |context, show| SessionRequest {
            agent: None,
            prompt: "x",
            title: None,
            context,
            focus: None,
            reply: SessionReply::None,
            show,
            call: "s1",
        };
        assert!(request(SessionContext::Fresh, SessionShow::Panel).delegated());
        assert!(request(SessionContext::Fork, SessionShow::Background).delegated());
        assert!(!request(SessionContext::Fork, SessionShow::Panel).delegated());
    }
}
