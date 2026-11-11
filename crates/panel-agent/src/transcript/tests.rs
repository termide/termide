use super::*;
use serde_json::json;

fn call(name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: "c1".into(),
        name: name.into(),
        arguments: args,
        extra_content: None,
    }
}

fn text_of(lines: &[Line<'_>]) -> Vec<String> {
    lines
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn skill_task_and_mcp_calls_open_with_a_glyph_and_an_action() {
    let colors = ThemeColors::default();
    let headline = |name: &str, args: Value| -> String {
        tool_headline(&call(name, args), None, 80, colors.info, &colors)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    assert_eq!(
        headline("skill", json!({ "name": "review", "args": "src/x.rs" })),
        "/ Using skill review src/x.rs"
    );
    assert_eq!(
        headline("skill", json!({ "name": "deploy" })),
        "/ Using skill deploy"
    );
    assert_eq!(
        headline(
            "question",
            json!({ "questions": [{ "question": "Which approach?" }, { "question": "Name?" }] })
        ),
        "¿ Asking Which approach? (+1)"
    );
    assert_eq!(
        headline(
            "task",
            json!({ "agent": "reviewer", "prompt": "\ncheck the diff\nthen report" })
        ),
        "& Delegating to reviewer · check the diff"
    );
    // Another agent's session: addressed by its id (a number, from an
    // older log, as it was written), the verb saying
    // what the message is, or listed — all with the `&` of a task.
    assert_eq!(
        headline(
            "send_message",
            json!({ "to": "1a2b3c4d", "kind": "info", "text": "\nI edit submit.rs\nmore" })
        ),
        "& Telling session 1a2b3c4d · I edit submit.rs"
    );
    assert_eq!(
        headline(
            "send_message",
            json!({ "to": "5e6f7a8b", "kind": "question", "text": "Which?" })
        ),
        "& Asking session 5e6f7a8b · Which?"
    );
    assert_eq!(
        headline(
            "send_message",
            json!({ "to": 1, "kind": "reply", "text": "Done" })
        ),
        "& Replying to session 1 · Done"
    );
    assert_eq!(headline("agents", json!({})), "& Listing agent panels ");
    // What the agent set going, listed or stopped by its id.
    assert_eq!(headline("jobs", json!({})), "& Listing background work ");
    assert_eq!(
        headline("jobs", json!({ "stop": "call_7" })),
        "& Stopping call_7"
    );
    assert_eq!(
        headline(
            "github__create_issue",
            json!({ "title": "Crash", "labels": ["bug"] })
        ),
        "* Using MCP github: create_issue labels=[\"bug\"] title=Crash"
    );
    // As an external agent reports an MCP call.
    assert_eq!(
        headline("mcp__termide__open", json!({})),
        "* Using MCP termide: open"
    );
    // Anything else keeps its name and a summary.
    assert_eq!(headline("lists", json!({})), "lists ");
    assert_eq!(mcp_parts("__x"), None);
    assert_eq!(mcp_parts("web_search"), None);
}

#[test]
fn a_typed_command_heads_the_user_block_over_its_folded_expansion() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::User {
        text: "Review a.rs.\nQuote the lines.".into(),
        at: String::new(),
        command: Some("/review a.rs".into()),
        images: Vec::new(),
    });
    let folded = text_of(transcript.lines(40, &colors, false)).join("\n");
    assert!(folded.contains("› /review a.rs"), "{folded}");
    assert!(folded.contains("… 2 more lines"), "{folded}");
    assert!(!folded.contains("Quote the lines."), "{folded}");

    transcript.toggle_expanded(0);
    let unfolded = text_of(transcript.lines(40, &colors, false)).join("\n");
    assert!(unfolded.contains("› /review a.rs"), "{unfolded}");
    assert!(unfolded.contains("  Review a.rs."), "{unfolded}");
    assert!(unfolded.contains("  Quote the lines."), "{unfolded}");
}

#[test]
fn another_agents_message_is_a_step_not_the_users_plate() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let message = UserMessage::text("Found the loader in app.rs.\nIt reads the config twice.")
        .with_origin(termide_agent_core::MessageOrigin {
            session: String::new(),
            agent: "default".into(),
            kind: termide_agent_core::OriginKind::Reply,
            panel: Some(3),
            hops: 1,
        });
    transcript.push(user_item(&message, "12:00:00".into(), None));
    transcript.set_expanded(0, false);
    let folded = text_of(transcript.lines(80, &colors, false)).join("\n");
    assert!(
        folded.contains("& Receiving reply from ▸ default (panel 3) · Found the loader in app.rs."),
        "{folded}"
    );
    assert!(!folded.contains('›'), "{folded}");
    assert!(!folded.contains("twice"), "{folded}");

    transcript.set_expanded(0, true);
    let unfolded = text_of(transcript.lines(80, &colors, false)).join("\n");
    assert!(
        unfolded.contains("It reads the config twice."),
        "{unfolded}"
    );
}

#[test]
fn a_prompt_keeps_its_line_breaks_in_the_user_block() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::User {
        text: "first line\n\n- one\n- two and a much longer tail".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    let rows: Vec<String> = text_of(transcript.lines(20, &colors, false))
        .into_iter()
        .map(|row| row.trim_end().to_string())
        .collect();
    let text = rows.join("\n");
    assert!(
        text.contains("› first line\n\n  - one\n  - two and a much\n  longer tail\n"),
        "{text}"
    );
}

/// A session of `exchanges` user / answer / tool exchanges.
fn long_session(exchanges: usize) -> Transcript {
    let mut transcript = Transcript::default();
    for n in 0..exchanges {
        transcript.push(Item::User {
            text: format!("question {n}: what does this function do?"),
            at: String::new(),
            command: None,
            images: Vec::new(),
        });
        transcript.push(Item::Tool {
            call: call("read", json!({ "path": format!("src/file{n}.rs") })),
            result: None,
            live: None,
            at: String::new(),
            duration_ms: Some(12),
            waited_ms: None,
            waiting: false,
        });
        transcript.push(Item::Assistant {
                text: format!(
                    "It **parses** the input `{n}` and returns a [result](https://example.com/{n}).\n\n\
                     - first point\n- second point\n\n```rust\nfn f() -> u32 {{ {n} }}\n```\n\n\
                     A closing paragraph long enough to wrap over a couple of rows in a panel \
                     of ordinary width, as answers do."
                ),
                streaming: false,
                error: None,
                at: String::new(),
                cost: None,
                run_ms: None,
            });
    }
    transcript
}

/// What the flattened layout holds: text, item per line, copy rows,
/// links, pictures, wide diagrams and each item's first line.
type Layout = (
    Vec<String>,
    Vec<usize>,
    Vec<RowCopy>,
    Vec<(usize, RowLink)>,
    Vec<PictureSpot>,
    Vec<DiagramSpot>,
    Vec<Option<usize>>,
);

/// The flattened layout, to compare one built bit by bit with one built
/// whole.
fn layout_of(transcript: &Transcript) -> Layout {
    (
        text_of(transcript.rendered()),
        transcript.line_item.clone(),
        transcript.flat_copy.clone(),
        transcript.flat_links.clone(),
        transcript.flat_pictures.clone(),
        transcript.flat_diagrams.clone(),
        (0..transcript.items().len() + 1)
            .map(|index| transcript.first_line_of(index))
            .collect(),
    )
}

/// Lay `transcript` out after a change, and check that only rebuilding
/// from the changed block gives what laying everything out again does.
fn check_incremental(transcript: &mut Transcript, width: u16) {
    let colors = ThemeColors::default();
    transcript.lines(width, &colors, false);
    let incremental = layout_of(transcript);
    transcript.mark_dirty(0);
    transcript.lines(width, &colors, false);
    assert_eq!(incremental, layout_of(transcript));
}

