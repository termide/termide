use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use termide_agent_core::FitStep;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use ratatui::text::Line;
use termide_agent_core::{
    now_millis, permission_channel, question_channel, suggestion_channel, Agent, AgentEvent,
    AssistantContent, AssistantMessage, Autorun, CancelToken, CompactionCall, CompactionReason,
    EntryKind, Failure, FailureKind, GoalRecord, LastRun, LimitPolicy, LoopRecord, Message,
    PermissionAnswer, PermissionPrompter, QuestionAnswer, QuestionReply, Request, ShellOutput,
    ShellRunner, StopReason, StreamEvent, Suggestion, SuggestionKind, SuggestionReply,
    ThinkingLevel, Timing, ToolCall, ToolContext, ToolDecision, ToolResultMessage, ToolUpdate,
    Usage, UserMessage, ACP_PROVIDER, MODEL_OPTION,
};
use termide_core::{
    AgentSnippet, ConfirmAction, LinkTarget, PanelConfig, SegmentKind, SnippetSource,
    SnippetWanted, TerminalPart,
};
use termide_ui::{ChoiceAction, ChoiceForm};

use crate::input::{file_completions, snippet_text};
use crate::pending::Pending;
use crate::render::context_bar;
use crate::runtime::{push_history, session_model};
use crate::submit::{parse_duration, parse_loop_args, slash_command};
use crate::toolset::{Blocked, ToolsetGuard, TOOLSET_ACTION};
use crate::unfinished::Restored;

mod banner;
mod commands;
mod connections;
mod delegates;
mod external;
mod failures;
mod goals;
mod history;
mod keys;
mod media;
mod modes;
mod prompt;
mod questions;
mod reopen;
mod runs;
mod selection;
mod sessions;
mod shell;
mod strip;

/// Replays one scripted assistant message per model call and records
/// which model each call asked for.
struct Scripted {
    replies: Mutex<Vec<AssistantMessage>>,
    models: Result<Vec<ModelInfo>, String>,
    seen_models: Mutex<Vec<String>>,
    /// The reasoning levels offered, and those each call asked for.
    levels: Vec<ThinkingLevel>,
    seen_thinking: Mutex<Vec<ThinkingLevel>>,
}

impl Scripted {
    fn new(replies: Vec<AssistantMessage>) -> Self {
        Self {
            replies: Mutex::new(replies),
            models: Ok(vec![
                ModelInfo {
                    id: "big".into(),
                    name: Some("Big Model".into()),
                    context_window: Some(64_000),
                },
                ModelInfo {
                    id: "m".into(),
                    name: None,
                    context_window: None,
                },
            ]),
            seen_models: Mutex::new(Vec::new()),
            levels: vec![
                ThinkingLevel::Off,
                ThinkingLevel::Low,
                ThinkingLevel::Medium,
                ThinkingLevel::High,
            ],
            seen_thinking: Mutex::new(Vec::new()),
        }
    }
}

impl Provider for Scripted {
    fn name(&self) -> &str {
        "scripted"
    }
    fn stream(
        &self,
        request: &Request<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        _cancel: &CancelToken,
    ) -> AssistantMessage {
        self.seen_models
            .lock()
            .unwrap()
            .push(request.model.id.clone());
        self.seen_thinking.lock().unwrap().push(request.thinking);
        let mut replies = self.replies.lock().unwrap();
        if replies.is_empty() {
            return AssistantMessage::failed("scripted", "m", StopReason::Error, "exhausted");
        }
        let reply = replies.remove(0);
        let thinking = reply.thinking_text();
        if !thinking.is_empty() {
            on_event(StreamEvent::ThinkingDelta(thinking));
        }
        on_event(StreamEvent::TextDelta(reply.plain_text()));
        reply
    }
    fn list_models(&self) -> Result<Vec<ModelInfo>, String> {
        self.models.clone()
    }
    fn thinking_levels(&self, _model: &str) -> Vec<ThinkingLevel> {
        self.levels.clone()
    }
}

fn reply_thinking(text: &str, thinking: &str) -> AssistantMessage {
    let mut message = reply(text);
    message
        .content
        .insert(0, AssistantContent::thinking(thinking));
    message
}

fn reply(text: &str) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantContent::Text { text: text.into() }],
        stop_reason: StopReason::Stop,
        usage: Usage {
            input: 100,
            output: 20,
            cache_read: 0,
            cache_write: 0,
            ..Default::default()
        },
        provider: "scripted".into(),
        model: "m".into(),
        error_message: None,
        failure: None,
        timestamp: 0,
    }
}

fn panel(replies: Vec<AssistantMessage>) -> AgentPanel {
    AgentPanel::new(setup(replies))
}

fn setup(replies: Vec<AssistantMessage>) -> AgentPanelSetup {
    setup_with(Arc::new(Scripted::new(replies)))
}

/// Command scripts the test catalog offers; tests fill it.
static COMMANDS: Mutex<Vec<CommandScript>> = Mutex::new(Vec::new());

/// Two agents: the default one and a terse reviewer on another model.
struct Agents;

impl AgentCatalog for Agents {
    fn list(&self) -> Vec<AgentEntry> {
        vec![
            AgentEntry {
                name: "default".into(),
                description: String::new(),
                icon: None,
            },
            AgentEntry {
                name: "review".into(),
                description: "Reviews diffs".into(),
                icon: Some("🔍".into()),
            },
            AgentEntry {
                name: "outside".into(),
                description: "An external agent".into(),
                icon: None,
            },
        ]
    }
    fn prompts(&self) -> Vec<PromptTemplate> {
        vec![PromptTemplate {
            name: "review".into(),
            description: "Review a file".into(),
            argument_hint: "<path>".into(),
            body: "Review $1 carefully.".into(),
        }]
    }
    fn commands(&self) -> Vec<CommandScript> {
        COMMANDS.lock().unwrap().clone()
    }
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        match name {
            "default" => Some(AgentProfile {
                system_prompt: "default prompt".into(),
                tools: ToolRegistry::new(),
                model: None,
                mode: None,
                late_tools: None,
                backend: None,
                offered: Vec::new(),
                skills: Vec::new(),
            }),
            "review" => Some(AgentProfile {
                system_prompt: "You review diffs.".into(),
                tools: ToolRegistry::new(),
                model: Some("big".into()),
                mode: Some(Mode::Edit),
                late_tools: None,
                backend: None,
                offered: Vec::new(),
                skills: Vec::new(),
            }),
            "outside" => Some(AgentProfile {
                system_prompt: String::new(),
                tools: ToolRegistry::new(),
                model: None,
                mode: None,
                late_tools: None,
                backend: Some(Arc::new(|setup: BackendSetup| {
                    Ok(Box::new(External::new(setup)) as Box<dyn Backend>)
                })),
                offered: Vec::new(),
                skills: Vec::new(),
            }),
            _ => None,
        }
    }
}

