//! What a terminal does with the commands its shell marks (see
//! [`crate::terminal::marks`]): jumping between their prompts, handing the
//! last one to an agent and copying its output.

use crate::clipboard::rows_lines;
use crate::terminal::marks::ShellCommand;
use crate::terminal::TerminalScreen;

/// Most output lines of a command handed to an agent; the first ones go.
pub const AGENT_COMMAND_LINES: usize = 1_000;

/// The absolute rows of `command` still held: where its prompt, its output
/// and its end are (the end exclusive). A command still running reaches
/// down to the cursor row; one without an output mark has its output
/// below its prompt row.
fn rows(screen: &TerminalScreen, command: &ShellCommand) -> (usize, usize, usize) {
    let total = screen.scrollback.len() + screen.lines.len();
    let absolute = |row: u64| screen.marks.absolute(row).unwrap_or(0).min(total);
    let prompt = absolute(command.prompt);
    let end = match command.end {
        Some(end) => absolute(end),
        None => (screen.scrollback.len() + screen.cursor.0 + 1).min(total),
    }
    .max(prompt);
    let output = absolute(command.output.unwrap_or(command.prompt + 1)).clamp(prompt, end);
    (prompt, output, end)
}

/// The last command as text, for an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandText {
    /// The prompt line with the command, then the output.
    pub text: String,
    pub running: bool,
    pub exit: Option<i32>,
    /// Output lines left out at the start.
    pub omitted: usize,
}

/// The last command the shell ran or runs, with at most `max_output` lines
/// of its output; `None` without one, or while a full-screen program has
/// the screen.
pub fn last_command(screen: &TerminalScreen, max_output: usize) -> Option<CommandText> {
    if screen.use_alt_screen {
        return None;
    }
    let command = screen.marks.last_command()?;
    let (prompt, output, end) = rows(screen, &command);
    let mut lines = rows_lines(screen, prompt, output);
    let mut out = rows_lines(screen, output, end);
    let omitted = out.len().saturating_sub(max_output);
    out.drain(..omitted);
    lines.extend(out);
    if lines.is_empty() {
        return None;
    }
    Some(CommandText {
        text: lines.join("\n"),
        running: command.end.is_none(),
        exit: command.exit,
        omitted,
    })
}

/// The output of the last command, line by line; `None` when it printed
/// nothing, or there is no command.
pub fn last_output(screen: &TerminalScreen) -> Option<Vec<String>> {
    if screen.use_alt_screen {
        return None;
    }
    let command = screen.marks.last_command()?;
    let (_, output, end) = rows(screen, &command);
    Some(rows_lines(screen, output, end)).filter(|lines| !lines.is_empty())
}

/// The scroll offset that puts at the top of the view the prompt before
/// its top line, or after it when `forward`; past the last prompt, the
/// live screen. `None` when there is nowhere to go, or a full-screen
/// program has the screen.
pub fn prompt_offset(screen: &TerminalScreen, forward: bool) -> Option<usize> {
    if screen.use_alt_screen {
        return None;
    }
    let held = screen.scrollback.len();
    let top = held - screen.scroll_offset.min(held);
    let prompts = screen
        .marks
        .commands()
        .filter_map(|c| screen.marks.absolute(c.prompt));
    let target = if forward {
        prompts.filter(|&p| p > top).min()
    } else {
        prompts.filter(|&p| p < top).max()
    };
    let offset = match target {
        Some(prompt) => held.saturating_sub(prompt),
        None if forward => 0,
        None => return None,
    };
    (offset != screen.scroll_offset).then_some(offset)
}