#[test]
fn a_layout_rebuilt_from_a_change_matches_one_rebuilt_whole() {
    let mut transcript = long_session(3);
    check_incremental(&mut transcript, 60);

    // A footer comes and ticks over the last block.
    transcript.set_live_footer(vec![Line::from("🕒 1s ⠙")]);
    check_incremental(&mut transcript, 60);
    transcript.set_live_footer(vec![Line::from("🕒 2s ⠹"), Line::from("✍️")]);
    check_incremental(&mut transcript, 60);

    // An answer streams in with a link, then a step follows it: the
    // answer gains the gap it leaves before a step.
    transcript.stream_answer("See [the docs](https://example.com) and ");
    check_incremental(&mut transcript, 60);
    transcript.stream_answer("more.\n\n```mermaid\ngraph LR\n  A --> B\n```\n");
    check_incremental(&mut transcript, 60);
    transcript.finish_assistant(
        "See [the docs](https://example.com) and more.".into(),
        None,
        None,
        String::new(),
        false,
    );
    check_incremental(&mut transcript, 60);
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "ls" })),
        result: None,
        live: Some("a\nb\nc\nd\n".into()),
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    check_incremental(&mut transcript, 60);

    // An answer that streams only blanks leaves, and the gap before it
    // with it.
    transcript.stream_answer("  ");
    check_incremental(&mut transcript, 60);
    transcript.finish_assistant(String::new(), None, None, String::new(), false);
    check_incremental(&mut transcript, 60);

    // A block in the middle unfolds; the width changes; the footer goes.
    transcript.toggle_expanded(1);
    check_incremental(&mut transcript, 60);
    check_incremental(&mut transcript, 41);
    transcript.set_live_footer(Vec::new());
    check_incremental(&mut transcript, 41);

    transcript.clear();
    check_incremental(&mut transcript, 41);
    assert!(transcript.rendered().is_empty());
}

#[test]
fn a_streamed_answer_shows_as_the_same_text_laid_out_at_once() {
    let colors = ThemeColors::default();
    let answer = "Here is the plan, with a [link](https://example.com).\n\n\
                      ## Steps\n\n1. read\n\n2. edit\n\n```rust\nfn f() {}\n\n```\n\n\
                      Then a paragraph long enough to wrap in a narrow panel, twice over.\n\n\
                      ```mermaid\ngraph LR\n  A --> B\n```\n\nDone.";
    let mut streamed = long_session(1);
    let mut end = 0;
    while end < answer.len() {
        let next = (end + 5).min(answer.len());
        streamed.stream_answer(&answer[end..next]);
        end = next;
        let mut whole = long_session(1);
        whole.stream_answer(&answer[..end]);
        let a = text_of(streamed.lines(40, &colors, false));
        let b = text_of(whole.lines(40, &colors, false));
        assert_eq!(a, b, "after {:?}", &answer[..end]);
        assert_eq!(layout_of(&streamed), layout_of(&whole));
    }
    assert!(streamed.stream_head.is_some());
    streamed.finish_assistant(answer.into(), None, None, String::new(), false);
    assert!(streamed.stream_head.is_none());
}

#[test]
fn a_spinner_frame_lays_out_only_the_footer() {
    let colors = ThemeColors::default();
    let mut transcript = long_session(3);
    transcript.lines(60, &colors, false);
    let items_end = transcript.line_count();
    transcript.set_live_footer(vec![Line::from("🕒 1s ⠙")]);
    assert_eq!(transcript.dirty_from, Some(transcript.items().len()));
    transcript.lines(60, &colors, false);
    assert_eq!(transcript.line_count(), items_end + 1);
    // The same frame again changes nothing.
    transcript.set_live_footer(vec![Line::from("🕒 1s ⠙")]);
    assert_eq!(transcript.dirty_from, None);
}

/// Timing of a long session's frames; run with `--ignored --nocapture`.
#[test]
#[ignore = "timing, not a check"]
fn time_frames_of_a_long_session() {
    use std::time::Instant;
    let colors = ThemeColors::default();
    let mut transcript = long_session(700);
    let start = Instant::now();
    let total = transcript.lines(100, &colors, false).len();
    eprintln!("first layout: {:?}, {total} lines", start.elapsed());

    let frames = 100;
    let start = Instant::now();
    for frame in 0..frames {
        transcript.set_live_footer(vec![Line::from(format!("🕒 {frame}s ⠙"))]);
        transcript.lines(100, &colors, false);
    }
    eprintln!("spinner frame: {:?}", start.elapsed() / frames);

    let start = Instant::now();
    for frame in 0..frames {
        transcript.stream_answer("Another sentence of a long streamed answer, word by word. ");
        if frame % 10 == 9 {
            transcript.stream_answer("\n\n");
        }
        transcript.set_live_footer(vec![Line::from(format!("🕒 {frame}s ⠙"))]);
        transcript.lines(100, &colors, false);
    }
    eprintln!("streaming frame: {:?}", start.elapsed() / frames);

    // A long answer: ~40 KB of paragraphs, lists and code.
    for n in 0..200 {
        transcript.stream_answer(&format!(
            "Paragraph {n} explains a step at some length, with `code` and **bold** words.\n\n\
                 ```rust\nlet x = {n};\n```\n\n- item one\n- item two\n\n"
        ));
    }
    transcript.lines(100, &colors, false);
    let start = Instant::now();
    for frame in 0..frames {
        transcript.stream_answer("more words of the tail ");
        transcript.set_live_footer(vec![Line::from(format!("🕒 {frame}s ⠙"))]);
        transcript.lines(100, &colors, false);
    }
    eprintln!(
        "streaming frame, 40 KB answer: {:?}",
        start.elapsed() / frames
    );

    let start = Instant::now();
    for _ in 0..frames {
        std::hint::black_box(transcript.first_line_of(1000));
    }
    eprintln!("first_line_of: {:?}", start.elapsed() / frames);
}

#[test]
fn the_live_footer_appears_after_the_last_line_and_clears() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::User {
        text: "hi".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    let base = transcript.lines(40, &colors, false).len();

    transcript.set_live_footer(vec![
        Line::from("✍️ 3m39s (↓3215, 15 tok/s)"),
        Line::from("🕒 3m41s ⠙"),
    ]);
    let with = transcript.lines(40, &colors, false);
    assert_eq!(with.len(), base + 2);
    assert_eq!(text_of(with).last().map(String::as_str), Some("🕒 3m41s ⠙"));

    transcript.set_live_footer(Vec::new());
    assert_eq!(transcript.lines(40, &colors, false).len(), base);
}

#[test]
fn a_finished_block_shows_its_byline_and_cost() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Assistant {
        text: "Done.".into(),
        streaming: false,
        error: None,
        at: "21:03:16".into(),
        cost: Some(Cost {
            prefill_ms: 600,
            gen_ms: 3600,
            input: 512,
            output: 40000,
        }),
        run_ms: None,
    });
    let lines = text_of(transcript.lines(60, &colors, false));
    // The answer (no reasoning here) carries the wall-clock time + status,
    // then the prefill / generation indicators.
    assert!(
        lines
            .iter()
            .any(|l| l.contains("21:03:16") && l.contains('✓')),
        "answer meta: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.contains("⏫") && l.contains("↑512")),
        "prefill line: {lines:?}"
    );
    // Large token counts are abbreviated (40000 -> 40k).
    assert!(
        lines.iter().any(|l| l.contains("✍") && l.contains("↓40k")),
        "generation line: {lines:?}"
    );

    // A finished tool folds to its headline; how long it took (`🕒`) and
    // its status wait under the fold.
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "cargo test" })),
        result: Some(ToolResultMessage::text(&call("shell", json!({})), "ok")),
        live: None,
        at: "21:03:20".into(),
        duration_ms: Some(1200),
        waited_ms: None,
        waiting: false,
    });
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(
        lines
            .last()
            .is_some_and(|l| !l.contains("🕒") && !l.contains('✓')),
        "tool meta: {lines:?}"
    );
    assert!(transcript.toggle_expanded(transcript.items().len() - 1));
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(
        lines.iter().any(|l| l.contains("🕒 1s") && l.contains('✓')),
        "tool meta: {lines:?}"
    );
    assert!(transcript.toggle_expanded(transcript.items().len() - 1));
    let lines = text_of(transcript.lines(60, &colors, false));
    // The shell headline uses the `$` prefix and the command.
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("$ Running ▸ cargo test")),
        "shell headline: {lines:?}"
    );
}

