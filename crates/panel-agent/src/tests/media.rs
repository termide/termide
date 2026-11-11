//! Diagrams, pictures, written pages and terminals in the transcript.

use super::*;

#[test]
fn a_diagram_scrolled_out_of_view_stays_whole() {
    let mut panel = panel(vec![reply(DIAGRAM_REPLY), tall_reply()]);
    send(&mut panel, "draw");
    let _ = render_text(&mut panel, 60, 40);
    assert!(shows_diagram(&panel), "a fresh answer shows its diagram");

    send(&mut panel, "more");
    let _ = render_text(&mut panel, 60, 40);
    assert!(shows_diagram(&panel), "scrolled away, it does not fold");
}

#[test]
fn a_diagram_taller_than_half_the_panel_is_a_card_until_unfolded() {
    let mut panel = panel(vec![reply(DIAGRAM_REPLY)]);
    send(&mut panel, "draw");
    let _ = render_text(&mut panel, 60, 40);
    assert!(shows_diagram(&panel), "half a tall panel holds it");

    let _ = render_text(&mut panel, 60, 8);
    assert!(!shows_diagram(&panel), "half a short one does not");
    assert!(
        transcript_text(&panel)
            .iter()
            .any(|l| l.contains("▸ mermaid")),
        "{:?}",
        transcript_text(&panel)
    );

    let answer = panel
        .transcript()
        .items()
        .iter()
        .position(|item| matches!(item, Item::Assistant { .. }))
        .unwrap();
    assert!(panel.transcript.toggle_expanded(answer));
    let _ = render_text(&mut panel, 60, 8);
    assert!(shows_diagram(&panel), "one click shows it whole");
    assert!(panel.transcript.toggle_expanded(answer));
    let _ = render_text(&mut panel, 60, 8);
    assert!(!shows_diagram(&panel), "the next folds it");
}

#[test]
fn a_wide_diagram_gets_a_scrollbar_row_and_one_that_fits_does_not() {
    let (mut panel, spot) = wide_diagram_panel();
    assert_eq!((spot.col, spot.view), (4, 35));
    assert!(spot.cols > spot.view);
    let rows = render_text(&mut panel, 40, 40);
    let track = &rows[screen_row(&panel, spot.track()) as usize];
    assert!(track.starts_with("  ┊ ━"), "{rows:#?}");
    assert!(track.contains('─'), "the thumb is shorter than the track");
    assert_eq!(panel.diagram_bars.len(), 1);

    let mut narrow = panel_with_reply(DIAGRAM_REPLY);
    let rows = render_text(&mut narrow, 60, 40);
    assert!(shows_diagram(&narrow));
    assert!(narrow.transcript.wide_diagrams().is_empty());
    assert!(narrow.diagram_bars.is_empty());
    assert!(!rows.iter().any(|row| row.contains('━')), "{rows:#?}");
}

#[test]
fn panning_shifts_the_diagram_but_not_its_bar_and_copies_what_shows() {
    let (mut panel, spot) = wide_diagram_panel();
    let before = panel.transcript.rendered().to_vec();
    let y = screen_row(&panel, spot.line + 1) as usize;
    let unpanned = render_text(&mut panel, 40, 40)[y].clone();

    let shift = |code| chord(code, KeyModifiers::SHIFT);
    assert!(!panel.handle_key(shift(KeyCode::Right)).is_empty());
    assert_eq!(panel.transcript.pan_of(&spot), crate::pan::KEY_STEP);
    let panned = render_text(&mut panel, 40, 40)[y].clone();
    let cut = |row: &str, skip: usize| -> String {
        let chars: Vec<char> = row.chars().collect();
        chars[..4]
            .iter()
            .chain(&chars[4 + skip..39 + skip])
            .collect()
    };
    assert_eq!(&panned[..panned.char_indices().nth(4).unwrap().0], "  ┊ ");
    let full: String = before[spot.line + 1]
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect();
    let full = format!("{full:<200}");
    assert_eq!(
        panned.chars().take(39).collect::<String>(),
        cut(&full, crate::pan::KEY_STEP),
        "{unpanned:?} → {panned:?}"
    );
    // The laid-out lines stay those of the whole diagram: block copy, links
    // and line mapping read the same as before.
    assert_eq!(panel.transcript.rendered(), &before[..]);
    // Left pans back; at the edge it redraws nothing.
    assert!(!panel.handle_key(shift(KeyCode::Left)).is_empty());
    assert!(panel.handle_key(shift(KeyCode::Left)).is_empty());
    assert_eq!(render_text(&mut panel, 40, 40)[y], unpanned);

    // A mouse selection over a panned row copies the cells it covers.
    panel.handle_key(shift(KeyCode::Right));
    let _ = render_text(&mut panel, 40, 40);
    let area = panel.transcript_area;
    let y = y as u16;
    panel.handle_mouse(
        wheel(
            MouseEventKind::Down(MouseButton::Left),
            4,
            y,
            KeyModifiers::NONE,
        ),
        area,
    );
    panel.handle_mouse(
        wheel(
            MouseEventKind::Drag(MouseButton::Left),
            38,
            y,
            KeyModifiers::NONE,
        ),
        area,
    );
    panel.handle_mouse(
        wheel(
            MouseEventKind::Up(MouseButton::Left),
            38,
            y,
            KeyModifiers::NONE,
        ),
        area,
    );
    let selection = panel.text_selection.expect("a selection");
    let copied = selection.text(&panel.shown_rows(), panel.transcript.rendered_copy(), 39);
    let shown: String = cut(&full, crate::pan::KEY_STEP).chars().skip(4).collect();
    assert_eq!(copied, shown.trim_end());
}