/// The view rows (from 0 at the top) of the commands that failed, for a
/// view of `rows` rows.
pub fn failed_rows(screen: &TerminalScreen, rows: usize) -> Vec<usize> {
    if screen.use_alt_screen {
        return Vec::new();
    }
    let top = screen.visual_to_absolute(0);
    let mut marked = Vec::new();
    for command in screen.marks.commands().filter(|c| c.failed()) {
        let (prompt, _, end) = self::rows(screen, command);
        let first = prompt.max(top);
        let last = end.min(top + rows);
        marked.extend((first..last).map(|abs| abs - top));
    }
    marked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::VtPerformer;
    use std::sync::{Arc, RwLock};

    /// A 5-row screen fed `bytes`.
    fn screen_after(bytes: &[u8]) -> TerminalScreen {
        let input = crate::pty_input::PtyInput::unspawned();
        let screen = Arc::new(RwLock::new(TerminalScreen::new(5, 20)));
        let mut performer = VtPerformer::new(input.replies(), Arc::clone(&screen));
        let mut parser = crate::terminal::VtParser::new();
        for byte in bytes {
            parser.advance(&mut performer, *byte);
        }
        performer.flush();
        let screen = screen.read().unwrap().clone();
        screen
    }

    const A: &str = "\x1b]133;A\x07";
    const C: &str = "\x1b]133;C\x07";

    fn d(code: i32) -> String {
        format!("\x1b]133;D;{code}\x07")
    }

    /// Three commands, the screen five rows: the first ones have gone
    /// into the scrollback.
    fn session() -> TerminalScreen {
        let text = format!(
            "{A}$ one\r\n{C}1\r\n{d}{A}$ two\r\n{C}2a\r\n2b\r\n{d2}{A}$ three\r\n{C}3\r\n{d}{A}$ ",
            d = d(0),
            d2 = d(1),
        );
        screen_after(text.as_bytes())
    }

    #[test]
    fn the_last_command_is_its_prompt_line_and_output() {
        let screen = session();
        assert_eq!(
            last_command(&screen, 10),
            Some(CommandText {
                text: "$ three\n3".into(),
                running: false,
                exit: Some(0),
                omitted: 0,
            })
        );
        assert_eq!(last_output(&screen), Some(vec!["3".to_string()]));
    }

    #[test]
    fn a_running_command_reaches_the_cursor_and_long_output_loses_its_start() {
        let text = format!("{A}$ build\r\n{C}a\r\nb\r\nc");
        let screen = screen_after(text.as_bytes());
        assert_eq!(
            last_command(&screen, 2),
            Some(CommandText {
                text: "$ build\nb\nc".into(),
                running: true,
                exit: None,
                omitted: 1,
            })
        );
    }

    #[test]
    fn without_marks_there_is_no_command() {
        let screen = screen_after(b"$ ls\r\nfile\r\n$ ");
        assert_eq!(last_command(&screen, 10), None);
        assert_eq!(prompt_offset(&screen, false), None);
    }

    /// Back from the live screen goes to the prompt above the view's top,
    /// then the one before; forward comes back down, then to the live
    /// screen.
    #[test]
    fn jumps_go_from_prompt_to_prompt() {
        let mut screen = session();
        // Rows: 0 `$ one`, 1 `1`, 2 `$ two`, 3 `2a`, 4 `2b`, 5 `$ three`,
        // 6 `3`, 7 `$ `; the screen shows rows 3..8.
        assert_eq!(screen.scrollback.len(), 3);
        assert_eq!(prompt_offset(&screen, false), Some(1));
        screen.set_scroll_offset(1);
        assert_eq!(prompt_offset(&screen, false), Some(3));
        screen.set_scroll_offset(3);
        assert_eq!(prompt_offset(&screen, false), None);
        assert_eq!(prompt_offset(&screen, true), Some(1));
        screen.set_scroll_offset(1);
        assert_eq!(prompt_offset(&screen, true), Some(0));
        screen.set_scroll_offset(0);
        assert_eq!(prompt_offset(&screen, true), None);
    }

    #[test]
    fn a_failed_command_marks_its_rows_in_view() {
        let mut screen = session();
        // `two` (rows 2..5) failed; the live view starts at row 3.
        assert_eq!(failed_rows(&screen, 5), vec![0, 1]);
        screen.set_scroll_offset(3);
        assert_eq!(failed_rows(&screen, 5), vec![2, 3, 4]);
    }
}
