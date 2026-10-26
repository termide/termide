//! `agents` and `send_message` for the agents in the panels: the calls run on
//! the agents' threads, the panels live on the UI thread, so a call sends its
//! request down a process-wide channel and the app answers it on its tick.
//!
//! A message goes to a session, not a panel: to the panel the session is
//! open in when it arrives, or, when it is open in none, into its log, to go
//! with its next request once the user opens it again. It goes in as the
//! sending agent's, headed with its name and panel, and counts how many messages agents have passed one another since the user
//! last wrote: past [`MAX_HOPS`] the next is refused, so two agents cannot
//! keep each other going without anyone watching.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use termide_agent_core::{MessageOrigin, OriginKind, Session, Tool, ToolContext, UserMessage};
use termide_agent_tools::{AgentsTool, PeerInfo, SendMessageTool};
use termide_panel_agent::AgentPanel;

use crate::app::App;

/// How many messages agents may pass one another before the user writes
/// again.
pub(super) const MAX_HOPS: u32 = 10;

/// How long a call waits for the app to answer.
const REPLY_WAIT: Duration = Duration::from_secs(10);

/// What a call asks of the app.
enum Request {
    List {
        /// The panel the call comes from.
        from: Option<u64>,
        project_root: PathBuf,
        reply: Sender<Result<Vec<PeerInfo>, String>>,
    },
    Send {
        from: Option<u64>,
        /// The sending session.
        session: Option<String>,
        agent: String,
        project_root: PathBuf,
        /// The session written to.
        to: String,
        kind: OriginKind,
        text: String,
        reply: Sender<Result<String, String>>,
    },
    /// A background task's answer, for the panel that handed it over.
    Report {
        /// The project of the panel.
        project_root: PathBuf,
        /// The panel that handed it over.
        to: u64,
        /// The session that handed it over, which the answer is for
        /// wherever it is by then; `None` leaves it to the panel.
        parent: Option<String>,
        /// The delegate.
        agent: String,
        text: String,
        /// The task was stopped with the panel's run: its answer waits for
        /// the panel's next request rather than start one.
        held: bool,
    },
    /// How many messages agents passed one another to get the run of a
    /// panel going.
    Hops {
        project_root: PathBuf,
        panel: u64,
        reply: Sender<Result<u32, String>>,
    },
}

fn port() -> &'static (Sender<Request>, Mutex<Receiver<Request>>) {
    static PORT: OnceLock<(Sender<Request>, Mutex<Receiver<Request>>)> = OnceLock::new();
    PORT.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        (sender, Mutex::new(receiver))
    })
}

/// Set once an app answers the requests.
static SERVED: AtomicBool = AtomicBool::new(false);

/// Send `request` and wait for the answer `answer` brings.
fn ask<T>(request: Request, answer: &Receiver<Result<T, String>>) -> Result<T, String> {
    if !SERVED.load(Ordering::Acquire) {
        return Err("no agent panels are open to message here".into());
    }
    port()
        .0
        .send(request)
        .map_err(|_| "the panels cannot be reached".to_string())?;
    match answer.recv_timeout(REPLY_WAIT) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
            Err("the panels did not answer".into())
        }
    }
}

/// `agents` and `send_message` for the agent `caller` of a panel in the
/// project at `project_root`.
pub(super) fn peer_tools(caller: &str, project_root: &Path) -> [Arc<dyn Tool>; 2] {
    let root = project_root.to_path_buf();
    let list = Arc::new(move |ctx: &ToolContext| {
        let (reply, answer) = mpsc::channel();
        ask(
            Request::List {
                from: ctx.panel,
                project_root: root.clone(),
                reply,
            },
            &answer,
        )
    });
    let root = project_root.to_path_buf();
    let agent = caller.to_string();
    let send = Arc::new(
        move |ctx: &ToolContext, to: &str, kind: OriginKind, text: &str| {
            let (reply, answer) = mpsc::channel();
            ask(
                Request::Send {
                    from: ctx.panel,
                    session: ctx.session.as_ref().and_then(|session| session.id.clone()),
                    agent: agent.clone(),
                    project_root: root.clone(),
                    to: to.to_string(),
                    kind,
                    text: text.to_string(),
                    reply,
                },
                &answer,
            )
        },
    );
    [
        Arc::new(AgentsTool::new(list)),
        Arc::new(SendMessageTool::new(send)),
    ]
}