#[test]
fn items_render_and_cache_per_width() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::OnFinish);
    transcript.push(Item::User {
        text: "Fix the bug".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    // Reasoning and the answer stream into their own blocks.
    transcript
        .stream_thinking("mulling this over\nsecond line of it\nthird line of it\nl4\nl5\nl6");
    transcript.stream_answer("Looking at `main.rs` now.");
    transcript.push(Item::Tool {
        call: call("read", json!({ "path": "main.rs" })),
        result: None,
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    // user(0), thinking(1), assistant(2), tool(3)
    assert_eq!(transcript.items().len(), 4);

    let lines = text_of(transcript.lines(40, &colors, false));
    // A plain gap, then the plate's top padding row, then the message: the
    // text is on the third line.
    assert_eq!(lines[0].trim_end(), "");
    assert_eq!(lines[1].trim_end(), "");
    assert_eq!(lines[2].trim_end(), "› Fix the bug");
    // Streaming, the long reasoning and the running tool show in full
    // with no fold marker.
    assert!(lines.iter().all(|l| !l.contains('▸') && !l.contains('▾')));
    assert!(lines.iter().any(|l| l.contains("l6")));
    assert!(!lines.iter().any(|l| l.contains("more lines")));
    assert!(!transcript.toggle_expanded(1));
    // Once it finishes, the thinking folds to its first line behind the
    // `▸ @` marker; the answer shows behind its own accent mark.
    assert!(transcript.finish_thinking("12:00:00", None));
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(lines
        .iter()
        .any(|l| l.starts_with("@ Thinking ▸ mulling this")));
    assert!(!lines.iter().any(|l| l.contains("second line")));
    assert!(lines.iter().all(|l| !l.contains("thought for")));
    assert!(lines.iter().any(|l| l.contains("› Looking at")));
    assert!(lines.iter().any(|l| l.contains("< Reading main.rs")));
    assert_eq!(transcript.line_count(), lines.len());
    assert_eq!(transcript.item_at_line(0), Some(0));
    assert_eq!(transcript.item_at_line(lines.len() - 1), Some(3));

    // A multi-line result: collapsed shows the headline alone, expanded
    // shows all of it.
    let body = (1..=8)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(transcript.with_tool("c1", |item| {
        if let Item::Tool { result, .. } = item {
            *result = Some(ToolResultMessage::text(&call("read", json!({})), &body));
        }
    }));
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(lines.iter().any(|l| l.starts_with("< Reading ▸ main.rs")));
    assert!(!lines.iter().any(|l| l.contains("line ")));
    assert!(transcript.toggle_expanded(3));
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(lines.iter().any(|l| l.contains("line 1")));
    assert!(lines.iter().any(|l| l.contains("line 8")));
    assert!(transcript.any_expanded());
    // Expanding the thinking block shows the full reasoning text.
    assert!(transcript.toggle_expanded(1));
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(lines.iter().any(|l| l.contains("mulling this")));

    // A different width re-wraps the prose without losing items. Tool
    // summary lines are one-liners clipped at draw time, so only the
    // markdown-rendered ones are checked against the width.
    let narrow = text_of(transcript.lines(14, &colors, false));
    assert!(narrow
        .iter()
        .filter(|l| l.contains("Looking"))
        .all(|l| l.chars().count() <= 14));
    assert!(narrow.iter().any(|l| l.contains("Looking at")));
    assert_eq!(transcript.items().len(), 4);
}

#[test]
fn an_unfolded_call_says_who_decided_it_unless_the_rules_did() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::Never);
    let finished = |note: Option<PermissionNote>| {
        let mut result = ToolResultMessage::text(&call("shell", json!({})), "ok");
        result.permission = note.map(Box::new);
        Item::Tool {
            call: call("shell", json!({ "command": "make" })),
            result: Some(result),
            live: None,
            at: "12:00".into(),
            duration_ms: Some(10),
            waited_ms: None,
            waiting: false,
        }
    };
    transcript.push(finished(Some(PermissionNote {
        lasting: Some(Lasting::Session),
        ..PermissionNote::new(DecidedBy::User, true)
    })));
    transcript.push(finished(Some(PermissionNote {
        reason: "not asked for".into(),
        ..PermissionNote::new(DecidedBy::Reviewer, false)
    })));
    transcript.push(finished(Some(PermissionNote::new(DecidedBy::Rules, true))));
    let lines = text_of(transcript.lines(80, &colors, false));
    let t = termide_i18n::t();
    assert!(lines
        .iter()
        .any(|l| l.contains(&format!("✓ {}", t.agent_perm_note_user_session()))));
    assert!(lines.iter().any(|l| l.contains(&format!(
        "✗ {}",
        t.agent_perm_note_reviewer_blocked_fmt("not asked for")
    ))));
    // What the rules allowed without asking is the common case, unsaid.
    let decisions = lines
        .iter()
        .filter(|l| l.starts_with("  ✓ ") || l.starts_with("  ✗ "))
        .count();
    assert_eq!(decisions, 2);
}

#[test]
fn autofold_off_leaves_every_block_expanded() {
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::Never);
    transcript.push(Item::User {
        text: "hi".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    // A long output is foldable, so autofold-off keeps it open.
    let body = (1..=8).map(|n| format!("line {n}")).collect::<Vec<_>>();
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "ls" })),
        result: Some(ToolResultMessage::text(
            &call("shell", json!({})),
            body.join("\n"),
        )),
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    assert!(transcript.any_expanded());
}

#[test]
fn short_blocks_are_not_foldable() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    // A call with no output and a one-line command has nothing to hide.
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "echo hi" })),
        result: Some(ToolResultMessage::text(&call("shell", json!({})), "")),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(300),
        waited_ms: None,
        waiting: false,
    });
    transcript.push(Item::Assistant {
        text: "the answer".into(),
        streaming: false,
        error: None,
        at: "12:00:01".into(),
        cost: None,
        run_ms: None,
    });
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(lines.iter().all(|l| !l.contains('▸') && !l.contains('▾')));
    // Its meta shares the headline row.
    assert!(lines
        .iter()
        .any(|l| l.starts_with("$ Running echo hi") && l.contains("🕒") && !l.contains('✓')));
    assert!(lines.iter().any(|l| l.contains("› the answer")));
    // No small block folds on request (tool 0, answer 1).
    assert!(!transcript.toggle_expanded(0));
    assert!(!transcript.toggle_expanded(1));
    assert!(!transcript.any_expanded());
}

#[test]
fn a_long_command_wraps_under_its_prompt() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let command = "cargo test --workspace --all-features -- --nocapture";
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": command })),
        result: None,
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let lines = text_of(transcript.lines(30, &colors, false));
    // No row is clipped: every one fits, continuation rows indent under
    // the command, past `$ Running `, and the rows read back as it.
    assert!(lines.iter().all(|l| width_of(l) <= 30), "{lines:?}");
    // A tool call has no dividing rule above it.
    assert!(lines[0].starts_with("$ Running cargo test"), "{lines:?}");
    let indent = " ".repeat(width_of("$ Running "));
    assert!(lines[1].starts_with(&indent));
    let joined: String = lines
        .iter()
        .map(|l| {
            l.strip_prefix("$ Running ")
                .or(l.strip_prefix(indent.as_str()))
                .unwrap()
        })
        .collect();
    assert_eq!(joined, command);
}

