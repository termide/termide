//! Cleaning and compacting command output for the model.
//!
//! A shell run under a real terminal, or a chatty build tool, produces output
//! that is mostly noise to a language model: colour and cursor escapes,
//! progress bars redrawn with carriage returns, spinner frames, and long runs
//! of near-identical "Compiling …" lines. The user still sees the raw stream
//! live in the panel and the full log stays on disk; this is only what the
//! model reads, so trimming the noise cuts tokens without losing signal.
//!
//! The passes are deliberately conservative — they never drop a line that
//! could be a warning or an error — and are ordered so each feeds the next:
//! escapes and carriage-return redraws first, then duplicate and blank-line
//! collapsing, then a few command-aware rules. Inspired by rtk
//! (`rtk-ai/rtk`), which proxies noisy CLIs to save tokens.

/// The outcome of cleaning: the text and how much was removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleaned {
    pub text: String,
    /// Bytes before cleaning.
    pub original_bytes: usize,
    /// Bytes after cleaning.
    pub cleaned_bytes: usize,
    /// Characters before cleaning, which tokens are estimated from.
    pub original_chars: usize,
    /// Characters after cleaning.
    pub cleaned_chars: usize,
}

impl Cleaned {
    /// Whether cleaning actually removed anything.
    #[must_use]
    pub fn shrank(&self) -> bool {
        self.cleaned_bytes < self.original_bytes
    }
}

/// Clean `raw` output of a `command` for the model. `command` selects the
/// command-aware rules (cargo's compile spam, say); an empty one runs only
/// the general passes.
#[must_use]
pub fn clean_output(raw: &str, command: &str) -> Cleaned {
    let original_bytes = raw.len();
    let stripped = strip_escapes(raw);
    let lines: Vec<String> = stripped.lines().map(apply_carriage_returns).collect();
    let lines = compact_command(command, lines);
    let lines = compact_test_output(command, lines);
    let lines = collapse_duplicates(&lines);
    let text = collapse_blank_runs(&lines);
    Cleaned {
        cleaned_bytes: text.len(),
        cleaned_chars: text.chars().count(),
        original_bytes,
        original_chars: raw.chars().count(),
        text,
    }
}

