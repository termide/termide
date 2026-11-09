//! Catching a model that reasons in circles.
//!
//! A model can get stuck restating the same few thoughts ("let me ask the
//! user" / "let me just start") until the output limit cuts it off, with
//! nothing to show for it. [`LoopGuard`] watches the streamed text one
//! sentence (or line) at a time and trips once most of the recent sentences
//! are ones it has already seen several times: a reasoning that drafts
//! something twice, or comes back to a point once, does not trip it. A
//! stutter — one short piece over and over, with no sentence long enough to
//! count — trips it too.
//!
//! What the loop does then — cut the call, nudge the model with
//! [`LoopPrompt`], stop — is up to the agent loop.

use std::collections::{HashMap, VecDeque};

/// The seed of the nudge sent after a loop is cut.
pub const SEED_LOOP: &str = include_str!("../assets/system/loop.md");

/// Sentences shorter than this (normalised) are too common to tell a loop:
/// "Wait.", "Hmm, ok.", a closing brace.
const MIN_UNIT_CHARS: usize = 24;
/// The occurrence of a sentence from which it counts as a repeat.
const STALE_AT: u32 = 3;
/// How many recent sentences are looked at…
const WINDOW: usize = 30;
/// …and how many of them must be repeats to call it a loop.
const STALE_IN_WINDOW: usize = 20;
/// A stream whose last this many bytes repeat one short piece over and
/// over ("PR PR PR …", the same short line) is stuck on it: a stutter has
/// no sentences long enough to count, so it is caught apart.
const STUTTER_SPAN: usize = 2000;
/// The longest piece a stutter repeats; anything longer is left to the
/// sentence count, so a draft written twice is no stutter.
const STUTTER_MAX_PERIOD: usize = 64;
/// How many bytes pass between two looks for a stutter.
const STUTTER_CHECK_EVERY: usize = 64;
/// The most points [`LoopReport::thoughts`] keeps.
const MAX_THOUGHTS: usize = 8;
/// The most characters one point keeps.
const MAX_THOUGHT_CHARS: usize = 240;

/// What a tripped guard found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopReport {
    /// How many sentences repeated one seen at least twice before.
    pub repeats: usize,
    /// The sentences the text kept coming back to, in the order they first
    /// became repeats: what the model could not decide between.
    pub thoughts: Vec<String>,
    /// Where the loop began, in bytes of the text fed: what comes before is
    /// the reasoning worth keeping.
    pub loop_start: usize,
}

/// Watches one stream of text (the reasoning, or the answer) for a loop.
#[derive(Debug, Default)]
pub struct LoopGuard {
    /// The sentence being read, not yet ended.
    pending: String,
    /// Bytes fed before `pending` began.
    pending_start: usize,
    /// Bytes fed so far.
    fed: usize,
    counts: HashMap<String, u32>,
    /// For each recent counted sentence, whether it was a repeat.
    window: VecDeque<bool>,
    stale_in_window: usize,
    repeats: usize,
    loop_start: Option<usize>,
    thoughts: Vec<String>,
    tripped: bool,
    /// The last bytes fed, for the stutter check: at least
    /// [`STUTTER_SPAN`] once that many came.
    tail: Vec<u8>,
    since_stutter_check: usize,
}

