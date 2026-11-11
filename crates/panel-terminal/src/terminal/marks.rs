//! Shell commands as the shell reports them with OSC 133 marks: where each
//! prompt starts (`A`), where its command's output starts (`C`), and where
//! it ended, with its exit code (`D`).
//!
//! Rows are kept as *stable* rows: the absolute row (scrollback index, then
//! screen row) plus the rows that have left the front of the scrollback
//! since the terminal started. An absolute row moves only when the
//! scrollback drops its oldest line; a stable row never does.

use std::collections::VecDeque;

/// How many commands a terminal remembers; older ones are forgotten.
const MAX_COMMANDS: usize = 2_000;

/// One prompt and the command typed at it, in stable rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShellCommand {
    /// The row the prompt starts on.
    pub prompt: u64,
    /// The row the output starts on, once the command runs.
    pub output: Option<u64>,
    /// The row past the command's last, once it ended.
    pub end: Option<u64>,
    /// The exit code the shell reported with the end.
    pub exit: Option<i32>,
}

impl ShellCommand {
    /// The prompt is still waiting for a command.
    pub fn is_pending(&self) -> bool {
        self.output.is_none() && self.end.is_none()
    }

    /// The command ended with a non-zero exit code.
    pub fn failed(&self) -> bool {
        self.end.is_some() && self.exit.is_some_and(|code| code != 0)
    }
}

/// The commands of a terminal's main screen.
#[derive(Clone, Debug, Default)]
pub struct ShellMarks {
    commands: VecDeque<ShellCommand>,
    /// Rows dropped off the front of the scrollback so far.
    dropped: u64,
}

impl ShellMarks {
    /// The stable row of absolute row `abs`.
    pub fn stable(&self, abs: usize) -> u64 {
        self.dropped + abs as u64
    }

    /// The absolute row of stable row `row`, unless it left the scrollback.
    pub fn absolute(&self, row: u64) -> Option<usize> {
        row.checked_sub(self.dropped).map(|abs| abs as usize)
    }

    pub fn commands(&self) -> impl DoubleEndedIterator<Item = &ShellCommand> {
        self.commands.iter()
    }

    /// The last command that ran or runs: not a prompt still waiting.
    pub fn last_command(&self) -> Option<ShellCommand> {
        self.commands
            .iter()
            .rev()
            .find(|c| !c.is_pending())
            .copied()
    }

    /// `A`: a prompt starts at stable row `row`. A prompt drawn again on
    /// the same row is the same prompt; one left without a command gives
    /// way; a command never reported ended ends here.
    pub fn prompt_start(&mut self, row: u64) {
        if let Some(last) = self.commands.back_mut() {
            if last.prompt == row && last.end.is_none() {
                return;
            }
            if last.is_pending() {
                self.commands.pop_back();
            } else if last.end.is_none() {
                last.end = Some(row.max(last.prompt + 1));
            }
        }
        self.push(ShellCommand {
            prompt: row,
            output: None,
            end: None,
            exit: None,
        });
    }

    /// `C`: the command typed at the last prompt starts its output at
    /// `row`. Without a prompt seen, the command starts there; a second
    /// report for a running command changes nothing.
    pub fn command_start(&mut self, row: u64) {
        match self.commands.back_mut() {
            Some(last) if last.is_pending() => last.output = Some(row.max(last.prompt)),
            Some(last) if last.end.is_none() => {}
            _ => self.push(ShellCommand {
                prompt: row,
                output: Some(row),
                end: None,
                exit: None,
            }),
        }
    }

    /// `D`: the running command ended with the cursor at stable row `row`,
    /// column `col`. A command that left nothing — not even its prompt,
    /// which `clear` takes along — is forgotten.
    pub fn command_end(&mut self, row: u64, col: usize, exit: Option<i32>) {
        let Some(last) = self.commands.back_mut() else {
            return;
        };
        if last.end.is_some() {
            return;
        }
        let end = (row + u64::from(col > 0)).max(last.output.unwrap_or(last.prompt));
        if end <= last.prompt {
            self.commands.pop_back();
            return;
        }
        last.end = Some(end);
        last.exit = exit;
    }

    /// `n` rows left the front of the scrollback. Commands wholly gone
    /// with them are forgotten.
    pub fn drop_front(&mut self, n: usize) {
        self.dropped += n as u64;
        let dropped = self.dropped;
        while self
            .commands
            .front()
            .is_some_and(|c| c.end.is_some_and(|end| end <= dropped))
        {
            self.commands.pop_front();
        }
    }