fn setup_with(provider: Arc<Scripted>) -> AgentPanelSetup {
    AgentPanelSetup {
        panel_id: None,
        cwd: PathBuf::from("/tmp"),
        agent: "default".into(),
        catalog: Arc::new(Agents),
        late_tools: None,
        hooks: None,
        backend: None,
        provider_backend: None,
        connections: None,
        connection: "local".into(),
        connection_settings: termide_config::Connection::default(),
        provider,
        provider_kind: "openai_compatible".into(),
        model: ModelSpec {
            provider: "scripted".into(),
            id: "m".into(),
            context_window: 1000,
            max_tokens: Some(100),
            thinking: ThinkingLevel::Off,
        },
        tools: ToolRegistry::new(),
        // `configured`, not the default `auto`: a reviewer would spend the
        // scripted replies on its own calls.
        rules: PermissionRules {
            mode: Mode::Configured,
            ..PermissionRules::default()
        },
        system_prompt: String::new(),
        compaction: CompactionPolicy::default(),
        compaction_prompts: CompactionPrompts::default(),
        plan_prompt: PlanPrompt::default(),
        goal_prompt: GoalPrompt::default(),
        handoff_prompt: HandoffPrompt::default(),
        loop_prompt: termide_agent_core::LoopPrompt::default(),
        side_models: termide_agent_core::SideModels::default(),
        reviewer: ReviewerSetup::default(),
        refusals: Refusals::default(),
        persist_rule: None,
        session_dir: None,
        session: None,
        // No runner by default: a test that exercises `!` installs one that
        // records what reached the shell, so nothing runs for real here.
        shell_run: None,
        terminals: None,
        fold: FoldMode::OnFinish,
        on_limit: LimitPolicy::Ask,
    }
}

fn chord(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
    let event = KeyEvent::new(code, modifiers);
    KeyChord {
        raw: event,
        canonical: event,
    }
}

/// The screen row of the state strip's row standing for `target`, as last
/// drawn.
fn strip_row(panel: &AgentPanel, target: crate::strip::StripTarget) -> u16 {
    panel
        .strip_hits
        .iter()
        .find(|(_, t, _)| *t == target)
        .map(|(y, ..)| *y)
        .unwrap_or_else(|| panic!("the strip shows {target:?}"))
}