/// Whether panel `panel` may hand a task over in the background: its
/// answer is one more message passed without the user, refused past
/// [`MAX_HOPS`] as any other.
pub(super) fn may_hand_over(project_root: &Path, panel: u64) -> Result<(), String> {
    let (reply, answer) = mpsc::channel();
    let hops = ask(
        Request::Hops {
            project_root: project_root.to_path_buf(),
            panel,
            reply,
        },
        &answer,
    )?;
    next_hops(hops).map(drop).map_err(|_| {
        format!(
            "the agents have passed {MAX_HOPS} messages to one another without the user: \
             hand nothing more over in the background, finish what you can and tell the user \
             where things stand"
        )
    })
}

/// Hand the answer of a background task the delegate `agent` ran to the
/// session `parent` that handed it over from panel `to`; the app delivers
/// it on its tick. Nobody waits for it: one that cannot be delivered is
/// logged. `held`: the task was stopped with the panel's run, so its answer
/// waits for the next request.
pub(super) fn report(
    project_root: &Path,
    to: u64,
    parent: Option<String>,
    agent: &str,
    text: String,
    held: bool,
) {
    let request = Request::Report {
        project_root: project_root.to_path_buf(),
        to,
        parent,
        agent: agent.to_string(),
        text,
        held,
    };
    if !SERVED.load(Ordering::Acquire) || port().0.send(request).is_err() {
        log::warn!("the answer of {agent}'s background task found no panel to go to");
    }
}

/// Whether `a` and `b` name one directory.
pub(in crate::app) fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || dunce::canonicalize(a)
            .ok()
            .zip(dunce::canonicalize(b).ok())
            .is_some_and(|(a, b)| a == b)
}

impl App {
    /// Answer what the agents asked of the panels. Called on every tick.
    pub(in crate::app) fn serve_peers(&mut self) {
        SERVED.store(true, Ordering::Release);
        let requests: Vec<Request> = port()
            .1
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .try_iter()
            .collect();
        for request in requests {
            match request {
                Request::List {
                    from,
                    project_root,
                    reply,
                } => {
                    let listed = self
                        .in_project(&project_root, |app| app.list_peers(from))
                        .and_then(|listed| listed);
                    let _ = reply.send(listed);
                }
                Request::Send {
                    from,
                    session,
                    agent,
                    project_root,
                    to,
                    kind,
                    text,
                    reply,
                } => {
                    let message = Outgoing {
                        from,
                        session,
                        agent,
                        to,
                        kind,
                        text,
                    };
                    let result = self
                        .in_project(&project_root, |app| app.send_to_peer(message))
                        .and_then(|sent| sent);
                    if result.is_ok() {
                        self.state.needs_redraw = true;
                    }
                    let _ = reply.send(result);
                }
                Request::Hops {
                    project_root,
                    panel,
                    reply,
                } => {
                    let hops = self.in_project(&project_root, |app| app.agent_hops_of(panel));
                    let _ = reply.send(hops);
                }
                Request::Report {
                    project_root,
                    to,
                    parent,
                    agent,
                    text,
                    held,
                } => match self
                    .in_project(&project_root, |app| {
                        app.report_to_session(to, parent.as_deref(), agent, text, held)
                    })
                    .and_then(|delivered| delivered)
                {
                    Ok(()) => self.state.needs_redraw = true,
                    Err(error) => log::warn!("a background task's answer was lost: {error}"),
                },
            }
        }
    }

    /// The agent panels of the current project, or of the one
    /// [`App::in_project`] stands in for it.
    fn agent_panels(&mut self) -> impl Iterator<Item = &mut AgentPanel> {
        self.layout_manager
            .iter_all_panels_mut()
            .filter_map(|panel| panel.as_any_mut().downcast_mut::<AgentPanel>())
    }

    fn list_peers(&mut self, from: Option<u64>) -> Result<Vec<PeerInfo>, String> {
        Ok(self
            .agent_panels()
            .filter_map(|panel| {
                Some(PeerInfo {
                    session: panel.session_id()?,
                    panel: panel.panel_id(),
                    agent: panel.agent_name().to_string(),
                    state: panel.peer_state().label(),
                    cwd: panel.cwd().to_path_buf(),
                    own: Some(panel.panel_id()) == from,
                })
            })
            .collect())
    }

    /// The panel the session `id` is open in.
    pub(in crate::app) fn panel_of_session(&mut self, id: &str) -> Option<&mut AgentPanel> {
        self.agent_panels()
            .find(|panel| panel.session_id().as_deref() == Some(id))
    }

