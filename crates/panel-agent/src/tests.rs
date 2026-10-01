use super::*;
use std::path::Path;
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::style::Modifier;
use ratatui::text::Line;
use termide_agent_core::{
    permission_channel, question_channel, Agent, AgentEvent, AssistantContent, AssistantMessage,
    CancelToken, Message, PermissionAnswer, PermissionPrompter, QuestionAnswer, QuestionReply,
    Request, StopReason, StreamEvent, ThinkingLevel, Timing, ToolCall, ToolContext, ToolDecision,
    ToolResultMessage, ToolUpdate, Usage, UserMessage,
};
use termide_core::{ConfirmAction, PanelConfig, SegmentKind};
use termide_ui::{ChoiceAction, ChoiceForm};

use crate::input::file_completions;
use crate::pending::Pending;
use crate::render::context_bar;
use crate::runtime::{push_history, session_model};
use crate::submit::{parse_duration, parse_loop_args, slash_command};
use crate::toolset::{Blocked, ToolsetGuard};

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
                    context_window: Some(64_000),
                },
                ModelInfo {
                    id: "m".into(),
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
        },
        provider: "scripted".into(),
        model: "m".into(),
        error_message: None,
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
            },
            AgentEntry {
                name: "review".into(),
                description: "Reviews diffs".into(),
            },
            AgentEntry {
                name: "outside".into(),
                description: "An external agent".into(),
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
        cwd: PathBuf::from("/tmp"),
        agent: "default".into(),
        catalog: Arc::new(Agents),
        late_tools: None,
        hooks: None,
        backend: None,
        provider_backend: None,
        connections: None,
        connection: "local".into(),
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
        reviewer: ReviewerSetup::default(),
        refusals: Refusals::default(),
        persist_rule: None,
        session_dir: None,
        session: None,
        fold: FoldMode::OnFinish,
    }
}

fn chord(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
    let event = KeyEvent::new(code, modifiers);
    KeyChord {
        raw: event,
        canonical: event,
    }
}

fn strip_text(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect()
}

#[test]
fn the_state_strip_shows_queued_messages_until_the_agent_takes_them() {
    let mut panel = AgentPanel::new(setup(vec![]));
    assert!(panel.state_lines(40).is_empty());
    panel.apply(AgentEvent::AgentStart);
    for text in ["first\nsecond line", "two", "three", "four", "five"] {
        panel.send(text.to_string());
    }
    // Queued while busy: nothing in the transcript, all of it in the strip.
    assert!(panel
        .transcript()
        .items()
        .iter()
        .all(|i| !matches!(i, Item::Notice { .. } | Item::User { .. })));
    let lines = strip_text(&panel.state_lines(40));
    assert!(lines[0].starts_with('╌'));
    assert!(lines[1].starts_with("› first") && lines[1].ends_with("queued"));
    assert!(!lines[1].contains("second line"));
    assert_eq!(lines[2], "› two");
    assert!(lines[4].contains("2 more queued"), "{lines:?}");
    assert!(lines.iter().all(|l| termide_ui::str_display_width(l) <= 40));
    // The agent takes the two oldest; the strip follows its count.
    panel.apply(AgentEvent::QueueUpdate {
        steering: 3,
        follow_up: 0,
    });
    let lines = strip_text(&panel.state_lines(40));
    assert!(lines[1].starts_with("› three"), "{lines:?}");
    assert_eq!(lines.len(), 4);
    panel.apply(AgentEvent::QueueUpdate {
        steering: 0,
        follow_up: 0,
    });
    assert!(panel.state_lines(40).is_empty());
}

#[test]
fn the_state_strip_sits_between_the_transcript_and_the_input() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.send("waiting its turn".to_string());
    let rows = render_text(&mut panel, 40, 12);
    let at = rows
        .iter()
        .position(|r| r.starts_with("› waiting its turn"))
        .expect("queued row");
    assert!(rows[at - 1].starts_with('╌'), "{rows:?}");
    // The input bar's titled border follows the strip directly.
    assert!(
        !rows[at + 1].trim().is_empty() && !rows[at + 1].starts_with('›'),
        "{rows:?}"
    );
    assert_eq!(at, rows.len() - 1 - panel.input_area.height as usize);
}

#[test]
fn a_pending_pause_lives_in_the_state_strip_and_a_pause_in_the_transcript() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    type_text(&mut panel, "/pause");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let t = termide_i18n::t();
    let lines = strip_text(&panel.state_lines(60));
    assert!(lines[1].contains(t.agent_notice_will_pause()), "{lines:?}");
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    // Once the pause takes effect the strip clears: the transcript's `‖`
    // line stands for it.
    assert!(panel.state_lines(80).is_empty());
    // History keeps only the event: the run's closing line, marked paused.
    let items = panel.transcript().items();
    assert!(items.iter().all(|i| !matches!(i, Item::Notice { .. })));
    assert!(matches!(
        items.last(),
        Some(Item::RunEnd {
            paused: true,
            ok: true,
            ..
        })
    ));
}

#[test]
fn a_pause_ticks_its_own_length_and_the_run_clock_resumes_from_the_request() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    let start = panel.run_start.expect("the run started");
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    // Paused: the run keeps its start, and the pause's line counts the
    // pause itself.
    assert_eq!(panel.run_start, Some(start));
    panel.pause_start = Some(Instant::now() - Duration::from_secs(3));
    panel.tick();
    let lines = strip_text(panel.transcript.lines(40, &panel.colors, false));
    // The duration alone, no time of day.
    assert_eq!(lines.last().map(|l| l.trim()), Some("‖ 3s"));
    // Resumed: the pause's length stays on its line, the run's clock goes
    // on from the request.
    panel.paused = false;
    panel.resuming = true;
    panel.apply(AgentEvent::AgentStart);
    assert_eq!(panel.run_start, Some(start));
    assert!(panel.pause_start.is_none());
    assert!(matches!(
        panel.transcript.items().last(),
        Some(Item::RunEnd { paused: true, elapsed_ms, .. }) if *elapsed_ms >= 3000
    ));
    // A fresh run (not a resume) starts its own clock.
    panel.apply(AgentEvent::AgentEnd);
    panel.apply(AgentEvent::AgentStart);
    assert_ne!(panel.run_start, Some(start));
}

#[test]
fn the_prompt_border_carries_the_run_controls() {
    let mut panel = AgentPanel::new(setup(vec![]));
    let (width, height) = (40, 12);
    // The controls on the border, as text, and a click on one by glyph.
    let border = |panel: &mut AgentPanel| {
        let rows = render_text(panel, width, height);
        rows[panel.input_area.y as usize].clone()
    };
    let click = |panel: &mut AgentPanel, glyph: &str| {
        let row = border(panel);
        let col = row[..row.find(glyph).expect("the control is on the border")]
            .chars()
            .count() as u16;
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: col,
                row: panel.input_area.y,
                modifiers: KeyModifiers::NONE,
            },
            Rect::new(0, 0, width, height),
        );
    };
    // Idle: no controls.
    assert!(!border(&mut panel).contains('['));
    // Working: pause and stop.
    panel.apply(AgentEvent::AgentStart);
    let row = border(&mut panel);
    assert!(row.ends_with("[‖][■]─"), "{row:?}");
    click(&mut panel, "[‖]");
    assert!(panel.pause_requested, "the pause control asks to pause");
    // A pause asked for: continue withdraws it.
    assert!(border(&mut panel).contains("[▶][■]"));
    click(&mut panel, "[▶]");
    assert!(
        !panel.pause_requested,
        "continue withdraws the pending pause"
    );
    // The strip's pending-pause line withdraws it too.
    click(&mut panel, "[‖]");
    let _ = render_text(&mut panel, width, height);
    let row = panel.pause_row.expect("the strip shows the pending pause");
    panel.handle_mouse(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row,
            modifiers: KeyModifiers::NONE,
        },
        Rect::new(0, 0, width, height),
    );
    assert!(!panel.pause_requested);
    // `/continue` withdraws a pending pause as well.
    click(&mut panel, "[‖]");
    type_text(&mut panel, "/continue");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !panel.pause_requested,
        "/continue withdraws the pending pause"
    );
    // Paused: continue alone, and the pause's line resumes too.
    click(&mut panel, "[‖]");
    panel.apply(AgentEvent::Paused);
    panel.apply(AgentEvent::AgentEnd);
    assert!(border(&mut panel).contains("[▶][■]"));
    let line = panel.transcript.line_count() - 1;
    assert!(panel.transcript.is_live_pause_line(line));
    // Stop while paused gives the run up: no controls, the pause rests.
    click(&mut panel, "[■]");
    assert!(!panel.paused);
    assert!(panel.pause_start.is_none());
    assert!(!border(&mut panel).contains('['));
    assert!(!panel.transcript.is_live_pause_line(line));
}

#[test]
fn the_stop_control_is_red_only_while_a_stop_is_under_way() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    let colors = panel.colors;
    assert_eq!(
        panel.run_button_color(RunButton::Pause),
        colors.border_focused
    );
    assert_eq!(
        panel.run_button_color(RunButton::Stop),
        colors.border_focused
    );
    panel.request_pause();
    panel.abort();
    // A stop under way leaves stop alone, red, and a second press adds
    // no second notice.
    assert_eq!(panel.run_buttons(), vec![RunButton::Stop]);
    assert_eq!(panel.run_button_color(RunButton::Stop), colors.error);
    let lines = panel.transcript.line_count();
    panel.abort();
    assert_eq!(panel.transcript.line_count(), lines);
    panel.apply(AgentEvent::AgentEnd);
    panel.apply(AgentEvent::AgentStart);
    assert_eq!(panel.run_buttons(), vec![RunButton::Pause, RunButton::Stop]);
    assert_eq!(
        panel.run_button_color(RunButton::Stop),
        colors.border_focused
    );
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

#[test]
fn shortcuts_work_on_a_cyrillic_layout() {
    // As the dispatcher builds it: the raw Cyrillic key, and its Latin
    // canonical form.
    let key = |c, latin, modifiers| KeyChord {
        raw: KeyEvent::new(KeyCode::Char(c), modifiers),
        canonical: KeyEvent::new(KeyCode::Char(latin), modifiers),
    };
    let mut panel = panel(vec![]);
    let long = (1..=8)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let call = ToolCall {
        id: "t1".into(),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": "ls" }),
        extra_content: None,
    };
    panel.transcript.push(Item::Tool {
        call: call.clone(),
        result: Some(ToolResultMessage::text(&call, long)),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    });
    // `Ctrl+щ` is `Ctrl+O`: unfold everything.
    assert!(!panel.transcript.any_expanded());
    panel.handle_key(key('щ', 'o', KeyModifiers::CONTROL));
    assert!(panel.transcript.any_expanded());
    // Typed into the input, the same letter stays Cyrillic.
    panel.handle_key(key('щ', 'o', KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "щ");
}

#[test]
fn a_drag_selects_transcript_text_for_copy() {
    let mut panel = panel(vec![reply("Hello from the model")]);
    type_text(&mut panel, "hi there");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let rows = render_text(&mut panel, 40, 12);
    let area = panel.transcript_area;
    let y = rows
        .iter()
        .position(|r| r.contains("Hello from"))
        .expect("the answer is on screen") as u16;
    let row = &rows[y as usize];
    let x = row[..row.find("Hello").unwrap()].chars().count() as u16;
    let mouse = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y), area);
    panel.handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), x + 9, y),
        area,
    );
    panel.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x + 9, y), area);
    // A drag is a selection, not a click: the chat keeps its focus state.
    assert!(!panel.chat_focus);
    let width = panel.transcript_area.width.saturating_sub(1) as usize;
    let selection = panel.text_selection.expect("a selection");
    assert_eq!(
        selection.text(panel.transcript.rendered(), width),
        "Hello from"
    );
    // It shows in the selection colours.
    let buf = render_buf(&mut panel, 40, 12);
    assert_eq!(buf[(x, y)].bg, ThemeColors::default().selection_bg);
    // A plain click clears it.
    panel.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y), area);
    panel.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y), area);
    assert!(panel.text_selection.is_none());
}

#[test]
fn the_input_grows_to_half_the_panel() {
    let mut panel = panel(vec![]);
    let text = (1..=12)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    panel.input_area_mut().insert_str(&text);
    // A 40-row panel lets the prompt take 20 rows; 12 lines fit in full.
    assert_eq!(panel.input_rows(40, 60), 12);
    // A 16-row one stops it at 8, and the prompt scrolls inside.
    assert_eq!(panel.input_rows(16, 60), 8);
}

#[test]
fn pasting_the_same_block_twice_unmasks_it() {
    let mut panel = panel(vec![]);
    let block = "1\n2\n3\n4\n5\n6";
    panel.paste(block);
    assert_eq!(panel.input_text(), "[#1 pasted 6 lines]");
    // The same text again: the placeholder turns into the text itself.
    panel.paste(block);
    assert_eq!(panel.input_text(), block);
    assert!(panel.pastes.is_empty());
    // Another block is masked as before.
    panel.paste("a\nb\nc\nd\ne\nf");
    assert!(panel.input_text().ends_with("[#2 pasted 6 lines]"));
}

#[test]
fn a_paste_of_more_than_five_lines_is_masked() {
    let mut panel = panel(vec![]);
    panel.paste("a\nb\nc\nd\ne");
    assert_eq!(
        panel.input_text(),
        "a\nb\nc\nd\ne",
        "five lines stay inline"
    );
    let mut panel = self::panel(vec![]);
    panel.paste("1\n2\n3\n4\n5\n6");
    assert_eq!(panel.input_text(), "[#1 pasted 6 lines]");
}

#[test]
fn a_selected_diff_keeps_its_colors() {
    let mut panel = panel(vec![]);
    let edit = |args| ToolCall {
        id: "e1".into(),
        name: "edit".into(),
        arguments: args,
        extra_content: None,
    };
    let call = edit(serde_json::json!({ "path": "a.rs" }));
    panel.transcript.push(Item::Tool {
        call: call.clone(),
        result: Some(ToolResultMessage::text(
            &call,
            "Edited a.rs.\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+new",
        )),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(100),
        waited_ms: None,
        waiting: false,
    });
    assert!(panel.transcript.toggle_expanded(0));
    panel.chat_focus = true;
    panel.selected = 0;
    let (width, height) = (40, 16);
    let buf = render_buf(&mut panel, width, height);
    let colors = ThemeColors::default();
    let row_of = |needle: &str| {
        (0..height)
            .find(|&y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap_or_else(|| panic!("no row {needle:?}"))
    };
    // Selected, the diff rows invert onto their own hue, the rest onto
    // the plain selection.
    assert_eq!(buf[(4, row_of("+new"))].bg, colors.success);
    assert_eq!(buf[(4, row_of("-old"))].bg, colors.error);
    assert_eq!(buf[(4, row_of("a.rs"))].bg, colors.fg);
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
        border_bottom_y: None,
    };
    panel.render(area, &mut buf, &ctx);
    buf
}

#[test]
fn a_finished_run_asks_for_attention_until_the_panel_is_shown_focused() {
    let mut panel = AgentPanel::new(setup(vec![]));
    assert!(!panel.needs_attention());
    panel.apply(AgentEvent::AgentStart);
    assert!(!panel.needs_attention());
    panel.apply(AgentEvent::AgentEnd);
    assert!(panel.needs_attention());
    // Rendered in an inactive group it keeps waiting.
    render_buf_focused(&mut panel, 40, 12, false);
    assert!(panel.needs_attention());
    render_buf(&mut panel, 40, 12);
    assert!(!panel.needs_attention());
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

#[test]
fn only_a_long_run_rings_when_it_ends() {
    let mut panel = AgentPanel::new(setup(vec![]));
    end_run_of(&mut panel, 1);
    assert!(panel.needs_attention());
    assert!(!rings(&panel.tick()));
    end_run_of(&mut panel, 60);
    assert!(rings(&panel.tick()));
}

#[test]
fn a_run_the_user_stopped_rings_no_bell() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.run_start = Some(Instant::now() - Duration::from_secs(60));
    panel.stop_requested = true;
    panel.apply(AgentEvent::AgentEnd);
    assert!(!rings(&panel.tick()));
}

#[test]
fn the_bell_rings_once_until_the_user_sees_the_panel() {
    let mut panel = AgentPanel::new(setup(vec![]));
    end_run_of(&mut panel, 60);
    assert!(rings(&panel.tick()));
    // Unseen, the next wait does not ring again.
    end_run_of(&mut panel, 60);
    assert!(!rings(&panel.tick()));
    // Shown focused in an unfocused window still counts as unseen.
    panel.handle_command(PanelCommand::SetHostFocus { focused: false });
    render_buf(&mut panel, 40, 12);
    end_run_of(&mut panel, 60);
    assert!(!rings(&panel.tick()));
    // Seen: the next wait rings.
    panel.handle_command(PanelCommand::SetHostFocus { focused: true });
    render_buf(&mut panel, 40, 12);
    end_run_of(&mut panel, 60);
    assert!(rings(&panel.tick()));
}