/// Remove ANSI/VT escape sequences and stray control bytes, keeping `\t`,
/// `\n` and `\r` (carriage returns are resolved per line afterwards).
fn strip_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                // CSI: ESC [ … final-byte in 0x40..=0x7e
                Some('[') => {
                    chars.next();
                    for p in chars.by_ref() {
                        if ('\x40'..='\x7e').contains(&p) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] … terminated by BEL or ST (ESC \)
                Some(']') => {
                    chars.next();
                    while let Some(p) = chars.next() {
                        if p == '\x07' {
                            break;
                        }
                        if p == '\x1b' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Other two-byte escapes (ESC ( B, ESC =, …): drop the next.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            // Keep the useful whitespace controls.
            '\t' | '\n' | '\r' => out.push(c),
            // Drop other C0 controls (BEL, backspace, form feed, …).
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// Resolve carriage-return redraws within one line: a progress bar rewrites
/// the line after each `\r`, so only the text after the last non-trailing
/// `\r` survives. A trailing `\r` (from a `\r\n` split) is dropped.
fn apply_carriage_returns(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    match line.rsplit('\r').next() {
        Some(last) => last.to_string(),
        None => line.to_string(),
    }
}

/// Replace three or more consecutive identical lines with one plus a `(×N)`
/// count — spinner frames and repeated warnings collapse, a couple of
/// repeats are left alone.
fn collapse_duplicates(lines: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let mut run = 1;
        while i + run < lines.len() && lines[i + run] == lines[i] {
            run += 1;
        }
        if run >= 3 && !lines[i].trim().is_empty() {
            out.push(format!("{} (×{run})", lines[i]));
        } else {
            for line in &lines[i..i + run] {
                out.push(line.clone());
            }
        }
        i += run;
    }
    out
}

/// Join lines, collapsing any run of blank lines to a single one and
/// trimming leading and trailing blanks. Ends with a newline when non-empty.
fn collapse_blank_runs(lines: &[String]) -> String {
    let mut out = String::new();
    let mut blank_pending = false;
    let mut wrote = false;
    for line in lines {
        if line.trim().is_empty() {
            blank_pending = wrote;
            continue;
        }
        if blank_pending {
            out.push('\n');
            blank_pending = false;
        }
        out.push_str(line);
        out.push('\n');
        wrote = true;
    }
    out
}

/// A command-aware rule: for a given command, a run of "progress" lines —
/// those whose first word is one of `words` — collapses to one summary,
/// keeping everything else (warnings, errors, results) verbatim. The set is
/// data, so a new tool is one more entry, not more code.
struct ProgressRule {
    /// The command this applies to, matched against its first word.
    command: &'static str,
    /// Leading words that mark a throwaway progress line.
    words: &'static [&'static str],
    /// What the collapsed run is labelled, e.g. `cargo progress`.
    label: &'static str,
}

/// The known noisy build/package tools. Extend by adding a row.
const PROGRESS_RULES: &[ProgressRule] = &[
    ProgressRule {
        command: "cargo",
        words: &[
            "Compiling",
            "Checking",
            "Downloading",
            "Downloaded",
            "Installing",
            "Updating",
            "Fresh",
            "Building",
        ],
        label: "cargo progress",
    },
    ProgressRule {
        command: "pip",
        words: &["Collecting", "Downloading", "Using", "Requirement"],
        label: "pip progress",
    },
    ProgressRule {
        command: "npm",
        words: &["added", "removed", "changed", "audited"],
        label: "npm progress",
    },
    ProgressRule {
        command: "go",
        words: &["go:"],
        label: "go progress",
    },
];

/// Apply the matching [`ProgressRule`] to `lines`, if any. A run of three or
/// more progress lines becomes one `[N <label> lines]` summary.
fn compact_command(command: &str, lines: Vec<String>) -> Vec<String> {
    let Some(word) = first_word(command) else {
        return lines;
    };
    let Some(rule) = PROGRESS_RULES.iter().find(|r| r.command == word) else {
        return lines;
    };
    let is_progress = |line: &str| {
        let t = line.trim_start();
        rule.words.iter().any(|word| {
            t.strip_prefix(word).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace() || c == ':')
            })
        })
    };
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if is_progress(&lines[i]) {
            let mut run = 1;
            while i + run < lines.len() && is_progress(&lines[i + run]) {
                run += 1;
            }
            if run >= 3 {
                out.push(format!("   [{run} {} lines]", rule.label));
            } else {
                out.extend_from_slice(&lines[i..i + run]);
            }
            i += run;
        } else {
            out.push(lines[i].clone());
            i += 1;
        }
    }
    out
}

/// Whether `command` is a test run whose per-test output is worth trimming.
/// Covers the runners a general-purpose dev tool sees most: Rust (`cargo test`
/// / `cargo nextest`), Python (`pytest`, `python -m pytest`), Go (`go test`),
/// and JS/TS (`jest`, `vitest`, and the `npm`/`pnpm`/`yarn`/`npx` wrappers).
fn is_test_command(command: &str) -> bool {
    let Some(first) = first_word(command) else {
        return false;
    };
    let has = |word: &str| command.split_whitespace().any(|w| w == word);
    match first {
        "cargo" => has("test") || has("nextest"),
        "pytest" | "py.test" | "jest" | "vitest" => true,
        "python" | "python3" => command.contains("pytest"),
        "go" => has("test"),
        "npm" | "pnpm" | "yarn" | "npx" | "bunx" | "bun" => {
            has("test") || command.contains("jest") || command.contains("vitest")
        }
        _ => false,
    }
}

