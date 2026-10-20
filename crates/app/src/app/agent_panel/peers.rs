//! `agents` and `send_message` for the agents in the panels: the calls run on
//! the agents' threads, the panels live on the UI thread, so a call sends its
//! request down a process-wide channel and the app answers it on its tick.
//!
//! A message goes in as the sending agent's, headed with its name and panel,
//! and counts how many messages agents have passed one another since the user
//! last wrote: past [`MAX_HOPS`] the next is refused, so two agents cannot
//! keep each other going without anyone watching.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use termide_agent_core::{MessageOrigin, OriginKind, Tool, ToolContext, UserMessage};
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
        /// The sending session, when termide's loop runs it.
        session: Option<String>,
        agent: String,
        project_root: PathBuf,
        to: u64,
        kind: OriginKind,
        text: String,
        reply: Sender<Result<String, String>>,
    },
    /// A background task's answer, for the panel that handed it over.
    Report {
        /// The project of the panel.
        project_root: PathBuf,
        to: u64,
        /// The delegate.
        agent: String,
        text: String,
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
        move |ctx: &ToolContext, to: u64, kind: OriginKind, text: &str| {
            let (reply, answer) = mpsc::channel();
            ask(
                Request::Send {
                    from: ctx.panel,
                    session: ctx.session.as_ref().and_then(|session| session.id.clone()),
                    agent: agent.clone(),
                    project_root: root.clone(),
                    to,
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
/// panel `to` that handed it over; the app delivers it on its tick. Nobody
/// waits for it: one that cannot be delivered is logged.
pub(super) fn report(project_root: &Path, to: u64, agent: &str, text: String) {
    let request = Request::Report {
        project_root: project_root.to_path_buf(),
        to,
        agent: agent.to_string(),
        text,
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
                    agent,
                    text,
                } => match self
                    .in_project(&project_root, |app| app.report_to_panel(to, agent, text))
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
            .map(|panel| PeerInfo {
                panel: panel.panel_id(),
                agent: panel.agent_name().to_string(),
                state: panel.peer_state().label(),
                cwd: panel.cwd().to_path_buf(),
                own: Some(panel.panel_id()) == from,
            })
            .collect())
    }

    /// How many messages agents passed one another to get the run of panel
    /// `id` going; none for a panel that is not open.
    pub(super) fn agent_hops_of(&mut self, id: u64) -> u32 {
        self.agent_panels()
            .find(|panel| panel.panel_id() == id)
            .map_or(0, |panel| panel.agent_hops())
    }

    /// Deliver a background task's answer to the panel `to` that handed it
    /// over, as the delegate's reply: one more message passed without the
    /// user.
    fn report_to_panel(&mut self, to: u64, agent: String, text: String) -> Result<(), String> {
        let Some(target) = self.agent_panels().find(|p| p.panel_id() == to) else {
            return Err(format!("panel {to} that handed it over is closed"));
        };
        let hops = target.agent_hops() + 1;
        target.deliver(UserMessage::text(text).with_origin(MessageOrigin {
            session: String::new(),
            agent,
            kind: OriginKind::Reply,
            panel: None,
            hops,
        }))
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
            .agent_panels()
            .find(|panel| panel.session_id().as_deref() == Some(parent_session.as_str()))
            .map(|panel| panel.panel_id());
        let missed = match target {
            // Reported: the panel takes it as a reply and works on it.
            Some(to) => self.report_to_panel(to, agent.clone(), text).err(),
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
        if Some(message.to) == message.from {
            return Err(format!("panel {} is this one", message.to));
        }
        let from = message.from;
        let sender = self
            .agent_panels()
            .find(|panel| Some(panel.panel_id()) == from)
            .map(|panel| (panel.agent_hops(), panel.delegated_by()));
        // A task's answer goes back as the delegate's last message; one sent
        // as well would reach the delegating agent twice.
        if let Some((_, Some(by))) = sender {
            if by == message.to {
                return Err(format!(
                    "you run a task panel {by} handed you: your answer goes back to it as \
                     your final message, so do not message it"
                ));
            }
        }
        let hops = next_hops(sender.map_or(0, |(hops, _)| hops))?;
        let Some(target) = self
            .agent_panels()
            .find(|panel| panel.panel_id() == message.to)
        else {
            return Err(format!(
                "no agent panel {} is open; `agents` lists them",
                message.to
            ));
        };
        let state = target.peer_state().label();
        let agent = target.agent_name().to_string();
        target.deliver(UserMessage::text(message.text).with_origin(MessageOrigin {
            session: message.session.unwrap_or_default(),
            agent: message.agent,
            kind: message.kind,
            panel: from,
            hops,
        }))?;
        Ok(format!(
            "Delivered to panel {} ({agent}, {state}). An answer, if any, comes as a message of its own.",
            message.to
        ))
    }
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
    to: u64,
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
                serde_json::json!({ "to": 2, "kind": "info", "text": "hi" }),
            ),
            &ctx,
            &mut |_| {},
            &CancelToken::new(),
        );
        assert!(sent.is_error);
    }
}