#[test]
fn typing_enter_runs_a_turn_and_renders_it() {
    let mut panel = panel(vec![reply("Hello from the model")]);
    type_text(&mut panel, "hi there");
    assert_eq!(panel.input_text(), "hi there");
    assert!(panel.captures_escape(), "non-empty input keeps Esc");

    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
    assert!(panel.input_text().is_empty());
    settle(&mut panel);

    let items = panel.transcript().items();
    assert!(matches!(&items[0], Item::User { text, .. } if text == "hi there"));
    assert!(items.iter().any(|item| matches!(
        item,
        Item::Assistant { text, streaming: false, .. } if text == "Hello from the model"
    )));
    let rows = render_text(&mut panel, 40, 16);
    assert!(rows.iter().any(|r| r.contains("› hi there")));
    assert!(rows.iter().any(|r| r.contains("Hello from the model")));
    assert!(
        rows.last().unwrap().starts_with("›"),
        "input box at the bottom"
    );
    assert!(
        rows[rows.len() - 2].starts_with("─"),
        "separator above the input"
    );

    // The knobs on the left, the figures flush right after the spacer.
    let segments = panel.status_segments();
    let split = segments
        .iter()
        .position(|s| s.kind == SegmentKind::Spacer)
        .expect("a spacer");
    let text = |segs: &[StatusSegment]| segs.iter().map(|s| s.text.as_str()).collect::<String>();
    assert_eq!(
            text(&segments[..split]),
            " Agent: default │ Permissions: configured │ Reasoning: off │ Tools: 0/0 │ Connection: local · OpenAI Compatible │ Model: m"
        );
    assert_eq!(text(&segments[split + 1..]), "↑100 ↓20 120/1k ▰▱▱▱▱▱▱▱ ");
}

#[test]
fn title_follows_the_first_prompt() {
    let mut fresh = panel(vec![reply("ok")]);
    // Empty conversation: the working directory.
    assert_eq!(fresh.title(), "Agent: /tmp");

    type_text(&mut fresh, "  make the   timeout configurable  ");
    fresh.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut fresh);
    assert_eq!(fresh.title(), "Agent: make the timeout configurable");

    // A long prompt is kept whole: the header cuts its end to the panel's
    // width, so a wide panel shows as much of it as fits.
    let mut wordy = panel(vec![reply("ok")]);
    type_text(&mut wordy, &"word ".repeat(30));
    wordy.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut wordy);
    assert_eq!(
        wordy.title(),
        format!("Agent: {}", "word ".repeat(30).trim_end())
    );
    assert_eq!(wordy.title_cut(), TitleCut::End);
}

#[test]
fn the_context_window_is_learned_from_the_provider() {
    let mut panel = panel(vec![reply("ok")]);
    // The configured window is only a fallback until the provider is known.
    assert_eq!(panel.model.context_window, 1000);
    // The provider reports the active model's real window; the panel always
    // adopts it, overriding the fallback.
    let models = vec![ModelInfo {
        id: panel.model.id.clone(),
        context_window: Some(48_000),
    }];
    assert!(panel.adopt_listed_models(&models));
    assert_eq!(panel.model.context_window, 48_000);
    // A second identical report is a no-op.
    assert!(!panel.adopt_listed_models(&models));
}

#[test]
fn a_model_left_to_the_provider_is_its_first_listed() {
    let mut base = setup(vec![reply("ok")]);
    base.model.id = String::new();
    let mut panel = AgentPanel::new(base);
    assert_eq!(panel.model_display(), "auto");
    // Before the list arrives there is nothing to send to: the text stays.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "hello");
    assert!(panel.transcript.items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text == termide_i18n::t().agent_notice_model_pending()
    )));
    let models = vec![
        ModelInfo {
            id: "first".into(),
            context_window: Some(64_000),
        },
        ModelInfo {
            id: "second".into(),
            context_window: None,
        },
    ];
    assert!(panel.adopt_listed_models(&models));
    assert_eq!(panel.model.id, "first");
    assert_eq!(panel.model.context_window, 64_000);
    // A new session in the panel starts on it too.
    assert_eq!(panel.configured_model.id, "first");
}

#[test]
fn a_model_without_a_reported_window_keeps_the_fallback() {
    // When the provider reports no window for the active model, the
    // configured fallback stays.
    let mut panel = panel(vec![reply("ok")]);
    let models = vec![ModelInfo {
        id: panel.model.id.clone(),
        context_window: None,
    }];
    assert!(!panel.adopt_listed_models(&models));
    assert_eq!(panel.model.context_window, 1000);
}

#[test]
fn an_empty_session_is_discarded_on_close() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        let path = panel.session.as_ref().unwrap().path().to_path_buf();
        assert!(path.exists());
        path
    };
    assert!(!path.exists(), "an unused session is removed on close");
}

#[test]
fn a_session_with_messages_survives_close() {
    let dir = tempfile::tempdir().unwrap();
    let path = {
        let mut panel = AgentPanel::new(AgentPanelSetup {
            session_dir: Some(dir.path().to_path_buf()),
            ..setup(vec![reply("ok")])
        });
        let path = panel.session.as_ref().unwrap().path().to_path_buf();
        type_text(&mut panel, "do something");
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        assert!(path.exists());
        path
    };
    assert!(path.exists(), "a session with a conversation is kept");
}

#[test]
fn switching_away_from_an_empty_session_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    let empty = panel.session.as_ref().unwrap().path().to_path_buf();
    assert!(empty.exists());
    // A new session: the empty one we leave is discarded, not listed.
    assert!(panel.switch_session(None));
    assert!(!empty.exists(), "the empty session is removed on switch");
    let fresh = panel.session.as_ref().unwrap().path().to_path_buf();
    assert!(fresh.exists());
    assert_ne!(empty, fresh);
}

#[test]
fn the_context_bar_fills_with_the_percentage() {
    assert_eq!(context_bar(0), "▱▱▱▱▱▱▱▱");
    assert_eq!(context_bar(12), "▰▱▱▱▱▱▱▱");
    assert_eq!(context_bar(50), "▰▰▰▰▱▱▱▱");
    assert_eq!(context_bar(100), "▰▰▰▰▰▰▰▰");
    assert_eq!(context_bar(200), "▰▰▰▰▰▰▰▰");
}

#[test]
fn activity_follows_the_events_and_totals_accumulate() {
    let mut panel = panel(vec![reply("hi")]);
    assert!(panel.activity.is_none());
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Prefill));
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
        "hello".into(),
    )));
    assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Generating));
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(reply("hello"))));
    // reply()'s usage is input 100 / output 20.
    assert_eq!((panel.session_input, panel.session_output), (100, 20));
    panel.apply(AgentEvent::ToolExecutionStart {
        call: termide_agent_core::ToolCall {
            id: "1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({}),
            extra_content: None,
        },
    });
    assert_eq!(panel.activity.map(|a| a.phase), Some(Phase::Tool));
    panel.apply(AgentEvent::AgentEnd);
    assert!(panel.activity.is_none());
}

#[test]
fn the_live_footer_shows_generation_meta_and_a_clock() {
    let text_of = |panel: &AgentPanel| -> Vec<String> {
        panel
            .live_footer_lines(40)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    };
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    // Prefill: the run clock alone — an animated glyph and the time since
    // the request, with no dividing rule and no generation line yet.
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        transcript::RUN_FRAMES
            .iter()
            .any(|f| lines[0].trim_start().starts_with(f)),
        "{lines:?}"
    );
    assert!(!lines[0].contains("🕒") && !lines[0].contains('╌'));

    // Once tokens stream, the `✍️` generation line joins the clock.
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
        "hello there".into(),
    )));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains('✍') && lines[0].contains('↓'));
    assert!(lines.iter().all(|l| !l.contains('⏫') && !l.contains('╌')));

    // A tool running after the reply: the clock alone, no generation.
    panel.apply(AgentEvent::ToolExecutionStart {
        call: termide_agent_core::ToolCall {
            id: "t1".into(),
            name: "bash".into(),
            arguments: serde_json::json!({ "command": "sleep 1" }),
            extra_content: None,
        },
    });
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(!lines[0].contains('✍'), "{lines:?}");
}

#[test]
fn the_live_footer_shows_the_prefill_until_the_first_token() {
    let text_of = |panel: &AgentPanel| -> Vec<String> {
        panel
            .live_footer_lines(60)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.to_string()).collect())
            .collect()
    };
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: Some(48_000),
    });
    // The model reads the prompt: a `⏫` line with the estimate, above the
    // run clock.
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("⏫") && lines[0].contains("↑~48k"),
        "{lines:?}"
    );

    // A server reporting its progress: a bar and the tokens read of the
    // total in place of the estimate.
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::PrefillProgress {
        processed: 24_000,
        total: 48_000,
        cached: 12_000,
    }));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains("▰▰▰▰▱▱▱▱") && lines[0].contains("↑24k/48k"),
        "{lines:?}"
    );
    assert!(!lines[0].contains('~'), "{lines:?}");

    // The first token ends the prefill: the `✍️` line takes its place.
    panel.apply(AgentEvent::MessageUpdate(StreamEvent::TextDelta(
        "hello".into(),
    )));
    let lines = text_of(&panel);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].contains('✍'), "{lines:?}");
    assert!(lines.iter().all(|l| !l.contains('⏫')), "{lines:?}");
}

#[test]
fn a_tall_focused_block_scrolls_to_its_end() {
    // A reply taller than the viewport.
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut panel = panel(vec![reply(&long)]);
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // Render short so the answer cannot fit, then focus its block.
    let _ = render_text(&mut panel, 40, 10);
    panel.chat_focus = true;
    panel.selected = panel.transcript().items().len() - 1;

    // Scrolling to the bottom must reach it: the focus keeps the block in
    // view without snapping the viewport back to the block's first line.
    panel.scroll_by(1000);
    let _ = render_text(&mut panel, 40, 10);
    assert!(panel.max_top() > 0, "the block is taller than the viewport");
    assert_eq!(
        panel.top,
        panel.max_top(),
        "a tall focused block still scrolls to its end"
    );
}

#[test]
fn a_selected_block_does_not_trap_scrolling() {
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut panel = panel(vec![reply(&long)]);
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = render_text(&mut panel, 40, 10);
    // Select the first block (the user message at the very top).
    panel.chat_focus = true;
    panel.selected = 0;
    let _ = render_text(&mut panel, 40, 10);

    // The selection near the top does not stop scrolling to the bottom.
    panel.scroll_by(1000);
    let _ = render_text(&mut panel, 40, 10);
    assert!(
        panel.max_top() > 0,
        "the content is taller than the viewport"
    );
    assert_eq!(panel.top, panel.max_top(), "scrolling down stays free");

    // And scrolling back to the top is equally free.
    panel.scroll_by(-1000);
    let _ = render_text(&mut panel, 40, 10);
    assert_eq!(panel.top, 0, "scrolling up stays free");
}

#[test]
fn arrow_navigation_brings_the_selected_block_into_view() {
    let long = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut panel = panel(vec![reply(&long)]);
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = render_text(&mut panel, 40, 10);
    // Scroll to the top and focus the chat on the first block.
    panel.scroll_by(-1000);
    panel.chat_focus = true;
    panel.selected = 0;
    let _ = render_text(&mut panel, 40, 10);
    assert_eq!(panel.top, 0);

    // Arrowing down to the tall answer scrolls it into view.
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    let _ = render_text(&mut panel, 40, 10);
    assert!(panel.top > 0, "the answer below is scrolled into view");
}

#[test]
fn the_session_records_the_provider_kind() {
    let dir = tempfile::tempdir().unwrap();
    let panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        provider_kind: "anthropic_compatible".into(),
        ..setup(vec![reply("ok")])
    });
    let recorded = panel.session.as_ref().unwrap().current_model().unwrap();
    assert_eq!(recorded.provider, "anthropic_compatible");
}

#[test]
fn the_reasoning_level_is_picked_from_the_chip_and_persists_in_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(Scripted::new(vec![reply("ok")]));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup_with(Arc::clone(&provider))
    });
    assert_eq!(chip(&panel, REASONING_ACTION), "off");
    let path = panel.session.as_ref().unwrap().path().to_path_buf();

    let events = panel.handle_status_action(REASONING_ACTION);
    let PanelEvent::ShowSelect { options, .. } = &events[0] else {
        panic!("a level picker: {events:?}");
    };
    assert_eq!(options, &["● off", "  low", "  medium", "  high"]);
    select(&mut panel, &events[0], 3);
    assert_eq!(chip(&panel, REASONING_ACTION), "high");

    // The next request asks for it.
    type_text(&mut panel, "hi");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(
        *provider.seen_thinking.lock().unwrap(),
        [ThinkingLevel::High]
    );

    // The choice is written to the session, so a resume brings it back.
    let reopened = Session::open(&path).unwrap();
    assert_eq!(reopened.current_thinking(), Some(ThinkingLevel::High));
    assert_eq!(
        session_model(&panel.configured_model, Some(&reopened)).thinking,
        ThinkingLevel::High
    );
}

#[test]
fn a_level_the_model_lacks_shows_as_the_nearest_it_has() {
    let provider = Scripted {
        levels: vec![ThinkingLevel::Low, ThinkingLevel::High],
        ..Scripted::new(vec![])
    };
    let mut setup = setup_with(Arc::new(provider));
    setup.model.thinking = ThinkingLevel::Max;
    let panel = AgentPanel::new(setup);
    assert_eq!(chip(&panel, REASONING_ACTION), "high");
}

#[test]
fn an_on_off_model_flips_from_the_chip() {
    let provider = Scripted {
        levels: vec![ThinkingLevel::Off, ThinkingLevel::High],
        ..Scripted::new(vec![])
    };
    let mut panel = AgentPanel::new(setup_with(Arc::new(provider)));
    assert_eq!(chip(&panel, REASONING_ACTION), "off");
    panel.handle_status_action(REASONING_ACTION);
    assert_eq!(chip(&panel, REASONING_ACTION), "on");
    assert_eq!(panel.model.thinking, ThinkingLevel::High);
    panel.handle_status_action(REASONING_ACTION);
    assert_eq!(chip(&panel, REASONING_ACTION), "off");
}

#[test]
fn a_model_that_cannot_be_asked_to_reason_has_no_chip() {
    let provider = Scripted {
        levels: Vec::new(),
        ..Scripted::new(vec![])
    };
    let mut panel = AgentPanel::new(setup_with(Arc::new(provider)));
    assert!(!panel
        .status_segments()
        .iter()
        .any(|s| s.action == Some(REASONING_ACTION)));
    assert!(panel.handle_status_action(REASONING_ACTION).is_empty());
}

#[test]
fn a_click_focuses_the_chat_and_selects_a_block() {
    let mut panel = panel(vec![reply("Hello from the model")]);
    type_text(&mut panel, "hi there");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // Render so the transcript area and its lines exist.
    let _ = render_text(&mut panel, 40, 12);
    assert!(!panel.chat_focus, "starts on the input");
    let area = panel.transcript_area;
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: area.x + 1,
        row: area.y,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(click, area);
    // The click lands on release, once it is clear no drag follows.
    assert!(!panel.chat_focus, "a press alone does not click");
    let release = MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        ..click
    };
    panel.handle_mouse(release, area);
    assert!(panel.chat_focus, "a click focuses the chat");
    assert!(panel
        .selected_block_text()
        .is_some_and(|t| !t.trim().is_empty()));

    // A click below the transcript (on the input) hands focus back.
    let below = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: area.x + 1,
        row: area.y + area.height + 1,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(below, area);
    assert!(
        !panel.chat_focus,
        "a click on the input returns focus to it"
    );
}

/// Select the tail of the prompt with Shift+arrows.
fn select_back(panel: &mut AgentPanel, steps: usize) {
    for _ in 0..steps {
        panel.handle_key(chord(KeyCode::Left, KeyModifiers::SHIFT));
    }
}

