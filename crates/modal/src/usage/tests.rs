use std::path::PathBuf;
use std::sync::Once;

use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::*;

static INIT_I18N: Once = Once::new();

fn setup() {
    INIT_I18N.call_once(|| {
        let _ = i18n::init();
    });
}

fn press(code: KeyCode) -> termide_core::KeyChord {
    termide_core::KeyChord::identity(KeyEvent {
        code,
        modifiers: KeyModifiers::empty(),
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

fn click(column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

fn periods() -> Vec<String> {
    ["Today", "7 days", "30 days", "All time"]
        .map(String::from)
        .to_vec()
}

fn modal(session: bool) -> UsageModal {
    let lines = session.then(|| vec![UsageLine::new("Model", "big")]);
    let scope = if session {
        UsageScopeTab::Session
    } else {
        UsageScopeTab::Project
    };
    UsageModal::new("Usage", periods(), 1, lines, scope)
}

fn render(modal: &mut UsageModal, width: u16) -> String {
    let area = Rect::new(0, 0, width, 30);
    let mut buf = Buffer::empty(area);
    modal.render(area, &mut buf, &Theme::default());
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn sessions() -> Vec<UsageLine> {
    vec![
        UsageLine::new("Tokens", "↑1k"),
        UsageLine::new("Top sessions", "first").with_link(Some(PathBuf::from("/s/1.jsonl"))),
        UsageLine::new("", "elsewhere"),
        UsageLine::new("", "second").with_link(Some(PathBuf::from("/s/2.jsonl"))),
    ]
}

/// The session tab needs nothing more; the others ask for their lines, and
/// each change of scope or period asks again.
#[test]
fn the_window_asks_for_the_view_chosen() {
    setup();
    let mut usage = modal(true);
    assert_eq!(usage.wants(), None);
    assert!(render(&mut usage, 100).contains("Model  big"));
    usage.handle_key(press(KeyCode::Tab)).unwrap();
    let project = UsageView {
        scope: UsageScopeTab::Project,
        period: 1,
    };
    assert_eq!(usage.wants(), Some(project));
    usage.set_counting(3, 10);
    assert!(usage.is_counting());
    assert!(render(&mut usage, 100).contains("3 of 10"));
    usage.set_lines(project, sessions());
    assert_eq!(usage.wants(), None);
    usage.handle_key(press(KeyCode::Right)).unwrap();
    assert_eq!(usage.wants().map(|v| v.period), Some(2));
    // Lines of a view no longer chosen are dropped.
    usage.set_lines(project, sessions());
    assert!(usage.wants().is_some());
    // The period stops at the ends.
    for _ in 0..5 {
        usage.handle_key(press(KeyCode::Right)).unwrap();
    }
    assert_eq!(usage.view().period, 3);
    // Back to the session tab, then round past the last scope.
    usage.handle_key(press(KeyCode::Tab)).unwrap();
    usage.handle_key(press(KeyCode::Tab)).unwrap();
    assert_eq!(usage.view().scope, UsageScopeTab::Session);
    // Without a session there is no session tab.
    let usage = modal(false);
    assert_eq!(usage.view().scope, UsageScopeTab::Project);
    assert_eq!(usage.scopes.len(), 2);
}

/// ↑↓ go through the sessions that can be opened, skipping the others, and
/// Enter opens the one picked; with none picked Enter closes.
#[test]
fn a_picked_session_is_opened() {
    setup();
    let mut usage = modal(false);
    let view = usage.view();
    usage.set_lines(view, sessions());
    render(&mut usage, 100);
    assert!(matches!(
        usage.handle_key(press(KeyCode::Enter)).unwrap(),
        Some(ModalResult::Cancelled)
    ));
    usage.handle_key(press(KeyCode::Down)).unwrap();
    usage.handle_key(press(KeyCode::Down)).unwrap();
    usage.handle_key(press(KeyCode::Down)).unwrap();
    match usage.handle_key(press(KeyCode::Enter)).unwrap() {
        Some(ModalResult::Confirmed(path)) => assert_eq!(path, PathBuf::from("/s/2.jsonl")),
        other => panic!("{other:?}"),
    }
}

/// A click on a tab chooses it, one on a session opens it, one on a line
/// that opens nothing does nothing.
#[test]
fn clicks_choose_tabs_and_open_sessions() {
    setup();
    let mut usage = modal(true);
    let text = render(&mut usage, 100);
    let (row, line) = text
        .lines()
        .enumerate()
        .find(|(_, line)| line.contains("All projects"))
        .unwrap();
    let column = line.find("All projects").unwrap();
    let column = line[..column].chars().count() as u16;
    usage
        .handle_mouse(click(column + 1, row as u16), Rect::default())
        .unwrap();
    assert_eq!(usage.view().scope, UsageScopeTab::All);
    let view = usage.view();
    usage.set_lines(view, sessions());
    let text = render(&mut usage, 100);
    let row_of = |needle: &str| text.lines().position(|line| line.contains(needle)).unwrap() as u16;
    let x = 50;
    assert!(usage
        .handle_mouse(click(x, row_of("elsewhere")), Rect::default())
        .unwrap()
        .is_none());
    match usage
        .handle_mouse(click(x, row_of("first")), Rect::default())
        .unwrap()
    {
        Some(ModalResult::Confirmed(path)) => assert_eq!(path, PathBuf::from("/s/1.jsonl")),
        other => panic!("{other:?}"),
    }
}

/// On a narrow screen the periods move under the scopes, and on a narrower
/// one give way to the one chosen.
#[test]
fn a_narrow_window_keeps_the_periods_in_sight() {
    setup();
    let mut usage = modal(true);
    usage.handle_key(press(KeyCode::Tab)).unwrap();
    let text = render(&mut usage, 50);
    let lines: Vec<&str> = text.lines().collect();
    let scopes = lines
        .iter()
        .position(|l| l.contains("All projects"))
        .unwrap();
    assert!(lines[scopes + 1].contains("30 days"), "{text}");
    let mut usage = modal(true);
    usage.handle_key(press(KeyCode::Tab)).unwrap();
    let text = render(&mut usage, 34);
    assert!(text.contains("‹ 7 days ›"), "{text}");
    assert!(!text.contains("30 days"), "{text}");
}