    /// Leave `message` in the log of the session `id`, which no panel has
    /// open: the project's own sessions are looked through first, then
    /// those of directories outside it that its panels worked in.
    fn leave_in_log(&self, id: &str, message: UserMessage) -> Result<(), String> {
        let root = termide_config::get_config_dir().ok().map(|dir| {
            dir.join(termide_agent_core::GLOBAL_AGENT_DIR)
                .join(termide_agent_core::SESSIONS_DIR)
        });
        let dirs = self.state.ai_sessions_dir().into_iter().chain(root);
        leave_in_log(dirs, id, message)
    }

    /// How many messages agents passed one another to get the run of panel
    /// `id` going; none for a panel that is not open.
    pub(super) fn agent_hops_of(&mut self, id: u64) -> u32 {
        self.agent_panels()
            .find(|panel| panel.panel_id() == id)
            .map_or(0, |panel| panel.agent_hops())
    }

    /// Deliver a background task's answer to the session `parent` that
    /// handed it over from panel `to`, as the delegate's reply: one more
    /// message passed without the user. The session may show in another
    /// panel by then, or in none: its log keeps the answer then. Without a
    /// session it goes to the panel.
    fn report_to_session(
        &mut self,
        to: u64,
        parent: Option<&str>,
        agent: String,
        text: String,
        held: bool,
    ) -> Result<(), String> {
        let origin = |hops| MessageOrigin {
            session: String::new(),
            agent,
            kind: OriginKind::Reply,
            panel: None,
            hops,
        };
        let target = match parent {
            Some(id) => self.panel_of_session(id),
            None => self.agent_panels().find(|p| p.panel_id() == to),
        };
        let Some(target) = target else {
            return match parent {
                Some(id) => {
                    // The count it started on is gone with the panel's run.
                    let message = UserMessage::text(text).with_origin(origin(1));
                    self.leave_in_log(id, message)
                }
                None => Err(format!("panel {to} that handed it over is closed")),
            };
        };
        let message = UserMessage::text(text).with_origin(origin(target.agent_hops() + 1));
        if held {
            target.deliver_held(message)
        } else {
            target.deliver(message)
        }
    }

    /// A delegated task carried on in a panel of its own after termide closed
    /// is over: its answer goes back to the panel that delegated it, which is
    /// what the `task` call was made for. Panel numbers are born anew each
    /// start of termide, so the delegating session names its panel here; with
    /// no panel holding that session, the delegate's own panel says the
    /// answer has nowhere to go, and it stays in its log.
    pub(in crate::app) fn delegate_answer_to_parent(
        &mut self,
        parent_session: String,
        from: PathBuf,
        agent: String,
        text: String,
    ) {
        let target = self
            .panel_of_session(&parent_session)
            .map(|panel| panel.panel_id());
        let missed = match target {
            // Reported: the panel takes it as a reply and works on it.
            Some(to) => self
                .report_to_session(to, Some(&parent_session), agent.clone(), text, false)
                .err(),
            None => Some(termide_i18n::t().agent_notice_no_parent_panel().to_string()),
        };
        if let Some(notice) = missed {
            log::warn!("a carried-on delegate's answer was lost: {notice}");
            if let Some(panel) = self
                .agent_panels()
                .find(|panel| panel.session_path() == Some(from.as_path()))
            {
                panel.notice(notice, termide_panel_agent::NoticeKind::Warn);
            }
        }
        self.state.needs_redraw = true;
    }

    fn send_to_peer(&mut self, message: Outgoing) -> Result<String, String> {
        let to = message.to;
        let from = message.from;
        if message.session.as_deref() == Some(to.as_str()) {
            return Err(format!("session {to} is this one"));
        }
        let sender = self
            .agent_panels()
            .find(|panel| Some(panel.panel_id()) == from)
            .map(|panel| (panel.agent_hops(), panel.delegated_by()));
        let hops = next_hops(sender.map_or(0, |(hops, _)| hops))?;
        let delivered = UserMessage::text(message.text).with_origin(MessageOrigin {
            session: message.session.unwrap_or_default(),
            agent: message.agent,
            kind: message.kind,
            panel: from,
            hops,
        });
        let Some(target) = self.panel_of_session(&to) else {
            // The user left the session the message is for: it is not the
            // one the panel shows now, so it waits in the session's log.
            self.leave_in_log(&to, delivered)?;
            return Ok(format!(
                "Session {to} is open in no panel: the message waits in its log, and its agent \
                 reads it when the user opens the session again. Do not wait for an answer."
            ));
        };
        let panel = target.panel_id();
        if Some(panel) == from {
            return Err(format!("session {to} is this one"));
        }
        // A task's answer goes back as the delegate's last message; one sent
        // as well would reach the delegating agent twice.
        if let Some((_, Some(by))) = sender {
            if by == panel {
                return Err(format!(
                    "you run a task session {to} handed you: your answer goes back to it as \
                     your final message, so do not message it"
                ));
            }
        }
        let state = target.peer_state().label();
        let agent = target.agent_name().to_string();
        target.deliver(delivered)?;
        Ok(format!(
            "Delivered to session {to} ({agent}, {state}, panel {panel}). An answer, if any, \
             comes as a message of its own."
        ))
    }
}