#[test]
fn shift_arrows_select_the_prompt_and_ctrl_a_takes_it_all() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "fix the flaky test");
    assert!(!panel.input_area().has_selection());
    select_back(&mut panel, 4);
    assert_eq!(
        panel.input_area().selected_text(),
        Some("test".to_string()),
        "Shift+Left extends the selection back over what was typed"
    );
    // The selection survives a redraw and shows inverted in the prompt.
    let rows = render_text(&mut panel, 30, 10);
    assert!(
        rows.iter().any(|row| row.contains("test")),
        "the prompt still shows its text: {rows:?}"
    );
    // A plain arrow drops the selection instead of extending it.
    panel.handle_key(chord(KeyCode::Left, KeyModifiers::NONE));
    assert!(!panel.input_area().has_selection());
    // Ctrl+A selects the whole prompt.
    panel.handle_key(chord(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(
        panel.input_area().selected_text(),
        Some("fix the flaky test".to_string())
    );
}

/// The one test that reaches the real system clipboard, so it is also the
/// one that pays for opening it (tens of seconds on some hosts). It puts
/// the clipboard back as it found it.
#[test]
fn ctrl_x_cuts_the_prompt_selection_and_ctrl_v_puts_it_back() {
    let before = termide_ui::clipboard::paste();
    let mut panel = panel(vec![]);
    type_text(&mut panel, "fix the flaky test");
    select_back(&mut panel, 5);
    let events = panel.handle_key(chord(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
    assert_eq!(panel.input_text(), "fix the flaky");
    assert!(!panel.input_area().has_selection());

    // With nothing selected, the clipboard keys are the prompt's to ignore.
    let events = panel.handle_key(chord(KeyCode::Char('x'), KeyModifiers::CONTROL));
    assert!(events.is_empty(), "nothing selected, nothing cut");

    // A machine without a display (CI) copies over OSC 52, which cannot be
    // read back, so the paste half runs only where the clipboard reads.
    if termide_ui::clipboard::paste().is_some() {
        let events = panel.handle_key(chord(KeyCode::Char('v'), KeyModifiers::CONTROL));
        assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
        assert_eq!(panel.input_text(), "fix the flaky test");
    }
    if let Some(text) = before {
        let _ = termide_ui::clipboard::copy(&text);
    }
}

/// The routing of the clipboard commands, kept off the real clipboard: what
/// is answered here is which side takes the key, not what it writes.
#[test]
fn the_clipboard_commands_answer_to_the_input_not_the_chat() {
    let mut panel = panel(vec![reply("an answer")]);
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = render_text(&mut panel, 40, 12);

    // Nothing selected in the prompt: the panel declines, so the key falls
    // through to whatever the app has for it.
    assert!(matches!(
        panel.handle_command(PanelCommand::Copy),
        CommandResult::Handled(false)
    ));
    assert!(matches!(
        panel.handle_command(PanelCommand::Cut),
        CommandResult::Handled(false)
    ));

    // A bracketed paste carries its own text, so no clipboard is read; it
    // belongs to the prompt.
    assert!(matches!(
        panel.handle_command(PanelCommand::PasteText {
            text: "pasted".into()
        }),
        CommandResult::NeedsRedraw(true)
    ));
    assert_eq!(panel.input_text(), "pasted");

    // While the chat holds focus its block keeps the clipboard: `Copy`
    // there is the block's, and the prompt is not asked.
    panel.chat_focus = true;
    assert!(matches!(
        panel.handle_command(PanelCommand::Cut),
        CommandResult::None
    ));
    assert!(matches!(
        panel.handle_command(PanelCommand::Paste),
        CommandResult::None
    ));
}

#[test]
fn dragging_in_the_prompt_selects_and_hands_focus_back_from_the_chat() {
    let mut panel = panel(vec![reply("Hello from the model")]);
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = render_text(&mut panel, 40, 12);
    // A draft in the prompt to drag a selection through, typed before the
    // chat takes focus (which swallows plain characters).
    type_text(&mut panel, "one two three");
    let _ = render_text(&mut panel, 40, 12);
    panel.chat_focus = true;
    let bar = panel.input_area;
    let at = |kind, column, row| MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    };
    let press = at(
        MouseEventKind::Down(MouseButton::Left),
        bar.x + 4,
        bar.y + bar.height - 1,
    );
    panel.handle_mouse(press, bar);
    assert!(!panel.chat_focus, "a press on the prompt focuses the input");
    // The prompt box is the bar's last row, so the drag stays on it.
    let drag = at(
        MouseEventKind::Drag(MouseButton::Left),
        bar.x + 8,
        bar.y + bar.height - 1,
    );
    let events = panel.handle_mouse(drag, bar);
    assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
    assert!(
        panel.input_area().has_selection(),
        "the drag selected prompt text"
    );
}

#[test]
fn a_resumed_block_shows_its_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session
        .append_message(&Message::User(UserMessage::text("earlier question")))
        .unwrap();
    session
        .append_message(&Message::Assistant(reply("earlier answer")))
        .unwrap();
    let path = session.path().to_path_buf();
    // Reopen: the restored blocks carry the wall-clock time from the log.
    let session = Session::open(&path).unwrap();
    let mut transcript = Transcript::default();
    for logged in &session.context_messages_with_times(&CompactionPrompts::default()) {
        push_history(&mut transcript, logged);
    }
    let has_time = transcript
        .items()
        .iter()
        .any(|item| matches!(item, Item::Assistant { at, .. } if !at.is_empty()));
    assert!(has_time, "a resumed answer keeps its time");
}

#[test]
fn a_resumed_session_keeps_its_timing() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    let tool_call = ToolCall {
        id: "t1".into(),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": "ls" }),
        extra_content: None,
    };
    let mut turn = reply("");
    turn.content = vec![
        AssistantContent::thinking("let me look"),
        AssistantContent::ToolCall(tool_call.clone()),
    ];
    session
        .append_timed_message(
            &Message::Assistant(turn),
            Some(Timing::Turn {
                prefill_ms: 1000,
                gen_ms: 2000,
            }),
        )
        .unwrap();
    session
        .append_timed_message(
            &Message::ToolResult(ToolResultMessage::text(&tool_call, "a\nb")),
            Some(Timing::Tool {
                duration_ms: 4000,
                waited_ms: Some(9000),
            }),
        )
        .unwrap();
    let session = Session::open(session.path()).unwrap();
    let mut transcript = Transcript::default();
    for logged in &session.context_messages_with_times(&CompactionPrompts::default()) {
        push_history(&mut transcript, logged);
    }
    // The turn's cost comes back from the timing and the turn's usage.
    assert!(transcript.items().iter().any(|item| matches!(
        item,
        Item::Thinking { cost: Some(cost), .. }
            if cost.prefill_ms == 1000 && cost.gen_ms == 2000 && cost.input == 100
    )));
    assert_eq!(transcript.tool_duration("t1"), Some(4000));
    assert_eq!(transcript.tool_wait("t1"), Some(9000));
}

#[test]
fn a_custom_agent_names_the_title() {
    let mut panel = panel(vec![reply("ok")]);
    // The default agent shows the localized "Agent" label.
    assert_eq!(panel.title(), "Agent: /tmp");
    // A custom agent replaces the label with its own capitalized name.
    assert!(panel.switch_agent("review"));
    assert_eq!(panel.title(), "Review: /tmp");
}

#[test]
fn a_named_session_titles_the_panel() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });

    type_text(&mut panel, "first request");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(panel.title(), "Agent: first request");

    // The context menu raises an input prompt carrying our action.
    let (label, action) = panel
        .context_menu_items()
        .into_iter()
        .find(|(_, action)| *action == RENAME_ACTION)
        .expect("a rename entry in the menu");
    assert_eq!(label, "Rename session");
    let events = panel.handle_status_action(action);
    let Some(PanelEvent::ShowInput { on_submit, .. }) = events.first() else {
        panic!("expected an input prompt, got {events:?}");
    };
    let InputAction::Custom(submit_action) = on_submit else {
        panic!("expected a custom action");
    };
    assert_eq!(submit_action, RENAME_ACTION);

    // The submitted name wins over the first prompt and survives a reopen.
    let result = panel.handle_command(PanelCommand::InputSubmitted {
        action: submit_action.clone(),
        text: "  timeout work  ".into(),
    });
    assert!(matches!(result, CommandResult::Handled(true)));
    assert_eq!(panel.title(), "Agent: timeout work");
    let path = panel.session_path().unwrap().to_path_buf();
    drop(panel);
    assert_eq!(Session::open(&path).unwrap().name(), Some("timeout work"));

    // Without a session log there is nothing to record the name in.
    let mut logless = AgentPanel::new(setup(vec![]));
    assert!(!logless.rename_session("x"));
}

#[test]
fn sessions_can_be_listed_switched_and_resumed() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    // A session is created eagerly, so the log exists before the first
    // prompt.
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // The panel menu is trimmed to the actions with no home elsewhere;
    // "New session" starts an empty one and leaves the old log alone.
    let items = panel.context_menu_items();
    let labels: Vec<&str> = items.iter().map(|(label, _)| label.as_str()).collect();
    assert_eq!(
        labels,
        vec!["Session info", "Rename session", "Delete session"]
    );
    panel.handle_status_action(NEW_SESSION_ACTION);
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert_eq!(panel.title(), "Agent: /tmp");

    // The picker lists both, newest first, marking the current one.
    let events = panel.handle_status_action(RESUME_ACTION);
    let Some(PanelEvent::ShowSelect {
        options, on_select, ..
    }) = events.first()
    else {
        panic!("expected a picker, got {events:?}");
    };
    assert_eq!(options.len(), 2);
    assert!(options[0].starts_with("● "), "{:?}", options[0]);
    assert!(options[1].contains("first task"), "{:?}", options[1]);
    let SelectAction::Custom(action) = on_select else {
        panic!("expected a custom action");
    };

    // Choosing the older one replays its transcript into the panel.
    let result = panel.handle_command(PanelCommand::SelectionMade {
        action: action.clone(),
        index: 1,
    });
    assert!(matches!(result, CommandResult::Handled(true)));
    assert_eq!(panel.session_path().unwrap(), first_path);
    assert_eq!(panel.title(), "Agent: first task");
    let items = panel.transcript().items();
    assert!(matches!(&items[0], Item::User { text, .. } if text == "first task"));
    assert!(items
        .iter()
        .any(|item| matches!(item, Item::Assistant { text, .. } if text == "one")));

    // The resumed agent keeps the old messages as context.
    type_text(&mut panel, "second task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let reopened = Session::open(&first_path).unwrap();
    assert_eq!(
        roles(&reopened.context_messages()),
        vec!["user", "assistant", "user", "assistant"]
    );
}

#[test]
fn slash_new_starts_a_fresh_session_and_keeps_the_old() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one")])
    });
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // /new opens a fresh session; the used one is left on disk to resume.
    type_text(&mut panel, "/new");
    panel.submit();
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert!(first_path.exists(), "the previous session log is kept");
    assert_eq!(panel.session_list().len(), 2);
}

#[test]
fn slash_clear_discards_the_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one")])
    });
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // /clear deletes the current session and starts a fresh one, so there
    // is nothing to resume back to: only the new empty session remains.
    type_text(&mut panel, "/clear");
    panel.submit();
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert!(!first_path.exists(), "the previous session log is removed");
    assert_eq!(panel.session_list().len(), 1);
}

#[test]
fn slash_rename_and_name_set_the_session_title() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    // With an argument, both /rename and /name set the title directly.
    type_text(&mut panel, "/rename my work");
    panel.submit();
    assert_eq!(panel.title(), "Agent: my work");
    type_text(&mut panel, "/name other");
    panel.submit();
    assert_eq!(panel.title(), "Agent: other");
    assert!(panel.input_text().is_empty());

    // Without an argument, it opens the rename prompt instead of sending.
    type_text(&mut panel, "/rename");
    let events = panel.submit();
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowInput { .. })));
    assert!(panel.transcript().items().is_empty(), "nothing was sent");
}

#[test]
fn loop_args_split_interval_from_prompt() {
    assert_eq!(parse_duration("30s"), Some(Duration::from_secs(30)));
    assert_eq!(parse_duration("5m"), Some(Duration::from_secs(300)));
    assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
    assert_eq!(parse_duration("90"), Some(Duration::from_secs(90)));
    assert_eq!(parse_duration("x"), None);
    assert_eq!(parse_duration("0"), None);
    assert_eq!(
        parse_loop_args("5m run the tests"),
        (Some(Duration::from_secs(300)), "run the tests")
    );
    assert_eq!(parse_loop_args("keep improving"), (None, "keep improving"));
}

#[test]
fn slash_loop_starts_and_stops() {
    let mut panel = panel(vec![reply("a")]);
    type_text(&mut panel, "/loop keep going");
    panel.submit();
    assert!(panel.loop_task.is_some(), "the loop started");

    type_text(&mut panel, "/loop stop");
    panel.submit();
    assert!(panel.loop_task.is_none(), "/loop stop ended it");

    // Esc also ends a waiting loop.
    type_text(&mut panel, "/loop 5m again");
    panel.submit();
    assert!(panel.loop_task.is_some());
    settle(&mut panel);
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.loop_task.is_none(), "Esc ended the loop");
}

#[test]
fn slash_completion_is_alphabetical() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "/c");
    let list = panel.completion.as_ref().expect("a completion popup");
    let names: Vec<&str> = list.items().iter().map(|i| i.value.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "the /-command list should be alphabetical");
    assert!(names.contains(&"clear") && names.contains(&"compact"));
}

#[test]
fn slash_pause_and_continue_report_when_idle() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/pause");
    panel.submit();
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("nothing is running"))));
    type_text(&mut panel, "/continue");
    panel.submit();
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("nothing to continue"))));
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

#[test]
fn goal_works_turn_by_turn_until_the_judge_says_done() {
    // Provider calls, in order: work turn, judge (continue), work turn,
    // judge (done).
    let mut panel = panel(vec![
        reply("starting the work"),
        reply("CONTINUE\ntests are still red"),
        reply("more work done"),
        reply("DONE\neverything is green"),
    ]);
    type_text(&mut panel, "/goal get the build green");
    panel.submit();
    assert!(panel.goal_task.is_some());
    settle_goal(&mut panel);

    // Two work turns were sent (the goal, then a continuation).
    let users = panel
        .transcript()
        .items()
        .iter()
        .filter(|i| matches!(i, Item::User { .. }))
        .count();
    assert_eq!(users, 2);
    // The judge ran and the goal ended with the success reason.
    assert!(panel.transcript().items().iter().any(
        |i| matches!(i, Item::Notice { text, .. } if text.contains("checking whether the goal"))
    ));
    assert!(panel.transcript().items().iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("goal reached: everything is green"))
        ));
}

#[test]
fn goal_stop_ends_an_active_goal() {
    let mut panel = panel(vec![reply("working")]);
    type_text(&mut panel, "/goal do the thing");
    panel.submit();
    settle(&mut panel);
    assert!(panel.goal_task.is_some());
    // Stop it before the judge would send another turn.
    type_text(&mut panel, "/goal stop");
    panel.submit();
    assert!(panel.goal_task.is_none());
    assert!(panel
            .transcript()
            .items()
            .iter()
            .any(|i| matches!(i, Item::Notice { text, .. } if text.contains(termide_i18n::t().agent_notice_goal_stopped()))));
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

#[test]
fn shift_enter_adds_a_line_and_esc_clears() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "one");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::SHIFT));
    type_text(&mut panel, "two");
    assert_eq!(panel.input_text(), "one\ntwo");
    let rows = render_text(&mut panel, 30, 6);
    assert!(rows[4].starts_with("› one"));
    assert!(rows[5].starts_with("  two"));

    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.input_text().is_empty());
    assert!(!panel.captures_escape());
    assert!(panel
        .handle_key(chord(KeyCode::Esc, KeyModifiers::NONE))
        .is_empty());
}

#[test]
fn a_large_paste_is_held_as_a_placeholder_and_expanded() {
    let mut panel = panel(vec![]);
    let big = (1..=40)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");

    // A large paste shows a short placeholder, not the whole block.
    panel.handle_command(PanelCommand::PasteText { text: big.clone() });
    assert_eq!(panel.input_text(), "[#1 pasted 40 lines]");
    // Its full text is spliced back for the message.
    assert_eq!(panel.expand_pastes(&panel.input_text()), big);

    // A small paste is inlined as-is, beside the placeholder.
    panel.handle_command(PanelCommand::PasteText {
        text: " review this".into(),
    });
    assert_eq!(panel.input_text(), "[#1 pasted 40 lines] review this");
    assert_eq!(
        panel.expand_pastes(&panel.input_text()),
        format!("{big} review this")
    );

    // Submitting sends the expanded text and drops the held paste.
    panel.submit();
    assert!(panel.input_text().is_empty());
    assert!(panel.pastes.is_empty());
    assert_eq!(panel.paste_seq, 0);
}

#[test]
fn f2_opens_the_rename_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    let events = panel.handle_key(chord(KeyCode::F(2), KeyModifiers::NONE));
    let Some(PanelEvent::ShowInput { on_submit, .. }) = events.first() else {
        panic!("F2 should open the rename prompt, got {events:?}");
    };
    assert!(
        matches!(on_submit, InputAction::Custom(a) if a == RENAME_ACTION),
        "{on_submit:?}"
    );
}

#[test]
fn f8_confirms_in_a_modal_then_deletes_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let first_path = panel.session_path().unwrap().to_path_buf();

    // F8 asks through an app confirmation modal — nothing is deleted yet,
    // and the panel raises no in-panel card of its own.
    let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    let Some(PanelEvent::ShowConfirm {
        on_confirm,
        message,
    }) = events.first()
    else {
        panic!("F8 should raise a confirmation modal, got {events:?}");
    };
    // An unnamed session is named in the UI language, its id below.
    let t = termide_i18n::t();
    let id = first_path.file_stem().unwrap().to_str().unwrap();
    assert_eq!(
        *message,
        format!(
            "{}\n{id}",
            t.agent_delete_confirm_fmt(t.agent_delete_this_session())
        )
    );
    assert!(
        matches!(on_confirm, ConfirmAction::Custom(a) if a == DELETE_SESSION_ACTION),
        "{on_confirm:?}"
    );
    assert!(panel.pending.is_none());
    assert!(first_path.exists());

    // The confirmed answer comes back as a Confirmed command: the log is
    // removed and a fresh session starts.
    panel.handle_command(PanelCommand::Confirmed {
        action: DELETE_SESSION_ACTION.to_string(),
    });
    settle(&mut panel);
    assert!(!first_path.exists(), "the session log is removed");
    assert_ne!(panel.session_path().unwrap(), first_path);
    assert!(panel.transcript().items().is_empty());

    // Cancelling the modal (no Confirmed command) keeps the session.
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let path = panel.session_path().unwrap().to_path_buf();
    panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    assert!(path.exists(), "cancelled delete keeps the log");
}