    /// The screen, from stable row `top` down, was erased. What ended on
    /// it is forgotten, what ended below `top` is cut there; a command
    /// still open goes on from the top, as the shell redraws its prompt
    /// or the program its output there.
    pub fn screen_cleared(&mut self, top: u64) {
        self.commands.retain_mut(|c| {
            match c.end {
                Some(_) if c.prompt >= top => return false,
                Some(end) => c.end = Some(end.min(top)),
                None => {
                    c.prompt = c.prompt.min(top);
                    c.output = c.output.map(|o| o.min(top));
                }
            }
            true
        });
    }

    fn push(&mut self, command: ShellCommand) {
        if self.commands.len() >= MAX_COMMANDS {
            self.commands.pop_front();
        }
        self.commands.push_back(command);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(prompt: u64, output: Option<u64>, end: Option<u64>, exit: Option<i32>) -> ShellCommand {
        ShellCommand {
            prompt,
            output,
            end,
            exit,
        }
    }

    fn all(marks: &ShellMarks) -> Vec<ShellCommand> {
        marks.commands().copied().collect()
    }

    #[test]
    fn a_command_runs_from_its_prompt_to_its_end() {
        let mut marks = ShellMarks::default();
        marks.prompt_start(0);
        marks.command_start(1);
        marks.command_end(3, 0, Some(2));
        marks.prompt_start(3);
        assert_eq!(
            all(&marks),
            vec![cmd(0, Some(1), Some(3), Some(2)), cmd(3, None, None, None)]
        );
        assert!(marks.commands().next().unwrap().failed());
        assert_eq!(
            marks.last_command(),
            Some(cmd(0, Some(1), Some(3), Some(2)))
        );
    }

    /// Output without a final newline leaves the cursor on its last row,
    /// which belongs to the command.
    #[test]
    fn an_unfinished_last_line_belongs_to_the_command() {
        let mut marks = ShellMarks::default();
        marks.prompt_start(0);
        marks.command_start(1);
        marks.command_end(1, 5, Some(0));
        assert_eq!(all(&marks), vec![cmd(0, Some(1), Some(2), Some(0))]);
    }

    /// An empty line at a prompt brings the next prompt without a command;
    /// a prompt redrawn on its row, and repeated marks, change nothing.
    #[test]
    fn empty_prompts_and_repeated_marks_leave_one_entry() {
        let mut marks = ShellMarks::default();
        marks.prompt_start(0);
        marks.prompt_start(1);
        marks.prompt_start(1);
        marks.command_start(2);
        marks.command_start(2);
        marks.command_end(4, 0, Some(0));
        marks.command_end(4, 0, Some(1));
        assert_eq!(all(&marks), vec![cmd(1, Some(2), Some(4), Some(0))]);
    }

    #[test]
    fn a_prompt_ends_a_command_never_reported_ended() {
        let mut marks = ShellMarks::default();
        marks.prompt_start(0);
        marks.command_start(1);
        marks.prompt_start(5);
        assert_eq!(
            all(&marks),
            vec![cmd(0, Some(1), Some(5), None), cmd(5, None, None, None)]
        );
    }

    #[test]
    fn rows_leaving_the_scrollback_forget_what_they_held() {
        let mut marks = ShellMarks::default();
        marks.prompt_start(0);
        marks.command_start(1);
        marks.command_end(2, 0, Some(0));
        marks.prompt_start(2);
        marks.command_start(3);
        marks.command_end(9, 0, Some(0));
        marks.drop_front(3);
        assert_eq!(all(&marks), vec![cmd(2, Some(3), Some(9), Some(0))]);
        assert_eq!(marks.absolute(2), None);
        assert_eq!(marks.absolute(9), Some(6));
        assert_eq!(marks.stable(6), 9);
    }

    /// `clear` erases its own prompt with the screen, so it leaves nothing,
    /// and the prompt after it starts at the top.
    #[test]
    fn clearing_the_screen_forgets_what_was_on_it() {
        let mut marks = ShellMarks::default();
        marks.prompt_start(10);
        marks.command_start(11);
        marks.command_end(13, 0, Some(1));
        marks.prompt_start(13);
        marks.command_start(14);
        marks.screen_cleared(12);
        marks.command_end(12, 0, Some(0));
        marks.prompt_start(12);
        assert_eq!(
            all(&marks),
            vec![
                cmd(10, Some(11), Some(12), Some(1)),
                cmd(12, None, None, None)
            ]
        );
    }

    /// Clearing the screen at a prompt (`Ctrl+L`) redraws that prompt at
    /// the top.
    #[test]
    fn a_waiting_prompt_moves_to_the_top_of_a_cleared_screen() {
        let mut marks = ShellMarks::default();
        marks.prompt_start(20);
        marks.screen_cleared(15);
        marks.prompt_start(15);
        assert_eq!(all(&marks), vec![cmd(15, None, None, None)]);
    }
}