#[test]
fn the_wheel_pans_a_wide_diagram_only_over_it() {
    let (mut panel, spot) = wide_diagram_panel();
    let area = panel.transcript_area;
    let on = screen_row(&panel, spot.line);
    let off = screen_row(&panel, spot.line) - 2;
    let none = KeyModifiers::NONE;
    let step = crate::pan::WHEEL_STEP;

    panel.handle_mouse(wheel(MouseEventKind::ScrollRight, 10, on, none), area);
    assert_eq!(panel.transcript.pan_of(&spot), step);
    panel.handle_mouse(wheel(MouseEventKind::ScrollLeft, 10, on, none), area);
    assert_eq!(panel.transcript.pan_of(&spot), 0);
    // Off the diagram, a horizontal wheel does nothing.
    assert!(panel
        .handle_mouse(wheel(MouseEventKind::ScrollRight, 10, off, none), area)
        .is_empty());
    assert_eq!(panel.transcript.pan_of(&spot), 0);

    // Shift+wheel, coalesced or not, pans it — on its scrollbar row too.
    let track = screen_row(&panel, spot.track());
    panel.handle_wheel(
        wheel(MouseEventKind::ScrollDown, 10, track, KeyModifiers::SHIFT),
        2,
        area,
    );
    assert_eq!(panel.transcript.pan_of(&spot), 2 * step);
    panel.handle_mouse(
        wheel(MouseEventKind::ScrollUp, 10, on, KeyModifiers::SHIFT),
        area,
    );
    assert_eq!(panel.transcript.pan_of(&spot), step);
    // The plain wheel still scrolls the transcript, over the diagram too.
    let top = panel.top;
    panel.follow = false;
    panel.handle_wheel(wheel(MouseEventKind::ScrollUp, 10, on, none), -1, area);
    assert_eq!(panel.transcript.pan_of(&spot), step);
    assert!(panel.top < top || top == 0);
}

#[test]
fn a_wide_diagrams_scrollbar_pans_it_on_click_and_drag() {
    let (mut panel, spot) = wide_diagram_panel();
    let area = panel.transcript_area;
    let y = screen_row(&panel, spot.track());
    let none = KeyModifiers::NONE;
    let right_end = spot.col + spot.view - 1;
    // A click at the track's far end brings the diagram's end into view.
    panel.handle_mouse(
        wheel(MouseEventKind::Down(MouseButton::Left), right_end, y, none),
        area,
    );
    panel.handle_mouse(
        wheel(MouseEventKind::Up(MouseButton::Left), right_end, y, none),
        area,
    );
    assert_eq!(panel.transcript.pan_of(&spot), spot.max_pan());
    assert!(panel.text_selection.is_none(), "no text was selected");
    let _ = render_text(&mut panel, 40, 40);
    // The thumb dragged back to the start pans back.
    let thumb = panel.diagram_bars[0].1;
    let grab = spot.col + thumb.thumb_pos;
    panel.handle_mouse(
        wheel(MouseEventKind::Down(MouseButton::Left), grab, y, none),
        area,
    );
    panel.handle_mouse(
        wheel(MouseEventKind::Drag(MouseButton::Left), spot.col, y, none),
        area,
    );
    panel.handle_mouse(
        wheel(MouseEventKind::Up(MouseButton::Left), spot.col, y, none),
        area,
    );
    assert_eq!(panel.transcript.pan_of(&spot), 0);
}