#[test]
fn f7_starts_a_new_session_and_f6_opens_the_switcher() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    type_text(&mut panel, "task one");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let first = panel.session_path().unwrap().to_path_buf();

    // F7 opens a fresh session, keeping the used one.
    panel.handle_key(chord(KeyCode::F(7), KeyModifiers::NONE));
    assert!(panel.transcript().items().is_empty());
    assert_ne!(panel.session_path().unwrap(), first);

    // F6 opens the session switcher.
    let events = panel.handle_key(chord(KeyCode::F(6), KeyModifiers::NONE));
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
}

#[test]
fn f3_shows_a_session_summary() {
    let mut panel = panel(vec![]);
    let events = panel.handle_key(chord(KeyCode::F(3), KeyModifiers::NONE));
    let Some(PanelEvent::ShowInfo { rows, .. }) = events.first() else {
        panic!("F3 should show a summary modal, got {events:?}");
    };
    for label in ["Provider", "Model", "Agent", "Directory", "Tokens"] {
        assert!(
            rows.iter().any(|(key, _)| key == label),
            "missing {label}: {rows:?}"
        );
    }
    assert!(
        rows.iter()
            .any(|(key, value)| key == "Model" && value == panel.model.id.as_str()),
        "{rows:?}"
    );
}

#[test]
fn f4_offers_a_rollback_picker_when_there_is_a_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    // Nothing changed yet: F4 says there is nothing to roll back.
    panel.handle_key(chord(KeyCode::F(4), KeyModifiers::NONE));
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("roll back"))));

    // Record a checkpoint, then F4 offers it in a picker.
    let file = dir.path().join("x.txt");
    std::fs::write(&file, "v1").unwrap();
    {
        let store = panel.checkpoints.clone().expect("a checkpoint store");
        let mut store = store.lock().unwrap();
        store.begin_run(None);
        store.save(&file).unwrap();
        store.end_run();
    }
    let events = panel.handle_key(chord(KeyCode::F(4), KeyModifiers::NONE));
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
}

#[test]
fn an_empty_session_shows_a_welcome_banner() {
    let mut panel = panel(vec![]);
    let rows = render_text(&mut panel, 60, 16);
    // The banner sits at the top, below one blank row, leaving the space
    // under it to the list of recent sessions.
    assert!(rows[1].contains("termide"), "{rows:#?}");
    let all = rows.join("\n");
    for label in ["connection", "model", "agent", "cwd"] {
        assert!(all.contains(label), "missing {label}: {all}");
    }
    // The banner is the empty-state: once a turn runs, real content shows.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let all = render_text(&mut panel, 60, 16).join("\n");
    assert!(
        !all.contains("coding agent"),
        "banner gone once used: {all}"
    );
    // Once the banner is gone it leaves no clickable fields behind.
    assert!(panel.banner_hits.is_empty());
}

#[test]
fn notices_before_the_first_request_go_under_the_banner() {
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        ..setup(vec![])
    });
    tx.send(LateTools::Ready {
        source: "github".into(),
        tools: vec![Arc::new(Late("github__search"))],
    })
    .unwrap();
    panel.tick();
    let rows = render_text(&mut panel, 60, 20);
    let all = rows.join("\n");
    // The banner stays up, the notice sits under it past a dashed rule.
    assert!(all.contains("coding agent"), "banner stays: {all}");
    let rule = rows.iter().position(|r| r.contains("╌╌╌")).expect("a rule");
    let notice = rows
        .iter()
        .position(|r| r.contains("mcp github: 1 tools connected"))
        .expect("the notice");
    let cwd = rows.iter().position(|r| r.contains("cwd")).unwrap();
    assert!(cwd < rule && rule < notice, "{rows:#?}");
    assert!(!panel.banner_hits.is_empty());

    // On a short panel the fields keep their rows and the newest notice
    // is the one shown.
    for i in 0..10 {
        panel.notice(format!("note {i}"), NoticeKind::Info);
    }
    let rows = render_text(&mut panel, 60, 16);
    let all = rows.join("\n");
    assert!(all.contains("cwd") && all.contains("note 9"), "{rows:#?}");
    assert!(!all.contains("note 0"), "{rows:#?}");

    // The first request takes the banner away; the notices stay above it.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let all = render_text(&mut panel, 60, 40).join("\n");
    assert!(!all.contains("coding agent"), "banner gone: {all}");
    assert!(all.contains("note 9"), "{all}");
    assert!(panel.banner_hits.is_empty());
}

#[test]
fn the_summary_reports_output_cleaning_savings() {
    use termide_agent_core::ToolCall;
    let mut panel = panel(vec![]);
    let call = ToolCall {
        id: "b1".into(),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": "ls" }),
        extra_content: None,
    };
    panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&call, "out")
            .with_details(serde_json::json!({ "raw_bytes": 1000_u64, "cleaned_bytes": 250_u64 })),
    });
    let events = panel.session_summary();
    let Some(PanelEvent::ShowInfo { rows, .. }) = events.first() else {
        panic!("expected a summary modal, got {events:?}");
    };
    let cleaned = rows
        .iter()
        .find(|(key, _)| key == "Output cleaned")
        .map(|(_, value)| value.as_str())
        .unwrap_or_else(|| panic!("no cleaning row: {rows:?}"));
    assert!(cleaned.contains("−75%"), "{cleaned}");
}

#[test]
fn slash_usage_opens_the_session_info_modal() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/usage");
    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, PanelEvent::ShowInfo { .. })),
        "expected an info modal, got {events:?}"
    );
}

#[test]
fn slash_prompt_opens_the_system_prompt() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/prompt");
    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        events.iter().any(|e| matches!(e, PanelEvent::ViewFile(_))),
        "expected the prompt file to open, got {events:?}"
    );
}

#[test]
fn handoff_offers_to_save_or_start_a_new_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.path().to_path_buf(),
        ..setup(vec![reply("# Handoff\n\n## Remaining\nFinish the parser.")])
    });
    type_text(&mut panel, "/handoff");
    panel.submit();

    // The brief is produced off-thread; a card offers what to do with it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline, "no handoff card");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(matches!(panel.pending, Some(Pending::Handoff { .. })));

    // Choosing "Save to HANDOFF.md" writes the brief to the panel's dir.
    panel.apply_form_action(ChoiceAction::Chosen(0));
    let written = std::fs::read_to_string(dir.path().join("HANDOFF.md")).unwrap();
    assert!(written.contains("Finish the parser."), "{written}");
    assert!(panel.pending.is_none());
}

#[test]
fn a_model_switch_in_a_fresh_session_updates_the_banner_not_the_transcript() {
    let mut panel = panel(vec![]);
    assert!(render_text(&mut panel, 60, 16)
        .join("\n")
        .contains("coding agent"));

    // No prompt yet: switching the model updates the banner in place and
    // pushes no notice, so the banner stays.
    assert!(panel.switch_model("gpt-5-brand-new", None));
    assert!(panel.is_fresh());
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { .. })));
    let all = render_text(&mut panel, 60, 16).join("\n");
    assert!(all.contains("coding agent"), "banner stays: {all}");
    assert!(all.contains("gpt-5-brand-new"), "banner shows it: {all}");

    // Once the conversation has begun, a switch is announced as before.
    type_text(&mut panel, "hi");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(!panel.is_fresh());
    assert!(panel.switch_model("gpt-6-next", None));
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("gpt-6-next"))));
}

#[test]
fn a_re_pickable_banner_value_is_bold_not_underlined() {
    let mut panel = panel(vec![]);
    let buf = render_buf(&mut panel, 60, 16);
    let (x, y) = (0..16u16)
        .find_map(|y| {
            let row: String = (0..60u16).map(|x| buf[(x, y)].symbol()).collect();
            row.find("default")
                .map(|at| (row[..at].chars().count() as u16, y))
        })
        .expect("the agent's name is on the banner");
    let cell = &buf[(x, y)];
    assert!(cell.modifier.contains(Modifier::BOLD));
    assert!(!cell.modifier.contains(Modifier::UNDERLINED));
}

#[test]
fn clicking_a_banner_field_reopens_its_picker() {
    let mut panel = panel(vec![]);
    // Rendering the empty-state banner records its clickable fields.
    let _ = render_text(&mut panel, 60, 16);
    assert_eq!(
        panel.banner_hits.len(),
        3,
        "the model, the agent and the tools are re-pickable"
    );
    let (rect, _) = panel
        .banner_hits
        .iter()
        .find(|(_, hit)| *hit == BannerHit::Action(AGENT_ACTION))
        .copied()
        .expect("the agent field is clickable");
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    let events = panel.handle_mouse(click, Rect::new(0, 0, 60, 16));
    assert!(
        events.iter().any(|e| matches!(
            e,
            PanelEvent::ShowSelect { on_select: SelectAction::Custom(a), .. } if a == AGENT_ACTION
        )),
        "clicking the agent field opens the agent picker, got {events:?}"
    );
}

#[test]
fn a_fresh_banner_offers_recent_sessions_to_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    // The very first session has nothing else to offer.
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(!all.contains("sessions"), "{all}");
    let first_path = panel.session_path().unwrap().to_path_buf();
    type_text(&mut panel, "first task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    // A new session lists the used one, but not itself (it is empty).
    panel.handle_status_action(NEW_SESSION_ACTION);
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(all.contains("sessions"), "{all}");
    assert!(all.contains("first task"), "{all}");
    let (rect, _) = panel
        .banner_hits
        .iter()
        .find(|(_, hit)| *hit == BannerHit::Session(0))
        .copied()
        .expect("the recent session is clickable");
    assert_eq!(
        panel
            .banner_hits
            .iter()
            .filter(|(_, hit)| matches!(hit, BannerHit::Session(_)))
            .count(),
        1
    );

    // A click on it opens it in place of the fresh one.
    let click = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x + 1,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(click, Rect::new(0, 0, 80, 24));
    assert_eq!(panel.session_path().unwrap(), first_path);
    assert!(matches!(
        &panel.transcript().items()[0],
        Item::User { text, .. } if text == "first task"
    ));
    // With a conversation on screen, the banner and its list are gone.
    assert!(panel.recent_sessions.is_empty());
}

#[test]
fn tab_walks_the_banner_sessions_and_enter_opens_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two"), reply("three")])
    });
    let mut first_path = None;
    for prompt in ["first task", "second task", "third task"] {
        first_path.get_or_insert_with(|| panel.session_path().unwrap().to_path_buf());
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    assert_eq!(panel.recent_sessions.len(), 3);

    // Tall enough, all are listed, newest first.
    let all = render_text(&mut panel, 80, 24).join("\n");
    assert!(
        all.find("third task") < all.find("second task")
            && all.find("second task") < all.find("first task"),
        "{all}"
    );

    // A short panel shows one row; Tab takes the keyboard into the list,
    // the cursor starts on the newest, and walking down scrolls.
    let all = render_text(&mut panel, 80, 12).join("\n");
    assert!(
        all.contains("third task") && !all.contains("first task"),
        "{all}"
    );
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(panel.chat_focus);
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.recent_selected, 2, "the cursor stops at the end");
    let all = render_text(&mut panel, 80, 12).join("\n");
    assert!(
        all.contains("first task") && !all.contains("third task"),
        "{all}"
    );
    // Typing goes nowhere while the list has the keyboard; Esc hands it back.
    type_text(&mut panel, "x");
    assert!(panel.input_text().is_empty());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!panel.chat_focus);

    // Back in the list the cursor is where it was; Home goes to the top,
    // End to the bottom, and Enter opens the session under it and returns
    // the keyboard to the prompt.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Home, KeyModifiers::NONE));
    assert_eq!(panel.recent_selected, 0);
    panel.handle_key(chord(KeyCode::End, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.session_path(), first_path.as_deref());
    assert!(!panel.chat_focus);
}

#[test]
fn the_banner_leaves_out_sessions_open_in_other_panels() {
    let dir = tempfile::tempdir().unwrap();
    let with_dir = |replies| AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(replies)
    };
    // One panel keeps its used session open; another's is used and closed.
    let mut busy = AgentPanel::new(with_dir(vec![reply("one")]));
    type_text(&mut busy, "kept open");
    busy.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut busy);
    let mut closed = AgentPanel::new(with_dir(vec![reply("two")]));
    type_text(&mut closed, "closed task");
    closed.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut closed);
    drop(closed);

    let mut fresh = AgentPanel::new(with_dir(vec![]));
    let all = render_text(&mut fresh, 80, 24).join("\n");
    assert!(all.contains("closed task"), "{all}");
    assert!(!all.contains("kept open"), "open elsewhere: {all}");

    // Closing the other panel releases its session, and the next tick lists
    // it.
    drop(busy);
    fresh.tick();
    let all = render_text(&mut fresh, 80, 24).join("\n");
    assert!(all.contains("kept open"), "{all}");
}

#[test]
fn f8_in_the_banner_list_deletes_the_picked_session_not_the_fresh_one() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    let mut first_path = None;
    for prompt in ["first task", "second task"] {
        first_path.get_or_insert_with(|| panel.session_path().unwrap().to_path_buf());
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    let first_path = first_path.unwrap();
    let fresh = panel.session_path().unwrap().to_path_buf();
    let confirm = |events: &[PanelEvent]| match events {
        [PanelEvent::ShowConfirm {
            message,
            on_confirm: ConfirmAction::Custom(action),
        }] => (message.clone(), action.clone()),
        other => panic!("expected a confirmation, got {other:?}"),
    };

    // From the prompt F8 still asks about the current session.
    let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    assert_eq!(confirm(&events).1, DELETE_SESSION_ACTION);

    // In the list it asks about the session under the cursor.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    let events = panel.handle_key(chord(KeyCode::F(8), KeyModifiers::NONE));
    let (message, action) = confirm(&events);
    assert!(message.contains("first task"), "{message}");
    panel.handle_command(PanelCommand::Confirmed { action });
    assert!(!first_path.exists());
    assert_eq!(panel.session_path(), Some(fresh.as_path()));
    assert_eq!(panel.recent_sessions.len(), 1);
    assert_eq!(panel.recent_selected, 0, "the cursor stays on the list");

    // Delete does the same; with the list empty the keyboard is back in the
    // prompt.
    let events = panel.handle_key(chord(KeyCode::Delete, KeyModifiers::NONE));
    let (message, action) = confirm(&events);
    assert!(message.contains("second task"), "{message}");
    panel.handle_command(PanelCommand::Confirmed { action });
    assert!(panel.recent_sessions.is_empty());
    assert!(!panel.chat_focus);
    assert!(fresh.exists(), "the fresh session is untouched");
}

#[test]
fn the_wheel_scrolls_the_banner_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("one"), reply("two")])
    });
    for prompt in ["first task", "second task"] {
        type_text(&mut panel, prompt);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
        panel.handle_status_action(NEW_SESSION_ACTION);
    }
    let _ = render_text(&mut panel, 80, 12);
    panel.handle_scroll(1, Rect::new(0, 0, 80, 12));
    let all = render_text(&mut panel, 80, 12).join("\n");
    assert!(
        all.contains("first task") && !all.contains("second task"),
        "{all}"
    );
    // Past the end it stops.
    panel.handle_scroll(5, Rect::new(0, 0, 80, 12));
    assert_eq!(panel.recent_top, 1);
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

#[test]
fn the_models_questions_are_answered_one_card_at_a_time() {
    let mut panel = panel(vec![]);
    let mut first = question("Which approach?", &["Channel", "Slot"], false);
    first.header = "Approach".into();
    let worker = ask_in_worker(
        &mut panel,
        vec![
            first,
            question("Which crates?", &["core", "ui", "app"], true),
        ],
    );
    {
        let form = panel.pending.as_ref().unwrap().form();
        assert_eq!(form.title(), "Agent asks: Approach (1/2)");
        assert_eq!(form.detail(), Some("Which approach?"));
        assert_eq!(form.options(), ["Channel", "Slot"]);
    }
    let rows = render_text(&mut panel, 60, 16);
    assert!(
        rows.iter().any(|r| r.contains("1. Channel  about Channel")),
        "{rows:?}"
    );
    assert!(rows.iter().any(|r| r.contains("3. Type your own answer")));
    // A single choice answers the question and brings up the next.
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(
        panel.pending.as_ref().unwrap().form().title(),
        "Agent asks (2/2)"
    );
    // Several picks, and an answer of the user's own among them.
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
    type_text(&mut panel, "docs");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let rows = render_text(&mut panel, 60, 16);
    assert!(rows.iter().any(|r| r.contains("[✓] core")), "{rows:?}");
    assert!(rows.iter().any(|r| r.contains("[ ] ui")), "{rows:?}");
    panel.handle_key(chord(KeyCode::Char('5'), KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(
        worker.join().unwrap(),
        QuestionReply::Answered(vec![
            QuestionAnswer {
                chosen: vec!["Slot".into()],
                custom: None,
            },
            QuestionAnswer {
                chosen: vec!["core".into(), "app".into()],
                custom: Some("docs".into()),
            },
        ])
    );
}

#[test]
fn escape_declines_the_models_question() {
    let mut panel = panel(vec![]);
    let worker = ask_in_worker(&mut panel, vec![question("Name?", &[], false)]);
    assert!(panel.captures_escape());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), QuestionReply::Declined);
}