#[test]
fn a_long_command_folds_to_its_first_line() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let script = (1..=9)
        .map(|n| format!("echo {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": script })),
        result: Some(ToolResultMessage::text(&call("shell", json!({})), "ok")),
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    // Collapsed: the first command line behind the marker, nothing else.
    let lines = text_of(transcript.lines(40, &colors, false));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("$ Running ▸ echo 1"), "{lines:?}");
    assert!(transcript.toggle_expanded(0));
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(lines.iter().any(|l| l.trim() == "echo 9"));
    assert!(lines[0].starts_with("$ Running ▾ echo 1"));
    // Continuation lines align under the command, past `$ Running ▾ `.
    let indent = " ".repeat(width_of("$ Running ▾ "));
    assert!(
        lines[1].starts_with(&format!("{indent}echo 2")),
        "{lines:?}"
    );
}

#[test]
fn a_suggested_command_reads_as_a_shell_call_with_its_own_action() {
    // `[Run]` sends an offered command through the shell, so its block is
    // a shell call's — the `$`, the command wrapped under itself — and
    // only the action says the user, not the agent, runs it.
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let offer = call(
        "suggest_command",
        json!({ "command": "cd x\nmake install", "why": "needs sudo" }),
    );
    transcript.push(Item::Tool {
        call: offer.clone(),
        result: Some(ToolResultMessage::text(&offer, "ok")),
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let lines = text_of(transcript.lines(40, &colors, false));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("$ Suggesting ▸ cd x"), "{lines:?}");
    assert!(transcript.toggle_expanded(0));
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(lines[0].starts_with("$ Suggesting ▾ cd x"), "{lines:?}");
    let indent = " ".repeat(width_of("$ Suggesting ▾ "));
    assert!(
        lines[1].starts_with(&format!("{indent}make install")),
        "{lines:?}"
    );
}

#[test]
fn finished_reasoning_folds_to_one_line_without_a_rule() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::OnFinish);
    transcript.stream_thinking("a quick thought\nand a second one");
    let lines = text_of(transcript.lines(60, &colors, false));
    // Streaming: all of it, no marker, no dividing rule.
    assert!(
        lines[0].starts_with("@ Thinking a quick thought"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("second one")));
    assert!(lines.iter().all(|l| !l.starts_with('╌')));
    let cost = Cost {
        prefill_ms: 1000,
        gen_ms: 4000,
        input: 512,
        output: 300,
    };
    assert!(transcript.finish_thinking("12:00:00", Some(cost)));
    // Finished: the first line alone; the timing is one unfold away.
    let lines = text_of(transcript.lines(60, &colors, false));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("@ Thinking ▸ a quick thought"),
        "{lines:?}"
    );
    assert!(
        !lines[0].contains("🕒") && !lines[0].contains("⏫") && !lines[0].contains("✍"),
        "{lines:?}"
    );
    // Unfolded: the whole text and the full cost lines.
    assert!(transcript.toggle_expanded(0));
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(
        lines[0].starts_with("@ Thinking ▾ a quick thought"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("second one")));
    assert!(lines.iter().any(|l| l.contains("↑512")));
}

/// Codex and Gemini CLI head a thought with its subject in bold; the
/// block shows the subject without the marks, and leaves bold inside a
/// line as it is.
#[test]
fn a_thoughts_bold_subject_shows_without_its_marks() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::OnFinish);
    transcript.stream_thinking("**Reading the report**\nIt has **four** columns.");
    let lines = text_of(transcript.lines(60, &colors, false));
    let lines: Vec<&str> = lines.iter().map(|l| l.trim_end()).collect();
    assert_eq!(
        lines,
        vec![
            "@ Thinking Reading the report",
            "           It has **four** columns."
        ]
    );
    assert!(transcript.finish_thinking("12:00:00", None));
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(
        lines[0].starts_with("@ Thinking ▸ Reading the report"),
        "{lines:?}"
    );
}

#[test]
fn unfolded_reasoning_keeps_its_line_breaks() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::OnFinish);
    transcript.stream_thinking("first step\nsecond step\n\nafter a gap");
    let lines = text_of(transcript.lines(60, &colors, false));
    let lines: Vec<&str> = lines.iter().map(|l| l.trim_end()).collect();
    assert_eq!(
        lines,
        vec![
            "@ Thinking first step",
            "           second step",
            "",
            "           after a gap"
        ]
    );
}

#[test]
fn a_highlight_covers_the_content_rows_of_a_block() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::User {
        text: "go".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    transcript.stream_thinking("a thought\nand more");
    transcript.finish_thinking("12:00:00", None);
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "ls" })),
        result: Some(ToolResultMessage::text(&call("shell", json!({})), "a\nb")),
        live: None,
        at: "12:00:01".into(),
        duration_ms: Some(100),
        waited_ms: None,
        waiting: false,
    });
    transcript.stream_answer("done");
    transcript.finish_assistant("done".into(), None, None, "12:00:02".into(), false);
    let lines = text_of(transcript.lines(40, &colors, false));
    let rows = |index: usize| {
        let (first, last) = transcript.content_lines_of(index).unwrap();
        lines[first..=last].to_vec()
    };
    // The user plate without the plain gaps around it.
    let user = rows(0);
    assert!(user.iter().any(|l| l.starts_with("› go")), "{user:?}");
    assert!(user
        .first()
        .is_some_and(|l| l.trim().is_empty() && !l.is_empty()));
    // A folded step is its single row, the first row included.
    assert_eq!(rows(1).len(), 1);
    assert!(rows(1)[0].starts_with("@ Thinking ▸ a thought"));
    assert_eq!(rows(2).len(), 1);
    assert!(rows(2)[0].starts_with("$ Running ▸ ls"));
    // Unfolded, the headline row is part of it too.
    assert!(transcript.toggle_expanded(2));
    let lines = text_of(transcript.lines(40, &colors, false));
    let (first, _) = transcript.content_lines_of(2).unwrap();
    assert!(lines[first].starts_with("$ Running ▾ ls"), "{lines:?}");
    // The answer without the blank line above it.
    let (first, _) = transcript.content_lines_of(3).unwrap();
    assert!(lines[first].starts_with("› done"), "{lines:?}");
}

#[test]
fn a_resting_clock_is_dim_and_a_live_pause_stands_out() {
    let colors = ThemeColors::default();
    let glyph_fg = |transcript: &mut Transcript, glyph: &str| {
        let lines = transcript.lines(40, &colors, false).to_vec();
        lines
            .iter()
            .rev()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content.starts_with(glyph))
            .and_then(|s| s.style.fg)
    };
    let mut transcript = Transcript::default();
    transcript.stream_answer("done");
    transcript.finish_assistant("done".into(), None, None, "12:00:00".into(), false);
    transcript.end_run(5_000, "12:00:00", true, false);
    assert_eq!(glyph_fg(&mut transcript, RUN_GLYPH), Some(colors.disabled));
    // A pause on is marked; once it ends it rests, dimmed.
    transcript.end_run(0, "", true, true);
    assert_eq!(
        glyph_fg(&mut transcript, PAUSED_GLYPH),
        Some(colors.warning)
    );
    transcript.finish_pause(3_000);
    assert_eq!(
        glyph_fg(&mut transcript, PAUSED_GLYPH),
        Some(colors.disabled)
    );
    // A pause that ended no longer ticks.
    assert!(!transcript.set_pause_length(9_000));
}

#[test]
fn a_clean_run_puts_its_total_on_the_answer() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::User {
        text: "go".into(),
        at: "21:00:00".into(),
        command: None,
        images: Vec::new(),
    });
    transcript.stream_answer("done");
    transcript.finish_assistant("done".into(), None, None, "21:03:41".into(), false);
    transcript.end_run(221_000, "21:03:41", true, false);
    assert_eq!(transcript.items().len(), 2, "no closing line");
    let lines = text_of(transcript.lines(40, &colors, false));
    // The times share their text's row when it fits; the total is `✻`.
    assert!(lines
        .iter()
        .any(|l| l.starts_with("› go") && l.contains("21:00:00 ✓")));
    assert!(lines
        .iter()
        .any(|l| l.starts_with("› done") && l.contains("21:03:41 ✓")));
    assert!(lines.iter().any(|l| l.trim() == "✻ 3m41s"), "{lines:?}");
    // Too narrow to share, the time takes its own row.
    let lines = text_of(transcript.lines(14, &colors, false));
    assert!(lines.iter().any(|l| l.trim() == "21:03:41 ✓"), "{lines:?}");
    // A paused or failed run, or one ending on a tool call, still closes
    // with a line that says how it ended.
    transcript.end_run(5_000, "21:04:00", false, false);
    assert!(matches!(
        transcript.items().last(),
        Some(Item::RunEnd { ok: false, .. })
    ));
}