impl LoopGuard {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read the next piece of the stream; `true` once it is a loop (and
    /// from then on).
    pub fn feed(&mut self, delta: &str) -> bool {
        for c in delta.chars() {
            let ends_sentence = c.is_whitespace() && self.pending.ends_with(['.', '?', '!']);
            if c == '\n' || ends_sentence {
                self.end_unit();
                self.fed += c.len_utf8();
                self.pending_start = self.fed;
            } else {
                self.pending.push(c);
                self.fed += c.len_utf8();
            }
        }
        if !self.tripped {
            self.watch_stutter(delta);
        }
        self.tripped
    }

    fn watch_stutter(&mut self, delta: &str) {
        self.tail.extend_from_slice(delta.as_bytes());
        if self.tail.len() > 4 * STUTTER_SPAN {
            self.tail.drain(..self.tail.len() - 2 * STUTTER_SPAN);
        }
        self.since_stutter_check += delta.len();
        if self.tail.len() < STUTTER_SPAN || self.since_stutter_check < STUTTER_CHECK_EVERY {
            return;
        }
        self.since_stutter_check = 0;
        let Some((start, period)) = stutter(&self.tail) else {
            return;
        };
        let run = &self.tail[start..];
        // A few rounds of the piece, so a short one reads as what it is; a
        // character split at either end comes out as a replacement mark.
        let shown = run.len().min(period.max(MAX_THOUGHT_CHARS / 2));
        self.tripped = true;
        self.repeats = run.len() / period;
        self.loop_start = Some(self.fed - run.len());
        self.thoughts = vec![clip(
            String::from_utf8_lossy(&run[..shown]).trim(),
            MAX_THOUGHT_CHARS,
        )];
    }

    /// Whether the stream has looped.
    #[must_use]
    pub fn tripped(&self) -> bool {
        self.tripped
    }

    /// What was found, once tripped.
    #[must_use]
    pub fn report(&self) -> Option<LoopReport> {
        self.tripped.then(|| LoopReport {
            repeats: self.repeats,
            thoughts: self.thoughts.clone(),
            loop_start: self.loop_start.unwrap_or(self.fed),
        })
    }

    fn end_unit(&mut self) {
        let text = std::mem::take(&mut self.pending);
        let key = normalise(&text);
        if key.chars().count() < MIN_UNIT_CHARS {
            return;
        }
        let count = self.counts.entry(key).or_insert(0);
        *count += 1;
        let stale = *count >= STALE_AT;
        if stale {
            self.repeats += 1;
            self.loop_start.get_or_insert(self.pending_start);
        }
        if *count == STALE_AT && self.thoughts.len() < MAX_THOUGHTS {
            self.thoughts.push(clip(text.trim(), MAX_THOUGHT_CHARS));
        }
        self.window.push_back(stale);
        if stale {
            self.stale_in_window += 1;
        }
        if self.window.len() > WINDOW && self.window.pop_front() == Some(true) {
            self.stale_in_window -= 1;
        }
        if self.window.len() >= WINDOW && self.stale_in_window >= STALE_IN_WINDOW {
            self.tripped = true;
        }
    }
}

/// Whether the last [`STUTTER_SPAN`] bytes of `tail` repeat one piece of at
/// most [`STUTTER_MAX_PERIOD`] bytes: where the repeating run begins in
/// `tail`, and the piece's length.
fn stutter(tail: &[u8]) -> Option<(usize, usize)> {
    let span = tail.get(tail.len().checked_sub(STUTTER_SPAN)?..)?;
    let period = (1..=STUTTER_MAX_PERIOD).find(|&p| span[p..] == span[..span.len() - p])?;
    let mut start = tail.len() - STUTTER_SPAN;
    while start > 0 && tail[start - 1] == tail[start - 1 + period] {
        start -= 1;
    }
    Some((start, period))
}

/// A sentence reduced to what tells it apart: lowercase words, no
/// punctuation or markup, single spaces.
fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
        } else if !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
    }
    out.truncate(out.trim_end().len());
    out
}

fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

/// The nudge the loop sends after cutting a looped call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopPrompt {
    /// The text; `{{thoughts}}` takes the points the model kept returning to.
    pub template: String,
}

impl Default for LoopPrompt {
    fn default() -> Self {
        Self::from_file(SEED_LOOP)
    }
}

impl LoopPrompt {
    /// Parse `loop.md`: the nudge is the body; front matter, if any, is
    /// left out.
    #[must_use]
    pub fn from_file(text: &str) -> Self {
        let (_, body) = crate::layers::split_front_matter(text);
        Self {
            template: body.trim().to_string(),
        }
    }