#[test]
fn permission_prompt_is_answered_in_the_panel() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "bash".into(),
            subject: "git push".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "git push" }),
                extra_content: None,
            },
            suggested_pattern: "git push *".into(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "git push".into(),
                pattern: Some("git push *".into()),
            }],
        })
    });

    // The question arrives as a form in the panel and a status line.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = panel.tick();
        if panel.pending.is_some() {
            assert!(events.iter().any(|e| matches!(
                e,
                PanelEvent::SetStatusMessage { message, .. } if message.contains("git push")
            )));
            // The call waits on the user, so the panel asks for the bell.
            assert!(rings(&events));
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    {
        let form = panel.pending.as_ref().unwrap().form();
        assert_eq!(form.title(), "Agent wants to run bash:");
        assert_eq!(form.detail(), Some("git push"));
        assert_eq!(
            form.options()[2],
            "Allow always in this project (git push *)"
        );
        assert_eq!(form.options()[3], "Allow always everywhere (git push *)");
        assert_eq!(form.options()[5], "Deny for this session (git push *)");
    }
    assert!(panel.captures_escape());
    let rows = render_text(&mut panel, 60, 14);
    assert!(
        rows.iter().any(|r| r.contains("2. Allow for this session")),
        "{rows:?}"
    );
    // Typing goes nowhere while the question is open; Down + Enter answer it.
    type_text(&mut panel, "x");
    assert_eq!(panel.input_text(), "");
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);

    // A digit answers at once; Esc declines.
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let request = termide_agent_core::PermissionRequest {
        tool: "edit".into(),
        subject: "src/x.rs".into(),
        call: termide_agent_core::ToolCall {
            id: "c".into(),
            name: "edit".into(),
            arguments: serde_json::json!({ "path": "src/x.rs" }),
            extra_content: None,
        },
        suggested_pattern: "src/x.rs".into(),
        can_persist: true,
        can_allow_session: true,
        parts: Vec::new(),
    };
    let asked = request.clone();
    let worker = std::thread::spawn(move || {
        let first = prompter.ask(&asked);
        let second = prompter.ask(&asked);
        (first, second)
    });
    let wait = |panel: &mut AgentPanel| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while panel.pending.is_none() {
            panel.tick();
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    wait(&mut panel);
    panel.handle_key(chord(KeyCode::Char('3'), KeyModifiers::NONE));
    // The fifth row takes a reason the model gets to read.
    wait(&mut panel);
    {
        let form = panel.pending.as_ref().unwrap().form();
        assert_eq!(
            form.height(60),
            12,
            "a detail line and divider, six answers, a reason row and a stop row"
        );
    }
    panel.handle_key(chord(KeyCode::Char('7'), KeyModifiers::NONE));
    type_text(&mut panel, "edit the test instead");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        worker.join().unwrap(),
        (
            PermissionAnswer::AllowAlways,
            PermissionAnswer::DenyWithReason("edit the test instead".into())
        )
    );
    let _ = request;
}

#[test]
fn a_compound_command_card_lists_the_parts_it_asks_about() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let command = "cd $BUILD && ./run && make install";
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "bash".into(),
            subject: command.into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": command }),
                extra_content: None,
            },
            suggested_pattern: "make install *".into(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![
                termide_agent_core::AskedPart {
                    text: "./run".into(),
                    pattern: None,
                },
                termide_agent_core::AskedPart {
                    text: "make install".into(),
                    pattern: Some("make install *".into()),
                },
            ],
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let form = panel.pending.as_ref().unwrap().form();
    let detail = form.detail().unwrap().to_string();
    assert!(detail.starts_with(command), "{detail}");
    let once = termide_i18n::t().agent_perm_part_once();
    assert!(detail.contains(&format!("• ./run ({once})")), "{detail}");
    assert!(detail.contains("• make install\n") || detail.ends_with("• make install"));
    // The recorded rule is the part's that can have one.
    assert!(
        form.options()[1].contains("(make install *)"),
        "{:?}",
        form.options()
    );
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);
    assert_eq!(
        panel
            .effective_rules()
            .evaluate_session("bash", "make install x"),
        Some(Decision::Allow)
    );
    assert_eq!(
        panel.effective_rules().evaluate_session("bash", "./run"),
        None
    );

    // With no part a rule can stand for, nothing is offered to remember.
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "bash".into(),
            subject: "cd $BUILD && ./run".into(),
            call: termide_agent_core::ToolCall {
                id: "d".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "cd $BUILD && ./run" }),
                extra_content: None,
            },
            suggested_pattern: String::new(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "./run".into(),
                pattern: None,
            }],
        })
    });
    while panel.pending.is_none() {
        panel.tick();
        std::thread::sleep(Duration::from_millis(5));
    }
    let options = panel.pending.as_ref().unwrap().form().options().to_vec();
    assert_eq!(options.len(), 2, "allow once and deny only: {options:?}");
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowOnce);
}

#[test]
fn always_is_not_offered_where_the_configured_rules_do_not_count() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "bash".into(),
            subject: "make".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "make" }),
                extra_content: None,
            },
            suggested_pattern: "make *".into(),
            can_persist: false,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "make".into(),
                pattern: Some("make *".into()),
            }],
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let options = panel.pending.as_ref().unwrap().form().options().to_vec();
    assert_eq!(options.len(), 4, "{options:?}");
    assert!(options.iter().all(|o| !o.contains("always")), "{options:?}");
    // The last row refuses for the session.
    panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
    assert_eq!(worker.join().unwrap(), PermissionAnswer::DenySession);
    assert_eq!(
        panel.effective_rules().evaluate_session("bash", "make all"),
        Some(Decision::Deny)
    );
}

#[test]
fn a_single_click_selects_and_a_double_click_answers_a_permission() {
    let mut panel = panel(vec![]);
    let (mut prompter, rx) = permission_channel(CancelToken::new());
    panel.permission_rx = rx;
    let worker = std::thread::spawn(move || {
        prompter.ask(&termide_agent_core::PermissionRequest {
            tool: "bash".into(),
            subject: "git push".into(),
            call: termide_agent_core::ToolCall {
                id: "c".into(),
                name: "bash".into(),
                arguments: serde_json::json!({ "command": "git push" }),
                extra_content: None,
            },
            suggested_pattern: "git push *".into(),
            can_persist: true,
            can_allow_session: true,
            parts: vec![termide_agent_core::AskedPart {
                text: "git push".into(),
                pattern: Some("git push *".into()),
            }],
        })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while panel.pending.is_none() {
        panel.tick();
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Render so the form's row geometry exists, then find the second option.
    let rows = render_text(&mut panel, 60, 16);
    let y = rows
        .iter()
        .position(|r| r.contains("2. Allow for this session"))
        .expect("the second option is on screen") as u16;
    let area = Rect::new(0, 0, 60, 16);
    let click = |panel: &mut AgentPanel, y: u16| {
        panel.handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 5,
                row: y,
                modifiers: KeyModifiers::NONE,
            },
            area,
        );
    };

    // A single click only moves the selection; the question stays open.
    click(&mut panel, y);
    assert!(panel.pending.is_some(), "a single click does not answer");
    assert_eq!(panel.pending.as_ref().unwrap().form().selected(), 1);

    // A second click on the same row (a double click) answers it.
    click(&mut panel, y);
    assert!(panel.pending.is_none());
    assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);
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
            Arc::new(|setup: BackendSetup| Ok(Box::new(External::new(setup)) as Box<dyn Backend>))
                as BackendFactory
        });
        Some(ConnectionChoice {
            name: entry.name,
            kind: entry.kind,
            provider: Arc::new(Scripted::new(vec![])),
            model: entry.model,
            context_window: 200_000,
            backend,
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

#[test]
fn a_new_session_records_the_connection_it_starts_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    let first = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(first.current_connection(), Some("local".to_string()));
    // A new session in the panel too.
    panel.switch_session(None);
    let second = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(second.current_connection(), Some("local".to_string()));
}

#[test]
fn the_connection_picker_switches_the_endpoint_and_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    let events = panel.handle_status_action(CONNECTION_ACTION);
    let Some(PanelEvent::ShowSelect { options, .. }) = events.first() else {
        panic!("a picker, got {events:?}");
    };
    assert!(options[0].starts_with("● local"), "{options:?}");
    assert!(options[1].contains("cloud") && options[1].contains("claude-x"));
    panel.handle_command(PanelCommand::SelectionMade {
        action: CONNECTION_ACTION.to_string(),
        index: 1,
    });
    // The connection's endpoint and model replace the ones in use.
    assert_eq!(panel.connection, "cloud");
    assert_eq!(panel.provider_kind, "anthropic_compatible");
    assert_eq!(panel.model.id, "claude-x");
    assert_eq!(panel.model.context_window, 200_000);
    assert!(!panel.external);
    // The chip names the connection beside its protocol.
    assert_eq!(panel.connection_display(), "cloud · Anthropic Compatible");
    // The log keeps it, so a reopened session reconnects there.
    let session = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(session.current_connection(), Some("cloud".to_string()));
    assert_eq!(
        session.current_model().map(|m| m.id),
        Some("claude-x".to_string())
    );
}

#[test]
fn a_cli_agent_connection_is_taken_on_only_before_the_first_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = connected_panel(dir.path());
    panel.transcript.push(Item::User {
        text: "go".into(),
        at: String::new(),
        command: None,
    });
    // Mid-conversation the CLI agent would not see it: refused.
    assert!(!panel.switch_connection("cli"));
    assert_eq!(panel.connection, "local");
    assert!(panel.transcript.items().iter().any(|item| matches!(
            item,
            Item::Notice { text, .. } if text == termide_i18n::t().agent_notice_connection_before_first()
        )));
    // In a fresh session it drives the CLI agent over ACP.
    let dir = tempfile::tempdir().unwrap();
    let mut fresh = connected_panel(dir.path());
    assert!(fresh.switch_connection("cli"));
    assert!(fresh.external);
    assert_eq!(fresh.provider_kind, "codex");
}

#[test]
fn the_toolset_guard_refuses_what_is_switched_off_in_context() {
    let blocked: Blocked = Arc::new(RwLock::new(
        ["bash".to_string(), "skill:review".to_string()].into(),
    ));
    let mut guard = ToolsetGuard { blocked };
    let ctx = ToolContext::new(PathBuf::from("/tmp"));
    let call = |name: &str, args: serde_json::Value| ToolCall {
        id: "c".into(),
        name: name.into(),
        arguments: args,
        extra_content: None,
    };
    let refused = |d: ToolDecision| matches!(d, ToolDecision::Block { .. });
    assert!(refused(
        guard.before_tool_call(&call("bash", serde_json::json!({})), &ctx)
    ));
    assert!(refused(guard.before_tool_call(
        &call("skill", serde_json::json!({ "name": "review" })),
        &ctx
    )));
    assert!(!refused(guard.before_tool_call(
        &call("skill", serde_json::json!({ "name": "deploy" })),
        &ctx
    )));
    assert!(!refused(
        guard.before_tool_call(&call("read", serde_json::json!({})), &ctx)
    ));
}

#[test]
fn switching_off_before_the_first_request_keeps_it_out_of_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![])
    });
    panel.offered_tools = vec!["read".into(), "bash".into()];
    panel.offered_skills = vec!["review".into()];
    assert!(panel.is_fresh());
    panel.apply_toolset(&["read".to_string()]);
    let off: BTreeSet<String> = ["bash".to_string(), "skill:review".to_string()].into();
    assert_eq!(panel.toolset_off, off);
    // Rebuilt at once without them: out of the context, nothing to refuse.
    assert_eq!(panel.context_off, off);
    assert!(panel.blocked.read().unwrap().is_empty());
    // The log keeps the set for a reopened session.
    let path = panel.session_path().unwrap().to_path_buf();
    let session = Session::open(&path).unwrap();
    assert_eq!(
        session.current_toolset(),
        Some(vec!["bash".to_string(), "skill:review".to_string()])
    );
    // Reopened, the session comes back with it, built without it: a new
    // worker has no cache to keep.
    drop(panel);
    let reopened = AgentPanel::new(AgentPanelSetup {
        session: Some(session),
        ..setup(vec![])
    });
    assert_eq!(reopened.toolset_off, off);
    assert_eq!(reopened.context_off, off);
    assert!(reopened.blocked.read().unwrap().is_empty());
}

#[test]
fn switching_off_mid_session_refuses_until_a_compaction_takes_it_out() {
    let mut panel = panel(vec![]);
    panel.offered_tools = vec!["read".into(), "bash".into()];
    panel.transcript.push(Item::User {
        text: "go".into(),
        at: String::new(),
        command: None,
    });
    assert!(!panel.is_fresh());
    panel.apply_toolset(&["read".to_string()]);
    // Still in the context (the prompt cache stays): refused, not gone.
    assert!(panel.context_off.is_empty());
    assert!(panel.blocked.read().unwrap().contains("bash"));
    let items = panel.toolset_items();
    let bash = items.iter().find(|i| i.key == "bash").unwrap();
    assert!(
        bash.enabled && !bash.checked && !bash.note.is_empty(),
        "{bash:?}"
    );
    // A compaction lets the next moment between runs rebuild without it.
    panel.apply(AgentEvent::Compacted {
        summary: "earlier".into(),
        kept: 1,
        tokens_before: 1000,
    });
    assert!(panel.context_stale);
    panel.tick();
    assert!(panel.context_off.contains("bash"));
    assert!(panel.blocked.read().unwrap().is_empty());
    // Out of the context now, it cannot come back in this session.
    let items = panel.toolset_items();
    let bash = items.iter().find(|i| i.key == "bash").unwrap();
    assert!(!bash.enabled);
    panel.apply_toolset(&["read".to_string(), "bash".to_string()]);
    assert!(
        panel.toolset_off.contains("bash"),
        "a locked item keeps its state"
    );
}

#[test]
fn up_takes_the_queue_back_into_the_input() {
    let mut panel = panel(vec![]);
    panel.apply(AgentEvent::AgentStart);
    for text in ["fix the test", "and the docs"] {
        type_text(&mut panel, text);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    }
    assert_eq!(panel.runtime.queue_lens(), (2, 0));
    type_text(&mut panel, "also");
    // Up at the top of the input takes both back, ahead of what is typed.
    for _ in 0..2 {
        panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    }
    assert_eq!(panel.input_text(), "fix the test\n\nand the docs\n\nalso");
    assert_eq!(panel.runtime.queue_lens(), (0, 0));
    assert!(panel.queued_texts.is_empty());
    assert!(
        panel.state_lines(60).is_empty(),
        "the strip no longer lists them"
    );
}

#[test]
fn a_call_that_fails_before_its_first_token_shows_no_cost() {
    let mut panel = AgentPanel::new(setup(vec![]));
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::MessageStart {
        prompt_tokens: None,
    });
    let failed = AssistantMessage::failed("p", "m", StopReason::Error, "connection refused");
    panel.apply(AgentEvent::MessageEnd(Message::Assistant(failed)));
    let answer = panel
        .transcript
        .items()
        .iter()
        .find(|item| matches!(item, Item::Assistant { .. }))
        .expect("the failure shows on an answer block");
    assert!(matches!(
        answer,
        Item::Assistant {
            cost: None,
            error: Some(_),
            ..
        }
    ));
}

#[test]
fn a_permission_wait_is_timed_apart_from_the_call() {
    let mut panel = panel(vec![]);
    let call = termide_agent_core::ToolCall {
        id: "c".into(),
        name: "bash".into(),
        arguments: serde_json::json!({ "command": "cat notes.txt" }),
        extra_content: None,
    };
    panel.apply(AgentEvent::AgentStart);
    panel.apply(AgentEvent::ToolExecutionStart { call: call.clone() });
    // The call started 7s ago and its question has been up for 5s.
    panel
        .tool_starts
        .insert("c".into(), Instant::now() - Duration::from_secs(7));
    let (reply, _rx) = std::sync::mpsc::channel();
    panel.pending = Some(Pending::Permission {
        envelope: PermissionEnvelope {
            id: 1,
            request: termide_agent_core::PermissionRequest {
                tool: "bash".into(),
                subject: "cat notes.txt".into(),
                call: call.clone(),
                suggested_pattern: "cat *".into(),
                can_persist: true,
                can_allow_session: true,
                parts: vec![termide_agent_core::AskedPart {
                    text: "cat notes.txt".into(),
                    pattern: Some("cat *".into()),
                }],
            },
            reply,
        },
        form: ChoiceForm::new("", vec![]),
        answers: Vec::new(),
    });
    panel.permission_wait = Some((Instant::now() - Duration::from_secs(5), 0));
    panel.tick();
    // While the question is up, the call's wait ticks as a pause.
    let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
    assert!(rows.iter().any(|r| r.contains("‖ 5s")), "{rows:?}");
    assert!(matches!(
        panel.transcript.items().last(),
        Some(Item::Tool { waiting: true, .. })
    ));
    // Answered: the wait rests, and the call's duration leaves it out.
    assert!(panel.answer_permission(PermissionAnswer::AllowOnce));
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&call, "notes"),
    });
    let Some(Item::Tool {
        waited_ms: Some(waited),
        waiting: false,
        duration_ms: Some(duration),
        ..
    }) = panel.transcript.items().last().cloned()
    else {
        panic!("the call keeps its wait and duration");
    };
    assert!((5000..6000).contains(&waited), "{waited}");
    assert!((1500..2500).contains(&duration), "{duration}");
    // Folded, the finished call hides both; unfolded, they show together.
    let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
    assert!(rows.iter().all(|r| !r.contains('‖')), "{rows:?}");
    let last = panel.transcript.items().len() - 1;
    assert!(panel.transcript.toggle_expanded(last));
    let rows = strip_text(panel.transcript.lines(60, &panel.colors, false));
    assert!(rows.iter().any(|r| r.contains("‖ 5s 🕒 2s")), "{rows:?}");
}