/// Leave `message` in the log of the session `id`, found in the first of
/// `dirs` that has it: it goes with the session's next request once the user
/// opens it again, as the answers held for that request do. A session open
/// anywhere — a panel of this termide or another — is not written to.
fn leave_in_log(
    dirs: impl IntoIterator<Item = PathBuf>,
    id: &str,
    message: UserMessage,
) -> Result<(), String> {
    let path = dirs
        .into_iter()
        .find_map(|dir| termide_agent_recall::find_log(&dir, id))
        .ok_or_else(|| {
            format!("no session {id} is open in a panel or kept; `agents` lists the open ones")
        })?;
    let mut session = Session::open_exclusive(&path)
        .map_err(|error| format!("session {id} cannot take the message: {error}"))?;
    let mut held = session.held_reports();
    held.push(message);
    session
        .append_held_reports(&held)
        .map(drop)
        .map_err(|error| format!("session {id} cannot take the message: {error}"))
}

/// The count a message from a panel whose run `sent` messages got going
/// carries, or why it may not go.
fn next_hops(sent: u32) -> Result<u32, String> {
    let hops = sent + 1;
    if hops > MAX_HOPS {
        return Err(format!(
            "the agents have passed {MAX_HOPS} messages to one another without the user: \
             send no more, finish what you can and tell the user where things stand"
        ));
    }
    Ok(hops)
}

/// A message on its way to another panel.
struct Outgoing {
    from: Option<u64>,
    session: Option<String>,
    agent: String,
    to: String,
    kind: OriginKind,
    text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_agent_core::{CancelToken, ToolCall};

    #[test]
    fn a_chain_of_messages_without_the_user_is_cut_off() {
        assert_eq!(next_hops(0), Ok(1));
        assert_eq!(next_hops(MAX_HOPS - 1), Ok(MAX_HOPS));
        assert!(next_hops(MAX_HOPS)
            .unwrap_err()
            .contains("without the user"));
    }

    fn from_peer(text: &str) -> UserMessage {
        UserMessage::text(text).with_origin(MessageOrigin {
            session: "s1".into(),
            agent: "default".into(),
            kind: OriginKind::Reply,
            panel: Some(1),
            hops: 1,
        })
    }

    #[test]
    fn a_message_to_a_closed_session_waits_in_its_log() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let nested = dir.path().join("sub");
        let session = Session::create(&nested, dir.path()).unwrap();
        let (id, path) = (session.id().to_string(), session.path().to_path_buf());
        drop(session);
        let dirs = || [other.path().to_path_buf(), dir.path().to_path_buf()];
        leave_in_log(dirs(), &id, from_peer("first")).unwrap();
        leave_in_log(dirs(), &id, from_peer("second")).unwrap();
        // They wait for the next request, one after another, as they came.
        let held = Session::open(&path).unwrap().held_reports();
        let texts: Vec<String> = held.iter().map(UserMessage::plain_text).collect();
        assert_eq!(texts, ["first", "second"]);
        assert_eq!(held[0].origin, from_peer("first").origin);
        assert!(leave_in_log(dirs(), "00000000", from_peer("x"))
            .unwrap_err()
            .contains("no session 00000000"));
    }

    #[test]
    fn a_session_open_in_a_panel_is_not_written_to_from_outside() {
        let dir = tempfile::tempdir().unwrap();
        let open = Session::create_exclusive(dir.path(), dir.path()).unwrap();
        let id = open.id().to_string();
        let refused = leave_in_log([dir.path().to_path_buf()], &id, from_peer("x"));
        assert!(refused.unwrap_err().contains("cannot take the message"));
        assert!(open.held_reports().is_empty());
    }

    #[test]
    fn without_an_app_the_tools_say_there_is_no_one_to_message() {
        let [agents, send] = peer_tools("default", Path::new("/proj"));
        let call = |name: &str, args| ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
            extra_content: None,
        };
        let ctx = ToolContext::new("/proj");
        let listed = agents.execute(
            &call("agents", serde_json::json!({})),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(listed.is_error && listed.plain_text().contains("no agent panels"));
        let sent = send.execute(
            &call(
                "send_message",
                serde_json::json!({ "to": "1a2b3c4d", "kind": "info", "text": "hi" }),
            ),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(sent.is_error);
    }
}