/// A per-test line reporting a pass or a skip — pure noise to the model, which
/// only needs the failures and the final count. Union of the common runners'
/// formats; failures (`FAILED` / `--- FAIL:` / `✗` / `ERROR`), the run headers
/// and the result summaries never match, so nothing needed is dropped.
fn is_passing_test_line(line: &str) -> bool {
    let t = line.trim_start();
    // Rust libtest: `test path::name ... ok` / `... ignored`.
    if t.starts_with("test ") && (t.ends_with(" ... ok") || t.ends_with(" ... ignored")) {
        return true;
    }
    // Rust nextest: `PASS`/`SKIP` first, then `[time] crate name`.
    if matches!(t.split_whitespace().next(), Some("PASS" | "SKIP")) && t.contains('[') {
        return true;
    }
    // Go: run markers and per-test passes/skips (failures use `--- FAIL:`).
    if t.starts_with("=== RUN")
        || t.starts_with("=== CONT")
        || t.starts_with("=== PAUSE")
        || t.starts_with("--- PASS:")
        || t.starts_with("--- SKIP:")
    {
        return true;
    }
    // pytest -v: `path::test PASSED/SKIPPED/XFAIL …` (failures say FAILED/ERROR).
    if t.contains("::")
        && t.split_whitespace()
            .any(|w| matches!(w, "PASSED" | "SKIPPED" | "XFAIL" | "XPASS"))
    {
        return true;
    }
    // jest / vitest pass marks (failures use ✗ / × / ❯).
    t.starts_with("\u{2713} ") || t.starts_with("\u{221a} ")
}

/// Drop passing and skipped test lines from a test run, keeping failures, the
/// run header and the result summary — the count of what was hidden stays in
/// that summary, so nothing the model needs is lost.
fn compact_test_output(command: &str, lines: Vec<String>) -> Vec<String> {
    if !is_test_command(command) {
        return lines;
    }
    lines
        .into_iter()
        .filter(|line| !is_passing_test_line(line))
        .collect()
}