#[test]
fn a_grant_reaches_the_panel_rules_so_it_survives_a_rebuild() {
    let pending = |panel: &mut AgentPanel| {
        let (reply, _rx) = std::sync::mpsc::channel();
        panel.pending = Some(Pending::Permission {
            envelope: PermissionEnvelope {
                id: 1,
                request: termide_agent_core::PermissionRequest {
                    tool: "bash".into(),
                    subject: "cat notes.txt".into(),
                    call: termide_agent_core::ToolCall {
                        id: "c".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({ "command": "cat notes.txt" }),
                        extra_content: None,
                    },
                    suggested_pattern: "cat *".into(),
                    can_persist: true,
                    can_allow_session: true,
                    parts: vec![termide_agent_core::AskedPart {
                        text: "cat notes.txt".into(),
                        pattern: Some("cat *".into()),
                    }],
                },
                reply,
            },
            form: ChoiceForm::new("", vec![]),
            answers: Vec::new(),
        });
    };

    // "Allow for the session" lands in the session rules, which
    // `effective_rules` — what every rebuilt agent starts from — carries
    // apart from the persistent rules; "deny for the session" too.
    let mut session = panel(vec![]);
    pending(&mut session);
    assert!(session.answer_permission(PermissionAnswer::AllowSession));
    assert_eq!(
        session.effective_rules().evaluate_session("bash", "cat x"),
        Some(Decision::Allow)
    );
    assert_eq!(session.effective_rules().evaluate("bash", "cat x"), None);
    pending(&mut session);
    assert!(session.answer_permission(PermissionAnswer::DenySession));
    assert_eq!(
        session.effective_rules().evaluate_session("bash", "cat x"),
        Some(Decision::Deny)
    );

    // "Allow always" lands in the persistent rules.
    let mut always = panel(vec![]);
    pending(&mut always);
    assert!(always.answer_permission(PermissionAnswer::AllowAlways));
    assert_eq!(
        always.rules.evaluate("bash", "cat x"),
        Some(Decision::Allow)
    );
}

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

#[test]
fn delegated_tasks_follow_the_session_mode() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(ModeWatcher(Arc::clone(&seen))),
        ..setup(vec![])
    });
    // Told the starting mode, then every change.
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert!(panel.switch_agent("review"));
    assert_eq!(
        *seen.lock().unwrap(),
        vec![Mode::Configured, Mode::Auto, Mode::Edit]
    );
}

#[test]
fn mode_switches_from_the_chip_and_with_shift_tab() {
    let mut panel = panel(vec![]);
    let hooks_mode = panel.mode.clone();
    assert_eq!(chip(&panel, MODE_ACTION), "configured");

    // Shift+Tab cycles and reports the new mode in the status line.
    let events = panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert!(events.iter().any(|e| matches!(
        e,
        PanelEvent::SetStatusMessage { message, .. } if message.ends_with("auto")
    )));
    assert_eq!(chip(&panel, MODE_ACTION), "auto");
    assert_eq!(hooks_mode.get(), Mode::Auto);

    // The chip opens a picker with the current mode marked.
    let events = panel.handle_status_action(MODE_ACTION);
    let picker = events.first().expect("picker");
    let PanelEvent::ShowSelect { options, .. } = picker else {
        panic!("expected a picker, got {picker:?}");
    };
    assert_eq!(options.len(), 6);
    assert!(options[4].starts_with("● auto"), "{:?}", options[4]);
    assert!(options[1].starts_with("  plan"), "{:?}", options[1]);
    assert!(matches!(
        select(&mut panel, picker, 2),
        CommandResult::Handled(true)
    ));
    assert_eq!(chip(&panel, MODE_ACTION), "edit");
    assert_eq!(hooks_mode.get(), Mode::Edit);
    let events = panel.tick();
    assert!(events.iter().any(|e| matches!(
        e,
        PanelEvent::SetStatusMessage { message, .. } if message.ends_with("edit")
    )));

    // Cycling wraps from all to ask, and a rebuilt agent starts in the
    // chosen mode.
    for _ in 0..3 {
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    }
    assert_eq!(chip(&panel, MODE_ACTION), "all");
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(chip(&panel, MODE_ACTION), "ask");
    panel.switch_session(None);
    assert_eq!(chip(&panel, MODE_ACTION), "ask");
    assert_eq!(panel.mode.get(), Mode::Ask);
}

#[test]
fn model_switches_are_recorded_and_followed_on_resume() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(Scripted::new(vec![reply("one"), reply("two")]));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup_with(Arc::clone(&provider))
    });
    let first_path = panel.session_path().unwrap().to_path_buf();
    // A fresh session records the model it starts on.
    assert_eq!(
        Session::open(&first_path).unwrap().current_model(),
        Some(termide_agent_core::SessionModel {
            provider: "openai_compatible".into(),
            id: "m".into(),
            context_window: Some(1000),
        })
    );
    assert_eq!(chip(&panel, MODEL_ACTION), "m");

    // The chip fetches the list off-thread; the picker marks the current
    // model and ends with the typed-id entry.
    let events = panel.handle_status_action(MODEL_ACTION);
    assert!(matches!(
        events.first(),
        Some(PanelEvent::SetStatusMessage { .. })
    ));
    let picker = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowSelect { .. }));
    let PanelEvent::ShowSelect { options, .. } = &picker else {
        unreachable!()
    };
    assert_eq!(options, &["  big", "● m", "  Enter a model id…"]);
    select(&mut panel, &picker, 0);
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    // The endpoint's context window comes along with the id.
    assert_eq!(panel.model.context_window, 64_000);
    // A fresh session updates the banner in place: the switch leaves no
    // notice, but it is still recorded in the session log.
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Notice { .. })));
    assert_eq!(
        Session::open(&first_path)
            .unwrap()
            .current_model()
            .map(|m| m.id),
        Some("big".to_string())
    );

    // The next run goes to the new model.
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(
        *provider.seen_models.lock().unwrap(),
        vec!["big".to_string()]
    );

    // A new session starts on the current model; the last picker entry
    // asks for an id by hand.
    panel.handle_status_action(NEW_SESSION_ACTION);
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    panel.handle_status_action(MODEL_ACTION);
    let picker = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowSelect { .. }));
    select(&mut panel, &picker, 2);
    let input = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowInput { .. }));
    let PanelEvent::ShowInput {
        initial_value,
        on_submit: InputAction::Custom(action),
        ..
    } = input
    else {
        panic!("expected an input prompt, got {input:?}");
    };
    assert_eq!(initial_value, "big");
    panel.handle_command(PanelCommand::InputSubmitted {
        action,
        text: " typed ".to_string(),
    });
    assert_eq!(chip(&panel, MODEL_ACTION), "typed");
    // A typed id keeps the window of the model it replaced.
    assert_eq!(panel.model.context_window, 64_000);

    // Reopening the first session returns to the model it last used.
    panel.session_choices = panel.session_list();
    let index = panel
        .session_choices
        .iter()
        .position(|s| s.path == first_path)
        .unwrap();
    panel.resume_choice(index);
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    // The window comes back from the log too.
    assert_eq!(panel.model.context_window, 64_000);
}

#[test]
fn model_picker_falls_back_to_a_typed_id() {
    let mut provider = Scripted::new(vec![]);
    provider.models = Err("HTTP 404: no such route".into());
    let mut panel = AgentPanel::new(setup_with(Arc::new(provider)));
    panel.handle_status_action(MODEL_ACTION);
    let input = wait_for(&mut panel, |e| matches!(e, PanelEvent::ShowInput { .. }));
    assert!(matches!(input, PanelEvent::ShowInput { initial_value, .. } if initial_value == "m"));
    assert!(panel.transcript().items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text.contains("HTTP 404")
    )));
}
#[test]
fn saved_state_names_the_directory_and_the_session_log() {
    let no_log = panel(vec![]);
    assert_eq!(
        no_log.to_state(Path::new("/unused")),
        Some(termide_core::PanelState::Agent {
            cwd: PathBuf::from("/tmp"),
            session: None,
            agent: None,
        })
    );

    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("done")])
    });
    type_text(&mut panel, "task");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let Some(termide_core::PanelState::Agent { cwd, session, .. }) =
        panel.to_state(Path::new("/unused"))
    else {
        panic!("agent state expected");
    };
    assert_eq!(cwd, PathBuf::from("/tmp"));
    let session = session.expect("session path");
    assert_eq!(panel.session_path(), Some(session.as_path()));

    // Rebuilding from that state brings the conversation back.
    let restored = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        session: Some(Session::open(&session).unwrap()),
        ..setup(vec![])
    });
    assert_eq!(restored.title(), "Agent: task");
    assert_eq!(restored.session_path(), Some(session.as_path()));
}

#[test]
fn a_successful_edit_reports_the_changed_file() {
    use termide_agent_core::ToolCall;
    let mut panel = panel(vec![]);
    let call = |name: &str| ToolCall {
        id: format!("{name}-1"),
        name: name.into(),
        arguments: serde_json::json!({}),
        extra_content: None,
    };
    let changed = |events: &[PanelEvent]| -> Vec<PathBuf> {
        events
            .iter()
            .filter_map(|e| match e {
                PanelEvent::FileChangedOnDisk(path) => Some(path.clone()),
                _ => None,
            })
            .collect()
    };

    let edit = call("edit");
    panel.apply(AgentEvent::ToolExecutionStart { call: edit.clone() });
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&edit, "Edited")
            .with_details(serde_json::json!({ "path": "/tmp/f.rs", "replacements": 1 })),
    });
    assert_eq!(changed(&panel.tick()), vec![PathBuf::from("/tmp/f.rs")]);

    // A failed edit, a read and a shell command report nothing.
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::error(&edit, "no match")
            .with_details(serde_json::json!({ "path": "/tmp/f.rs" })),
    });
    let read = call("read");
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&read, "…")
            .with_details(serde_json::json!({ "path": "/tmp/g.rs" })),
    });
    let bash = call("bash");
    panel.apply(AgentEvent::ToolExecutionEnd {
        result: ToolResultMessage::text(&bash, "ok"),
    });
    assert!(changed(&panel.tick()).is_empty());
}
#[test]
fn the_system_prompt_can_be_opened_as_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        system_prompt: "You are terse.\n".into(),
        ..setup(vec![])
    });
    // It has no menu slot; `/prompt` is its entry point.
    type_text(&mut panel, "/prompt");
    let events = panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let Some(PanelEvent::ViewFile(path)) = events.first() else {
        panic!("expected a viewer, got {events:?}");
    };
    assert_eq!(path, &dir.path().join("system-prompt.md"));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "You are terse.\n");
    // The prompt file is not mistaken for a session.
    assert!(panel.session_list().iter().all(|s| s.path != *path));
}

#[test]
fn a_system_prompt_block_shows_once_and_on_change() {
    let mut panel = AgentPanel::new(AgentPanelSetup {
        system_prompt: "Base prompt.".into(),
        ..setup(vec![reply("a"), reply("b"), reply("c")])
    });
    let count = |p: &AgentPanel| {
        p.transcript()
            .items()
            .iter()
            .filter(|i| matches!(i, Item::System { .. }))
            .count()
    };

    type_text(&mut panel, "hi");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(count(&panel), 1, "shown with the first message");

    // Unchanged prompt: not repeated.
    type_text(&mut panel, "again");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(count(&panel), 1, "not repeated when unchanged");

    // A different agent has a different prompt: shown again.
    assert!(panel.switch_agent("review"));
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(count(&panel), 2, "shown again after it changed");
}

#[test]
fn switching_agents_changes_prompt_model_and_mode_and_is_saved() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Arc::new(Scripted::new(vec![reply("ok")]));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup_with(Arc::clone(&provider))
    });
    assert_eq!(chip(&panel, AGENT_ACTION), "default");

    let events = panel.handle_status_action(AGENT_ACTION);
    let picker = events.first().expect("picker");
    let PanelEvent::ShowSelect { options, .. } = picker else {
        panic!("expected a picker, got {picker:?}");
    };
    assert_eq!(
        options,
        &[
            "● default",
            "  review · Reviews diffs",
            "  outside · An external agent"
        ]
    );
    assert!(matches!(
        select(&mut panel, picker, 1),
        CommandResult::Handled(true)
    ));

    assert_eq!(chip(&panel, AGENT_ACTION), "review");
    assert_eq!(chip(&panel, MODEL_ACTION), "big");
    assert_eq!(chip(&panel, MODE_ACTION), "edit");
    assert_eq!(panel.system_prompt, "You review diffs.");
    // A fresh session keeps its banner: the switch leaves no notice (it is
    // still applied and recorded, checked below on resume).
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|item| matches!(item, Item::Notice { .. })));

    // The next run goes to the reviewer's model, and the layout state
    // names the agent so a restore comes back as it.
    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(
        *provider.seen_models.lock().unwrap(),
        vec!["big".to_string()]
    );
    let Some(termide_core::PanelState::Agent { agent, .. }) = panel.to_state(Path::new("/unused"))
    else {
        panic!("agent state expected");
    };
    assert_eq!(agent.as_deref(), Some("review"));
    assert_eq!(
        Session::open(panel.session_path().unwrap())
            .unwrap()
            .current_model()
            .unwrap()
            .id,
        "big"
    );

    // A new session starts as the current agent; reopening the first
    // one comes back as the agent it last ran as, prompt and tools too.
    let first_path = panel.session_path().unwrap().to_path_buf();
    panel.handle_status_action(NEW_SESSION_ACTION);
    assert_eq!(chip(&panel, AGENT_ACTION), "review");
    panel.switch_agent("default");
    assert_eq!(panel.system_prompt, "default prompt");
    assert_eq!(chip(&panel, MODEL_ACTION), "big", "the model stays");
    assert_eq!(chip(&panel, MODE_ACTION), "edit", "the mode stays");
    panel.session_choices = panel.session_list();
    let index = panel
        .session_choices
        .iter()
        .position(|s| s.path == first_path)
        .unwrap();
    panel.resume_choice(index);
    assert_eq!(chip(&panel, AGENT_ACTION), "review");
    assert_eq!(panel.system_prompt, "You review diffs.");
    assert_eq!(
        Session::open(&first_path)
            .unwrap()
            .current_agent()
            .as_deref(),
        Some("review")
    );
    assert!(!panel.switch_agent("missing"));
}
#[test]
fn o_opens_the_selected_block_in_a_read_only_panel() {
    let mut panel = AgentPanel::new(setup(vec![reply("the answer here")]));
    type_text(&mut panel, "do it");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE)); // chat focus, last block (assistant)
    let events = panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::NONE));
    let path = events.iter().find_map(|e| match e {
        PanelEvent::ViewFile(path) => Some(path.clone()),
        _ => None,
    });
    let path = path.expect("o should open a ViewFile panel");
    assert!(std::fs::read_to_string(&path)
        .unwrap()
        .contains("the answer here"));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn tab_moves_focus_to_the_chat_and_arrows_fold_blocks() {
    // The answer folds only when its reasoning is long enough to be worth
    // hiding.
    let thinking = "l1\nl2\nl3\nl4\nl5\nl6";
    let mut panel = AgentPanel::new(setup(vec![reply_thinking("the answer", thinking)]));
    type_text(&mut panel, "do it");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // A user block and an assistant block exist.
    assert!(panel.transcript().items().len() >= 2);
    assert!(!panel.chat_focus);

    // Tab moves focus to the chat, on the last (answer) block, which is not
    // foldable. Blocks are user(0), thinking(1), answer(2); the clean run
    // closed on the answer, so its total sits there, not on a closing line.
    assert!(matches!(
        panel.transcript().items().last(),
        Some(Item::Assistant {
            run_ms: Some(_),
            ..
        })
    ));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(panel.chat_focus);
    assert_eq!(panel.selected, 2);
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.selected, 2);

    // Up walks to the reasoning block, which folds; Space expands it, again
    // folds it.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.selected, 1);
    assert!(!panel.transcript().any_expanded());
    panel.handle_key(chord(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(panel.transcript().any_expanded());
    panel.handle_key(chord(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(!panel.transcript().any_expanded());

    // Right unfolds it and Left folds it, as in the file manager's tree; a
    // second press in the same direction leaves it as it is.
    panel.handle_key(chord(KeyCode::Right, KeyModifiers::NONE));
    assert!(panel.transcript.is_expanded(1));
    panel.handle_key(chord(KeyCode::Right, KeyModifiers::NONE));
    assert!(panel.transcript.is_expanded(1));
    panel.handle_key(chord(KeyCode::Left, KeyModifiers::NONE));
    assert!(!panel.transcript.is_expanded(1));
    panel.handle_key(chord(KeyCode::Left, KeyModifiers::NONE));
    assert!(!panel.transcript.is_expanded(1));
    assert!(panel.input_text().is_empty(), "the arrows stay in the chat");

    // Up walks to the user block; a printable key does not type.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.selected, 0);
    panel.handle_key(chord(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(panel.input_text().is_empty());

    // Tab returns focus to the input.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(!panel.chat_focus);
    type_text(&mut panel, "hi");
    assert_eq!(panel.input_text(), "hi");
}

#[test]
fn ctrl_o_sets_how_fresh_blocks_fold() {
    let thinking = "l1\nl2";
    let mut panel = AgentPanel::new(setup(vec![
        reply_thinking("one", thinking),
        reply_thinking("two", thinking),
        reply_thinking("three", thinking),
    ]));
    let turn = |panel: &mut AgentPanel, text: &str| {
        type_text(panel, text);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(panel);
    };
    let reasoning_expanded = |panel: &AgentPanel| -> Vec<bool> {
        let items = panel.transcript().items();
        (0..items.len())
            .filter(|&i| matches!(items[i], Item::Thinking { .. }))
            .map(|i| panel.transcript.is_expanded(i))
            .collect()
    };
    turn(&mut panel, "first");
    assert_eq!(reasoning_expanded(&panel), [false]);
    // Unfolding everything has the next turn arrive unfolded too.
    panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::CONTROL));
    turn(&mut panel, "second");
    assert_eq!(reasoning_expanded(&panel), [true, true]);
    // Folding everything back returns fresh blocks to the configured mode.
    panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::CONTROL));
    turn(&mut panel, "third");
    assert_eq!(reasoning_expanded(&panel), [false, false, false]);
}

#[test]
fn file_completions_list_paths_and_at_mentions_insert_them() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
    std::fs::write(dir.path().join("README.md"), "hi").unwrap();
    std::fs::create_dir_all(dir.path().join(".git")).unwrap();
    std::fs::write(dir.path().join(".git/HEAD"), "ref").unwrap();

    // The walker skips .git and offers files and directories.
    let all = file_completions(dir.path(), "");
    let values: Vec<&str> = all.iter().map(|i| i.value.as_str()).collect();
    assert!(values.contains(&"README.md"), "{values:?}");
    assert!(values.contains(&"src/"), "{values:?}");
    assert!(values.contains(&"src/main.rs"), "{values:?}");
    assert!(!values.iter().any(|v| v.contains(".git")), "{values:?}");
    // A prefix filters, name matches rank first.
    let main = file_completions(dir.path(), "main");
    assert_eq!(main.first().map(|i| i.value.as_str()), Some("src/main.rs"));
    // What git ignores is not offered; the match is fuzzy on the path.
    std::fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    std::fs::write(dir.path().join("target/debug/main"), "").unwrap();
    std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    let values: Vec<String> = file_completions(dir.path(), "srmain")
        .into_iter()
        .map(|i| i.value)
        .collect();
    assert_eq!(values, ["src/main.rs"]);

    // Typing @ opens the file popup; selecting a file replaces the token.
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.path().to_path_buf(),
        ..setup(vec![])
    });
    type_text(&mut panel, "look at @READ");
    assert!(panel.completion.is_some(), "no @ completion popup");
    assert!(panel.completion_span.is_some());
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "README.md"));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "look at README.md ");
    assert!(panel.completion.is_none());

    // A directory keeps the popup open for its contents.
    type_text(&mut panel, "@src");
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "look at README.md @src/");
    assert!(panel.completion.is_some(), "dir did not reopen the popup");
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "src/main.rs"));
}