fn strip_text(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn type_text(panel: &mut AgentPanel, text: &str) {
    for c in text.chars() {
        panel.handle_key(chord(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

fn settle(panel: &mut AgentPanel) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.tick();
        if !panel.is_busy() {
            return;
        }
        assert!(Instant::now() < deadline, "agent did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Tick until an event matching `wanted` arrives, returning it.
fn wait_for(panel: &mut AgentPanel, wanted: fn(&PanelEvent) -> bool) -> PanelEvent {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(event) = panel.tick().into_iter().find(&wanted) {
            return event;
        }
        assert!(Instant::now() < deadline, "event did not arrive");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Text of the bold chip that carries `action`.
fn chip(panel: &AgentPanel, action: &str) -> String {
    panel
        .status_segments()
        .into_iter()
        .find(|s| s.action == Some(action) && s.kind == SegmentKind::Active)
        .map(|s| s.text)
        .expect("chip present")
}

fn select(panel: &mut AgentPanel, event: &PanelEvent, index: usize) -> CommandResult {
    let PanelEvent::ShowSelect {
        on_select: SelectAction::Custom(action),
        ..
    } = event
    else {
        panic!("expected a picker, got {event:?}");
    };
    panel.handle_command(PanelCommand::SelectionMade {
        action: action.clone(),
        index,
    })
}

fn render_text(panel: &mut AgentPanel, width: u16, height: u16) -> Vec<String> {
    let buf = render_buf(panel, width, height);
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn render_buf(panel: &mut AgentPanel, width: u16, height: u16) -> Buffer {
    render_buf_focused(panel, width, height, true)
}

fn render_buf_focused(panel: &mut AgentPanel, width: u16, height: u16, is_focused: bool) -> Buffer {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    let colors = ThemeColors::default();
    let config = PanelConfig {
        tab_size: 4,
        word_wrap: false,
        show_line_numbers: false,
        show_hidden_files: false,
    };
    let ctx = RenderContext {
        theme: &colors,
        config: &config,
        is_focused,
        panel_index: 0,
        terminal_width: width,
        terminal_height: height,
        border_right_x: None,
        border_left_x: None,
        border_bottom_y: None,
    };
    panel.render(area, &mut buf, &ctx);
    buf
}

fn rings(events: &[PanelEvent]) -> bool {
    events
        .iter()
        .any(|e| matches!(e, PanelEvent::RequestAttention))
}

/// A run that started `secs` ago ends now.
fn end_run_of(panel: &mut AgentPanel, secs: u64) {
    panel.apply(AgentEvent::AgentStart);
    panel.run_start = Some(Instant::now() - Duration::from_secs(secs));
    panel.apply(AgentEvent::AgentEnd);
}

/// How a summary call went, for the compaction events of the tests.
fn summary_call() -> CompactionCall {
    CompactionCall {
        reason: CompactionReason::Threshold,
        stop_reason: StopReason::Stop,
        duration_ms: 1_500,
        usage: Usage::default(),
        thinking_chars: 0,
        tool_calls: Vec::new(),
        input_cut: Vec::new(),
    }
}

/// Select the tail of the prompt with Shift+arrows.
fn select_back(panel: &mut AgentPanel, steps: usize) {
    for _ in 0..steps {
        panel.handle_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
    }
}

/// Drive the panel until the active goal finishes, or fail on a deadline.
fn settle_goal(panel: &mut AgentPanel) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.goal_task.is_some() {
        panel.tick();
        assert!(Instant::now() < deadline, "goal did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A panel whose `/loop 5m check the build` ran once and waits for its
/// next run.
fn looping_panel() -> AgentPanel {
    let mut panel = panel(vec![reply("built")]);
    assert!(panel.strip_rows(80).is_empty());
    type_text(&mut panel, "/loop 5m check the build");
    panel.submit();
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel
        .loop_task
        .as_ref()
        .is_some_and(|t| t.next_at.is_none())
    {
        panel.tick();
        assert!(Instant::now() < deadline, "the loop did not wait");
        std::thread::sleep(Duration::from_millis(5));
    }
    panel
}

fn roles(messages: &[Message]) -> Vec<&'static str> {
    messages
        .iter()
        .map(|m| match m {
            Message::User(_) => "user",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "tool_result",
        })
        .collect()
}

/// The session logs in a test's session directory, sorted by name.
fn session_files(dir: &tempfile::TempDir) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    files
}

fn fork_request(text: &str) -> UserMessage {
    UserMessage::text(text).with_origin(termide_agent_core::MessageOrigin {
        session: "1a2b3c4d".into(),
        agent: "default".into(),
        kind: termide_agent_core::OriginKind::Fork,
        panel: Some(1),
        hops: 1,
    })
}

/// The rows of the open rewind list: each message and the files rewinding
/// to it puts back.
fn rewind_rows(panel: &AgentPanel) -> Vec<(String, String)> {
    panel
        .rewind_picker
        .as_ref()
        .expect("the rewind list is open")
        .list
        .items()
        .iter()
        .map(|item| (item.label.clone(), item.description.clone()))
        .collect()
}

/// Open the rewind list with Esc and pick the message `back` steps before
/// the newest.
fn rewind_with_esc(panel: &mut AgentPanel, back: usize) {
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.rewind_picker.is_some(), "Esc opened the rewind list");
    for _ in 0..back {
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    }
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
}

/// A panel with a session that has been asked `prompts` in turn, the files
/// in `edits` changed by the request of the same index.
fn asked(dir: &Path, prompts: &[&str], edits: &[(usize, &Path)]) -> AgentPanel {
    std::fs::create_dir_all(dir).unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.to_path_buf(),
        session_dir: Some(dir.join("sessions")),
        ..setup(prompts.iter().map(|p| reply(&format!("re: {p}"))).collect())
    });
    for (at, prompt) in prompts.iter().enumerate() {
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        // The request "edits" its file the way the hook records the edit
        // tool, while it runs.
        for (_, file) in edits.iter().filter(|(of, _)| *of == at) {
            let store = panel.checkpoints.clone().unwrap();
            store.lock().unwrap().save(file).unwrap();
            std::fs::write(file, format!("after {prompt}")).unwrap();
        }
        settle(&mut panel);
    }
    panel
}

fn users(panel: &AgentPanel) -> Vec<String> {
    panel
        .transcript()
        .items()
        .iter()
        .filter_map(|i| match i {
            Item::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A panel in a fresh session whose banner lists sessions with these first
/// prompts, newest last in `prompts`, and their paths in the same order.
fn panel_with_recent_sessions(
    dir: &std::path::Path,
    prompts: &[&str],
) -> (AgentPanel, Vec<std::path::PathBuf>) {
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.to_path_buf()),
        ..setup(prompts.iter().map(|_| reply("done")).collect())
    });
    let mut paths = Vec::new();
    for prompt in prompts {
        paths.push(panel.session_path().unwrap().to_path_buf());
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    (panel, paths)
}

fn ctrl_f() -> KeyChord {
    chord(KeyCode::Char('f'), KeyModifiers::CONTROL)
}

fn ask_in_worker(
    panel: &mut AgentPanel,
    questions: Vec<termide_agent_core::Question>,
) -> std::thread::JoinHandle<QuestionReply> {
    let (asker, rx) = question_channel(CancelToken::new());
    panel.question_rx = rx;
    let worker = std::thread::spawn(move || asker.ask(questions));
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    worker
}

fn question(text: &str, labels: &[&str], multi_select: bool) -> termide_agent_core::Question {
    termide_agent_core::Question {
        header: String::new(),
        question: text.into(),
        options: labels
            .iter()
            .map(|label| termide_agent_core::QuestionOption {
                label: (*label).into(),
                description: format!("about {label}"),
            })
            .collect(),
        multi_select,
    }
}

/// Three connections: a local endpoint, a hosted one, and a CLI agent.
struct Connections;

impl ConnectionCatalog for Connections {
    fn list(&self) -> Vec<ConnectionEntry> {
        [
            ("local", "openai_compatible", "m"),
            ("cloud", "anthropic_compatible", "claude-x"),
            ("cli", "codex", ""),
        ]
        .into_iter()
        .map(|(name, kind, model)| ConnectionEntry {
            name: name.into(),
            kind: kind.into(),
            model: model.into(),
        })
        .collect()
    }

    fn build(&self, name: &str, _agent: &str) -> Option<ConnectionChoice> {
        let entry = self.list().into_iter().find(|entry| entry.name == name)?;
        let backend: Option<BackendFactory> = (entry.kind == "codex").then(|| {
            Arc::new(|setup: BackendSetup| {
                Ok(Box::new(External::following_modes(setup)) as Box<dyn Backend>)
            }) as BackendFactory
        });
        Some(ConnectionChoice {
            name: entry.name,
            kind: entry.kind,
            provider: Arc::new(Scripted::new(vec![])),
            model: entry.model,
            context_window: 200_000,
            backend,
            // Each connection's reviewer reviews under a model of its name.
            side_models: termide_agent_core::SideModels::default(),
            reviewer: ReviewerSetup {
                model: termide_agent_core::ModelChoice {
                    own: None,
                    session_model: Some(format!("{name}-reviewer")),
                },
                ..ReviewerSetup::default()
            },
            settings: termide_config::Connection::default(),
        })
    }
}

/// An endpoint that answers nothing, named by the build that made it, so a
/// test sees a panel take a new one or keep the one it has.
struct Endpoint {
    tag: String,
}

impl Provider for Endpoint {
    fn name(&self) -> &str {
        &self.tag
    }
    fn stream(
        &self,
        _request: &Request<'_>,
        _on_event: &mut dyn FnMut(StreamEvent),
        _cancel: &CancelToken,
    ) -> AssistantMessage {
        AssistantMessage::failed(&self.tag, "m", StopReason::Error, "never asked")
    }
}

/// The `local` connection, its settings moved by the test; each build hands
/// out an endpoint named by its number, so a rebuild is visible.
struct Moving {
    settings: std::sync::Mutex<termide_config::Connection>,
    builds: std::sync::atomic::AtomicUsize,
}

impl Moving {
    fn new() -> Self {
        Self {
            settings: std::sync::Mutex::new(termide_config::Connection {
                base_url: "http://127.0.0.1:10000/v1".into(),
                ..termide_config::Connection::default()
            }),
            builds: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn settings(&self) -> termide_config::Connection {
        self.settings.lock().unwrap().clone()
    }

    fn move_to(&self, url: &str) {
        self.settings.lock().unwrap().base_url = url.into();
    }

    fn panel(self: &Arc<Self>, dir: &std::path::Path) -> AgentPanel {
        let catalog = Arc::clone(self);
        let built = catalog.settings();
        AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.to_path_buf()),
            connections: Some(catalog),
            connection: "local".into(),
            connection_settings: built,
            provider: Arc::new(Endpoint { tag: "ep0".into() }),
            ..setup(vec![])
        })
    }
}

impl ConnectionCatalog for Moving {
    fn list(&self) -> Vec<ConnectionEntry> {
        vec![ConnectionEntry {
            name: "local".into(),
            kind: "openai_compatible".into(),
            model: "m".into(),
        }]
    }

    fn build(&self, name: &str, _agent: &str) -> Option<ConnectionChoice> {
        (name == "local").then(|| ConnectionChoice {
            name: name.into(),
            kind: "openai_compatible".into(),
            provider: Arc::new(Endpoint {
                tag: format!(
                    "ep{}",
                    self.builds
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        + 1
                ),
            }),
            model: "m".into(),
            context_window: 200_000,
            backend: None,
            side_models: termide_agent_core::SideModels::default(),
            reviewer: ReviewerSetup::default(),
            settings: self.settings(),
        })
    }
}

fn connected_panel(dir: &std::path::Path) -> AgentPanel {
    AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.to_path_buf()),
        connections: Some(Arc::new(Connections)),
        ..setup(vec![])
    })
}

/// A 1×1 PNG.
const DOT_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F, 0x15, 0xC4,
    0x89, 0x00, 0x00, 0x00, 0x0B, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60, 0x00, 0x02, 0x00,
    0x00, 0x05, 0x00, 0x01, 0x7A, 0x5E, 0xAB, 0x3F, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44,
    0xAE, 0x42, 0x60, 0x82,
];

/// A catalog that records the modes the panel reports.
struct ModeWatcher(Arc<Mutex<Vec<Mode>>>);

impl AgentCatalog for ModeWatcher {
    fn list(&self) -> Vec<AgentEntry> {
        Agents.list()
    }
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        Agents.resolve(name)
    }
    fn set_mode(&self, mode: Mode) {
        self.0.lock().unwrap().push(mode);
    }
}

/// The test catalog plus skills in a directory of its own: `review`,
/// which the template of the same name hides, and `deploy`.
struct Skilled(tempfile::TempDir);

impl Skilled {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("review", "---\nname: review\n---\nSkill review of $1.\n"),
            (
                "deploy",
                "---\nname: deploy\nargument-hint: <version>\n---\nShip $1.\n",
            ),
        ] {
            std::fs::create_dir_all(dir.path().join(name)).unwrap();
            std::fs::write(dir.path().join(name).join("SKILL.md"), body).unwrap();
        }
        Self(dir)
    }
}

