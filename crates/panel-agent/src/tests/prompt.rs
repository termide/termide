//! Focus between the chat and the prompt, and selecting and editing in the prompt.

use super::*;

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

/// The prompt's cut and paste through the keys, on a stand-in clipboard so
/// the user's is never touched.
#[test]
fn ctrl_x_cuts_the_prompt_selection_and_ctrl_v_puts_it_back() {
    termide_ui::clipboard::use_fake_clipboard("");
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

    let events = panel.handle_key(chord(KeyCode::Char('v'), KeyModifiers::CONTROL));
    assert!(events.iter().any(|e| matches!(e, PanelEvent::NeedsRedraw)));
    assert_eq!(panel.input_text(), "fix the flaky test");
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
fn typing_on_a_chat_block_hands_focus_back_to_the_prompt() {
    let mut panel = panel(vec![reply("Hello from the model")]);
    type_text(&mut panel, "hi");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    assert!(panel.chat_focus);
    // A non-Latin key types its own character, not the canonical one the
    // chat's shortcuts match on.
    let event = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
    panel.handle_key(KeyChord {
        raw: event('р'),
        canonical: event('h'),
    });
    assert!(!panel.chat_focus);
    type_text(&mut panel, "ow");
    assert_eq!(panel.input_text(), "рow");
    // A chat shortcut stays a shortcut, and a Ctrl chord does not type.
    panel.handle_key(chord(KeyCode::Tab, KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Char(' '), KeyModifiers::NONE));
    panel.handle_key(chord(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(panel.chat_focus);
    assert_eq!(panel.input_text(), "рow");
}

#[test]
fn dragging_in_the_prompt_selects_and_hands_focus_back_from_the_chat() {
    let mut panel = panel(vec![reply("Hello from the model")]);
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let _ = render_text(&mut panel, 40, 12);
    // A draft in the prompt to drag a selection through, typed before the
    // chat takes focus.
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
