use std::sync::{Arc, Mutex};

use crossterm::event::KeyEvent;
use termide_debug::dap::{Adapter, Console, Frame, LaunchSpec, OutputLine};

use super::*;

fn variable(name: &str, value: &str, reference: i64, children: Option<Vec<Variable>>) -> Variable {
    Variable {
        name: name.into(),
        value: value.into(),
        type_name: None,
        reference,
        expanded: children.is_some(),
        children,
        expensive: false,
    }
}

fn stopped_session() -> SharedSession {
    let mut s = Session::detached(LaunchSpec {
        adapter: Adapter::LldbDap,
        debugger_settings: Default::default(),
        adapter_args: Vec::new(),
        program: "app".into(),
        args: Vec::new(),
        cwd: "/p".into(),
        build: None,
        env: Vec::new(),
        label: "app".into(),
        console: Console::Panel,
        tty: None,
    });
    s.status = Status::Stopped {
        reason: "breakpoint".into(),
        description: None,
    };
    s.frames = vec![
        Frame {
            id: 1,
            name: "app::main".into(),
            path: Some("/p/src/main.rs".into()),
            line: 4,
        },
        Frame {
            id: 2,
            name: "start".into(),
            path: None,
            line: 0,
        },
    ];
    s.scopes = vec![variable(
        "Locals",
        "",
        5,
        Some(vec![
            variable("a", "41", 0, None),
            variable("v", "size=2", 7, Some(vec![variable("[0]", "1", 0, None)])),
        ]),
    )];
    s.output.push_back(OutputLine {
        kind: OutputKind::Stdout,
        text: "hello".into(),
    });
    Arc::new(Mutex::new(Some(s)))
}

fn render(panel: &mut DebugPanel) -> Vec<String> {
    let area = Rect::new(0, 0, 50, 20);
    let mut buf = Buffer::empty(area);
    let colors = termide_core::ThemeColors::default();
    let ctx = RenderContext {
        theme: &colors,
        config: &termide_core::PanelConfig {
            tab_size: 4,
            word_wrap: false,
            show_line_numbers: false,
            show_hidden_files: false,
        },
        is_focused: true,
        panel_index: 0,
        terminal_width: 50,
        terminal_height: 20,
        border_right_x: None,
        border_bottom_y: None,
    };
    panel.render(area, &mut buf, &ctx);
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

fn key(code: KeyCode) -> KeyChord {
    KeyChord::identity(KeyEvent::new(code, KeyModifiers::NONE))
}

#[test]
fn the_tree_flattens_with_open_nodes_only() {
    let slot = stopped_session();
    let guard = slot.lock().unwrap();
    let rows = flatten(&guard.as_ref().unwrap().scopes);
    let names: Vec<(&str, usize)> = rows.iter().map(|r| (r.name.as_str(), r.depth)).collect();
    assert_eq!(names, [("Locals", 0), ("a", 1), ("v", 1), ("[0]", 2)]);
    assert_eq!(rows[3].path, [0, 1, 0]);
}

#[test]
fn the_panel_shows_where_it_stopped_the_stack_variables_and_output() {
    let mut panel = DebugPanel::new(stopped_session());
    let screen = render(&mut panel).join("\n");
    assert!(screen.contains("app::main  main.rs:4"), "{screen}");
    assert!(screen.contains("a = 41"), "{screen}");
    assert!(screen.contains("▾ v = size=2"), "{screen}");
    assert!(screen.contains("hello"), "{screen}");
}

#[test]
fn a_long_output_line_wraps_instead_of_being_cut() {
    let slot = stopped_session();
    let tail = "only accepted from the same UNIX user";
    slot.lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .output
        .push_back(OutputLine {
            kind: OutputKind::Console,
            text: format!("closing connection from different user (127.0.0.1:54426): {tail}"),
        });
    let mut panel = DebugPanel::new(slot);
    let screen = render(&mut panel).join("\n");
    assert!(screen.contains("closing connection"), "{screen}");
    assert!(screen.contains("UNIX user"), "{screen}");
    assert!(screen.contains("hello"), "{screen}");
}

#[test]
fn wrapping_counts_columns_not_bytes() {
    assert_eq!(wrap("", 4), [""]);
    assert_eq!(wrap("abcdef", 4), ["abcd", "ef"]);
    assert_eq!(wrap("абвгд", 2), ["аб", "вг", "д"]);
    assert_eq!(wrap("日本語", 4), ["日本", "語"]);
}

#[test]
fn left_closes_a_variable_and_then_goes_to_its_parent() {
    let slot = stopped_session();
    let mut panel = DebugPanel::new(Arc::clone(&slot));
    render(&mut panel);
    // Locals, a, v, [0]: select [0], then ← goes up to v, ← again closes it.
    for _ in 0..3 {
        panel.handle_key(key(KeyCode::Down));
    }
    panel.handle_key(key(KeyCode::Left));
    assert_eq!(panel.var_selected, 2);
    panel.handle_key(key(KeyCode::Left));
    let guard = slot.lock().unwrap();
    let v = &guard.as_ref().unwrap().scopes[0].children.as_ref().unwrap()[1];
    assert!(!v.expanded);
}

#[test]
fn e_asks_for_an_expression() {
    let mut panel = DebugPanel::new(stopped_session());
    let events = panel.handle_key(key(KeyCode::Char('e')));
    assert!(matches!(
        events.as_slice(),
        [PanelEvent::ShowInput { on_submit: InputAction::Custom(a), .. }] if a == EVALUATE
    ));
}

#[test]
fn r_asks_to_run_the_launch_again_and_leaves_the_session_to_the_app() {
    let slot = stopped_session();
    let mut panel = DebugPanel::new(Arc::clone(&slot));
    let events = panel.handle_key(key(KeyCode::Char('r')));
    assert!(matches!(events.as_slice(), [PanelEvent::RestartDebug]));
    // The app ends the session when it starts the next one, not the panel.
    assert!(matches!(
        slot.lock().unwrap().as_ref().unwrap().status,
        Status::Stopped { .. }
    ));
}

#[test]
fn closing_the_panel_ends_the_session() {
    let slot = stopped_session();
    drop(DebugPanel::new(Arc::clone(&slot)));
    assert_eq!(slot.lock().unwrap().as_ref().unwrap().status, Status::Ended);
}
