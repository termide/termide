//! The form that sets a debug launch up: what to debug, how to build it and
//! under which debugger, filled in from the file it was opened from. It is
//! saved as a command with a `debugger` in the project's `commands.toml`,
//! then started.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use termide_config::commands::{
    CommandCwd, CommandItem, CommandMetadata, CommandParam, CommandParamType, CommandsMetadata,
};
use termide_debug::dap::{self, Adapter};
use termide_debug::{shell_quote, Debugger};

use super::debug::NativeDebugger;
use super::App;

/// Every debugger the form offers, as `commands.toml` spells them: the
/// adapters first, which show their session in the debug panel.
const DEBUGGERS: [&str; 8] = [
    "lldb-dap", "gdb-dap", "debugpy", "dlv-dap", "lldb", "gdb", "dlv", "pdb",
];

/// What the form starts with.
#[derive(Debug, Default)]
struct SetupGuess {
    /// What the launch is about, for its default name.
    subject: String,
    program: String,
    build: String,
    debugger: String,
}

impl App {
    /// Open the debug setup form, filled in for `file`.
    pub(in crate::app) fn open_debug_setup(&mut self, file: Option<&Path>) {
        let t = termide_i18n::t();
        let guess = self.debug_setup_guess(file);
        let text = |name: &str, label: &str, value: String| CommandParam {
            name: name.to_string(),
            label: label.to_string(),
            param_type: CommandParamType::Text,
            options: Vec::new(),
            default: Some(value),
        };
        let name = if guess.subject.is_empty() {
            t.debug_select_title().to_string()
        } else {
            t.debug_setup_default_name_fmt(&guess.subject)
        };
        let params = vec![
            text("name", t.debug_setup_name(), name),
            text("program", t.debug_setup_program(), guess.program),
            text("build", t.debug_setup_build(), guess.build),
            CommandParam {
                name: "debugger".to_string(),
                label: t.debug_setup_debugger().to_string(),
                param_type: CommandParamType::Select,
                options: DEBUGGERS.iter().map(|d| d.to_string()).collect(),
                default: Some(guess.debugger),
            },
            text(
                "debugger_args",
                t.debug_setup_debugger_args(),
                String::new(),
            ),
        ];
        let modal = termide_modal::CommandParamsModal::new(String::new(), params)
            .with_title(t.debug_setup_title());
        self.state.set_pending_action(
            termide_state::PendingAction::DebugSetup,
            crate::state::ActiveModal::CommandParams(Box::new(modal)),
        );
        self.state.needs_redraw = true;
    }

    /// Save what the form holds as a project command and start it.
    pub(in crate::app) fn handle_debug_setup(
        &mut self,
        values: &HashMap<String, String>,
    ) -> Result<()> {
        let t = termide_i18n::t();
        let get = |key: &str| {
            values
                .get(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(program) = get("program") else {
            self.show_debug_message(t.debug_setup_no_program().to_string());
            return Ok(());
        };
        let debugger = get("debugger").unwrap_or_else(|| DEBUGGERS[0].to_string());
        // Split as a shell would, so a quoted value stays one flag.
        let debugger_args = match get("debugger_args") {
            Some(text) => match dap::split_words(&text) {
                Some(words) => words,
                None => {
                    self.show_debug_message(t.debug_setup_bad_args_fmt(&text));
                    return Ok(());
                }
            },
            None => Vec::new(),
        };

        let dir = self.project_root.join(".termide");
        let mut metadata = CommandsMetadata::load(&dir);
        let key = metadata.free_name(&termide_modal::sanitize_filename(&format!(
            "debug-{}",
            program_stem(&program)
        )));
        let meta = CommandMetadata {
            command: Some(program.clone()),
            display_name: get("name"),
            debugger: Some(debugger),
            debugger_args,
            build: get("build"),
            // A debug launch runs in the project root whatever mode says; the
            // command says so, so the saved file matches what happens.
            cwd: Some(CommandCwd::Project),
            ..Default::default()
        };
        metadata.entries.insert(key.clone(), meta.clone());
        if let Err(e) = metadata.save(&dir) {
            self.show_error_modal(t.command_run_failed_fmt(&e.to_string()));
            return Ok(());
        }
        log::info!(
            "Debug launch `{key}` saved to {}",
            dir.join("commands.toml").display()
        );
        self.state.cache.commands_registry = None;
        self.state.cache.hotkey_table = None;
        // The user has just written it: no need to ask about it.
        self.approve_project_command(&key);

        self.start_command(CommandItem {
            name: key,
            command: Some(program),
            mode: Default::default(),
            cwd: CommandCwd::Project,
            is_project: true,
            metadata: Some(meta),
        })
    }

    /// The form's fields for `file`. Commands run in the project root, so
    /// paths are written from there.
    fn debug_setup_guess(&self, file: Option<&Path>) -> SetupGuess {
        let native = match self.native_debugger() {
            Some(NativeDebugger::Adapter(adapter)) => adapter.as_str(),
            Some(NativeDebugger::Terminal(debugger)) => debugger.as_str(),
            None => Adapter::LldbDap.as_str(),
        };
        let mut guess = SetupGuess {
            debugger: native.to_string(),
            ..Default::default()
        };
        let Some(file) = file else {
            return guess;
        };
        let ext = file
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        match ext.as_str() {
            "rs" => {
                guess.build = "cargo build".to_string();
                if let Some(bin) = termide_debug::cargo::bins_for(file)
                    .ok()
                    .and_then(|bins| bins.into_iter().next())
                {
                    guess.program = self.relative(&bin.program);
                    guess.build = bin.build_command(&self.project_root);
                    guess.subject = bin.name;
                }
            }
            "py" | "pyw" => {
                guess.program = self.relative(file);
                guess.subject = file_name(file);
                guess.debugger = if self.adapter_installed(Adapter::Debugpy) {
                    Adapter::Debugpy.as_str()
                } else {
                    Debugger::Pdb.as_str()
                }
                .to_string();
            }
            "go" => {
                let package = file
                    .parent()
                    .map(|dir| self.relative(dir))
                    .filter(|dir| !dir.is_empty() && dir != ".")
                    .map(|dir| format!("./{dir}"))
                    .unwrap_or_else(|| ".".to_string());
                guess.subject = file_name(file);
                if self.adapter_installed(Adapter::Delve) {
                    guess.program = package;
                    guess.debugger = Adapter::Delve.as_str().to_string();
                } else {
                    guess.program = format!("debug {package}");
                    guess.debugger = Debugger::Dlv.as_str().to_string();
                }
            }
            _ => guess.subject = file_name(file),
        }
        guess
    }

    /// `path` as a command run in the project root names it, quoted for
    /// the command line.
    fn relative(&self, path: &Path) -> String {
        let shown = path.strip_prefix(&self.project_root).unwrap_or(path);
        shell_quote(&shown.to_string_lossy())
    }
}

fn file_name(file: &Path) -> String {
    file.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The name of the program a command line starts: `app` for
/// `target/debug/app --verbose`.
fn program_stem(command: &str) -> String {
    dap::split_words(command)
        .and_then(|words| words.into_iter().next())
        .and_then(|program| {
            Path::new(&program)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
        })
        .filter(|s| !s.is_empty() && s != ".")
        .unwrap_or_else(|| "launch".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launch_is_named_after_its_program() {
        assert_eq!(program_stem("target/debug/app --verbose"), "app");
        assert_eq!(program_stem("scripts/run.py"), "run");
        assert_eq!(program_stem("."), "launch");
        assert_eq!(program_stem(""), "launch");
    }
}