#[test]
fn a_failed_run_puts_its_total_on_the_failed_answer() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.finish_assistant(
        String::new(),
        Some("connection refused".into()),
        None,
        "11:35:56".into(),
        false,
    );
    transcript.end_run(3_000, "11:35:56", false, false);
    // No separate closing line: the answer already has the time and `✗`.
    assert_eq!(transcript.items().len(), 1);
    let lines = text_of(transcript.lines(50, &colors, false));
    assert_eq!(
        lines.iter().filter(|l| l.contains("11:35:56")).count(),
        1,
        "{lines:?}"
    );
    assert_eq!(lines.last().map(|l| l.trim()), Some("✻ 3s"), "{lines:?}");
    // An answer that went fine before the run was aborted keeps its `✓`,
    // so the run's `✗` needs its own line.
    transcript.stream_answer("partial");
    transcript.finish_assistant("partial".into(), None, None, "11:36:00".into(), false);
    transcript.end_run(1_000, "11:36:01", false, false);
    assert!(matches!(
        transcript.items().last(),
        Some(Item::RunEnd { ok: false, .. })
    ));
}

#[test]
fn a_call_prefix_is_accent_while_running_then_success() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "true" })),
        result: None,
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let fg_of = |lines: &[Line<'static>], content: &str| {
        lines[0]
            .spans
            .iter()
            .find(|s| s.content == content)
            .and_then(|s| s.style.fg)
    };
    let lines = transcript.lines(40, &colors, false).to_vec();
    assert_eq!(fg_of(&lines, "$ "), Some(colors.info));
    let mut transcript = Transcript::default();
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "true" })),
        result: Some(ToolResultMessage::text(&call("shell", json!({})), "")),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(500),
        waited_ms: None,
        waiting: false,
    });
    let lines = transcript.lines(40, &colors, false).to_vec();
    assert_eq!(fg_of(&lines, "$ "), Some(colors.success));
    assert_ne!(fg_of(&lines, "true"), Some(colors.success));
}

#[test]
fn a_thinking_marker_is_accent_while_streaming_then_dim() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.stream_thinking("Weighing the options.");
    let at_fg = |transcript: &mut Transcript| {
        transcript
            .lines(60, &colors, false)
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == "@ ")
            .and_then(|s| s.style.fg)
    };
    assert_eq!(at_fg(&mut transcript), Some(colors.info));
    transcript.finish_thinking("12:00:00", None);
    assert_eq!(at_fg(&mut transcript), Some(colors.disabled));
}

#[test]
fn a_failed_one_row_call_is_painted_instead_of_marked() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "exit 1" })),
        result: Some(ToolResultMessage::error(
            &call("shell", json!({})),
            "[exit code 1]",
        )),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(500),
        waited_ms: None,
        waiting: false,
    });
    let lines = transcript.lines(40, &colors, false).to_vec();
    assert_eq!(lines.len(), 1);
    let text = text_of(&lines)[0].clone();
    assert!(
        text.starts_with("$ Running ▸ exit 1") && !text.contains('✗'),
        "{text}"
    );
    // Only the prefix takes the error color; the command stays dim.
    let fg_of = |content: &str| {
        lines[0]
            .spans
            .iter()
            .find(|s| s.content == content)
            .unwrap()
            .style
            .fg
    };
    assert_eq!(fg_of("$ "), Some(colors.error));
    assert_eq!(fg_of("exit 1"), Some(colors.disabled));
    // Unfolded, the output is there and the meta row keeps the `✗`.
    assert!(transcript.toggle_expanded(0));
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(lines.iter().any(|l| l.contains("🕒") && l.contains('✗')));
}

#[test]
fn a_one_row_block_carries_no_fold_marker() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.stream_thinking("a quick thought");
    assert!(transcript.finish_thinking("12:00:00", None));
    // Unfolded it is a single row, so there is nothing to fold.
    let lines = text_of(transcript.lines(60, &colors, false));
    assert_eq!(lines, vec!["@ Thinking a quick thought".to_string()]);
    assert!(!transcript.toggle_expanded(0));
    assert!(!transcript.any_expanded());
    // Narrow enough to wrap, the same thought folds to its first row.
    let lines = text_of(transcript.lines(20, &colors, false));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("@ Thinking ▸ "), "{lines:?}");
    assert!(transcript.toggle_expanded(0));
}

#[test]
fn folding_immediately_keeps_running_blocks_to_one_line() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    assert_eq!(transcript.fold, FoldMode::Immediately);
    transcript.stream_thinking("a first thought\nthe latest one");
    let lines = text_of(transcript.lines(60, &colors, false));
    // Streaming reasoning is one row, following its latest line.
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("▸ the latest one"), "{lines:?}");
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "cargo build" })),
        result: None,
        live: Some("Compiling a\nCompiling b\n".into()),
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let lines = text_of(transcript.lines(60, &colors, false));
    // The running call is its headline alone, its output folded away.
    assert!(lines.iter().any(|l| l.contains("cargo build")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("Compiling")), "{lines:?}");
}

#[test]
fn a_running_tool_shows_in_full_then_folds_to_one_line() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::OnFinish);
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "cargo build" })),
        result: None,
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    // While it runs, every line of the live output shows, with no fold
    // marker, and a click does not fold it.
    let output = (1..=8)
        .map(|n| format!("step {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let live_output = output.clone();
    transcript.with_tool("c1", |item| {
        if let Item::Tool { live, .. } = item {
            *live = Some(live_output);
        }
    });
    let lines = text_of(transcript.lines(40, &colors, false));
    assert_eq!(lines.len(), 9, "{lines:?}");
    assert_eq!(lines[0].trim_end(), "$ Running cargo build");
    assert!(lines.iter().any(|l| l.trim() == "step 1"));
    assert!(!transcript.toggle_expanded(0));
    // Finished, it folds to its headline with the meta at the row's end.
    transcript.with_tool("c1", |item| {
        if let Item::Tool {
            result,
            live,
            at,
            duration_ms,
            ..
        } = item
        {
            *result = Some(ToolResultMessage::text(&call("shell", json!({})), &output));
            *live = None;
            *at = "12:00:00".into();
            *duration_ms = Some(3000);
        }
    });
    let lines = text_of(transcript.lines(40, &colors, false));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("$ Running ▸ cargo build"), "{lines:?}");
    assert!(!lines[0].contains("🕒") && !lines[0].contains('✓'));
    assert!(width_of(&lines[0]) <= 40);
    // Too long for the row, the headline is clipped.
    let lines = text_of(transcript.lines(20, &colors, false));
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains('…'), "{lines:?}");
    assert!(width_of(&lines[0]) <= 20, "{lines:?}");
}

#[test]
fn a_blank_line_sets_an_answer_apart_from_the_next_step() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Assistant {
        text: "Let me look.".into(),
        error: None,
        at: "12:00:00".into(),
        cost: None,
        run_ms: None,
        streaming: false,
    });
    // Last in the chat, the answer ends at its meta row.
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(!lines.last().is_some_and(String::is_empty), "{lines:?}");
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "ls" })),
        result: None,
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let lines = text_of(transcript.lines(40, &colors, false));
    let tool = lines.iter().position(|l| l.starts_with("$ ")).unwrap();
    assert_eq!(lines[tool - 1], "", "{lines:?}");
    assert!(!lines[tool - 2].is_empty(), "{lines:?}");
    // The gap belongs to the answer, outside its highlight.
    assert_eq!(transcript.item_at_line(tool - 1), Some(0));
    assert_eq!(transcript.content_lines_of(0), Some((1, tool - 2)));
}

