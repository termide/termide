use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{buffer::Buffer, layout::Rect};
use termide_core::{
    CommandResult, HotkeyTable, KeyChord, LinkTarget, PanelCommand, PanelEvent, RenderContext,
    ThemeColors,
};
use termide_modal::FindField;

use super::{Viewer, ViewerPanel};
use termide_richtext::{Builder, Rendered};

/// A stand-in front-end: paragraphs split on blank lines; `#id` is an anchor
/// line, `->url` a link line.
fn layout(src: &str, width: u16, colors: &ThemeColors, is_light: bool) -> Rendered {
    let mut b = Builder::new(width, colors, is_light);
    for para in src.split("\n\n") {
        if let Some(id) = para.strip_prefix('#') {
            b.add_anchor(id.to_string());
            b.text(id);
        } else if let Some(url) = para.strip_prefix("->") {
            b.start_link(url.to_string());
            b.text(url);
            b.end_link();
        } else {
            b.text(para);
        }
        b.end_paragraph();
    }
    b.finish()
}

/// A file-backed viewer over `src`, laid out at 80 columns in a 10-row view.
fn view_from(src: &str) -> Viewer {
    let mut v = Viewer::new(layout);
    v.file_path = PathBuf::from("/x/doc.md");
    v.title = "doc.md".to_string();
    v.source = src.to_string();
    v.last_area = Rect::new(0, 0, 80, 10);
    v.relayout_if_needed(80);
    v
}

fn fetched(url: &str) -> Viewer {
    Viewer::from_source(layout, "p".into(), "x".into(), Some(url.into()))
}

/// A panel with nothing of its own, for the shared input handlers.
struct Plain(Viewer);

impl ViewerPanel for Plain {
    fn view(&self) -> &Viewer {
        &self.0
    }
    fn view_mut(&mut self) -> &mut Viewer {
        &mut self.0
    }
}

fn chord(code: KeyCode, modifiers: KeyModifiers) -> KeyChord {
    let key = KeyEvent::new(code, modifiers);
    KeyChord {
        raw: key,
        canonical: key,
    }
}

#[test]
fn selection_copies_across_lines() {
    let mut v = view_from("alpha\n\nbeta");
    v.cursor = (0, 0);
    v.anchor = Some((0, 0));
    let last = v.line_count() - 1;
    v.cursor = (last, v.line_len(last));
    let text = v.selected_text();
    assert!(text.contains("alpha"), "{text:?}");
    assert!(text.contains("beta"), "{text:?}");
}

#[test]
fn a_terminal_paste_goes_into_the_open_find_bar_and_searches() {
    let mut v = view_from("one two\n\ntwo three two");
    let paste = |text: &str| PanelCommand::PasteText { text: text.into() };
    assert!(matches!(
        v.handle_command(paste("two")),
        CommandResult::None
    ));
    v.open_find();
    let result = v.handle_command(paste("two"));
    assert!(matches!(result, CommandResult::Handled(true)));
    assert_eq!(v.find_bar.as_ref().unwrap().find_text(), "two");
    assert_eq!(v.matches.len(), 3, "the paste ran the search");
}

#[test]
fn search_finds_matches_and_moves_cursor() {
    let mut v = view_from("one two\n\ntwo three two");
    v.open_find();
    v.find_bar
        .as_mut()
        .unwrap()
        .set_text(FindField::Find, "two".to_string());
    v.run_search();
    assert_eq!(v.matches.len(), 3, "{:?}", v.matches);
    // Cursor jumps to the first match.
    assert_eq!(v.cursor, v.matches[v.match_idx]);
    let first = v.cursor;
    v.step_match(true);
    assert_ne!(v.cursor, first, "next match should move the cursor");
}

#[test]
fn relative_link_resolves_and_navigates_in_place() {
    let mut v = fetched("https://ex.com/dir/");
    let evs = v.activate_link("sub/x.html");
    match evs.as_slice() {
        [PanelEvent::NavigateUrl(u)] => assert_eq!(u, "https://ex.com/dir/sub/x.html"),
        _ => panic!("{evs:?}"),
    }
    assert_eq!(v.history.last().unwrap(), "https://ex.com/dir/sub/x.html");
}

