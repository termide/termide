//! What the language server says of a file `edit` or `write` just changed:
//! its errors, appended to the tool's result so the agent sees a mistake in
//! the same step that made it, as opencode, Crush and Claude Code do. The
//! tools hold no LSP machinery: the app installs the check that tells the
//! server and waits for its answer. Without one (headless mode, a test) the
//! result says nothing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, PoisonError, RwLock};

use termide_agent_core::CancelToken;

/// An error a language server reports in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileError {
    /// 1-based line.
    pub line: u32,
    /// 1-based column.
    pub column: u32,
    pub message: String,
    /// The tool that reports it: `rustc`, `Pyright`...
    pub source: Option<String>,
    pub code: Option<String>,
}

/// The server's answer for a changed file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LspReport {
    /// The file's errors after the change.
    pub errors: Vec<FileError>,
    /// Its errors before the change, when they are known: only the errors
    /// that came with the change are then shown.
    pub before: Option<Vec<FileError>>,
}

/// Tells the language server the file changed and waits for its errors;
/// `None` when no server answered for it in time.
pub type LspCheck = Arc<dyn Fn(&Path, &CancelToken) -> Option<LspReport> + Send + Sync>;

/// Errors listed at most; the rest are counted.
const SHOWN: usize = 20;

static CHECK: RwLock<Option<LspCheck>> = RwLock::new(None);

/// Install the check `edit` and `write` run on the file they changed.
pub fn set_lsp_check(check: LspCheck) {
    *CHECK.write().unwrap_or_else(PoisonError::into_inner) = Some(check);
}

/// What to append to the result of a change to `path`: the errors the
/// language server reports, or nothing.
pub(crate) fn errors_after_change(path: &Path, cancel: &CancelToken) -> Option<String> {
    let check = CHECK
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    describe(&check(path, cancel)?)
}

/// The errors of `report` as the agent reads them: the new ones when the
/// errors before the change are known, every one otherwise.
fn describe(report: &LspReport) -> Option<String> {
    let (shown, kept) = match &report.before {
        Some(before) => {
            let new = new_errors(before, &report.errors);
            let kept = report.errors.len() - new.len();
            (new, kept)
        }
        None => (report.errors.iter().collect(), 0),
    };
    if shown.is_empty() {
        return None;
    }
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let mut text = format!(
        "\n\nThe language server reports {} {}error{} in this file:",
        shown.len(),
        if report.before.is_some() { "new " } else { "" },
        plural(shown.len())
    );
    for error in shown.iter().take(SHOWN) {
        let mut lines = error.message.trim_end().lines();
        let first = lines.next().unwrap_or_default();
        text.push_str(&format!("\n  {}:{} {first}", error.line, error.column));
        match (&error.source, &error.code) {
            (Some(source), Some(code)) => text.push_str(&format!(" ({source} {code})")),
            (Some(label), None) | (None, Some(label)) => text.push_str(&format!(" ({label})")),
            (None, None) => {}
        }
        // A message of several lines keeps the rest under its first.
        for line in lines {
            text.push_str(&format!("\n    {line}"));
        }
    }
    if shown.len() > SHOWN {
        text.push_str(&format!("\n  ...and {} more", shown.len() - SHOWN));
    }
    if kept > 0 {
        text.push_str(&format!(
            "\n{kept} error{} that {} there before the change {} not listed.",
            plural(kept),
            if kept == 1 { "was" } else { "were" },
            if kept == 1 { "is" } else { "are" }
        ));
    }
    Some(text)
}

/// The errors of `after` that `before` did not have. Errors are told apart
/// by what they say, not where: a change moves the lines below it.
fn new_errors<'a>(before: &[FileError], after: &'a [FileError]) -> Vec<&'a FileError> {
    let key = |e: &FileError| (e.message.clone(), e.source.clone(), e.code.clone());
    let mut old: HashMap<_, usize> = HashMap::new();
    for error in before {
        *old.entry(key(error)).or_default() += 1;
    }
    after
        .iter()
        .filter(|error| match old.get_mut(&key(error)) {
            Some(left) if *left > 0 => {
                *left -= 1;
                false
            }
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(line: u32, message: &str) -> FileError {
        FileError {
            line,
            column: 5,
            message: message.into(),
            source: Some("rustc".into()),
            code: Some("E0308".into()),
        }
    }

    /// With nothing known of the file before, every error is listed.
    #[test]
    fn every_error_is_listed_when_nothing_was_known_before() {
        let report = LspReport {
            errors: vec![
                error(3, "mismatched types\nexpected `u32`, found `&str`"),
                error(9, "unknown name"),
            ],
            before: None,
        };
        assert_eq!(
            describe(&report).unwrap(),
            "\n\nThe language server reports 2 errors in this file:\
             \n  3:5 mismatched types (rustc E0308)\
             \n    expected `u32`, found `&str`\
             \n  9:5 unknown name (rustc E0308)"
        );
    }

    /// An error the file had before is not repeated, wherever the change
    /// moved it; one more of the same kind is new.
    #[test]
    fn only_new_errors_are_listed_when_the_old_ones_are_known() {
        let report = LspReport {
            errors: vec![
                error(4, "old trouble"),
                error(7, "mismatched types"),
                error(8, "mismatched types"),
            ],
            before: Some(vec![error(2, "old trouble"), error(5, "mismatched types")]),
        };
        assert_eq!(
            describe(&report).unwrap(),
            "\n\nThe language server reports 1 new error in this file:\
             \n  8:5 mismatched types (rustc E0308)\
             \n2 errors that were there before the change are not listed."
        );
    }

    /// No new error, nothing to say; a long list is cut.
    #[test]
    fn nothing_new_says_nothing_and_a_long_list_is_cut() {
        let before = vec![error(1, "old")];
        let report = LspReport {
            errors: before.clone(),
            before: Some(before),
        };
        assert_eq!(describe(&report), None);
        assert_eq!(describe(&LspReport::default()), None);

        let many = LspReport {
            errors: (1..=25).map(|n| error(n, "bad")).collect(),
            before: None,
        };
        let text = describe(&many).unwrap();
        assert!(text.contains("25 errors"), "{text}");
        assert!(text.contains("\n  20:5 bad"), "{text}");
        assert!(!text.contains("\n  21:5 bad"), "{text}");
        assert!(text.ends_with("...and 5 more"), "{text}");
    }
}