#[test]
fn question_answers_wrap_under_their_item() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::Never);
    let body = "The user answered:\n- Which approach? → keep the channel open between turns";
    let question = call(
        "question",
        json!({ "questions": [{ "question": "Which approach?" }] }),
    );
    transcript.push(Item::Tool {
        result: Some(ToolResultMessage::text(&question, body)),
        call: question,
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let lines = text_of(transcript.lines(30, &colors, false));
    let first = lines
        .iter()
        .position(|l| l.starts_with("  - Which"))
        .expect("the answer line");
    // Nothing is clipped: every row fits, and the words all come through.
    assert!(lines.iter().all(|l| width_of(l) <= 30), "{lines:#?}");
    assert!(lines[first + 1].starts_with("    "), "{lines:#?}");
    let joined: String = lines[first..]
        .iter()
        .take_while(|l| l.starts_with("  "))
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.contains("open between turns"), "{joined}");
}

#[test]
fn an_edit_result_is_colored_like_a_diff() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::Never);
    let body = "Edited a.rs (1 replacement).\n\n--- a/a.rs\n+++ b/a.rs\n\
                    @@ -1,2 +1,2 @@\n keep\n-old\n+new\n--- dashes\n";
    transcript.push(Item::Tool {
        call: call("edit", json!({ "path": "a.rs" })),
        result: Some(ToolResultMessage::text(&call("edit", json!({})), body)),
        live: None,
        at: String::new(),
        duration_ms: None,
        waited_ms: None,
        waiting: false,
    });
    let lines = transcript.lines(30, &colors, false).to_vec();
    let style_of = |needle: &str| {
        let line = lines
            .iter()
            .find(|l| l.spans.iter().any(|s| s.content == needle))
            .unwrap_or_else(|| panic!("no line {needle:?}"));
        let span = line.spans.iter().find(|s| s.content == needle).unwrap();
        (span.style, line)
    };
    let (added, row) = style_of("+new");
    assert_eq!(added.fg, Some(colors.success));
    assert!(added.bg.is_some());
    // The tint runs across the whole row, as in the git diff panel.
    assert_eq!(width_of(&text_of(std::slice::from_ref(row))[0]), 30);
    let (removed, _) = style_of("-old");
    assert_eq!(removed.fg, Some(colors.error));
    // Inside a hunk a line of dashes is a removal, not a file header.
    assert_eq!(style_of("--- dashes").0.fg, Some(colors.error));
    assert_eq!(style_of("--- a/a.rs").0.fg, Some(colors.info));
    assert_eq!(style_of(" keep").0.fg, Some(colors.fg));
    // Without details (an older session), the text shows as it came.
    assert_eq!(style_of("Edited a.rs (1 replacement).").0.bg, None);
}

/// An edit result as the tool returns it: the model's text, which names
/// the file, and the details the UI draws from.
fn edit_result(text: &str, strategy: &str) -> ToolResultMessage {
    let diff = "--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,2 @@\n keep\n-old\n-gone\n+new\n";
    ToolResultMessage::text(&call("edit", json!({})), text).with_details(json!({
        "path": "a.rs",
        "replacements": 1,
        "strategy": strategy,
        "diff": diff,
    }))
}

fn push_edit(transcript: &mut Transcript, path: &str, result: ToolResultMessage) {
    transcript.push(Item::Tool {
        call: call("edit", json!({ "path": path })),
        result: Some(result),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    });
}

#[test]
fn an_edit_shows_its_hunks_and_counts_from_the_details() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::Never);
    // The model was told the diff is too large; the user still sees it.
    push_edit(
        &mut transcript,
        "a.rs",
        edit_result(
            "Edited /abs/a.rs (1 replacement). The diff is too large to show inline.",
            "",
        ),
    );
    let lines = text_of(transcript.lines(40, &colors, false));
    assert!(
        lines[0].contains("a.rs") && lines[0].ends_with("+1 −2"),
        "{lines:?}"
    );
    assert!(lines.iter().any(|l| l.contains("@@ -1,3 +1,2 @@")));
    assert!(lines.iter().any(|l| l.contains("+new")));
    // The summary and the file header repeat the headline's path.
    assert!(!lines
        .iter()
        .any(|l| l.contains("Edited") || l.contains("--- a/")));
    assert!(!lines.iter().any(|l| l.contains("whitespace")));
}

#[test]
fn an_edit_wraps_a_long_path_and_keeps_its_counts() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let path = "/a/very/long/path/that/does/not/fit/the/row/at/all/a.rs";
    push_edit(
        &mut transcript,
        path,
        edit_result("Edited a.rs (1 replacement).", ""),
    );
    // Folded, and unfolded, the path wraps whole and the counts follow it.
    let folded = text_of(transcript.lines(40, &colors, false));
    assert!(folded.len() > 1, "{folded:?}");
    assert!(folded.iter().all(|row| width_of(row) <= 40), "{folded:?}");
    assert!(!folded.concat().contains('…'), "{folded:?}");
    let joined: String = folded.iter().map(|row| row.trim()).collect();
    assert!(joined.contains(path), "{folded:?}");
    assert!(folded.last().unwrap().ends_with("+1 −2"), "{folded:?}");
    assert!(transcript.toggle_expanded(0));
    let unfolded = text_of(transcript.lines(40, &colors, false));
    assert!(
        unfolded.iter().any(|row| row.ends_with("+1 −2")),
        "{unfolded:?}"
    );
}

#[test]
fn a_folded_task_wraps_its_subject_under_the_action() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Tool {
            call: call(
                "task",
                json!({ "agent": "clear", "prompt": "check every call site of the parser and report which ones skip validation" }),
            ),
            result: Some(ToolResultMessage::text(&call("task", json!({})), "done")),
            live: None,
            at: "12:00:00".into(),
            duration_ms: Some(10),
            waited_ms: None,
            waiting: false,
        });
    let rows = text_of(transcript.lines(40, &colors, false));
    assert!(rows.len() > 1, "{rows:?}");
    assert!(rows.iter().all(|row| width_of(row) <= 40), "{rows:?}");
    assert!(!rows.concat().contains('…'), "{rows:?}");
    let joined = rows
        .iter()
        .map(|row| row.trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(joined.ends_with("which ones skip validation"), "{rows:?}");
    // Continuation rows line up under the subject, past the action.
    let indent = rows[0].find("clear ·").unwrap();
    let indent = width_of(&rows[0][..indent]);
    assert!(rows[1].starts_with(&" ".repeat(indent)), "{rows:?}");
}

#[test]
fn a_wrapped_fetch_headline_links_every_row_to_its_url() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let url = "https://docs.rs/termide/latest/termide/links/index.html";
    transcript.push(Item::Tool {
        call: call("fetch", json!({ "url": url })),
        result: Some(ToolResultMessage::text(&call("fetch", json!({})), "page")),
        live: None,
        at: "12:00:00".into(),
        duration_ms: Some(10),
        waited_ms: None,
        waiting: false,
    });
    let rows = text_of(transcript.lines(40, &colors, false));
    assert!(rows.len() > 1, "{rows:?}");
    // The second row's piece of the address opens the whole of it, and
    // lights the rows above and below with it.
    let col = rows[1].len() - rows[1].trim_start().len();
    let (href, regions) = transcript.link_at(1, col).expect("a link");
    assert_eq!(href, url);
    assert_eq!(regions.len(), rows.len(), "{regions:?}");
    // The indent before it is not the link.
    assert!(transcript.link_at(1, col - 1).is_none());
}

#[test]
fn an_answer_link_sits_past_the_mark_and_the_gap() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Assistant {
        text: "see [x](a.md)".into(),
        error: None,
        at: String::new(),
        cost: None,
        run_ms: None,
        streaming: false,
    });
    let rows = text_of(transcript.lines(40, &colors, false));
    let line = rows.iter().position(|r| r.contains("see x")).unwrap();
    let row = &rows[line];
    let col = row[..row.find(" x").unwrap()].chars().count() + 1;
    assert_eq!(
        transcript
            .link_at(line, col)
            .map(|(href, _)| href)
            .as_deref(),
        Some("a.md")
    );
    assert!(transcript.link_at(line, col - 1).is_none());
}