/// The first whitespace-separated word of `command`, ignoring a leading
/// `VAR=val` assignment so `RUST_LOG=info cargo …` still counts as cargo.
fn first_word(command: &str) -> Option<&str> {
    command
        .split_whitespace()
        .find(|w| !w.contains('=') || w.starts_with('='))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_colour_and_resolves_progress_redraws() {
        let raw = "\x1b[32mok\x1b[0m\nprogress 10%\rprogress 50%\rprogress 100%\ndone\n";
        let cleaned = clean_output(raw, "");
        assert_eq!(cleaned.text, "ok\nprogress 100%\ndone\n");
        assert!(cleaned.shrank());
    }

    #[test]
    fn drops_osc_titles_and_stray_controls() {
        let raw = "\x1b]0;my title\x07hello\x08\x07 world\n";
        let cleaned = clean_output(raw, "");
        assert_eq!(cleaned.text, "hello world\n");
    }

    #[test]
    fn collapses_repeated_lines_and_blank_runs() {
        let raw = "start\n\n\n\nspin\nspin\nspin\nspin\n\nend\n";
        let cleaned = clean_output(raw, "");
        // Blank runs collapse to a single separator, not vanish.
        assert_eq!(cleaned.text, "start\n\nspin (×4)\n\nend\n");
    }

    #[test]
    fn keeps_a_pair_of_repeats_untouched() {
        let raw = "warn: x\nwarn: x\n";
        let cleaned = clean_output(raw, "");
        assert_eq!(cleaned.text, "warn: x\nwarn: x\n");
    }

    #[test]
    fn cargo_progress_collapses_but_errors_survive() {
        let raw = "   Compiling a v0.1\n   Compiling b v0.1\n   Compiling c v0.1\n   Compiling d v0.1\nerror[E0001]: boom\n   Compiling e v0.1\n    Finished dev\n";
        let cleaned = clean_output(raw, "cargo test");
        assert_eq!(
            cleaned.text,
            "   [4 cargo progress lines]\nerror[E0001]: boom\n   Compiling e v0.1\n    Finished dev\n"
        );
        // Without the cargo hint the progress lines stay.
        let plain = clean_output(raw, "make");
        assert!(plain.text.contains("Compiling a v0.1"));
    }

    #[test]
    fn libtest_passes_are_hidden_but_failures_and_summary_survive() {
        let raw = "\nrunning 3 tests\ntest a::works ... ok\ntest a::also ... ok\ntest a::skipme ... ignored\ntest a::boom ... FAILED\n\nfailures:\n\n---- a::boom stdout ----\npanicked at 'boom'\n\ntest result: FAILED. 2 passed; 1 failed; 1 ignored;\n";
        let cleaned = clean_output(raw, "cargo test");
        assert!(!cleaned.text.contains("works ... ok"));
        assert!(!cleaned.text.contains("also ... ok"));
        assert!(!cleaned.text.contains("skipme ... ignored"));
        assert!(cleaned.text.contains("test a::boom ... FAILED"));
        assert!(cleaned.text.contains("running 3 tests"));
        assert!(cleaned.text.contains("panicked at 'boom'"));
        assert!(cleaned
            .text
            .contains("test result: FAILED. 2 passed; 1 failed"));
        assert!(cleaned.shrank());
        // Without a test command the pass lines stay.
        assert!(clean_output(raw, "cat log").text.contains("works ... ok"));
    }

    #[test]
    fn nextest_passes_are_hidden_but_failures_survive() {
        let raw = "    PASS [   0.02s] mycrate a::works\n    PASS [   0.10s] mycrate a::also\n    FAIL [   0.01s] mycrate a::boom\n  Summary [   0.2s] 3 tests run: 2 passed, 1 failed\n";
        let cleaned = clean_output(raw, "cargo nextest run");
        assert!(!cleaned.text.contains("a::works"));
        assert!(!cleaned.text.contains("a::also"));
        assert!(cleaned.text.contains("FAIL [   0.01s] mycrate a::boom"));
        assert!(cleaned.text.contains("Summary"));
        assert!(cleaned.text.contains("2 passed, 1 failed"));
    }

    #[test]
    fn pytest_go_and_jest_hide_passes_but_keep_failures() {
        // pytest -v
        let pytest = "tests/test_a.py::test_ok PASSED [ 50%]\ntests/test_a.py::test_bad FAILED [100%]\n=== 1 failed, 1 passed in 0.1s ===\n";
        let c = clean_output(pytest, "pytest -v");
        assert!(!c.text.contains("test_ok PASSED"));
        assert!(c.text.contains("test_bad FAILED"));
        assert!(c.text.contains("1 failed, 1 passed"));

        // go test -v
        let go = "=== RUN   TestOk\n--- PASS: TestOk (0.00s)\n=== RUN   TestBad\n--- FAIL: TestBad (0.01s)\n    x_test.go:9: boom\nFAIL\n";
        let c = clean_output(go, "go test ./...");
        assert!(!c.text.contains("=== RUN"));
        assert!(!c.text.contains("--- PASS: TestOk"));
        assert!(c.text.contains("--- FAIL: TestBad"));
        assert!(c.text.contains("boom"));
        assert!(c.text.contains("FAIL"));

        // jest / vitest
        let jest = "\u{2713} adds numbers\n\u{2717} breaks things\n  expected 1 to be 2\nTests: 1 failed, 1 passed\n";
        let c = clean_output(jest, "npx jest");
        assert!(!c.text.contains("adds numbers"));
        assert!(c.text.contains("breaks things"));
        assert!(c.text.contains("1 failed, 1 passed"));
    }

    #[test]
    fn cargo_word_survives_an_env_prefix() {
        assert_eq!(first_word("RUST_LOG=info cargo build"), Some("cargo"));
        assert_eq!(first_word("cargo build"), Some("cargo"));
        assert_eq!(first_word("ls -la"), Some("ls"));
    }
}