#[test]
fn slash_commands_expand_prompt_templates() {
    let mut expanding = panel(vec![reply("done")]);
    type_text(&mut expanding, "/review src/x.rs");
    expanding.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut expanding);
    let items = expanding.transcript().items();
    assert!(
        matches!(&items[0], Item::User { text, .. } if text == "Review src/x.rs carefully."),
        "{items:?}"
    );

    // An unknown command is refused with the names on offer; a path is text.
    let mut panel = panel(vec![reply("ok")]);
    type_text(&mut panel, "/nope");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.transcript().items().iter().any(
        |item| matches!(item, Item::Notice { text, .. } if text.contains("available: review"))
    ));
    assert_eq!(panel.input_text(), "/nope", "the input is kept for editing");
    assert_eq!(slash_command("/usr/bin/ls -la"), None);
    assert_eq!(slash_command("/review a b"), Some(("review", "a b")));
    assert_eq!(slash_command("/"), None);

    // The picker puts the command into the input, ready for arguments.
    let events = panel.handle_status_action(PROMPTS_ACTION);
    let picker = events.first().expect("picker");
    let PanelEvent::ShowSelect { options, .. } = picker else {
        panic!("expected a picker, got {picker:?}");
    };
    assert_eq!(options, &["/review <path> · Review a file"]);
    select(&mut panel, picker, 0);
    assert_eq!(panel.input_text(), "/review ");
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

#[test]
fn slash_runs_a_skill_after_templates_and_by_its_prefix() {
    let mut panel = skilled_panel(None);
    assert_eq!(send(&mut panel, "/deploy v1"), "Ship v1.");
    assert_eq!(send(&mut panel, "/review a.rs"), "Review a.rs carefully.");
    assert_eq!(
        send(&mut panel, "/skill:review a.rs"),
        "Skill review of a.rs."
    );
    assert_eq!(
        slash_command("/skill:review a"),
        Some(("skill:review", "a"))
    );
    assert_eq!(slash_command("/skill:"), None);
    assert_eq!(slash_command("/other:review"), None);
}

#[test]
fn an_expanded_command_keeps_what_was_typed() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Skilled::new()),
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("ok")])
    });
    send(&mut panel, "/deploy v1");
    assert!(
        panel.transcript().items().iter().any(|item| matches!(
            item,
            Item::User { text, command: Some(command), .. }
                if text == "Ship v1." && command == "/deploy v1"
        )),
        "{:?}",
        panel.transcript().items()
    );
    // The input history recalls the command, not its expansion.
    assert_eq!(panel.history(), ["/deploy v1"]);

    // The log keeps it, so a reopened session shows the same headline.
    let path = panel
        .session
        .as_ref()
        .expect("a session")
        .path()
        .to_path_buf();
    let reopened = Session::open(&path).unwrap();
    let mut transcript = Transcript::default();
    for logged in &reopened.context_messages_with_times(&CompactionPrompts::default()) {
        push_history(&mut transcript, logged);
    }
    assert!(transcript.items().iter().any(|item| matches!(
        item,
        Item::User { command: Some(command), .. } if command == "/deploy v1"
    )));
}

#[test]
fn slash_completion_offers_a_hidden_skill_by_its_prefix() {
    let mut panel = skilled_panel(None);
    type_text(&mut panel, "/");
    let list = panel.completion.as_ref().expect("a completion popup");
    let names: Vec<&str> = list.items().iter().map(|i| i.value.as_str()).collect();
    assert!(names.contains(&"deploy"), "{names:?}");
    assert!(names.contains(&"review") && names.contains(&"skill:review"));
}

#[test]
fn shadowed_names_are_reported_when_the_panel_opens() {
    // A fresh session says so under its banner, which stays up.
    let fresh = skilled_panel(None);
    assert!(fresh.banner_shown());
    assert!(fresh.transcript().items().iter().any(|item| matches!(
        item,
        Item::Notice { text, kind: NoticeKind::Warn } if text.contains("/skill:review")
    )));

    // A resumed session has no banner; the warning is a notice all the same.
    let dir = tempfile::tempdir().unwrap();
    let mut session = Session::create(dir.path(), dir.path()).unwrap();
    session
        .append_timed_message(&Message::User(UserMessage::text("hi")), None)
        .unwrap();
    let resumed = skilled_panel(Some(Session::open(session.path()).unwrap()));
    assert!(!resumed.banner_shown());
    assert!(resumed.transcript().items().iter().any(|item| matches!(
        item,
        Item::Notice { text, .. } if text.starts_with("/review")
    )));
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

#[test]
fn late_tools_join_the_registry_and_failures_are_reported() {
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        ..setup(vec![])
    });
    tx.send(LateTools::Ready {
        source: "github".into(),
        tools: vec![Arc::new(Late("github__search"))],
    })
    .unwrap();
    tx.send(LateTools::Failed {
        source: "ghost".into(),
        error: "cannot start ghost-server".into(),
    })
    .unwrap();
    let events = panel.tick();
    assert!(!events.is_empty());
    assert!(panel.tools.get("github__search").is_some());
    let notices: Vec<String> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|item| match item {
            Item::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        notices,
        [
            "mcp github: 1 tools connected",
            "mcp ghost: cannot start ghost-server"
        ]
    );
    // The worker got them too: the next run sees the tool in its registry.
    let runtime = std::mem::replace(&mut panel.runtime, Box::new(Idle));
    let agent = runtime.into_agent().expect("agent");
    assert!(agent.tools().get("github__search").is_some());
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

#[test]
fn a_server_s_new_set_replaces_its_old_one_and_a_removed_server_takes_its_tools() {
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        ..setup(vec![])
    });
    let ready = |names: &[&'static str]| LateTools::Ready {
        source: "github".into(),
        tools: names
            .iter()
            .map(|name| Arc::new(Late(name)) as Arc<dyn termide_agent_core::Tool>)
            .collect(),
    };
    tx.send(ready(&["github__a", "github__b"])).unwrap();
    panel.tick();
    // The server changed its list: `a` leaves, `c` arrives, `b` stays.
    tx.send(ready(&["github__b", "github__c"])).unwrap();
    panel.tick();
    assert!(panel.tools.get("github__a").is_none());
    assert!(panel.tools.get("github__b").is_some());
    assert!(panel.tools.get("github__c").is_some());
    assert_eq!(panel.mcp_arrived.len(), 2);
    // Under the banner the server's one line says where it stands now.
    assert_eq!(notices(&panel), ["mcp github: 2 tools connected"]);

    // Dropped from the configuration: every tool of it goes, the worker's too.
    tx.send(LateTools::Gone {
        source: "github".into(),
    })
    .unwrap();
    panel.tick();
    assert!(panel.tools.get("github__b").is_none() && panel.mcp_arrived.is_empty());
    assert_eq!(
        notices(&panel).last().unwrap(),
        "mcp github: removed from the configuration"
    );
    let runtime = std::mem::replace(&mut panel.runtime, Box::new(Idle));
    let agent = runtime.into_agent().expect("agent");
    assert!(agent.tools().get("github__b").is_none());
    assert!(agent.tools().get("github__c").is_none());
}

#[test]
fn a_server_s_line_tracks_the_toolset_under_the_banner_and_logs_it_after() {
    let (tx, rx) = mpsc::channel();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        catalog: Arc::new(Servers(Arc::clone(&asked))),
        ..setup(vec![])
    });
    let ready = || LateTools::Ready {
        source: "github".into(),
        tools: vec![
            Arc::new(Late("github__a")) as Arc<dyn termide_agent_core::Tool>,
            Arc::new(Late("github__b")),
        ],
    };
    // A built-in tool beside the server's, as an agent offers one.
    panel.offered_tools = vec!["bash".into()];
    tx.send(ready()).unwrap();
    panel.tick();
    let keys = |panel: &AgentPanel| -> Vec<String> {
        panel
            .toolset_items()
            .into_iter()
            .map(|item| item.key)
            .collect()
    };
    let all = keys(&panel);
    let without = |gone: &[&str]| -> Vec<String> {
        all.iter()
            .filter(|key| !gone.contains(&key.as_str()))
            .cloned()
            .collect()
    };
    let before = notices(&panel).len();

    // Under the banner the server keeps one line, rewritten in place.
    panel.apply_toolset(&without(&["github__a", "github__b"]));
    assert_eq!(notices(&panel).len(), before);
    assert!(notices(&panel).contains(&"mcp github: 0 of 2 tools on".to_string()));
    panel.apply_toolset(&all);
    assert!(notices(&panel).contains(&"mcp github: 2 tools connected".to_string()));
    panel.press_toolset_button("mcp-reload:github");
    assert!(notices(&panel).contains(&"mcp github: connecting".to_string()));
    tx.send(ready()).unwrap();
    panel.tick();
    assert_eq!(notices(&panel).len(), before);
    assert!(notices(&panel).contains(&"mcp github: 2 tools connected".to_string()));

    // In a conversation every change is a line of its own.
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let builtin = all
        .iter()
        .find(|key| !key.starts_with("github__"))
        .cloned()
        .expect("a built-in tool");
    panel.apply_toolset(&without(&["github__a", builtin.as_str()]));
    let seen = notices(&panel);
    let tail = &seen[seen.len() - 2..];
    assert_eq!(tail[0], "mcp github: 1 of 2 tools on");
    assert_eq!(tail[1], format!("Tools switched off: {builtin}; on: —"));
    panel.press_toolset_button("mcp-reload:github");
    tx.send(ready()).unwrap();
    panel.tick();
    assert_eq!(
        notices(&panel).last().unwrap(),
        "mcp github: reconnected, 2 tools"
    );
    assert_eq!(
        *asked.lock().unwrap(),
        ["reconnect github", "reconnect github"]
    );
}

#[test]
fn a_sign_in_is_asked_for_and_its_address_shown() {
    let (tx, rx) = mpsc::channel();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        late_tools: Some(rx),
        ..setup(vec![])
    });
    tx.send(LateTools::NeedsLogin {
        source: "plane".into(),
    })
    .unwrap();
    panel.tick();
    assert_eq!(
        notices(&panel),
        ["mcp plane: needs sign-in — /mcp login plane"]
    );
    // The sign-in under way takes the server's line, with the address.
    tx.send(LateTools::LoginStarted {
        source: "plane".into(),
        url: "https://as.example/authorize?x=1".into(),
    })
    .unwrap();
    panel.tick();
    let seen = notices(&panel);
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].contains("https://as.example/authorize?x=1"),
        "{seen:?}"
    );
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
            },
            McpServerState {
                name: "plane".into(),
                status: McpStatus::NeedsLogin,
                sign_in: McpSignIn::SignedOut,
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

#[test]
fn mcp_lists_reloads_and_signs_in_and_out() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Servers(Arc::clone(&asked))),
        ..setup(vec![])
    });
    for command in [
        "/mcp",
        "/mcp reload",
        "/mcp login plane",
        "/mcp logout plane",
        "/mcp what",
    ] {
        type_text(&mut panel, command);
        panel.submit();
    }
    assert_eq!(
        *asked.lock().unwrap(),
        ["reload", "login plane", "logout plane"]
    );
    assert_eq!(
        notices(&panel),
        [
            "mcp db: 4 tools",
            "mcp plane: needs sign-in",
            "MCP configuration reloaded — connecting: new; removed: —; unchanged: db",
            "mcp plane: signed out",
            "Usage: /mcp [reload [<server>] | login <server> | logout <server>]",
        ]
    );
    // Nothing of it went to the model.
    assert!(!panel.is_busy());
}

#[test]
fn the_toolset_list_heads_every_server_with_its_buttons() {
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        catalog: Arc::new(Servers(Arc::clone(&asked))),
        ..setup(vec![])
    });
    let events = panel.handle_status_action(crate::toolset::TOOLSET_ACTION);
    let Some(PanelEvent::ShowChecklist { groups, prompt, .. }) = events.first() else {
        panic!("the toolset list opens");
    };
    assert!(prompt.contains("r reconnects"), "{prompt}");
    let summary: Vec<(String, String, Vec<String>)> = groups
        .iter()
        .map(|g| {
            (
                g.name.clone(),
                g.note.clone(),
                g.buttons
                    .iter()
                    .map(|b| format!("{}{}", b.key, b.id))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "MCP: db".into(),
                String::new(),
                vec!["rmcp-reload:db".into()]
            ),
            (
                "MCP: plane".into(),
                "needs sign-in".into(),
                vec!["rmcp-reload:plane".into(), "lmcp-login:plane".into()]
            ),
        ]
    );
    // A button applies the ticks and does what it says.
    panel.handle_command(PanelCommand::ChecklistDone {
        action: crate::toolset::TOOLSET_ACTION.into(),
        checked: Vec::new(),
        pressed: Some("mcp-login:plane".into()),
    });
    panel.handle_command(PanelCommand::ChecklistDone {
        action: crate::toolset::TOOLSET_ACTION.into(),
        checked: Vec::new(),
        pressed: Some("mcp-reload:db".into()),
    });
    // The same one server from the input.
    type_text(&mut panel, "/mcp reload db");
    panel.submit();
    assert_eq!(
        *asked.lock().unwrap(),
        ["login plane", "reconnect db", "reconnect db"]
    );
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

