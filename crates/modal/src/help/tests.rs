use std::sync::Once;

use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

use super::*;

static INIT_I18N: Once = Once::new();

fn config() -> Config {
    INIT_I18N.call_once(|| {
        let _ = i18n::init();
    });
    let mut config = Config::default();
    config.normalize();
    config
}

fn key(code: KeyCode, modifiers: KeyModifiers) -> termide_core::KeyChord {
    termide_core::KeyChord::identity(KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    })
}

fn press(code: KeyCode) -> termide_core::KeyChord {
    key(code, KeyModifiers::empty())
}

fn screen() -> Rect {
    Rect::new(0, 0, 100, 30)
}

/// Render `modal` and return what the screen shows.
fn draw(modal: &mut HelpModal) -> String {
    let mut buf = Buffer::empty(screen());
    modal.render(screen(), &mut buf, &Theme::default());
    let area = buf.area;
    (area.y..area.y + area.height)
        .map(|y| {
            (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn section_of(config: &Config, panel: &str) -> usize {
    HelpGenerator::section_for_panel(&HelpGenerator::generate(config), panel).unwrap()
}

#[test]
fn it_opens_on_the_section_of_the_panel_it_was_called_from() {
    let config = config();
    let mut modal = HelpModal::new(&config, Some("git_status"));
    draw(&mut modal);
    assert!(modal.scroll > 0, "scrolled down to the section");
    assert_eq!(modal.current_section(), section_of(&config, "git_status"));

    let mut modal = HelpModal::new(&config, Some("help"));
    draw(&mut modal);
    assert_eq!(
        modal.scroll, 0,
        "a panel without a section opens at the top"
    );
}

#[test]
fn enter_asks_for_the_panel_at_the_section_in_view() {
    let config = config();
    let mut modal = HelpModal::new(&config, Some("editor"));
    draw(&mut modal);
    let result = modal.handle_key(press(KeyCode::Enter)).unwrap();
    assert!(matches!(
        result,
        Some(ModalResult::Confirmed(at)) if at == section_of(&config, "editor")
    ));
}

#[test]
fn tab_steps_from_section_to_section() {
    let config = config();
    let mut modal = HelpModal::new(&config, None);
    draw(&mut modal);
    assert_eq!(modal.current_section(), 0);
    modal.handle_key(press(KeyCode::Tab)).unwrap();
    assert_eq!(modal.current_section(), 1);
    modal.handle_key(press(KeyCode::Tab)).unwrap();
    modal.handle_key(press(KeyCode::BackTab)).unwrap();
    assert_eq!(modal.current_section(), 1);
}

#[test]
fn typing_narrows_the_help_and_backspace_widens_it() {
    let config = config();
    let mut modal = HelpModal::new(&config, None);
    let all = draw(&mut modal);
    let lines = |modal: &HelpModal| modal.shown.as_ref().unwrap().lines.len();
    let before = lines(&modal);
    for c in "stage".chars() {
        modal.handle_key(press(KeyCode::Char(c))).unwrap();
    }
    let narrowed = draw(&mut modal);
    assert!(lines(&modal) < before);
    assert!(narrowed.contains("› stage"));
    assert_ne!(narrowed, all);

    for c in "zzqx".chars() {
        modal.handle_key(press(KeyCode::Char(c))).unwrap();
    }
    assert!(draw(&mut modal).contains(i18n::t().editor_search_no_matches()));

    modal
        .handle_key(key(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    draw(&mut modal);
    assert_eq!(lines(&modal), before);
}

#[test]
fn the_help_key_and_escape_close_it() {
    let config = config();
    let mut modal = HelpModal::new(&config, None);
    draw(&mut modal);
    let closed = |r: Option<ModalResult<usize>>| matches!(r, Some(ModalResult::Cancelled));
    assert!(closed(modal.handle_key(press(KeyCode::F(1))).unwrap()));
    assert!(closed(modal.handle_key(press(KeyCode::Esc)).unwrap()));
}

#[test]
fn the_filter_keeps_matching_entries_and_whole_matching_sections() {
    let config = config();
    let sections = HelpGenerator::generate(&config);
    let theme = Theme::default();
    let all = HelpGenerator::format(&sections, 80, &theme, "");
    assert_eq!(all.anchors.len(), sections.len());

    // A section's header keeps all of it, in any case.
    let header = sections[1].header.to_lowercase();
    let by_header = HelpGenerator::format(&sections, 80, &theme, &header);
    assert!(by_header.anchors.iter().any(|(s, _)| *s == 1));

    let none = HelpGenerator::format(&sections, 80, &theme, "no such key anywhere");
    assert!(none.anchors.is_empty());
    assert_eq!(none.section_at(0), None);
}