#[test]
fn folded_reasoning_still_clips_to_one_row() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Thinking {
        text: "a long first line of reasoning that goes well past the row\nmore".into(),
        streaming: false,
        at: "12:00:00".into(),
        cost: None,
    });
    let rows = text_of(transcript.lines(40, &colors, false));
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(rows[0].ends_with('…'), "{rows:?}");
}

#[test]
fn a_loose_edit_match_is_noted() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::Never);
    push_edit(
        &mut transcript,
        "a.rs",
        edit_result(
            "Edited a.rs (1 replacement, matched ignoring indentation).",
            "ignoring indentation",
        ),
    );
    let joined = text_of(transcript.lines(60, &colors, false)).join("\n");
    assert!(joined.contains("whitespace"), "{joined}");
}

#[test]
fn a_failed_edit_wraps_its_error() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.set_fold(FoldMode::Never);
    push_edit(
        &mut transcript,
        "a.rs",
        ToolResultMessage::error(
            &call("edit", json!({})),
            "old_string not found in /a/long/path/to/a.rs; read the file again.",
        ),
    );
    let joined = text_of(transcript.lines(30, &colors, false)).join(" ");
    assert!(joined.contains("again."), "{joined}");
}

#[test]
fn a_run_closes_with_its_total_time() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::User {
        text: "go".into(),
        at: "21:00:00".into(),
        command: None,
        images: Vec::new(),
    });
    transcript.push(Item::RunEnd {
        elapsed_ms: 221_000,
        at: "21:03:41".into(),
        ok: true,
        paused: false,
        live: false,
        cut: false,
    });
    let lines = text_of(transcript.lines(60, &colors, false));
    // The frozen run clock, right-aligned with no rule above it.
    let last = lines.last().unwrap();
    assert_eq!(last.trim(), "✻ 3m41s · 21:03:41 ✓", "{lines:?}");
    assert!(lines.iter().all(|l| !l.starts_with('╌')), "{lines:?}");
    // A paused run rests on `‖` and carries no status.
    transcript.push(Item::RunEnd {
        elapsed_ms: 72_000,
        at: String::new(),
        ok: true,
        paused: true,
        live: true,
        cut: false,
    });
    let lines = text_of(transcript.lines(60, &colors, false));
    assert_eq!(lines.last().unwrap().trim(), "‖ 1m12s");
    // It is a closing line, not a block: never folded, never selected.
    assert!(!transcript.toggle_expanded(1));
    assert!(!transcript.is_selectable(1));
    assert_eq!(transcript.selectable_near(1), Some(0));
}

#[test]
fn notices_carry_no_rule() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.push(Item::Assistant {
        text: "done".into(),
        streaming: false,
        error: None,
        at: String::new(),
        cost: None,
        run_ms: None,
    });
    transcript.push(Item::RunEnd {
        elapsed_ms: 5000,
        at: "12:00:05".into(),
        ok: false,
        paused: false,
        live: false,
        cut: false,
    });
    transcript.push(Item::Notice {
        text: "goal stopped".into(),
        kind: NoticeKind::Warn,
    });
    transcript.push(Item::Notice {
        text: "queue cleared".into(),
        kind: NoticeKind::Info,
    });
    let rule = |lines: &[String]| lines.iter().filter(|l| l.starts_with('╌')).count();
    let lines = text_of(transcript.lines(40, &colors, false));
    // The answer opens with a blank line; neither the run's closing line
    // nor the notices draw a rule: each notice is its glyph and text.
    assert_eq!(lines[0], "", "{lines:?}");
    assert_eq!(rule(&lines), 0, "{lines:?}");
    let n = lines.len();
    assert!(lines[n - 3].trim().starts_with("✻ ") && lines[n - 3].ends_with('✗'));
    assert_eq!(lines[n - 2], "! goal stopped");
    assert_eq!(lines[n - 1], "· queue cleared");
    // A long notice wraps under its glyph.
    transcript.push(Item::User {
        text: "next".into(),
        at: String::new(),
        command: None,
        images: Vec::new(),
    });
    transcript.push(Item::Notice {
        text: "a long notice that has to wrap onto a second row".into(),
        kind: NoticeKind::Info,
    });
    let lines = text_of(transcript.lines(20, &colors, false));
    let at = lines
        .iter()
        .position(|l| l.starts_with("· a long"))
        .unwrap();
    assert!(!lines[at - 1].starts_with('╌'));
    assert!(lines[at + 1].starts_with("  "), "{lines:?}");
    assert!(lines.iter().all(|l| width_of(l) <= 20), "{lines:?}");
}

#[test]
fn errors_and_notices_are_visible() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    transcript.finish_assistant(
        String::new(),
        Some("HTTP 500".into()),
        None,
        "12:00:00".into(),
        false,
    );
    transcript.push(Item::Notice {
        text: "compacted 1200 tokens".into(),
        kind: NoticeKind::Info,
    });
    transcript.push(Item::Tool {
        call: call("shell", json!({ "command": "exit 1" })),
        result: Some(ToolResultMessage::error(
            &call("shell", json!({})),
            "[exit code 1]",
        )),
        live: None,
        at: "12:00:05".into(),
        duration_ms: Some(500),
        waited_ms: None,
        waiting: false,
    });
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(lines.iter().any(|l| l.contains("✗ HTTP 500")));
    assert!(lines.iter().any(|l| l.contains("· compacted 1200 tokens")));
    // A notice can be selected, so an error can be copied.
    assert!(transcript.is_selectable(1));
    // Shell headline uses `$`; folded to one row, it shows neither how
    // long it took nor a status glyph.
    assert!(lines
        .iter()
        .any(|l| l.contains("$ Running ▸ exit 1") && !l.contains("🕒") && !l.contains('✗')));
}

#[test]
fn a_turn_error_after_reasoning_shows_on_the_answer() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    // Reasoning streams, then the turn fails with no answer text.
    transcript.stream_thinking("weighing it\nl2\nl3\nl4\nl5\nl6");
    transcript.finish_thinking("12:00:00", None);
    transcript.finish_assistant(
        String::new(),
        Some("HTTP 500".into()),
        None,
        "12:00:01".into(),
        true,
    );
    // The error and its failed status land on an answer block, even though
    // the answer text is empty; the reasoning block carries no status.
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(lines.iter().any(|l| l.contains("✗ HTTP 500")));
    assert!(lines
        .iter()
        .any(|l| l.contains("12:00:01") && l.contains('✗')));
}

#[test]
fn a_long_error_wraps_to_the_width() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let error = "the request failed because the endpoint is unreachable and \
             the retries were exhausted after several attempts"
        .to_string();
    transcript.finish_assistant(String::new(), Some(error), None, "12:00:01".into(), true);
    let width = 30;
    let lines = text_of(transcript.lines(width, &colors, false));
    // The error is marked with `✗` and reflows instead of spilling past the
    // width; every wrapped row stays within it.
    assert!(lines.iter().any(|l| l.contains('✗')));
    let wrapped = lines
        .iter()
        .filter(|l| l.contains("request") || l.contains("retries"))
        .count();
    assert!(wrapped >= 2, "a long error should wrap to several rows");
    assert!(lines.iter().all(|l| l.chars().count() <= width as usize));
}

