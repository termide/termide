//! Keys on other layouts, selecting and copying from the chat, links, the input size and pastes.

use super::*;

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
        name: "shell".into(),
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
        selection.text(
            panel.transcript.rendered(),
            panel.transcript.rendered_copy(),
            width,
        ),
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
fn a_copied_command_comes_out_whole_without_the_chat_decoration() {
    let command = "git push origin main --tags --force-with-lease";
    let mut panel = panel(vec![reply(&format!(
        "Run it yourself, as it needs your login:\n\n```sh\n{command}\nls\n```"
    ))]);
    type_text(&mut panel, "push");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let rows = render_text(&mut panel, 30, 30);
    let first = rows
        .iter()
        .position(|r| r.contains("┊ git"))
        .expect("the command is on screen");
    let last = rows
        .iter()
        .position(|r| r.contains("┊ ls"))
        .expect("the code block ends on screen");
    assert!(last > first + 1, "the command wraps: {rows:#?}");
    // Dragged from the left edge of the first row to the end of `ls` (the
    // block's byline sits right of it on the same row).
    let area = panel.transcript_area;
    let ls = &rows[last];
    let ls_end = ls[..ls.find("ls").unwrap()].chars().count() as u16 + 1;
    let mouse = |kind, column, row: usize| MouseEvent {
        kind,
        column,
        row: row as u16,
        modifiers: KeyModifiers::NONE,
    };
    let (left, right) = (area.x, ls_end);
    panel.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), left, first),
        area,
    );
    panel.handle_mouse(
        mouse(MouseEventKind::Drag(MouseButton::Left), right, last),
        area,
    );
    panel.handle_mouse(
        mouse(MouseEventKind::Up(MouseButton::Left), right, last),
        area,
    );
    let selection = panel.text_selection.expect("a selection");
    let width = panel.transcript_area.width.saturating_sub(1) as usize;
    assert_eq!(
        selection.text(
            panel.transcript.rendered(),
            panel.transcript.rendered_copy(),
            width,
        ),
        format!("{command}\nls")
    );
}

#[test]
fn a_ctrl_click_on_a_url_follows_it() {
    let mut panel = panel(vec![reply("See https://docs.rs now")]);
    type_text(&mut panel, "hi there");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let rows = render_text(&mut panel, 40, 12);
    let area = panel.transcript_area;
    let y = rows
        .iter()
        .position(|r| r.contains("https://docs.rs"))
        .expect("the URL is on screen") as u16;
    let row = &rows[y as usize];
    let x = row[..row.find("docs.rs").unwrap()].chars().count() as u16;
    let mouse = |kind, column, modifiers| MouseEvent {
        kind,
        column,
        row: y,
        modifiers,
    };
    let click = |panel: &mut AgentPanel, column, modifiers| {
        panel.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), column, modifiers),
            area,
        );
        panel.handle_mouse(
            mouse(MouseEventKind::Up(MouseButton::Left), column, modifiers),
            area,
        )
    };
    let opens = |events: &[PanelEvent]| events.iter().any(|e| matches!(e, PanelEvent::OpenLink(_)));
    let events = click(&mut panel, x, KeyModifiers::CONTROL);
    assert!(
        matches!(events.as_slice(), [PanelEvent::OpenLink(LinkTarget::Url(url))] if url == "https://docs.rs"),
        "{events:?}"
    );
    // Ctrl+Alt+click opens it outside instead.
    let events = click(&mut panel, x, KeyModifiers::CONTROL | KeyModifiers::ALT);
    assert!(
        matches!(events.as_slice(), [PanelEvent::OpenLinkExternal(LinkTarget::Url(url))] if url == "https://docs.rs"),
        "{events:?}"
    );
    // A plain click selects the block instead, and Ctrl+click beside the URL
    // opens nothing.
    assert!(!opens(&click(&mut panel, x, KeyModifiers::NONE)));
    assert!(!opens(&click(&mut panel, x - 9, KeyModifiers::CONTROL)));
}

#[test]
fn a_markdown_link_is_followed_and_lit_whole() {
    let mut panel = panel(vec![reply("Read [the docs](https://docs.rs/x) first")]);
    type_text(&mut panel, "hi there");
    panel.handle_key(chord(KeyCode::Enter, KeyModifiers::NONE));
    settle(&mut panel);
    let rows = render_text(&mut panel, 40, 12);
    let area = panel.transcript_area;
    let y = rows
        .iter()
        .position(|r| r.contains("the docs"))
        .expect("the link is on screen") as u16;
    let row = &rows[y as usize];
    let x = row[..row.find("the docs").unwrap()].chars().count() as u16;
    let mouse = |kind, column, modifiers| MouseEvent {
        kind,
        column,
        row: y,
        modifiers,
    };
    // Ctrl over "the" lights "the docs", both words of the one link.
    let events = panel.handle_mouse(mouse(MouseEventKind::Moved, x, KeyModifiers::CONTROL), area);
    assert!(
        matches!(events.as_slice(), [PanelEvent::NeedsRedraw]),
        "{events:?}"
    );
    let buf = render_buf(&mut panel, 40, 12);
    let info = ThemeColors::default().info;
    // Each word is lit; the space between them is not part of the link.
    for dx in (0..3).chain(4..8) {
        let cell = &buf[(x + dx, y)];
        assert!(
            cell.modifier.contains(Modifier::UNDERLINED) && cell.fg == info,
            "{dx}: {cell:?}"
        );
    }
    // Releasing Ctrl puts it out.
    panel.handle_mouse(mouse(MouseEventKind::Moved, x, KeyModifiers::NONE), area);
    assert!(panel.hovered_link.is_none());
    // Ctrl+click on "docs" follows the hidden address.
    panel.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            x + 5,
            KeyModifiers::CONTROL,
        ),
        area,
    );
    let events = panel.handle_mouse(
        mouse(
            MouseEventKind::Up(MouseButton::Left),
            x + 5,
            KeyModifiers::CONTROL,
        ),
        area,
    );
    assert!(
        matches!(events.as_slice(), [PanelEvent::OpenLink(LinkTarget::Url(url))] if url == "https://docs.rs/x"),
        "{events:?}"
    );
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
    // The headline's path, past its success-colored prefix.
    let y = row_of("a.rs");
    let x = (0..width)
        .find(|&x| buf[(x, y)].symbol() == "a" && buf[(x + 1, y)].symbol() == ".")
        .unwrap();
    assert_eq!(buf[(x, y)].bg, colors.fg);

    // A panel out of focus draws no cursor, so it does not draw the eye.
    let buf = render_buf_focused(&mut panel, width, height, false);
    assert!(
        (0..height).all(|y| (0..width).all(|x| buf[(x, y)].bg != colors.fg)),
        "no block shown inverted"
    );
}
