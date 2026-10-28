//! Incremental highlighting must end where a fresh parse of the final text
//! does, whatever the edits and whatever was on screen along the way.

use super::*;
use crate::global_highlighter;
use tree_sitter::Point;

/// A document edited the way a buffer is, logging tree-sitter edits.
struct TestDoc {
    rope: Rope,
    document: u64,
    edits: Vec<InputEdit>,
}

impl TestDoc {
    fn new(text: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1_000_000);
        Self {
            rope: Rope::from_str(text),
            document: NEXT.fetch_add(1, Ordering::Relaxed),
            edits: Vec::new(),
        }
    }

    fn point(&self, byte: usize) -> Point {
        let row = self.rope.byte_to_line(byte);
        Point::new(row, byte - self.rope.line_to_byte(row))
    }

    /// Replace chars `chars` with `text`.
    fn replace(&mut self, chars: Range<usize>, text: &str) {
        let start_byte = self.rope.char_to_byte(chars.start);
        let old_end_byte = self.rope.char_to_byte(chars.end);
        let start_position = self.point(start_byte);
        let old_end_position = self.point(old_end_byte);
        self.rope.remove(chars.clone());
        self.rope.insert(chars.start, text);
        let new_end_byte = start_byte + text.len();
        self.edits.push(InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position: self.point(new_end_byte),
        });
    }

    fn lines(&self) -> usize {
        self.rope.len_lines()
    }
}

impl DocumentSource for TestDoc {
    fn rope(&self) -> &Rope {
        &self.rope
    }

    fn revision(&self) -> DocumentRevision {
        DocumentRevision {
            document: self.document,
            edit: self.edits.len() as u64,
        }
    }

    fn edits_since(&self, since: DocumentRevision) -> Option<Vec<InputEdit>> {
        (since.document == self.document)
            .then(|| self.edits.get(since.edit as usize..).map(<[_]>::to_vec))
            .flatten()
    }
}

fn cache_for(language: &str) -> HighlightCache {
    let mut cache = HighlightCache::new(global_highlighter(), false, Color::White);
    cache.set_syntax(language);
    assert_eq!(cache.current_syntax(), Some(language));
    assert!(
        global_highlighter()
            .incremental_highlights(language)
            .is_some(),
        "{language} should take the incremental path"
    );
    cache
}

/// Every line of `cache`'s document highlight, all of them filled.
fn all_lines(cache: &mut HighlightCache, doc: &TestDoc) -> Vec<LineSegments> {
    cache.sync_document(doc, 0..doc.lines());
    cache
        .document
        .lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            line.clone()
                .unwrap_or_else(|| panic!("line {i} left unhighlighted"))
        })
        .collect()
}

/// Highlight of `text` from scratch.
fn fresh(language: &str, text: &str) -> Vec<LineSegments> {
    let mut cache = cache_for(language);
    all_lines(&mut cache, &TestDoc::new(text))
}

fn assert_matches_fresh(language: &str, cache: &mut HighlightCache, doc: &TestDoc, context: &str) {
    assert!(
        check_against_fresh(language, cache, doc, context),
        "{language}, {context}: the incremental tree differs from a fresh parse"
    );
}

/// Check the cached lines against the tree they come from, then against a
/// fresh parse. Returns whether the kept tree equals a fresh parse's.
///
/// The first check is exact: lines kept across edits must equal the lines
/// highlighted anew from the same tree, or a stale line was missed. The
/// second holds whenever tree-sitter's incremental reparse lands on the tree
/// a fresh parse builds — always for well-formed edits, though not always
/// inside error recovery, where reused subtrees can tip an ambiguity the
/// other way (the same as in any editor built on tree-sitter).
fn check_against_fresh(
    language: &str,
    cache: &mut HighlightCache,
    doc: &TestDoc,
    context: &str,
) -> bool {
    let text = doc.rope.to_string();
    let kept = all_lines(cache, doc);
    cache.document.clear_lines();
    let recomputed = all_lines(cache, doc);
    assert_lines_eq(
        &kept,
        &recomputed,
        &text,
        &format!("{language}, {context}, same tree"),
    );

    let mut fresh_cache = cache_for(language);
    let expected = all_lines(&mut fresh_cache, &TestDoc::new(&text));
    let tree = |c: &HighlightCache| c.document.tree().map(|t| t.root_node().to_sexp());
    if tree(cache) != tree(&fresh_cache) {
        return false;
    }
    assert_lines_eq(
        &kept,
        &expected,
        &text,
        &format!("{language}, {context}, fresh parse"),
    );
    true
}

fn assert_lines_eq(got: &[LineSegments], want: &[LineSegments], text: &str, context: &str) {
    assert_eq!(got.len(), want.len(), "{context}: line count");
    for (i, (got, want)) in got.iter().zip(want).enumerate() {
        assert_eq!(got, want, "{context}: line {i} differs; text:\n{text}");
    }
}