#[test]
fn file_backed_links_go_to_the_app() {
    // A web link from a file-backed view, and a local file, are the app's
    // to open (inside: a viewer, the image preview), not in place.
    let mut v = view_from("x");
    let evs = v.activate_link("https://ex.com");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::OpenLink(LinkTarget::Url(u))] if u == "https://ex.com"),
        "{evs:?}"
    );
    let evs = v.activate_link("/pics/logo.png");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::OpenLink(LinkTarget::Path(p))] if p == Path::new("/pics/logo.png")),
        "{evs:?}"
    );
    // A scheme the app does not open goes to the system opener.
    let evs = v.activate_link("mailto:a@b.c");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::OpenExternal(_)]),
        "{evs:?}"
    );
}

#[test]
fn empty_link_is_noop() {
    let mut v = view_from("x");
    assert!(v.activate_link("").is_empty(), "empty href");
}

#[test]
fn same_page_anchor_scrolls() {
    let mut src = String::from("top");
    for i in 0..40 {
        src.push_str(&format!("\n\nline {i}"));
    }
    src.push_str("\n\n#target\n\nafter");
    let mut v = view_from(&src);
    assert!(
        v.doc.anchors.iter().any(|(id, _)| id == "target"),
        "anchor not registered: {:?}",
        v.doc.anchors
    );
    let before = v.top;
    let evs = v.activate_link("#target");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::NeedsRedraw]),
        "{evs:?}"
    );
    assert!(
        v.top > before,
        "should scroll down to the anchor (top={}, before={before})",
        v.top
    );
}

#[test]
fn external_open_leaves_a_fetched_page_in_place() {
    // `O`/`Alt+Click` on a link in a fetched page hands the resolved
    // address to the browser instead of replacing the page.
    let mut v = fetched("https://ex.com/a/");
    let evs = v.activate_link_external("b");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::OpenLinkExternal(LinkTarget::Url(u))] if u == "https://ex.com/a/b"),
        "{evs:?}"
    );
    assert_eq!(v.history.len(), 1);
    // A file link from a file-backed view goes to its system application.
    let mut v = view_from("x");
    let evs = v.activate_link_external("/pics/logo.png");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::OpenLinkExternal(LinkTarget::Path(p))] if p == Path::new("/pics/logo.png")),
        "{evs:?}"
    );
    // A same-page anchor there has nowhere outside to go: it scrolls.
    assert!(matches!(
        v.activate_link_external("#x").as_slice(),
        [PanelEvent::NeedsRedraw]
    ));
}

#[test]
fn open_external_keys_match_o_and_alt_enter() {
    // The default `viewer.keybindings.open_external` is the file manager's
    // pair: `O` (the plain letter key) and `Alt+Enter`.
    let mut kb = termide_config::ViewerKeybindings::default();
    kb.with_defaults();
    let mut t = HotkeyTable::new();
    t.insert("open_external", &kb.open_external);
    let key = KeyEvent::new;
    assert!(t.matches(
        "open_external",
        &key(KeyCode::Char('o'), KeyModifiers::NONE)
    ));
    assert!(t.matches("open_external", &key(KeyCode::Enter, KeyModifiers::ALT)));
    assert!(!t.matches("open_external", &key(KeyCode::Enter, KeyModifiers::NONE)));
}

#[test]
fn history_back_and_forward() {
    let mut v = fetched("https://ex.com/a");
    v.activate_link("https://ex.com/b");
    v.activate_link("https://ex.com/c");
    let back = v.go_back();
    assert!(
        matches!(back.as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/b"),
        "{back:?}"
    );
    assert!(
        matches!(v.go_back().as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/a")
    );
    assert!(v.go_back().is_empty(), "no history before the first page");
    assert!(
        matches!(v.go_forward().as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/b")
    );
}

#[test]
fn bracket_history_works_on_cyrillic_layout() {
    let mut p = Plain(fetched("https://ex.com/a"));
    p.0.activate_link("https://ex.com/b"); // history [a, b], idx = 1
                                           // On a Russian layout the `[` key emits 'х'; the normalizer puts '[' in
                                           // `canonical`, which the viewer matches.
    let chord = KeyChord {
        raw: KeyEvent::new(KeyCode::Char('х'), KeyModifiers::NONE),
        canonical: KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE),
    };
    let evs = p.handle_viewer_key(chord).expect("a viewer key");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::NavigateUrl(u)] if u == "https://ex.com/a"),
        "{evs:?}"
    );
}