impl AgentCatalog for Skilled {
    fn list(&self) -> Vec<AgentEntry> {
        Agents.list()
    }
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        Agents.resolve(name)
    }
    fn prompts(&self) -> Vec<PromptTemplate> {
        Agents.prompts()
    }
    fn skills(&self) -> Vec<SkillInfo> {
        ["deploy", "review"]
            .into_iter()
            .map(|name| SkillInfo {
                name: name.into(),
                description: format!("{name} skill"),
                argument_hint: String::new(),
                model_invocable: true,
                path: self.0.path().join(name).join("SKILL.md"),
            })
            .collect()
    }
}

fn skilled_panel(session: Option<Session>) -> AgentPanel {
    AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Skilled::new()),
        session,
        ..setup(vec![reply("ok"), reply("ok")])
    })
}

fn send(panel: &mut AgentPanel, text: &str) -> String {
    type_text(panel, text);
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(panel);
    panel
        .transcript()
        .items()
        .iter()
        .rev()
        .find_map(|item| match item {
            Item::User { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Definitions with a typo'd key and a tool text named after no tool.
struct Misdefined;

impl AgentCatalog for Misdefined {
    fn list(&self) -> Vec<AgentEntry> {
        Agents.list()
    }
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        Agents.resolve(name)
    }
    fn definition_problems(&self) -> Vec<termide_agent_core::DefinitionProblem> {
        vec![
            termide_agent_core::DefinitionProblem::UnknownKey {
                file: "/ai/agents/review/AGENT.md".into(),
                key: "descripton".into(),
            },
            termide_agent_core::DefinitionProblem::UnknownTool {
                file: "/ai/tools/web-search.md".into(),
            },
        ]
    }
}

/// A tool with nothing behind it, standing in for one an MCP server sent.
struct Late(&'static str);

impl termide_agent_core::Tool for Late {
    fn name(&self) -> &str {
        self.0
    }
    fn description(&self) -> &str {
        "late"
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    fn execute(
        &self,
        call: &termide_agent_core::ToolCall,
        _ctx: &termide_agent_core::ToolContext,
        _on_update: &mut dyn FnMut(ToolUpdate),
        _cancel: &CancelToken,
    ) -> ToolResultMessage {
        ToolResultMessage::text(call, "late")
    }
}

fn notices(panel: &AgentPanel) -> Vec<String> {
    panel
        .transcript()
        .items()
        .iter()
        .filter_map(|item| match item {
            Item::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A catalog with MCP servers that records what `/mcp` asked of it.
struct Servers(Arc<Mutex<Vec<String>>>);

impl AgentCatalog for Servers {
    fn list(&self) -> Vec<AgentEntry> {
        Agents.list()
    }
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        Agents.resolve(name)
    }
    fn mcp_status(&self) -> Vec<termide_agent_core::McpServerState> {
        use termide_agent_core::{McpServerState, McpSignIn, McpStatus};
        vec![
            McpServerState {
                name: "db".into(),
                status: McpStatus::Ready { tools: 4 },
                sign_in: McpSignIn::None,
                launch: "db-server".into(),
                fingerprint: String::new(),
            },
            McpServerState {
                name: "plane".into(),
                status: McpStatus::NeedsLogin,
                sign_in: McpSignIn::SignedOut,
                launch: "https://plane.example/mcp".into(),
                fingerprint: String::new(),
            },
        ]
    }
    fn mcp_reconnect(&self, server: &str) -> Result<termide_agent_core::McpReload, String> {
        self.0.lock().unwrap().push(format!("reconnect {server}"));
        Ok(termide_agent_core::McpReload {
            started: vec![server.to_string()],
            ..Default::default()
        })
    }
    fn mcp_reload(&self) -> Option<termide_agent_core::McpReload> {
        self.0.lock().unwrap().push("reload".into());
        Some(termide_agent_core::McpReload {
            started: vec!["new".into()],
            removed: Vec::new(),
            kept: vec!["db".into()],
        })
    }
    fn mcp_login(&self, server: &str) -> Result<(), String> {
        self.0.lock().unwrap().push(format!("login {server}"));
        Ok(())
    }
    fn mcp_logout(&self, server: &str) -> Result<bool, String> {
        self.0.lock().unwrap().push(format!("logout {server}"));
        Ok(true)
    }
}

/// A backend with nothing behind it, to swap out of a panel in a test.
struct Idle;

impl Backend for Idle {
    fn prompt(&self, _message: UserMessage) -> Result<(), PromptError> {
        Err(PromptError::Stopped)
    }
    fn steer(&self, _message: UserMessage) {}
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn drain(&self) -> Vec<AgentEvent> {
        Vec::new()
    }
    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

/// A backend whose goal check is refused with the error it holds.
struct RefusesJudge(PromptError);

impl Backend for RefusesJudge {
    fn prompt(&self, _message: UserMessage) -> Result<(), PromptError> {
        Err(PromptError::Stopped)
    }
    fn steer(&self, _message: UserMessage) {}
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn drain(&self) -> Vec<AgentEvent> {
        Vec::new()
    }
    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn judge(&self, _goal: String) -> Result<(), PromptError> {
        Err(self.0)
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

/// A panel with a goal set and due to be checked, on a backend refusing the
/// check with `error`.
fn goal_check_refused(error: PromptError) -> AgentPanel {
    let mut panel = panel(vec![]);
    panel.busy = true;
    type_text(&mut panel, "/goal get the build green");
    panel.submit();
    panel.busy = false;
    panel.goal_task.as_mut().unwrap().first_turn = None;
    panel.runtime = Box::new(RefusesJudge(error));
    panel.run_goal_judge();
    panel
}

thread_local! {
    /// What the last [`External`] started on this thread was handed: the
    /// session to resume and the history, as `role:text`.
    static EXTERNAL_START: std::cell::RefCell<Option<(Option<String>, Vec<String>)>> =
        const { std::cell::RefCell::new(None) };
    /// The system prompt and `plan_seen` it was handed.
    static EXTERNAL_PROMPT: std::cell::RefCell<Option<(String, bool)>> =
        const { std::cell::RefCell::new(None) };
}

/// An "external agent" for tests: answers every prompt with one text
/// message, through the same events as the real ACP backend, and like it
/// reports the session it opened when it was given none to resume.
struct External {
    events: Mutex<Vec<AgentEvent>>,
    /// Whether the panel's modes reach it, as with Claude Code, Codex and
    /// Gemini CLI.
    follows_mode: bool,
    /// The names of termide's tools it was last served, when its model
    /// calls them, as Claude Code's does.
    served: Option<Arc<Mutex<Vec<String>>>>,
    /// Whether, on its own tools, it takes termide's MCP servers' beside
    /// them, as Codex and Gemini CLI do; they go to `served`.
    takes_mcp: bool,
    /// What it was served when told the MCP servers had all answered.
    settled: Arc<Mutex<Option<Vec<String>>>>,
}

impl External {
    fn new(setup: BackendSetup) -> Self {
        EXTERNAL_PROMPT.with(|prompt| {
            *prompt.borrow_mut() = Some((setup.system_prompt.clone(), setup.plan_seen));
        });
        let resume = setup
            .resume
            .map(|r| format!("{}:{}", r.agent, r.session_id));
        let history = setup
            .history
            .iter()
            .map(|message| match message {
                Message::User(user) => format!("user:{}", user.plain_text()),
                Message::Assistant(assistant) => format!("assistant:{}", assistant.plain_text()),
                Message::ToolResult(result) => format!("result:{}", result.tool_name),
            })
            .collect();
        let mut events = Vec::new();
        if resume.is_none() {
            events.push(AgentEvent::ExternalSession {
                agent: "outside".into(),
                session_id: "ext-1".into(),
            });
        }
        EXTERNAL_START.with(|start| *start.borrow_mut() = Some((resume, history)));
        Self {
            events: Mutex::new(events),
            follows_mode: false,
            served: None,
            takes_mcp: false,
            settled: Arc::default(),
        }
    }

    fn taking_mcp_tools(setup: BackendSetup, served: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            follows_mode: true,
            served: Some(served),
            takes_mcp: true,
            ..Self::new(setup)
        }
    }

    fn on_termides_tools(setup: BackendSetup, served: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            follows_mode: true,
            served: Some(served),
            ..Self::new(setup)
        }
    }

    fn following_modes(setup: BackendSetup) -> Self {
        Self {
            follows_mode: true,
            ..Self::new(setup)
        }
    }
}

impl Backend for External {
    fn prompt(&self, message: UserMessage) -> Result<(), PromptError> {
        let mut events = self.events.lock().unwrap();
        events.push(AgentEvent::AgentStart);
        events.push(AgentEvent::MessageEnd(Message::User(message)));
        // Like an ACP adapter: the message starts with its first text.
        events.push(AgentEvent::MessageStart {
            prompt_tokens: None,
        });
        events.push(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
            "from outside".into(),
        )));
        events.push(AgentEvent::MessageEnd(Message::Assistant(
            AssistantMessage {
                content: vec![AssistantContent::Text {
                    text: "from outside".into(),
                }],
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                provider: "acp".into(),
                model: "outside".into(),
                error_message: None,
                failure: None,
                timestamp: 0,
            },
        )));
        events.push(AgentEvent::AgentEnd);
        Ok(())
    }
    /// Like an ACP adapter's retry: the failed turn goes again and logs no
    /// new message.
    fn retry(&self) -> Result<(), PromptError> {
        let mut events = self.events.lock().unwrap();
        events.push(AgentEvent::AgentStart);
        events.push(AgentEvent::TurnStart);
        events.push(AgentEvent::MessageStart {
            prompt_tokens: None,
        });
        events.push(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
            "from outside".into(),
        )));
        events.push(AgentEvent::MessageEnd(Message::Assistant(
            AssistantMessage {
                content: vec![AssistantContent::Text {
                    text: "from outside".into(),
                }],
                stop_reason: StopReason::Stop,
                usage: Usage::default(),
                provider: "acp".into(),
                model: "outside".into(),
                error_message: None,
                failure: None,
                timestamp: 0,
            },
        )));
        events.push(AgentEvent::AgentEnd);
        Ok(())
    }
    fn steer(&self, _message: UserMessage) {}
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn drain(&self) -> Vec<AgentEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
    fn follows_mode(&self) -> bool {
        self.follows_mode
    }
    fn runs_host_tools(&self) -> bool {
        self.served.is_some() && !self.takes_mcp
    }
    fn takes_mcp_tools(&self) -> bool {
        self.takes_mcp
    }
    fn host_tools_settled(&self) {
        let served = self.served.as_ref().map(|s| s.lock().unwrap().clone());
        *self.settled.lock().unwrap() = Some(served.unwrap_or_default());
    }
    fn update_host_tools(&self, tools: ToolRegistry) -> Result<(), PromptError> {
        let served = self.served.as_ref().ok_or(PromptError::Unsupported)?;
        *served.lock().unwrap() = tools.names().iter().map(|n| n.to_string()).collect();
        Ok(())
    }
}

/// A catalog whose MCP servers stand as a test sets them.
struct Statuses(Arc<Mutex<Vec<termide_agent_core::McpServerState>>>);

impl AgentCatalog for Statuses {
    fn list(&self) -> Vec<AgentEntry> {
        Agents.list()
    }
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        Agents.resolve(name)
    }
    fn mcp_status(&self) -> Vec<termide_agent_core::McpServerState> {
        self.0.lock().unwrap().clone()
    }
}

/// An external agent that advertises two models and records the one picked.
struct ModelBackend {
    picked: Arc<Mutex<Option<String>>>,
    /// Whether the agent's handshake is still running.
    starting: Arc<AtomicBool>,
}

impl Backend for ModelBackend {
    fn prompt(&self, _message: UserMessage) -> Result<(), PromptError> {
        Ok(())
    }
    fn steer(&self, _message: UserMessage) {}
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn drain(&self) -> Vec<AgentEvent> {
        Vec::new()
    }
    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn available_models(&self) -> Vec<BackendModel> {
        vec![
            BackendModel {
                id: "m-fast".into(),
                name: "Fast".into(),
            },
            BackendModel {
                id: "m-slow".into(),
                name: "Slow".into(),
            },
        ]
    }
    fn current_model(&self) -> Option<String> {
        Some(
            self.picked
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "m-fast".into()),
        )
    }
    fn is_starting(&self) -> bool {
        self.starting.load(Ordering::SeqCst)
    }
    fn select_model(&self, model_id: String) -> Result<(), String> {
        *self.picked.lock().unwrap() = Some(model_id);
        Ok(())
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

/// An external agent that follows the panel's mode and records it.
struct ModeBackend {
    modes: Arc<Mutex<Vec<Mode>>>,
    setup_mode: Mode,
    prompt: String,
    tools: Vec<String>,
}

impl Backend for ModeBackend {
    fn prompt(&self, _message: UserMessage) -> Result<(), PromptError> {
        Ok(())
    }
    fn steer(&self, _message: UserMessage) {}
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn drain(&self) -> Vec<AgentEvent> {
        Vec::new()
    }
    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn follows_mode(&self) -> bool {
        true
    }
    fn set_mode(&self, mode: Mode) {
        self.modes.lock().unwrap().push(mode);
    }
    fn context_usage(&self) -> Option<(u64, u64)> {
        Some((975, 1_000_000))
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

/// An external agent with a model, a reasoning effort and a fast mode among
/// its settings, which records every change asked of it.
struct OptionsBackend {
    asked: Arc<Mutex<Vec<(String, String)>>>,
    state: Mutex<(String, String, String)>,
    busy: Arc<AtomicBool>,
}

impl OptionsBackend {
    fn new(asked: &Arc<Mutex<Vec<(String, String)>>>, busy: &Arc<AtomicBool>) -> Self {
        Self {
            asked: Arc::clone(asked),
            state: Mutex::new(("m-fast".into(), "low".into(), "off".into())),
            busy: Arc::clone(busy),
        }
    }
}

fn values(pairs: &[(&str, &str)]) -> Vec<BackendModel> {
    pairs
        .iter()
        .map(|(id, name)| BackendModel {
            id: (*id).into(),
            name: (*name).into(),
        })
        .collect()
}

impl Backend for OptionsBackend {
    fn prompt(&self, message: UserMessage) -> Result<(), PromptError> {
        self.asked
            .lock()
            .unwrap()
            .push(("prompt".into(), message.plain_text()));
        Ok(())
    }
    fn agent_commands(&self) -> Vec<termide_agent_core::AgentCommand> {
        ["compact", "deploy"]
            .into_iter()
            .map(|name| termide_agent_core::AgentCommand {
                name: name.into(),
                description: format!("{name} it"),
                hint: String::new(),
            })
            .collect()
    }
    fn steer(&self, message: UserMessage) {
        self.asked
            .lock()
            .unwrap()
            .push(("prompt".into(), message.plain_text()));
    }
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {}
    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
    fn drain(&self) -> Vec<AgentEvent> {
        Vec::new()
    }
    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn available_models(&self) -> Vec<BackendModel> {
        values(&[("m-fast", "Fast"), ("m-slow", "Slow")])
    }
    fn current_model(&self) -> Option<String> {
        Some(self.state.lock().unwrap().0.clone())
    }
    fn select_model(&self, model_id: String) -> Result<(), String> {
        self.asked
            .lock()
            .unwrap()
            .push((MODEL_OPTION.into(), model_id.clone()));
        self.state.lock().unwrap().0 = model_id;
        Ok(())
    }
    fn config_options(&self) -> Vec<BackendOption> {
        let state = self.state.lock().unwrap();
        vec![
            BackendOption {
                id: "effort".into(),
                name: "Effort".into(),
                category: Some("thought_level".into()),
                current: state.1.clone(),
                values: values(&[("low", "Low"), ("high", "High")]),
            },
            BackendOption {
                id: "fast".into(),
                name: "Fast mode".into(),
                category: Some("model_config".into()),
                current: state.2.clone(),
                values: values(&[("off", "Off"), ("on", "On")]),
            },
        ]
    }
    fn set_option(&self, id: String, value: String) -> Result<(), String> {
        self.asked.lock().unwrap().push((id.clone(), value.clone()));
        let mut state = self.state.lock().unwrap();
        match id.as_str() {
            "effort" => state.1 = value,
            "fast" => state.2 = value,
            _ => return Err(format!("no {id}")),
        }
        Ok(())
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

/// A runner that records what reached the shell and answers from a table, so
/// nothing runs for real and a test can see the exact command.
fn recording_runner() -> (ShellRunner, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let recorder = Arc::clone(&seen);
    let runner = ShellRunner::new(move |command, cancel| {
        recorder.lock().unwrap().push(command.to_string());
        if cancel.is_cancelled() {
            return Err("stopped".to_string());
        }
        Ok(ShellOutput {
            text: format!("out of {command}"),
            failed: false,
        })
    });
    (runner, seen)
}

/// A runner that records what reached the shell and then behaves like a long
/// command: it only returns once stopped, so a test sees it still running.
fn blocking_runner() -> (ShellRunner, Arc<Mutex<Vec<String>>>) {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let recorder = Arc::clone(&seen);
    let runner = ShellRunner::new(move |command, cancel| {
        recorder.lock().unwrap().push(command.to_string());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cancel.is_cancelled() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if cancel.is_cancelled() {
            return Err("stopped".to_string());
        }
        Ok(ShellOutput {
            text: "done".to_string(),
            failed: false,
        })
    });
    (runner, seen)
}

/// A panel with a runner installed, and the scripted provider behind it so a
/// test can see whether a model was asked anything.
fn panel_with_runner(
    replies: Vec<AssistantMessage>,
) -> (AgentPanel, Arc<Mutex<Vec<String>>>, Arc<Scripted>) {
    let (runner, seen) = recording_runner();
    let provider = Arc::new(Scripted::new(replies));
    let panel = AgentPanel::new(AgentPanelSetup {
        shell_run: Some(runner),
        ..setup_with(provider.clone())
    });
    (panel, seen, provider)
}

/// Tick until the hand-run job is done and its output is in the transcript.
fn wait_for_bang_idle(panel: &mut AgentPanel) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.shell_job.is_some() {
        panel.tick();
        assert!(Instant::now() < deadline, "the command never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Tick until `seen` holds `want` commands.
fn wait_for_commands(panel: &mut AgentPanel, seen: &Mutex<Vec<String>>, want: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().len() < want {
        panel.tick();
        assert!(Instant::now() < deadline, "the command never ran");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A hand-run command's block: the command, its output, whether it failed and
/// how long it took; `None` for any other item.
fn user_command(item: &Item) -> Option<(String, String, bool, Option<u32>)> {
    let Item::Tool {
        call,
        result: Some(result),
        duration_ms,
        ..
    } = item
    else {
        return None;
    };
    (call.id == crate::transcript::USER_COMMAND_ID && call.name == "shell").then(|| {
        (
            call.arguments["command"].as_str().unwrap_or("").to_string(),
            result.plain_text(),
            result.is_error,
            *duration_ms,
        )
    })
}

/// Tick until the transcript holds a hand-run block for `command` whose text
/// contains `wanted`.
fn wait_for_bang_output(panel: &mut AgentPanel, command: &str, wanted: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.tick();
        let command = command.strip_prefix('$').unwrap_or(command);
        let headed = panel
            .transcript()
            .items()
            .iter()
            .filter_map(user_command)
            .any(|(c, output, ..)| c == command && output.contains(wanted));
        if headed {
            return;
        }
        assert!(Instant::now() < deadline, "the output never landed");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Put a suggestion on the panel's channel and wait for its card.
fn suggest_in_worker(
    panel: &mut AgentPanel,
    command: &str,
    why: &str,
) -> std::thread::JoinHandle<SuggestionReply> {
    offer_in_worker(panel, SuggestionKind::Command, command, why)
}

/// Put an offer of `kind` on the panel's channel and wait for its card.
fn offer_in_worker(
    panel: &mut AgentPanel,
    kind: SuggestionKind,
    command: &str,
    why: &str,
) -> std::thread::JoinHandle<SuggestionReply> {
    let (suggester, rx) = suggestion_channel(CancelToken::new());
    panel.suggestion_rx = rx;
    let command = command.to_string();
    let why = why.to_string();
    let worker = std::thread::spawn(move || suggester.suggest(Suggestion { kind, command, why }));
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline, "the card never appeared");
        std::thread::sleep(Duration::from_millis(5));
    }
    worker
}

/// A reply that failed on a usage limit resetting in an hour.
fn limited() -> AssistantMessage {
    let resets_at = crate::failure::unix_now() + 3_600;
    AssistantMessage::failed(
        "scripted",
        "m",
        StopReason::Error,
        "You've hit your session limit",
    )
    .with_failure(termide_agent_core::Failure {
        kind: termide_agent_core::FailureKind::QuotaExhausted,
        resets_at: Some(resets_at),
    })
}

/// The rows of the failure card that is up.
fn failure_rows(panel: &AgentPanel) -> Vec<String> {
    match &panel.pending {
        Some(Pending::Failure { form, .. }) => form.options().to_vec(),
        _ => panic!("no failure card is up"),
    }
}

/// The rows and the detail of the failure card that is up.
fn failure_card(panel: &AgentPanel) -> (Vec<String>, String) {
    match &panel.pending {
        Some(Pending::Failure { form, .. }) => (
            form.options().to_vec(),
            form.detail().unwrap_or_default().to_string(),
        ),
        _ => panic!("no failure card is up"),
    }
}

/// Move the reset of the failure the card is about to `at`.
fn set_reset(panel: &mut AgentPanel, at: u64) {
    match &mut panel.pending {
        Some(Pending::Failure { failure, .. }) => failure.resets_at = Some(at),
        _ => panic!("no failure card is up"),
    }
}

/// A session log in `dir` holding one request, as the panel would have left
/// it; `finish` writes what followed it.
fn logged_request(dir: &Path, finish: impl FnOnce(&mut Session)) -> PathBuf {
    let mut session = Session::create(dir, Path::new("/tmp")).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("build it")))
        .unwrap();
    finish(&mut session);
    session.path().to_path_buf()
}

fn reopen(path: &Path, dir: &Path, replies: Vec<AssistantMessage>) -> AgentPanel {
    AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.to_path_buf()),
        session: Some(Session::open_exclusive(path).unwrap()),
        ..setup(replies)
    })
}

/// The carry-on card's detail, failing when no such card is up.
fn carry_on_card(panel: &AgentPanel) -> String {
    match &panel.pending {
        Some(Pending::Unfinished { form }) => {
            assert_eq!(form.title(), termide_i18n::t().agent_unfinished_title());
            form.detail().unwrap_or_default().to_string()
        }
        _ => panic!("no carry-on card"),
    }
}

fn reply_calling(call: ToolCall) -> AssistantMessage {
    AssistantMessage {
        content: vec![AssistantContent::ToolCall(call)],
        stop_reason: StopReason::ToolUse,
        ..reply("")
    }
}

/// A session another termide holds, as this process sees it: the lock of its
/// lease file taken on a handle of its own, naming `pid`.
fn hold_elsewhere(log: &Path, pid: u32) -> std::fs::File {
    use std::io::Write;
    let mut lock = log.as_os_str().to_os_string();
    lock.push(".lock");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(PathBuf::from(lock))
        .unwrap();
    file.try_lock().unwrap();
    write!(file, "{pid}").unwrap();
    file
}

/// The run answered, at last: no `done` block means nothing went through.
fn answered_done(panel: &AgentPanel) -> bool {
    panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, error: None, .. } if text == "done"))
}

/// A connection catalog whose every endpoint answers from one scripted
/// provider, so a request retried after a switch meets its answer there.
struct Switchable {
    provider: Arc<dyn Provider>,
}

impl ConnectionCatalog for Switchable {
    fn list(&self) -> Vec<ConnectionEntry> {
        [
            ("local", "openai_compatible", "m"),
            ("cloud", "anthropic_compatible", "claude-x"),
            ("cli", "codex", ""),
        ]
        .into_iter()
        .map(|(name, kind, model)| ConnectionEntry {
            name: name.into(),
            kind: kind.into(),
            model: model.into(),
        })
        .collect()
    }
    fn build(&self, name: &str, _agent: &str) -> Option<ConnectionChoice> {
        let entry = self.list().into_iter().find(|entry| entry.name == name)?;
        let backend: Option<BackendFactory> = (name == "cli").then(|| {
            Arc::new(|setup: BackendSetup| {
                Ok(Box::new(External::following_modes(setup)) as Box<dyn Backend>)
            }) as BackendFactory
        });
        Some(ConnectionChoice {
            name: entry.name,
            kind: entry.kind,
            provider: Arc::clone(&self.provider),
            model: entry.model,
            context_window: 200_000,
            backend,
            side_models: termide_agent_core::SideModels::default(),
            reviewer: ReviewerSetup::default(),
            settings: termide_config::Connection::default(),
        })
    }
}

/// A panel on `dir`'s log, on the `local` endpoint, answering from `replies`.
fn on_endpoint(dir: &Path, replies: Vec<AssistantMessage>) -> AgentPanel {
    let provider = Arc::new(Scripted::new(replies));
    let mut setup = setup_with(Arc::clone(&provider));
    setup.session_dir = Some(dir.to_path_buf());
    setup.connections = Some(Arc::new(Switchable {
        provider: Arc::clone(&provider) as Arc<dyn Provider>,
    }));
    AgentPanel::new(setup)
}

/// When the card that is up tries again by itself, if it does.
fn auto_at(panel: &AgentPanel) -> Option<u64> {
    match &panel.pending {
        Some(Pending::Failure { auto_at, .. }) => *auto_at,
        _ => panic!("no failure card is up"),
    }
}

fn set_auto_at(panel: &mut AgentPanel, at: u64) {
    match &mut panel.pending {
        Some(Pending::Failure { auto_at, .. }) => *auto_at = Some(at),
        _ => panic!("no failure card is up"),
    }
}

/// A backend whose agent moved one command to the background, which
/// termide still runs; it notes the ids of the calls it is asked to stop.
#[derive(Default)]
struct Backgrounded(Arc<Mutex<Vec<String>>>);

impl Backend for Backgrounded {
    fn prompt(&self, _message: UserMessage) -> Result<(), PromptError> {
        Err(PromptError::Stopped)
    }
    fn steer(&self, _message: UserMessage) {}
    fn queue_lens(&self) -> (usize, usize) {
        (0, 0)
    }
    fn abort(&self) {}
    fn is_busy(&self) -> bool {
        false
    }
    fn drain(&self) -> Vec<AgentEvent> {
        Vec::new()
    }
    fn update(&self, _update: Box<dyn FnOnce(&mut Agent) + Send>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn compact(&self, _focus: Option<String>) -> Result<(), PromptError> {
        Err(PromptError::Unsupported)
    }
    fn background_calls(&self) -> Vec<termide_agent_core::BackgroundCall> {
        vec![termide_agent_core::BackgroundCall {
            call: ToolCall {
                id: "t1".into(),
                name: "shell".into(),
                arguments: serde_json::json!({ "command": "cargo test --workspace" }),
                extra_content: None,
            },
            elapsed: std::time::Duration::from_secs(134),
        }]
    }
    fn stop_background_call(&self, id: &str) -> bool {
        self.0.lock().unwrap().push(id.to_string());
        true
    }
    fn into_agent(self: Box<Self>) -> Option<Agent> {
        None
    }
}

fn from_peer(kind: termide_agent_core::OriginKind, hops: u32) -> UserMessage {
    UserMessage::text("the config loader moved").with_origin(termide_agent_core::MessageOrigin {
        session: String::new(),
        agent: "reviewer".into(),
        kind,
        panel: Some(7),
        hops,
    })
}

/// A catalog whose project brings a server and a hook not approved yet; the
/// server's launch is what `ext` holds now, and a test may change it.
struct Untrusted {
    asked: Arc<Mutex<Vec<String>>>,
    ext: Arc<Mutex<&'static str>>,
}

impl Untrusted {
    fn new(asked: &Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            asked: Arc::clone(asked),
            ext: Arc::new(Mutex::new("npx ext-server")),
        }
    }
}

/// The mock's fingerprint of a launch.
fn mock_fingerprint(launch: &str) -> String {
    format!("fp-{}", launch.replace(' ', "-"))
}

impl AgentCatalog for Untrusted {
    fn list(&self) -> Vec<AgentEntry> {
        Agents.list()
    }
    fn resolve(&self, name: &str) -> Option<AgentProfile> {
        Agents.resolve(name)
    }
    fn mcp_status(&self) -> Vec<termide_agent_core::McpServerState> {
        use termide_agent_core::{McpServerState, McpSignIn, McpStatus};
        let launch = *self.ext.lock().unwrap();
        vec![McpServerState {
            name: "ext".into(),
            status: McpStatus::NeedsApproval,
            sign_in: McpSignIn::None,
            launch: launch.into(),
            fingerprint: mock_fingerprint(launch),
        }]
    }
    fn held_hooks(&self) -> Vec<crate::HeldHook> {
        vec![crate::HeldHook {
            name: "check".into(),
            launch: "./check.sh".into(),
            fingerprint: mock_fingerprint("./check.sh"),
        }]
    }
    fn trust(&self, name: &str, shown: &str) -> Result<crate::Trusted, String> {
        self.asked.lock().unwrap().push(format!("trust {name}"));
        let (hook, launch) = match name {
            "ext" => (false, *self.ext.lock().unwrap()),
            "check" => (true, "./check.sh"),
            _ => return Err(format!("Nothing named {name} waits for approval")),
        };
        if shown != mock_fingerprint(launch) {
            return Ok(crate::Trusted::Changed {
                hook,
                launch: launch.into(),
                fingerprint: mock_fingerprint(launch),
            });
        }
        Ok(crate::Trusted::Approved((!hook).then(|| {
            termide_agent_core::McpReload {
                started: vec!["ext".into()],
                ..Default::default()
            }
        })))
    }
}

/// An answer with a two-node flowchart in it.
const DIAGRAM_REPLY: &str =
    "Here:\n\n```mermaid\nflowchart TD\n  A[Start] --> B[Finish]\n```\n\ndone";

/// A reply tall enough to push what came before it out of a short view.
fn tall_reply() -> AssistantMessage {
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    reply(&long)
}

/// The transcript as last laid out, one string per line.
fn transcript_text(panel: &AgentPanel) -> Vec<String> {
    panel
        .transcript()
        .rendered()
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

fn shows_diagram(panel: &AgentPanel) -> bool {
    transcript_text(panel).iter().any(|l| l.contains("Start"))
}

/// An answer with a sequence diagram far wider than a 40-column panel.
const WIDE_DIAGRAM_REPLY: &str = "Here:\n\n```mermaid\nsequenceDiagram\n\
    Alice->>Bob: a rather long message that goes on and on and on\n\
    Bob->>Alice: ok\n```\n\ndone";

/// A 40×40 panel showing [`WIDE_DIAGRAM_REPLY`], its answer selected in
/// the chat, and the wide diagram on screen.
fn wide_diagram_panel() -> (AgentPanel, crate::pan::DiagramSpot) {
    let mut panel = panel(vec![reply(WIDE_DIAGRAM_REPLY)]);
    send(&mut panel, "draw");
    let _ = render_text(&mut panel, 40, 40);
    let spot = match panel.transcript.wide_diagrams() {
        [spot] => *spot,
        spots => panic!("one wide diagram expected: {spots:?}"),
    };
    panel.chat_focus = true;
    panel.selected = spot.item;
    (panel, spot)
}

/// The screen row flattened line `line` is drawn on.
fn screen_row(panel: &AgentPanel, line: usize) -> u16 {
    panel.transcript_area.y + (line - panel.top) as u16
}

fn wheel(kind: MouseEventKind, column: u16, row: u16, modifiers: KeyModifiers) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }
}

fn panel_with_reply(text: &str) -> AgentPanel {
    let mut panel = panel(vec![reply(text)]);
    send(&mut panel, "draw");
    panel
}

/// A `w`×`h` red PNG as base64.
fn png(w: u32, h: u32) -> String {
    use base64::Engine;
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        w,
        h,
        image::Rgb([200, 30, 30]),
    ));
    let mut bytes = Vec::new();
    image
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .unwrap();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// A finished `read` that returned a `w`×`h` picture.
fn picture_read(w: u32, h: u32) -> Item {
    let call = ToolCall {
        id: "r1".into(),
        name: "read".into(),
        arguments: serde_json::json!({ "path": "shot.png" }),
        extra_content: None,
    };
    let result = ToolResultMessage::text(&call, "Image shot.png")
        .with_images(vec![("image/png".into(), png(w, h))]);
    Item::Tool {
        call,
        result: Some(result),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    }
}

/// An answer of 40 short paragraphs, `line 1` to `line 40`.
fn tall_answer() -> Item {
    Item::Assistant {
        text: (1..=40)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
        streaming: false,
        error: None,
        at: String::new(),
        cost: None,
        run_ms: None,
    }
}

/// A panel drawing pictures in half blocks, without asking the terminal.
fn picture_panel() -> AgentPanel {
    let mut panel = panel(vec![]);
    panel.graphics = Some(Graphics::halfblocks());
    panel.graphics_asked = true;
    panel
}

/// Whether any cell in rows `rows` of `buf` is painted with a colour.
fn painted(buf: &Buffer, rows: std::ops::Range<u16>) -> bool {
    rows.flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .any(|(x, y)| matches!(buf[(x, y)].bg, ratatui::style::Color::Rgb(..)))
}

/// Whether `buf` holds a terminal graphics sequence.
fn has_graphics(buf: &Buffer) -> bool {
    buf.content
        .iter()
        .any(|cell| cell.symbol().starts_with('\x1b'))
}

/// A finished `write` of `path`, failed or not.
fn wrote(path: &str, ok: bool) -> Item {
    let call = ToolCall {
        id: "w1".into(),
        name: "write".into(),
        arguments: serde_json::json!({ "path": path, "content": "<p>hi</p>" }),
        extra_content: None,
    };
    let mut result = ToolResultMessage::text(&call, "Wrote 9 bytes");
    result.is_error = !ok;
    Item::Tool {
        call,
        result: Some(result),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    }
}

/// Terminals as a test sets them out, recording what the panel asks of them.
#[derive(Default)]
struct Terminals {
    entries: Mutex<Vec<(String, termide_agent_core::TerminalEntry)>>,
    released: Mutex<Vec<String>>,
}

impl termide_agent_core::TerminalSessions for Terminals {
    fn run(
        &self,
        _run: &termide_agent_core::TerminalRun<'_>,
        _on_output: &mut dyn FnMut(&[u8]),
    ) -> Result<termide_agent_core::TerminalRunOutcome, String> {
        Err("not in this test".into())
    }
    fn free_name(&self, _owner: &str, stem: &str) -> String {
        stem.to_string()
    }
    fn list(&self, owner: &str) -> Vec<termide_agent_core::TerminalEntry> {
        let entries = self.entries.lock().unwrap();
        entries
            .iter()
            .filter(|(of, _)| of == owner)
            .map(|(_, entry)| entry.clone())
            .collect()
    }
    fn read(
        &self,
        _owner: &str,
        _name: &str,
    ) -> Result<termide_agent_core::TerminalOutput, String> {
        Err("not in this test".into())
    }
    fn send(
        &self,
        _send: &termide_agent_core::TerminalSend<'_>,
    ) -> Result<termide_agent_core::TerminalSendOutcome, String> {
        Err("not in this test".into())
    }
    fn screen(
        &self,
        _owner: &str,
        _name: &str,
    ) -> Result<termide_agent_core::TerminalScreenText, String> {
        Err("not in this test".into())
    }
    fn interrupt(&self, _owner: &str, _name: &str) -> Result<(), String> {
        Ok(())
    }
    fn close(&self, owner: &str, name: &str) -> Result<(), String> {
        let mut entries = self.entries.lock().unwrap();
        entries.retain(|(of, entry)| !(of == owner && entry.name == name));
        Ok(())
    }
    fn release(&self, owner: &str) {
        self.released.lock().unwrap().push(owner.to_string());
        self.entries.lock().unwrap().retain(|(of, _)| of != owner);
    }
}