#[test]
fn a_diagrams_pan_clamps_to_its_width() {
    let (mut panel, spot) = wide_diagram_panel();
    for _ in 0..100 {
        panel.handle_key(chord(KeyCode::Right, KeyModifiers::SHIFT));
    }
    assert_eq!(panel.transcript.pan_of(&spot), spot.max_pan());
    // Wider, the same diagram has less to pan: the offset clamps to it.
    let _ = render_text(&mut panel, 50, 40);
    let wider = panel.transcript.wide_diagrams()[0];
    assert_eq!(wider.key(), spot.key());
    assert!(wider.max_pan() < spot.max_pan());
    assert_eq!(panel.transcript.pan_of(&wider), wider.max_pan());
}

#[test]
fn a_fresh_picture_shows_under_its_folded_block() {
    let mut panel = picture_panel();
    panel.transcript.push(picture_read(400, 400));
    let buf = render_buf(&mut panel, 60, 30);
    let spots = panel.transcript.pictures().to_vec();
    assert_eq!(spots.len(), 1);
    // Half blocks: 10×20 px cells, at most half the panel's 30 rows.
    assert_eq!(spots[0].rows, 15);
    assert!(!panel.transcript.is_expanded(0), "the block stays folded");
    let y = (spots[0].line - panel.top) as u16;
    assert!(painted(&buf, y..y + spots[0].rows), "the picture is drawn");
}

#[test]
fn a_picture_is_centred_across_the_panel() {
    let mut panel = picture_panel();
    panel.transcript.push(picture_read(400, 400));
    let buf = render_buf(&mut panel, 60, 30);
    let spot = panel.transcript.pictures()[0];
    // 59 text columns, the gutter left out; 15 rows of 20 px are 30 columns
    // of 10 px, centred in the 57 after the indent.
    assert_eq!((spot.cols, spot.rows), (30, 15));
    assert_eq!(spot.col, crate::transcript::PICTURE_INDENT + 13);
    let y = (spot.line - panel.top) as u16;
    let painted_cols: Vec<u16> = (0..60)
        .filter(|&x| matches!(buf[(x, y)].bg, ratatui::style::Color::Rgb(..)))
        .collect();
    assert_eq!(painted_cols.first(), Some(&spot.col));
    assert_eq!(painted_cols.last(), Some(&(spot.col + spot.cols - 1)));
}

#[test]
fn a_wide_panel_lets_a_picture_take_half_its_width() {
    let mut panel = picture_panel();
    panel.transcript.push(picture_read(1000, 1000));
    let _ = render_buf(&mut panel, 120, 30);
    // Half the width, 60 columns of 10 px, is 30 rows of 20 px: more than
    // half the height, so the bound, but the input bar stays in sight.
    assert_eq!(panel.transcript.pictures()[0].rows, 26);
    let _ = render_buf(&mut panel, 80, 30);
    // 40 columns are 20 rows: still more than half the height.
    assert_eq!(panel.transcript.pictures()[0].rows, 20);
    let _ = render_buf(&mut panel, 20, 30);
    // A narrow panel keeps half its height, the width bounding it anyway.
    assert_eq!(panel.transcript.pictures()[0].cols, 16);
}

#[test]
fn a_picture_cut_by_the_edge_shows_its_placeholder() {
    let mut panel = picture_panel();
    panel.transcript.push(picture_read(400, 400));
    panel.transcript.push(tall_answer());
    let _ = render_buf(&mut panel, 60, 200);
    let spot = panel.transcript.pictures()[0];
    // Its first row just above the view: the picture's block is still on
    // screen, so nothing folds, but the picture is not wholly in sight.
    panel.follow = false;
    panel.top = spot.line + 1;
    let buf = render_buf(&mut panel, 60, 30);
    assert_eq!(panel.transcript.pictures().len(), 1, "not folded");
    assert!(!painted(&buf, 0..spot.rows - 1), "not drawn cut");
}

#[test]
fn a_picture_scrolled_away_stays_until_its_block_is_folded_by_hand() {
    let mut panel = picture_panel();
    panel.transcript.push(picture_read(400, 200));
    panel.transcript.push(tall_answer());
    let _ = render_buf(&mut panel, 60, 30);
    let anchor = transcript_text(&panel)
        .iter()
        .position(|l| l.trim() == "line 20")
        .unwrap();
    panel.follow = false;
    panel.top = anchor;
    let rows = render_text(&mut panel, 60, 30);
    assert!(rows[0].trim_start().starts_with("line 20 "), "{rows:?}");
    // Out of sight it is not drawn, but it is not folded either.
    assert_eq!(panel.transcript.pictures().len(), 1);
    assert!(!transcript_text(&panel).iter().any(|l| l.contains("▸ PNG")));

    // The block unfolded and folded again by hand folds the picture too.
    assert!(panel.transcript.toggle_expanded(0));
    assert!(panel.transcript.toggle_expanded(0));
    let _ = render_buf(&mut panel, 60, 30);
    assert!(panel.transcript.pictures().is_empty());
    assert!(
        transcript_text(&panel)
            .iter()
            .any(|l| l.trim() == "▸ PNG 400×200"),
        "{:?}",
        transcript_text(&panel)
    );
}