/// An "external agent" for tests: answers every prompt with one text
/// message, through the same events as the real ACP backend.
struct External {
    events: Mutex<Vec<AgentEvent>>,
}

impl External {
    fn new(_setup: BackendSetup) -> Self {
        Self {
            events: Mutex::new(Vec::new()),
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
}

#[test]
fn an_external_agent_replaces_the_loop_and_hides_its_knobs() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        ..setup(vec![reply("native")])
    });
    type_text(&mut panel, "hello");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);

    assert!(panel.switch_agent("outside"));
    assert_eq!(chip(&panel, AGENT_ACTION), "outside");
    let texts: Vec<String> = panel
        .status_segments()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert!(!texts.iter().any(|t| t.starts_with("Mode")), "{texts:?}");
    assert!(!texts.iter().any(|t| t.starts_with("Model")), "{texts:?}");
    assert!(texts.contains(&" (acp)".to_string()));
    // The earlier conversation is shown, and marked as unknown to the agent.
    let items = panel.transcript().items();
    assert!(matches!(&items[0], Item::User { text, .. } if text == "hello"));
    assert!(items.iter().any(
            |i| matches!(i, Item::Notice { text, .. } if text.contains("not known to the external agent"))
        ));

    // Model and mode are not ours any more.
    let events = panel.handle_status_action(MODE_ACTION);
    assert!(!events
        .iter()
        .any(|e| matches!(e, PanelEvent::ShowSelect { .. })));
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(panel.mode.get(), Mode::Configured);

    type_text(&mut panel, "go");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Assistant { text, .. } if text == "from outside")));
    // It reports no tokens and times no prefill: no made-up indicators.
    assert!(panel.transcript().items().iter().all(
        |i| !matches!(i, Item::Assistant { text, cost: Some(_), .. } if text == "from outside")
    ));
    // The log records both the switch and the external agent's answer.
    let session = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(session.current_agent().as_deref(), Some("outside"));
    assert_eq!(session.context_messages().len(), 4);

    // Back to the built-in loop.
    assert!(panel.switch_agent("default"));
    assert!(!panel.external);
    assert_eq!(chip(&panel, MODE_ACTION), "configured");
}

/// An external agent that advertises two models and records the one picked.
struct ModelBackend {
    picked: Arc<Mutex<Option<String>>>,
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

#[test]
fn an_external_agent_that_follows_the_mode_gets_termides_prompt_tools_and_mode() {
    let seen: Arc<Mutex<Option<ModeBackend>>> = Arc::new(Mutex::new(None));
    let modes = Arc::new(Mutex::new(Vec::new()));
    let (for_factory, modes_for_factory) = (Arc::clone(&seen), Arc::clone(&modes));
    let mut panel = AgentPanel::new(AgentPanelSetup {
        backend: Some(Arc::new(move |setup: BackendSetup| {
            let backend = |modes| ModeBackend {
                modes,
                setup_mode: setup.mode.get(),
                prompt: setup.system_prompt.clone(),
                tools: setup
                    .host_tools
                    .as_ref()
                    .map(|host| host.tools.names().iter().map(|n| n.to_string()).collect())
                    .unwrap_or_default(),
            };
            *for_factory.lock().unwrap() = Some(backend(Arc::new(Mutex::new(Vec::new()))));
            Ok(Box::new(backend(Arc::clone(&modes_for_factory))) as Box<dyn Backend>)
        })),
        ..setup(vec![])
    });
    assert!(panel.external);
    {
        let seen = seen.lock().unwrap();
        let handed = seen.as_ref().unwrap();
        assert_eq!(handed.setup_mode, Mode::Configured);
        assert_eq!(handed.prompt, panel.system_prompt);
        assert_eq!(
            handed.tools,
            panel
                .tools
                .names()
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
        );
    }
    // The context's fill and size come from the agent's report.
    panel.tick();
    assert_eq!(
        (panel.context_tokens, panel.model.context_window),
        (975, 1_000_000)
    );
    // Its own loop cannot pause between steps: a run shows stop alone.
    panel.busy = true;
    assert_eq!(panel.run_buttons(), vec![RunButton::Stop]);
    assert!(!panel.request_pause());
    panel.busy = false;
    // The Mode chip is there, and a switch reaches the agent.
    assert_eq!(chip(&panel, MODE_ACTION), "configured");
    panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(*modes.lock().unwrap(), vec![Mode::Auto]);
}

#[test]
fn token_totals_keep_the_cache_apart_from_what_is_billed_in_full() {
    let mut billed = panel(vec![]);
    billed.apply(AgentEvent::AgentStart);
    billed.apply(AgentEvent::MessageEnd(Message::Assistant(
        AssistantMessage {
            usage: Usage {
                input: 4,
                output: 63,
                cache_read: 919,
                cache_write: 1009,
            },
            ..reply("ok")
        },
    )));
    // Written to the cache is billed in full; read from it is apart.
    assert_eq!(billed.token_totals(), "↑1k ↻919 ↓63");
    let texts: Vec<String> = billed
        .status_segments()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert!(texts.iter().any(|t| t.contains("↻919")), "{texts:?}");
    // No cache, no cache figure.
    let mut plain = panel(vec![]);
    plain.session_input = 12;
    plain.session_output = 3;
    assert_eq!(plain.token_totals(), "↑12 ↓3");
}

#[test]
fn an_external_agent_lists_and_switches_models() {
    let dir = tempfile::tempdir().unwrap();
    let picked = Arc::new(Mutex::new(None));
    let for_factory = Arc::clone(&picked);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        backend: Some(Arc::new(move |_setup: BackendSetup| {
            Ok(Box::new(ModelBackend {
                picked: Arc::clone(&for_factory),
            }) as Box<dyn Backend>)
        })),
        ..setup(vec![])
    });
    assert!(panel.external);
    // A tick adopts the advertised current model for the banner and chip.
    panel.tick();
    assert_eq!(panel.model.id, "m-fast");
    let texts: Vec<String> = panel
        .status_segments()
        .into_iter()
        .map(|s| s.text)
        .collect();
    assert!(texts.iter().any(|t| t == "m-fast"), "{texts:?}");

    // The Model chip opens the agent's model list.
    let events = panel.handle_status_action(MODEL_ACTION);
    let Some(PanelEvent::ShowSelect { options, .. }) = events.first() else {
        panic!("expected a model picker, got {events:?}");
    };
    assert_eq!(options.len(), 2);
    assert!(options.iter().any(|o| o.contains("Slow")), "{options:?}");

    // Picking the second switches it over ACP and updates the chip.
    panel.handle_command(PanelCommand::SelectionMade {
        action: MODEL_ACTION.to_string(),
        index: 1,
    });
    assert_eq!(*picked.lock().unwrap(), Some("m-slow".to_string()));
    assert_eq!(panel.model.id, "m-slow");
}

#[test]
fn a_cli_provider_pre_selects_its_configured_model_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let picked = Arc::new(Mutex::new(None));
    let for_factory = Arc::clone(&picked);
    let mut base = setup(vec![]);
    // A CLI provider with a configured model to pre-select.
    base.provider_kind = "claude_code".into();
    base.model = ModelSpec {
        id: "m-slow".into(),
        ..base.model
    };
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        backend: Some(Arc::new(move |_setup: BackendSetup| {
            Ok(Box::new(ModelBackend {
                picked: Arc::clone(&for_factory),
            }) as Box<dyn Backend>)
        })),
        ..base
    });
    assert!(panel.external);
    // The agent starts on "m-fast"; a tick applies the configured "m-slow".
    panel.tick();
    assert_eq!(*picked.lock().unwrap(), Some("m-slow".to_string()));
    assert_eq!(panel.model.id, "m-slow");
}

#[test]
fn arrow_keys_recall_earlier_requests_and_bring_the_draft_back() {
    let mut panel = panel(vec![reply("a"), reply("b")]);
    for request in ["first request", "second request"] {
        type_text(&mut panel, request);
        panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
        settle(&mut panel);
    }
    type_text(&mut panel, "half typ");
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "second request");
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "first request");
    // Past the oldest it stays; back down it returns to the draft.
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "first request");
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "half typ");
    assert!(panel.history_pos.is_none());
    // Typing ends browsing; the arrows then move inside a multi-line input.
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::SHIFT));
    type_text(&mut panel, "more");
    panel.handle_key(chord(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "half typ\nmore");
    assert_eq!(format_tokens(32_000), "32k");
    assert_eq!(format_tokens(262_144), "262k");
    assert_eq!(format_tokens(1_000_000), "1M");
    assert_eq!(format_tokens(1_250_000), "1.2M");
    assert_eq!(format_tokens(512), "512");
}

#[test]
fn typing_a_slash_offers_templates_and_tab_or_enter_completes() {
    let mut panel = panel(vec![reply("ok")]);
    type_text(&mut panel, "/re");
    let popup = panel.completion.as_ref().expect("popup");
    assert_eq!(popup.items()[0].value, "review");
    let rows = render_text(&mut panel, 60, 12);
    assert!(
        rows.iter()
            .any(|r| r.contains("/review <path>  Review a file")),
        "{rows:?}"
    );
    // Tab completes and closes the popup; the space invites arguments.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "/review ");
    assert!(panel.completion.is_none());

    // Enter on a partial name completes; on the full name it sends.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    type_text(&mut panel, "/rev");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "/review ");
    assert!(panel.transcript().items().is_empty());
    panel.handle_key(chord(KeyCode::Backspace, KeyModifiers::NONE));
    assert!(
        panel.completion.is_some(),
        "a lone /review shows the popup again"
    );
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert!(matches!(
        panel.transcript().items().first(),
        Some(Item::User { text, .. }) if text == "Review  carefully."
    ));

    // No match, no popup; Esc closes an open one.
    type_text(&mut panel, "/zzz");
    assert!(panel.completion.is_none());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    type_text(&mut panel, "/r");
    assert!(panel.completion.is_some());
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.completion.is_none());
    assert_eq!(
        panel.input_text(),
        "/r",
        "Esc closes the popup, not the input"
    );
}
#[test]
fn slash_compact_is_built_in_and_reports_through_the_transcript() {
    let mut panel = panel(vec![]);
    type_text(&mut panel, "/comp");
    let popup = panel.completion.as_ref().expect("popup");
    assert!(popup.items().iter().any(|i| i.value == "compact"));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "/compact ");
    type_text(&mut panel, "the tests");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "", "the command is consumed, not sent");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        panel.tick();
        let failed = panel.transcript().items().iter().any(
            |item| matches!(item, Item::Notice { text, .. } if text.contains("too few messages")),
        );
        if failed {
            break;
        }
        assert!(Instant::now() < deadline, "no compaction notice");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(panel
        .transcript()
        .items()
        .iter()
        .all(|i| !matches!(i, Item::User { .. })));
}
#[cfg(unix)]
#[test]
fn command_scripts_run_and_a_project_one_asks_first() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let script = |name: &str, body: &str, trusted: bool| {
        let path = dir.path().join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        CommandScript::from_file(&path, trusted).unwrap()
    };
    let gather = script(
            "gather",
            "#!/bin/sh\n# description: Gather context\n# argument-hint: <topic>\necho \"Context about $1\"\n",
            true,
        );
    let project = script(
        "scan",
        "#!/bin/sh\n# description: Scan\necho scanned\n",
        false,
    );
    *COMMANDS.lock().unwrap() = vec![gather, project];
    let mut panel = panel(vec![reply("a"), reply("b")]);
    let wait_user = |panel: &mut AgentPanel, text: &str| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            panel.tick();
            if panel
                .transcript()
                .items()
                .iter()
                .any(|i| matches!(i, Item::User { text: t, .. } if t == text))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{:?}",
                panel.transcript().items()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    };

    // The trusted script runs unasked and its output is the request.
    type_text(&mut panel, "/ga");
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "gather"));
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    type_text(&mut panel, "parsers");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(panel.input_text(), "");
    wait_user(&mut panel, "Context about parsers");
    settle(&mut panel);

    // The project's script asks; "run for this session" runs it now and
    // next time without asking.
    type_text(&mut panel, "/scan");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let form = panel.pending.as_ref().expect("a card asks").form();
    assert!(form.title().starts_with("Run the project command /scan"));
    assert_eq!(form.options().len(), 4);
    panel.handle_key(chord(KeyCode::Char('2'), KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    wait_user(&mut panel, "scanned");
    settle(&mut panel);
    assert!(panel.allowed_commands.contains("scan"));

    // "Don't run" leaves nothing behind.
    *COMMANDS.lock().unwrap() = vec![script("other", "#!/bin/sh\necho x\n", false)];
    type_text(&mut panel, "/other");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.pending.is_some());
    panel.handle_key(chord(KeyCode::Char('4'), KeyModifiers::NONE));
    assert!(panel.pending.is_none() && panel.command_run.is_none());
    *COMMANDS.lock().unwrap() = Vec::new();
}
#[test]
fn undo_restores_the_files_and_rewinds_the_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let sessions = dir.path().join("sessions");
    let file = dir.path().join("notes.txt");
    std::fs::write(&file, "before").unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        cwd: dir.path().to_path_buf(),
        session_dir: Some(sessions),
        ..setup(vec![reply("a"), reply("b"), reply("c")])
    });
    type_text(&mut panel, "task one");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    // Nothing changed files yet: /undo has nothing to offer.
    type_text(&mut panel, "/undo");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert!(panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::Notice { text, .. } if text.contains("nothing to undo"))));

    // The second request "edits" the file: the store records it the way
    // the hook does for the edit tool.
    let leaf = panel
        .session
        .as_ref()
        .unwrap()
        .leaf_id()
        .map(str::to_string);
    {
        let store = panel.checkpoints.as_ref().unwrap();
        let mut store = store.lock().unwrap();
        store.begin_run(leaf);
        store.save(&file).unwrap();
        std::fs::write(&file, "after").unwrap();
        store.end_run();
    }
    type_text(&mut panel, "task two");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    assert_eq!(panel.session.as_ref().unwrap().context_messages().len(), 4);

    type_text(&mut panel, "/un");
    assert!(panel
        .completion
        .as_ref()
        .unwrap()
        .items()
        .iter()
        .any(|i| i.value == "undo"));
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE)); // completes
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE)); // sends /undo
    let form = panel.pending.as_ref().expect("undo card").form();
    assert!(form.title().contains("notes.txt"), "{}", form.title());
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    let events = panel.tick();
    assert!(events
        .iter()
        .any(|e| matches!(e, PanelEvent::FileChangedOnDisk(p) if p == &file)));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before");
    // The conversation is back at the end of task one, on disk too.
    assert_eq!(panel.session.as_ref().unwrap().context_messages().len(), 2);
    assert!(!panel
        .transcript()
        .items()
        .iter()
        .any(|i| matches!(i, Item::User { text, .. } if text == "task two")));
    assert!(panel.transcript().items().iter().any(
        |i| matches!(i, Item::Notice { text, .. } if text.contains("undid the last request"))
    ));
    let reopened = Session::open(panel.session_path().unwrap()).unwrap();
    assert_eq!(reopened.context_messages().len(), 2);
}
#[test]
fn plan_mode_adds_its_instructions_and_offers_to_carry_the_plan_out() {
    let dir = tempfile::tempdir().unwrap();
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session_dir: Some(dir.path().to_path_buf()),
        system_prompt: "Base prompt.".into(),
        plan_prompt: PlanPrompt::from_file("---\nrequest: Do it.\n---\nPlan first."),
        ..setup(vec![reply("1. change a\n2. change b"), reply("done")])
    });
    // configured → auto → all → ask → plan
    for _ in 0..4 {
        panel.handle_key(chord(KeyCode::BackTab, KeyModifiers::SHIFT));
    }
    assert_eq!(panel.mode.get(), Mode::Plan);
    assert_eq!(chip(&panel, MODE_ACTION), "plan");
    let shown = panel.write_system_prompt().unwrap();
    assert_eq!(
        std::fs::read_to_string(shown).unwrap(),
        "Base prompt.\n\nPlan first."
    );

    type_text(&mut panel, "add a feature");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let form = panel.pending.as_ref().expect("plan card").form();
    assert!(form.title().starts_with("Plan mode"), "{}", form.title());

    // Esc keeps planning; the next answer offers again.
    panel.handle_key(chord(KeyCode::Esc, KeyModifiers::NONE));
    assert!(panel.pending.is_none());
    assert_eq!(panel.mode.get(), Mode::Plan);

    panel.offer_plan();
    panel.handle_key(chord(KeyCode::Char('1'), KeyModifiers::NONE));
    let _ = panel.tick();
    assert_eq!(panel.mode.get(), Mode::Edit);
    settle(&mut panel);
    let users: Vec<&str> = panel
        .transcript()
        .items()
        .iter()
        .filter_map(|i| match i {
            Item::User { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(users, ["add a feature", "Do it."]);
    let shown = panel.write_system_prompt().unwrap();
    assert_eq!(std::fs::read_to_string(shown).unwrap(), "Base prompt.");
    assert!(panel.pending.is_none(), "no card outside plan mode");
}