const RUST: &str = r#"//! Module docs.
use std::collections::HashMap;

/// A point.
#[derive(Debug, Clone)]
pub struct Point { x: f64, y: f64 }

fn main() {
    let name = "Иван 🙂";
    let raw = r"C:\path";
    let mut map: HashMap<&str, i32> = HashMap::new();
    map.insert("один", 1); // comment
    println!("{name} {:?}", map);
    /* block
       comment */
    let c = 'ё';
}
"#;

/// Edit by hand the way people type: one character at a time, newlines,
/// joins, a block comment opened and closed, multi-byte text.
#[test]
fn rust_typing_session_matches_a_fresh_parse() {
    let language = "rust";
    let mut doc = TestDoc::new(RUST);
    let mut cache = cache_for(language);
    // Everything highlighted up front, so later edits must invalidate the
    // right lines rather than lean on lines never computed.
    all_lines(&mut cache, &doc);

    fn line_start(doc: &TestDoc, line: usize) -> usize {
        doc.rope.line_to_char(line)
    }
    type Step = (&'static str, fn(&mut TestDoc));
    let steps: [Step; 9] = [
        ("type a statement char by char", |d: &mut TestDoc| {
            let at = line_start(d, 8) + 4;
            for (k, ch) in "let s = \"ё\";\n    ".chars().enumerate() {
                d.replace(at + k..at + k, &ch.to_string());
            }
        }),
        ("open a block comment at the top", |d: &mut TestDoc| {
            d.replace(0..0, "/*");
        }),
        ("close it on line 5", |d: &mut TestDoc| {
            let at = line_start(d, 5);
            d.replace(at..at, "*/");
        }),
        ("remove both", |d: &mut TestDoc| {
            let at = line_start(d, 5);
            d.replace(at..at + 2, "");
            d.replace(0..2, "");
        }),
        ("open an unterminated string", |d: &mut TestDoc| {
            let at = line_start(d, 9) + 4;
            d.replace(at..at, "\"🙂");
        }),
        ("delete multi-byte characters", |d: &mut TestDoc| {
            let line = d.rope.line(9).to_string();
            let emoji = line.chars().position(|c| c == '🙂').unwrap();
            let at = line_start(d, 9) + emoji;
            d.replace(at..at + 1, "");
            let at = line_start(d, 9) + 4;
            d.replace(at..at + 1, "");
        }),
        ("insert and delete whole lines", |d: &mut TestDoc| {
            let at = line_start(d, 3);
            d.replace(at..at, "const A: u8 = 1;\nconst B: &str = \"b\";\n");
            let from = line_start(d, 10);
            let to = line_start(d, 13);
            d.replace(from..to, "");
        }),
        ("join two lines", |d: &mut TestDoc| {
            let at = line_start(d, 2) - 1;
            d.replace(at..at + 1, " ");
        }),
        ("replace the whole text", |d: &mut TestDoc| {
            let len = d.rope.len_chars();
            d.replace(0..len, "fn x() -> u8 { 1 }\n");
        }),
    ];
    for (context, step) in steps {
        step(&mut doc);
        assert_matches_fresh(language, &mut cache, &doc, context);
    }
}

/// Deterministic pseudo-random numbers (no extra dependency).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

/// Random edits — syntax-heavy snippets, multi-byte text, line breaks,
/// deletions — synced with a random window on screen each time, as a
/// scrolled editor would. Every few steps the whole document is compared
/// with a fresh parse.
fn random_session(language: &str, source: &str, seed: u64) -> usize {
    const SNIPPETS: &[&str] = &[
        "\n",
        "\n\n",
        "{",
        "}",
        "(",
        ")",
        "\"",
        "'",
        "/*",
        "*/",
        "//",
        "#",
        "ё",
        "🙂",
        "Иван ",
        "x",
        " = 1;",
        "fn ",
        "def ",
        "if ",
        "    ",
        "\\",
        "`",
        "-- ",
        "<",
        ">",
        "",
    ];
    let mut rng = Lcg(seed);
    let mut doc = TestDoc::new(source);
    let mut cache = cache_for(language);
    let mut diverged = 0;
    for step in 0..120 {
        // Several edits may land between two renders.
        for _ in 0..=rng.next(3) {
            let len = doc.rope.len_chars();
            let start = rng.next(len + 1);
            let end = if rng.next(3) == 0 {
                (start + rng.next(12)).min(len)
            } else {
                start
            };
            let text = SNIPPETS[rng.next(SNIPPETS.len())];
            doc.replace(start..end, text);
        }
        let lines = doc.lines();
        let top = rng.next(lines);
        cache.sync_document(&doc, top..top + 1 + rng.next(30));
        if step % 15 == 14 {
            let context = format!("seed {seed}, step {step}");
            if !check_against_fresh(language, &mut cache, &doc, &context) {
                diverged += 1;
            }
        }
    }
    let context = format!("seed {seed}, end");
    if !check_against_fresh(language, &mut cache, &doc, &context) {
        diverged += 1;
    }
    diverged
}

const PYTHON: &str = "import os\n\nclass A:\n    \"\"\"Doc\n    string.\"\"\"\n    def f(self, x: int) -> str:\n        return f\"{x} ё\"  # c\n\nprint(A().f(1))\n";
const C: &str = "#include <stdio.h>\n/* c */\nint main(void) {\n    char *s = \"hi\\n\";\n    printf(\"%s\", s);\n    return 0;\n}\n";
const TYPESCRIPT: &str = "interface P { x: number }\nconst f = (p: P): string => `v=${p.x}`;\n// note\nexport default f;\n";
const YAML: &str = "key: value\nlist:\n  - a\n  - \"b ё\"\nblock: |\n  text\n  more\n# c\n";
const BASH: &str = "#!/bin/sh\nname=\"world\"\nif [ -n \"$name\" ]; then\n  echo \"hi $name\" # c\nfi\ncat <<EOF\nheredoc\nEOF\n";
const RUBY: &str = "class A\n  def f(x)\n    \"v=#{x}\" # c\n  end\nend\n=begin\nblock\n=end\n";

#[test]
fn random_edit_sessions_match_a_fresh_parse() {
    const SEEDS: u64 = 3;
    for (language, source) in [
        ("rust", RUST),
        ("python", PYTHON),
        ("c", C),
        ("typescript", TYPESCRIPT),
        ("yaml", YAML),
        ("bash", BASH),
        ("ruby", RUBY),
    ] {
        let mut diverged = 0;
        for seed in 1..=SEEDS {
            diverged += random_session(language, source, seed);
        }
        eprintln!(
            "{language}: {diverged} of {} checks with a diverging tree",
            SEEDS * 9
        );
    }
}

/// The incremental path's own capture walk must colour a file exactly as
/// `tree_sitter_highlight` does, which the fallback path still uses.
#[test]
fn query_walk_agrees_with_tree_sitter_highlight() {
    for (language, source) in [
        ("rust", RUST),
        ("python", PYTHON),
        ("c", C),
        ("typescript", TYPESCRIPT),
        ("yaml", YAML),
        ("bash", BASH),
        ("ruby", RUBY),
    ] {
        let mut cache = cache_for(language);
        let reference = cache
            .highlight_whole_text(source)
            .expect("tree_sitter_highlight pass");
        let walked = all_lines(&mut cache, &TestDoc::new(source));
        // `highlight_whole_text` splits on '\n' only and the samples use no
        // other line break, so the lines correspond one to one.
        assert_eq!(walked, reference, "{language}");
    }
}

/// Lines off screen stay unhighlighted until they scroll into view, and an
/// unchanged document does no work on the next sync.
#[test]
fn only_visible_stale_lines_are_highlighted() {
    let text = RUST.repeat(20);
    let mut doc = TestDoc::new(&text);
    let mut cache = cache_for("rust");
    cache.sync_document(&doc, 10..20);
    let filled = |cache: &HighlightCache| {
        cache
            .document
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.is_some())
            .map(|(i, _)| i)
            .collect::<Vec<_>>()
    };
    assert_eq!(filled(&cache), (10..20).collect::<Vec<_>>());

    // An edit on line 15 restales only what it touched.
    let at = doc.rope.line_to_char(15);
    doc.replace(at..at, "x");
    cache.sync_document(&doc, 0..0);
    assert!(cache.document.lines[15].is_none());
    assert!(cache.document.lines[12].is_some());
    assert!(cache.document.lines[19].is_some());
}

#[test]
fn replacing_the_document_reparses_from_scratch() {
    // A reload gives the buffer a new document id; whatever the old tree
    // was, the new text must be highlighted on its own.
    let mut cache = cache_for("rust");
    let first = TestDoc::new("/* open comment\nfn a() {}\n");
    all_lines(&mut cache, &first);
    let second = TestDoc::new("fn a() {}\n");
    assert_eq!(all_lines(&mut cache, &second), fresh("rust", "fn a() {}\n"));
}

#[test]
fn a_language_change_reparses_with_the_new_grammar() {
    let mut cache = cache_for("rust");
    let doc = TestDoc::new("x = 1  # c\n");
    all_lines(&mut cache, &doc);
    cache.set_syntax("python");
    assert_eq!(all_lines(&mut cache, &doc), fresh("python", "x = 1  # c\n"));
}