#[test]
fn without_graphics_a_picture_is_a_card() {
    let mut panel = panel(vec![]);
    panel.graphics_asked = true;
    panel.transcript.push(picture_read(400, 300));
    let _ = render_buf(&mut panel, 60, 30);
    assert!(panel.transcript.pictures().is_empty());
    assert!(transcript_text(&panel)
        .iter()
        .any(|l| l.trim() == "PNG 400×300"));
}

#[test]
fn the_terminal_is_asked_only_once_a_picture_is_laid_out() {
    let mut panel = panel(vec![]);
    let _ = render_buf(&mut panel, 60, 30);
    assert!(!panel.ask_graphics(), "no picture, no question");
    assert!(!panel.graphics_asked);
}

#[test]
fn a_pasted_picture_shows_inside_the_message() {
    let mut panel = picture_panel();
    let user = UserMessage::text("look [#1 pasted image 200×100]")
        .with_images(vec![("image/png".into(), png(200, 100))]);
    panel
        .transcript
        .push(transcript::user_item(&user, "12:00:00".into(), None));
    let _ = render_buf(&mut panel, 60, 30);
    let spot = panel.transcript.pictures()[0];
    let (first, last) = panel.transcript.content_lines_of(0).unwrap();
    assert!(
        spot.line > first && spot.line + (spot.rows as usize) <= last + 1,
        "{spot:?} in {first}..={last}"
    );
    // A short message with a picture still folds, to the picture's card.
    assert!(panel.transcript.toggle_expanded(0));
    assert!(panel.transcript.toggle_expanded(0));
    let _ = render_buf(&mut panel, 60, 30);
    assert!(panel.transcript.pictures().is_empty());
}

#[test]
fn where_moving_resends_a_picture_it_is_a_frame_until_it_stands_still() {
    let mut panel = picture_panel();
    panel.graphics = Some(Graphics::sixel());
    panel.transcript.push(picture_read(100, 100));
    panel.transcript.push(tall_answer());
    panel.follow = false;
    let _ = render_buf(&mut panel, 60, 40);
    // A picture just come into view stands nowhere yet: a frame.
    let buf = render_buf(&mut panel, 60, 40);
    let spot = panel.transcript.pictures()[0];
    let (x, y) = (spot.col, spot.line as u16);
    assert!(!has_graphics(&buf));
    assert_eq!(buf[(x, y)].symbol(), "┌");
    assert!(!panel.pictures_settled());

    std::thread::sleep(crate::render::PICTURE_SETTLE);
    assert!(panel.pictures_settled(), "a tick redraws it");
    let buf = render_buf(&mut panel, 60, 40);
    assert!(has_graphics(&buf), "standing still, it is drawn");

    // Moved by a row, it is a frame again.
    panel.top = 1;
    let buf = render_buf(&mut panel, 60, 40);
    assert!(!has_graphics(&buf));
    assert_eq!(buf[(x, y - 1)].symbol(), "┌");
}

#[test]
fn where_moving_is_cheap_a_moving_picture_is_drawn() {
    let mut panel = picture_panel();
    panel.transcript.push(picture_read(100, 100));
    panel.transcript.push(tall_answer());
    panel.follow = false;
    let _ = render_buf(&mut panel, 60, 40);
    panel.top = 1;
    let buf = render_buf(&mut panel, 60, 40);
    let spot = panel.transcript.pictures()[0];
    let y = (spot.line - 1) as u16;
    assert!(painted(&buf, y..y + spot.rows));
    assert!(!panel.pictures_settled());
}