    /// The nudge for `report`.
    #[must_use]
    pub fn render(&self, report: &LoopReport) -> String {
        let thoughts = report
            .thoughts
            .iter()
            .map(|thought| format!("- {thought}"))
            .collect::<Vec<_>>()
            .join("\n");
        self.template.replace("{{thoughts}}", &thoughts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "Actually, I should present the scope question to the owner first.";
    const B: &str = "OK let me just start implementing and see what happens.";
    const C: &str = "Let me start by enabling the table on the default build path.";

    fn fresh_reasoning(n: usize) -> String {
        (0..n)
            .map(|i| {
                format!("Step {i}: the function number {i} reads the file and checks its header.\n")
            })
            .collect()
    }

    #[test]
    fn going_round_the_same_thoughts_trips() {
        let mut guard = LoopGuard::new();
        let head = fresh_reasoning(10);
        assert!(!guard.feed(&head));
        let mut tripped_at = None;
        for round in 0..40 {
            if guard.feed(&format!("{A}\n\n{B} {C}\n\n")) {
                tripped_at = Some(round);
                break;
            }
        }
        let round = tripped_at.expect("the loop trips");
        assert!(round < 15, "early, not at the output limit: {round}");
        let report = guard.report().unwrap();
        assert_eq!(report.thoughts, vec![A, B, C]);
        // What came before the circling is kept, and the first two rounds
        // too: a point is a repeat only on its third showing.
        let kept = format!("{head}{A}\n\n{B} {C}\n\n{A}\n\n{B} {C}\n\n");
        assert_eq!(report.loop_start, kept.len());
        assert!(report.repeats >= STALE_IN_WINDOW);
    }

    #[test]
    fn varied_reasoning_and_a_second_draft_do_not_trip() {
        let mut guard = LoopGuard::new();
        assert!(!guard.feed(&fresh_reasoning(200)));
        // The same code drafted twice is no loop.
        let draft: String = (0..40)
            .map(|i| format!("    let value_{i} = compute_something(input, {i});\n"))
            .collect();
        assert!(!guard.feed(&draft));
        assert!(!guard.feed(&draft));
        // Short fillers between steps never count.
        for i in 0..100 {
            assert!(!guard.feed("Wait. Hmm.\nOk.\n}\n"));
            assert!(!guard.feed(&format!("Then step {i} checks the header of part {i}.\n")));
        }
        assert!(guard.report().is_none());
    }

    #[test]
    fn a_stutter_on_a_short_piece_trips() {
        let mut guard = LoopGuard::new();
        let head = fresh_reasoning(10);
        assert!(!guard.feed(&head));
        let mut fed = 0;
        while !guard.feed("PR ") {
            fed += 3;
            assert!(fed < 3 * STUTTER_SPAN, "caught soon after the span");
        }
        let report = guard.report().unwrap();
        // The reasoning before the stutter is kept, none of the stutter.
        assert_eq!(report.loop_start, head.len());
        assert!(report.thoughts[0].starts_with("PR PR PR"), "{report:?}");
        assert!(report.repeats * 3 >= STUTTER_SPAN);
    }

    #[test]
    fn a_long_run_that_varies_is_no_stutter() {
        let mut guard = LoopGuard::new();
        // A table of distinct values, a rule shorter than the span.
        let table: String = (0..400).map(|i| format!("0x{i:02x}, ")).collect();
        assert!(!guard.feed(&table));
        assert!(!guard.feed(&"=".repeat(STUTTER_SPAN / 2)));
        assert!(!guard.feed(&fresh_reasoning(5)));
        assert!(guard.report().is_none());
    }

    #[test]
    fn deltas_split_anywhere_read_the_same() {
        let text: String = (0..30).map(|_| format!("{A} {B}\n")).collect();
        let mut whole = LoopGuard::new();
        whole.feed(&text);
        let mut pieces = LoopGuard::new();
        let chars: Vec<char> = text.chars().collect();
        for chunk in chars.chunks(7) {
            pieces.feed(&chunk.iter().collect::<String>());
        }
        assert!(whole.tripped());
        assert_eq!(whole.report(), pieces.report());
    }

    #[test]
    fn sentences_differing_in_case_and_punctuation_are_one() {
        assert_eq!(
            normalise("**OK** — let me just START implementing!"),
            normalise("ok, let me just start implementing.")
        );
    }

    #[test]
    fn a_custom_file_replaces_the_nudge() {
        let custom =
            LoopPrompt::from_file("---\nnote: mine\n---\nStop. You kept saying:\n{{thoughts}}\n");
        let report = LoopReport {
            repeats: 20,
            thoughts: vec![A.into()],
            loop_start: 0,
        };
        assert_eq!(
            custom.render(&report),
            format!("Stop. You kept saying:\n- {A}")
        );
    }

    #[test]
    fn the_nudge_lists_the_thoughts() {
        let report = LoopReport {
            repeats: 20,
            thoughts: vec![A.into(), B.into()],
            loop_start: 0,
        };
        let text = LoopPrompt::default().render(&report);
        assert!(text.contains(&format!("- {A}\n- {B}")));
        assert!(!text.contains("{{thoughts}}"));
    }
}
