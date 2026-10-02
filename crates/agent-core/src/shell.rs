//! Running a shell command on the user's own word.
//!
//! Two paths lead a command to the shell because the user asked for it, and
//! both run through one [`ShellRunner`] so what runs is what they saw:
//!
//! - a command typed in the agent panel's shell mode (`$`), run there and then;
//! - a card the `suggest_command` tool put up, confirmed with `[Run]`.
//!
//! It lives outside the agent's tool registry on purpose. A command the user
//! typed is not something the agent was given leave to do, so it must not
//! depend on whether the agent profile's `tools` list kept `bash`; and a
//! command confirmed on a card arrives in the middle of a tool call, where the
//! loop is busy and cannot be asked to run another.
//!
//! The app builds it over the same `bash` tool the agent uses, so timeouts,
//! output cleaning and the full-log file behave as they do for a call the agent
//! made itself.

use std::sync::Arc;

use crate::cancel::CancelToken;

/// What a hand-run command produced: what it printed, and whether it ended
/// badly (a non-zero exit, a timeout, a kill). The text carries the detail in
/// both cases; `failed` is what the UI marks red without parsing the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellOutput {
    pub text: String,
    pub failed: bool,
}

/// How a [`ShellRunner`] reaches the shell.
type Run = dyn Fn(&str, &CancelToken) -> Result<ShellOutput, String> + Send + Sync;

/// Runs a shell command the user asked for, and reports what it printed.
#[derive(Clone)]
pub struct ShellRunner {
    run: Arc<Run>,
}

impl std::fmt::Debug for ShellRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellRunner").finish_non_exhaustive()
    }
}

impl ShellRunner {
    /// A runner that takes `command` to the shell through `run`. It blocks
    /// until the command finishes; `cancel` stops it. An `Err` is why it
    /// could not run at all — no shell, a spawn failure — not a non-zero exit,
    /// which belongs in the text along with the output.
    #[must_use]
    pub fn new(
        run: impl Fn(&str, &CancelToken) -> Result<ShellOutput, String> + Send + Sync + 'static,
    ) -> Self {
        Self { run: Arc::new(run) }
    }

    /// Run `command`. The caller has established that the user asked for it:
    /// they typed it, or they confirmed the card that showed it.
    pub fn run(&self, command: &str, cancel: &CancelToken) -> Result<ShellOutput, String> {
        (self.run)(command, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runner_hands_the_command_to_what_it_was_built_with() {
        use std::sync::Mutex;
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let recorder = Arc::clone(&seen);
        let runner = ShellRunner::new(move |command, cancel| {
            assert!(!cancel.is_cancelled(), "the runner is told to stop");
            recorder.lock().unwrap().push(command.to_string());
            Ok(ShellOutput {
                text: "done".to_string(),
                failed: false,
            })
        });
        assert_eq!(runner.run("ls", &CancelToken::new()).unwrap().text, "done");
        assert_eq!(seen.lock().unwrap().as_slice(), ["ls"]);
    }

    /// A cancelled run stops the command rather than starting it late.
    #[test]
    fn a_cancelled_run_reports_it() {
        let runner = ShellRunner::new(|_, cancel| {
            if cancel.is_cancelled() {
                Err("stopped".to_string())
            } else {
                Ok(ShellOutput {
                    text: "ran".to_string(),
                    failed: false,
                })
            }
        });
        assert_eq!(runner.run("ls", &CancelToken::new()).unwrap().text, "ran");
        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(runner.run("ls", &cancel), Err("stopped".to_string()));

        // A non-zero exit is text plus the flag, not an error: it ran.
        let failing = ShellRunner::new(|_, _| {
            Ok(ShellOutput {
                text: "nope
[exit code 1]"
                    .to_string(),
                failed: true,
            })
        });
        let out = failing.run("false", &CancelToken::new()).unwrap();
        assert!(out.failed);
        assert!(out.text.contains("[exit code 1]"));
    }
}