#[test]
fn a_written_page_is_a_card_that_opens_the_file() {
    let mut panel = panel(vec![]);
    panel.transcript.push(wrote("out/report.html", true));
    let rows = render_text(&mut panel, 60, 12);
    let y = rows
        .iter()
        .position(|r| r.contains("↗ HTML report.html"))
        .unwrap_or_else(|| panic!("{rows:?}")) as u16;
    let area = panel.transcript_area;
    let click = |panel: &mut AgentPanel, modifiers| {
        let mouse = |kind| MouseEvent {
            kind,
            column: area.x + 4,
            row: area.y + y,
            modifiers,
        };
        panel.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left)), area);
        panel.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left)), area)
    };
    let report = panel.cwd.join("out/report.html");
    let events = click(&mut panel, KeyModifiers::NONE);
    assert!(
        matches!(events.as_slice(), [PanelEvent::OpenLink(LinkTarget::Path(p))] if *p == report),
        "{events:?}"
    );
    let events = click(&mut panel, KeyModifiers::ALT);
    assert!(
        matches!(events.as_slice(), [PanelEvent::OpenLinkExternal(LinkTarget::Path(p))] if *p == report),
        "{events:?}"
    );
    // `o` on the block opens the file too, not the call's output.
    panel.selected = 0;
    let events = panel.open_selected_in_panel();
    assert!(
        matches!(events.as_slice(), [PanelEvent::OpenLink(LinkTarget::Path(p))] if *p == report),
        "{events:?}"
    );
}

#[test]
fn only_a_viewable_file_written_is_a_card() {
    for (item, card) in [
        (wrote("notes.md", true), true),
        (wrote("flow.mmd", true), true),
        (wrote("logo.svg", true), false),
        (wrote("main.rs", true), false),
        (wrote("page.html", false), false),
    ] {
        let mut panel = panel(vec![]);
        panel.transcript.push(item);
        let rows = render_text(&mut panel, 60, 12);
        assert_eq!(rows.iter().any(|r| r.contains('↗')), card, "{rows:?}");
    }
}

/// The session's terminals show in the strip and the status bar; a row
/// opens its terminal, its `✕` closes it once confirmed, and they end with
/// the session the panel leaves.
#[test]
fn the_sessions_terminals_show_in_the_strip_and_end_with_it() {
    use termide_agent_core::{TerminalEntry, TerminalState};
    let dir = tempfile::tempdir().unwrap();
    let session = Session::create(dir.path(), dir.path()).unwrap();
    let owner = session.id().to_string();
    let terminals = Arc::new(Terminals::default());
    let entry = |name: &str, state| TerminalEntry {
        name: name.into(),
        state,
        age: Duration::from_secs(60),
        log: None,
        shell: termide_agent_core::ShellSyntax::Posix,
        at_prompt: false,
    };
    terminals.entries.lock().unwrap().extend([
        (
            owner.clone(),
            entry(
                "dev",
                TerminalState::Running {
                    command: "npm run dev".into(),
                    elapsed: Duration::from_secs(5),
                },
            ),
        ),
        (owner.clone(), entry("tests", TerminalState::Idle)),
        ("other".to_string(), entry("x", TerminalState::Idle)),
    ]);
    let mut panel = AgentPanel::new(AgentPanelSetup {
        session: Some(session),
        terminals: Some(Arc::clone(&terminals) as _),
        ..setup(vec![])
    });
    panel.tick();

    let rows = panel.strip_rows(80);
    let lines = crate::strip::strip_lines(&rows, 80, &ThemeColors::default());
    let text: Vec<String> = lines
        .iter()
        .map(|line| line.spans.iter().map(|s| s.content.as_ref()).collect())
        .collect();
    // Newest first.
    assert!(text[0].starts_with("$ Terminal · tests · idle"), "{text:?}");
    assert!(
        text[1].starts_with("$ Terminal · dev  npm run dev"),
        "{text:?}"
    );
    assert!(text[1].trim_end().ends_with("✕"), "{text:?}");
    assert_eq!(rows.len(), 2, "another session's terminal is not shown");
    let status: String = panel.segments().iter().map(|s| s.text.as_str()).collect();
    assert!(status.contains("terminals: 2"), "{status}");

    // The row opens its terminal in a panel.
    let events = panel.click_strip(rows[1].target, false);
    assert!(
        matches!(&events[..], [PanelEvent::OpenAgentTerminal { session, name }]
            if *session == owner && name == "dev"),
        "{events:?}"
    );
    // `✕` asks first, then closes it.
    let events = panel.click_strip(rows[0].target, true);
    let Some(PanelEvent::ShowConfirm {
        on_confirm: termide_core::ConfirmAction::Custom(action),
        ..
    }) = events.first()
    else {
        panic!("a confirmation expected: {events:?}");
    };
    panel.handle_command(PanelCommand::Confirmed {
        action: action.clone(),
    });
    assert_eq!(panel.strip_rows(80).len(), 1);

    // Leaving the session ends its terminals; the panel's close does too.
    assert!(panel.switch_session(None));
    assert_eq!(*terminals.released.lock().unwrap(), [owner]);
    assert!(panel.strip_rows(80).is_empty());
    drop(panel);
    assert_eq!(
        terminals.released.lock().unwrap().len(),
        1,
        "no session left to end"
    );
}