#[test]
fn ctrl_a_selects_the_whole_page_for_ctrl_c() {
    let mut p = Plain(view_from("Top\n\nmiddle\n\nend"));
    let whole: Vec<String> = (0..p.0.line_count()).map(|i| p.0.line_text(i)).collect();
    p.handle_viewer_key(chord(KeyCode::Char('a'), KeyModifiers::CONTROL));
    assert_eq!(p.0.top, 0, "the view stays where it was");
    let evs = p
        .handle_viewer_key(chord(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .expect("a viewer key");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::CopyToClipboard(t)] if *t == whole.join("\n")),
        "{evs:?}"
    );
    assert!(whole.iter().any(|l| l.contains("Top")));
    assert!(whole.iter().any(|l| l.contains("end")));
}

#[test]
fn a_key_that_is_no_viewers_is_left_to_the_panel() {
    let mut p = Plain(view_from("x"));
    assert!(p
        .handle_viewer_key(chord(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .is_none());
}

#[test]
fn enter_follows_the_link_under_the_cursor_through_the_panel() {
    struct Intercepting(Viewer);
    impl ViewerPanel for Intercepting {
        fn view(&self) -> &Viewer {
            &self.0
        }
        fn view_mut(&mut self) -> &mut Viewer {
            &mut self.0
        }
        fn follow_link(&mut self, href: &str) -> Vec<PanelEvent> {
            vec![PanelEvent::CopyToClipboard(href.to_string())]
        }
    }
    let mut p = Intercepting(view_from("->https://ex.com"));
    let evs = p
        .handle_viewer_key(chord(KeyCode::Enter, KeyModifiers::NONE))
        .expect("a viewer key");
    assert!(
        matches!(evs.as_slice(), [PanelEvent::CopyToClipboard(u)] if u == "https://ex.com"),
        "{evs:?}"
    );
}

/// End-to-end contract the mouse dispatcher relies on: after a render the
/// viewer reports the bar it drew, a press on the thumb hit-tests, and the
/// offset it maps to actually scrolls the viewer.
#[test]
fn reports_its_scrollbar_and_scrolls_to_a_dragged_offset() {
    use termide_core::ScrollAxis;

    let src = (0..200)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut v = view_from(&src);

    let area = Rect::new(0, 0, 80, 10);
    let mut buf = Buffer::empty(Rect::new(0, 0, 80, 12));
    let colors = v.colors;
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
        terminal_width: 80,
        terminal_height: 12,
        border_right_x: Some(79),
        border_left_x: None,
        border_bottom_y: Some(11),
    };
    v.render(area, &mut buf, &ctx, None);

    let CommandResult::ScrollBars(bars) = v.handle_command(PanelCommand::GetScrollBars) else {
        panic!("viewer did not report its scrollbars");
    };
    let bar = bars.vertical.expect("200 paragraphs overflow 10 rows");
    assert_eq!(bar.x, 79, "bar must sit on the right border");
    assert!(bar.hits_thumb(bar.x, bar.y + bar.thumb_pos));

    // Grab the thumb's top row, drag to the track's end.
    let target = bar.offset_for_drag(bar.x, bar.y + bar.len, 0);
    assert_eq!(
        target,
        bar.max_offset(),
        "dragging to the end pins to the end"
    );
    v.handle_command(PanelCommand::SetScrollOffset {
        axis: ScrollAxis::Vertical,
        offset: target,
    });
    assert_eq!(v.top, target.min(v.line_count() - 1));
    assert!(v.top > 0, "the viewer actually scrolled");
}

#[test]
fn a_followed_link_spins_in_the_title_and_keeps_the_page() {
    let mut v = view_from("page");
    v.start_loading(5, "https://ex.com/b.md".into());
    assert!(v.title().ends_with(" https://ex.com/b.md"));
    assert!(termide_config::constants::SPINNER_FRAMES
        .iter()
        .any(|f| v.title().starts_with(f)));
    assert!(matches!(v.tick().as_slice(), [PanelEvent::NeedsRedraw]));
    assert_eq!(v.source, "page");
    v.stop_loading();
    assert_eq!(v.title(), "doc.md");
    assert!(v.tick().is_empty());
}