#[test]
fn the_system_prompt_folds_with_a_marker() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let prompt = (1..=8)
        .map(|n| format!("rule {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    transcript.push(Item::System { text: prompt });
    // Collapsed: a `▸` marker after the `#`, the first line, a note, then
    // the last few — the ellipsis sits between the first and the last few.
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(lines.iter().any(|l| l.contains("# ▸ rule 1")));
    assert!(lines.iter().any(|l| l.contains("… 3 more lines")));
    assert!(lines.iter().any(|l| l.contains("rule 5")));
    assert!(lines.iter().any(|l| l.contains("rule 8")));
    // The middle lines are the hidden ones.
    assert!(!lines.iter().any(|l| l.contains("rule 2")));
    assert!(!lines.iter().any(|l| l.contains("rule 4")));
    // Expanded: a `▾` marker and the whole prompt.
    assert!(transcript.toggle_expanded(0));
    let lines = text_of(transcript.lines(60, &colors, false));
    assert!(lines.iter().any(|l| l.contains("# ▾ rule 1")));
    assert!(lines.iter().any(|l| l.contains("rule 4")));
    assert!(lines.iter().any(|l| l.contains("rule 8")));
    // The `#` is dim like the prompt itself, not an accent.
    let hash_fg = transcript
        .lines(60, &colors, false)
        .iter()
        .flat_map(|l| l.spans.iter())
        .find(|s| s.content == "# ")
        .and_then(|s| s.style.fg);
    assert_eq!(hash_fg, Some(colors.disabled));
}

#[test]
fn web_tools_head_with_their_glyph_and_subject() {
    let colors = ThemeColors::default();
    let text = |call: &ToolCall| -> String {
        tool_headline(call, None, 80, colors.info, &colors)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    let t = termide_i18n::t();
    assert_eq!(
        text(&call("fetch", json!({ "url": "https://docs.rs" }))),
        format!("↓ {} https://docs.rs", t.agent_tool_fetch())
    );
    // The fetched URL is dim, as a link.
    let spans = tool_headline(
        &call("fetch", json!({ "url": "https://docs.rs" })),
        None,
        80,
        colors.info,
        &colors,
    );
    let url = spans.last().unwrap();
    assert_eq!(url.content, "https://docs.rs");
    assert_eq!(url.style.fg, Some(colors.disabled));
    assert_eq!(
        text(&call(
            "web_search",
            json!({ "query": "rust tui", "limit": 3 })
        )),
        format!("? {} rust tui", t.agent_tool_web_search())
    );
    assert_eq!(
        text(&call(
            "recall",
            json!({ "queries": ["why no tokio", "почему без tokio"] })
        )),
        format!("↺ {} why no tokio (+1)", t.agent_tool_recall())
    );
    assert_eq!(
        text(&call("recall", json!({ "open": "commit:.@08c2e3b1" }))),
        format!("↺ {} commit:.@08c2e3b1", t.agent_tool_recall())
    );
}

/// `debug` heads with a localized verb per action, never the raw
/// arguments, and links the file a breakpoint names.
#[test]
fn debug_calls_head_with_their_action() {
    let colors = ThemeColors::default();
    let text = |args: Value| -> String {
        tool_headline(&call("debug", args), None, 80, colors.info, &colors)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    let t = termide_i18n::t();
    assert_eq!(
        text(json!({ "action": "start", "command": "debug-server" })),
        format!("→ {} debug-server", t.agent_tool_debug_start())
    );
    assert_eq!(
        text(json!({ "action": "start", "file": "main.py" })),
        format!("→ {} main.py", t.agent_tool_debug_start())
    );
    assert_eq!(
        text(json!({ "action": "breakpoint", "file": "src/a.rs", "line": 42 })),
        format!("● {} src/a.rs:42", t.agent_tool_debug_set_breakpoint())
    );
    assert_eq!(
        text(json!({ "action": "breakpoint", "file": "src/a.rs", "line": 42, "set": false })),
        format!("● {} src/a.rs:42", t.agent_tool_debug_clear_breakpoint())
    );
    assert_eq!(
        text(json!({ "action": "step_over" })),
        format!("→ {} ", t.agent_tool_debug_step_over())
    );
    assert_eq!(
        text(json!({ "action": "evaluate", "expression": " a\n+ 1 " })),
        format!("→ {} a + 1", t.agent_tool_debug_evaluate())
    );
    assert_eq!(
        text(json!({ "action": "frame", "frame": 2 })),
        format!("→ {} 2", t.agent_tool_debug_frame())
    );
    assert!(!text(json!({ "action": "state" })).contains('{'));
    assert_eq!(
        headline_href(&call(
            "debug",
            json!({ "action": "breakpoint", "file": "src/a.rs", "line": 4 })
        ))
        .as_deref(),
        Some("src/a.rs")
    );
    assert_eq!(
        headline_href(&call(
            "debug",
            json!({ "action": "start", "command": "debug-server" })
        )),
        None
    );
}

/// `terminals` heads with a `$` and a localized verb per action, never
/// its raw arguments.
#[test]
fn terminal_calls_head_with_their_action() {
    let colors = ThemeColors::default();
    let text = |args: Value| -> String {
        tool_headline(&call("terminals", args), None, 80, colors.info, &colors)
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    };
    let t = termide_i18n::t();
    assert_eq!(text(json!({})), format!("$ {} ", t.agent_tool_terminals()));
    assert_eq!(
        text(json!({ "action": "list" })),
        format!("$ {} ", t.agent_tool_terminals())
    );
    assert_eq!(
        text(json!({ "action": "read", "session": "py" })),
        format!("$ {} py", t.agent_tool_terminals_read())
    );
    assert_eq!(
        text(json!({ "action": "send", "session": "py", "input": "1+1", "keys": ["Enter"] })),
        format!("$ {} py · 1+1 · Enter", t.agent_tool_terminals_send())
    );
    assert_eq!(
        text(json!({ "action": "send", "session": "py", "keys": ["Ctrl+C"] })),
        format!("$ {} py · Ctrl+C", t.agent_tool_terminals_send())
    );
    assert_eq!(
        text(json!({ "action": "interrupt", "session": "build" })),
        format!("$ {} build", t.agent_tool_terminals_interrupt())
    );
    assert_eq!(
        text(json!({ "action": "close", "session": "build" })),
        format!("$ {} build", t.agent_tool_terminals_close())
    );
    assert!(!text(json!({ "action": "screen", "session": "py" })).contains('{'));
}

#[test]
fn call_summaries_truncate_and_flatten() {
    let long = "x".repeat(50);
    let summary = summarize_call(
        &call("shell", json!({ "command": format!("echo {long}\nls") })),
        20,
    );
    assert_eq!(summary.chars().count(), 20);
    assert!(summary.ends_with('…'));
    assert!(!summary.contains('\n'));
    assert_eq!(
        summarize_call(&call("edit", json!({ "path": "a.rs" })), 40),
        "a.rs"
    );
    assert_eq!(
        summarize_call(&call("fetch", json!({ "url": "https://docs.rs" })), 40),
        "https://docs.rs"
    );
    assert_eq!(
        summarize_call(
            &call("web_search", json!({ "query": "rust tui", "limit": 5 })),
            40
        ),
        "rust tui"
    );
    assert_eq!(summarize_call(&call("mcp", json!({})), 40), "");
    assert_eq!(
        summarize_call(&call("mcp", json!({ "q": 1 })), 40),
        "{\"q\":1}"
    );
}

/// A finished `task` shows under its fold what the subagent spent, as
/// the session's totals show tokens, beside how long it took.
#[test]
fn a_subagents_block_shows_what_it_spent() {
    let colors = ThemeColors::default();
    let mut transcript = Transcript::default();
    let task = call("task", json!({ "agent": "search", "prompt": "find it" }));
    let spent = termide_agent_core::Usage {
        input: 1200,
        output: 300,
        cache_read: 2000,
        cache_write: 0,
        ..Default::default()
    };
    transcript.push(Item::Tool {
        call: task.clone(),
        result: Some(ToolResultMessage::text(&task, "found").with_spent(spent)),
        live: None,
        at: "21:03:20".into(),
        duration_ms: Some(4000),
        waited_ms: None,
        waiting: false,
    });
    assert!(transcript.toggle_expanded(0));
    let lines = text_of(transcript.lines(80, &colors, false));
    assert!(
        lines.iter().any(|l| l.contains("↑1k ↻2k ↓300 🕒 4s")),
        "{lines:?}"
    );
}
