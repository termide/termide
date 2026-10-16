//! Debug launches: a command-line debugger started in a terminal panel with
//! the breakpoints of the project's files set.
//!
//! What to debug comes from the commands with a `debugger` in
//! `commands.toml`, or — for a Python or Go file — from the file alone.

use std::path::{Path, PathBuf};

use anyhow::Result;
use termide_config::commands::{CommandItem, CommandsRegistry};
use termide_debug::{breakpoints, Breakpoint, Debugger, Launch};

use super::App;
use crate::PanelExt;

/// One entry of the debug launch list.
#[derive(Debug, Clone)]
pub(super) enum DebugChoice {
    /// A command with a `debugger`.
    Command(CommandItem),
    /// The launch a file needs no configuration for.
    File { launch: Launch, cwd: PathBuf },
}

impl App {
    /// Start debugging: the only launch there is, or a list to choose from.
    /// `file` is the file the request came from.
    pub(in crate::app) fn start_debug(&mut self, file: Option<PathBuf>) -> Result<()> {
        // Read the files afresh, as the Commands menu does when it opens.
        let registry = CommandsRegistry::load_merged(Some(&self.project_root)).unwrap_or_default();
        let mut choices: Vec<DebugChoice> = registry
            .debug_commands()
            .into_iter()
            .cloned()
            .map(DebugChoice::Command)
            .collect();
        if let Some(file) = file.as_deref() {
            if let Some(launch) = Launch::for_file(file) {
                let cwd = file
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.project_root.clone());
                choices.push(DebugChoice::File { launch, cwd });
            }
        }

        let t = termide_i18n::t();
        match choices.len() {
            0 => {
                self.state.set_info(t.debug_nothing_to_launch().to_string());
                Ok(())
            }
            1 => self.run_debug_choice(choices.remove(0)),
            _ => {
                let labels = choices
                    .iter()
                    .map(|choice| choice_label(choice, file.as_deref()))
                    .collect();
                let modal = termide_modal::SelectModal::single(t.debug_select_title(), "", labels);
                self.debug_choices = Some(choices);
                self.state.set_pending_action(
                    termide_state::PendingAction::ChooseDebugLaunch,
                    crate::state::ActiveModal::Select(Box::new(modal)),
                );
                Ok(())
            }
        }
    }

    /// The launch picked from the list `start_debug` opened.
    pub(in crate::app) fn handle_debug_choice(
        &mut self,
        value: Box<dyn std::any::Any>,
    ) -> Result<()> {
        let Some(choices) = self.debug_choices.take() else {
            return Ok(());
        };
        match value
            .downcast_ref::<usize>()
            .and_then(|&i| choices.into_iter().nth(i))
        {
            Some(choice) => self.run_debug_choice(choice),
            None => Ok(()),
        }
    }

    fn run_debug_choice(&mut self, choice: DebugChoice) -> Result<()> {
        match choice {
            // Through the usual entry, so its parameters are asked for first.
            DebugChoice::Command(command) => self.start_command(command),
            DebugChoice::File { launch, cwd } => self.launch_debugger(&launch, cwd, &[]),
        }
    }

    /// Run a command that has a `debugger`: its command line is the program
    /// to debug. It runs in the project root, where a path such as
    /// `target/debug/app` is meant from, whichever panel is focused.
    pub(in crate::app) fn run_debug_command(
        &mut self,
        command: &CommandItem,
        env: &[(String, String)],
    ) -> Result<()> {
        let name = command.debugger().unwrap_or_default();
        let Some(debugger) = Debugger::parse(name) else {
            self.show_error_modal(termide_i18n::t().debug_unknown_debugger_fmt(name));
            return Ok(());
        };
        let launch = Launch {
            debugger,
            target: command.command.clone().unwrap_or_default(),
            build: command.metadata.as_ref().and_then(|m| m.build.clone()),
        };
        let cwd = self.project_root.clone();
        self.launch_debugger(&launch, cwd, env)
    }

    /// Open a terminal in `cwd` running `launch` with the project's
    /// breakpoints set.
    fn launch_debugger(
        &mut self,
        launch: &Launch,
        cwd: PathBuf,
        env: &[(String, String)],
    ) -> Result<()> {
        use termide_panel_terminal::Terminal;

        self.sync_editor_breakpoints();
        breakpoints::flush();
        let set = project_breakpoints(&breakpoints::store().all(), &self.project_root);
        let script_dir = std::env::temp_dir().join("termide-debug");
        let line = match launch.command_line(&set, &script_dir) {
            Ok(line) => line,
            Err(e) => {
                self.show_error_modal(termide_i18n::t().command_run_failed_fmt(&format!("{e:#}")));
                return Ok(());
            }
        };
        log::info!(
            "Debugging with {} ({} breakpoints) in {:?}: {}",
            launch.debugger.as_str(),
            set.len(),
            cwd,
            line
        );

        self.close_help_panels();
        let term_height = self.state.terminal.height.saturating_sub(3);
        let term_width = self.state.terminal.width.saturating_sub(2);
        match Terminal::new_with_cwd_env(term_height, term_width, Some(cwd), env) {
            Ok(mut terminal) => {
                let _ = terminal.send_command(&line);
                self.add_panel(Box::new(terminal));
                self.auto_save_layout();
            }
            Err(e) => {
                log::error!("Failed to create a terminal for the debugger: {e}");
                self.show_error_modal(termide_i18n::t().command_run_failed_fmt(&e.to_string()));
            }
        }
        Ok(())
    }

    /// Remove every breakpoint, from the store and from open editors.
    pub(in crate::app) fn clear_breakpoints(&mut self) {
        breakpoints::store().clear();
        breakpoints::flush();
        for panel in self.all_editor_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.reload_breakpoints();
            }
        }
        self.state
            .set_info(termide_i18n::t().debug_breakpoints_cleared().to_string());
        self.state.needs_redraw = true;
    }

    /// Write the breakpoints of every open editor to the store: an edit made
    /// by the app (a rename across files) moves them without the editor
    /// seeing input.
    pub(in crate::app) fn sync_editor_breakpoints(&mut self) {
        for panel in self.all_editor_panels_mut() {
            if let Some(editor) = panel.as_editor_mut() {
                editor.sync_breakpoints();
            }
        }
    }

    /// The panels of the current project and of the projects parked in the
    /// background.
    fn all_editor_panels_mut(&mut self) -> Vec<&mut Box<dyn termide_core::Panel>> {
        let mut panels: Vec<_> = self.layout_manager.iter_all_panels_mut().collect();
        for (_, parked) in self.open_projects.parked_mut() {
            panels.extend(parked.panels_mut());
        }
        panels
    }
}

fn choice_label(choice: &DebugChoice, file: Option<&Path>) -> String {
    match choice {
        DebugChoice::Command(command) => command
            .metadata
            .as_ref()
            .and_then(|m| m.display_name.clone())
            .unwrap_or_else(|| command.name.clone()),
        DebugChoice::File { launch, .. } => {
            let name = file
                .and_then(Path::file_name)
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            termide_i18n::t().debug_current_file_fmt(&name, launch.debugger.as_str())
        }
    }
}

/// The breakpoints in files under `root`: others belong to other projects,
/// and a debugger would only report them as unresolved. Paths are resolved
/// through symlinks where they exist, as the running program names its
/// files that way (on macOS `/var` is `/private/var`, and pdb then misses a
/// breakpoint set by the other name).
fn project_breakpoints(all: &[Breakpoint], root: &Path) -> Vec<Breakpoint> {
    let canonical_root = dunce::canonicalize(root).ok();
    all.iter()
        .filter_map(|b| {
            let canonical = dunce::canonicalize(&b.path).ok();
            let inside = b.path.starts_with(root)
                || canonical
                    .as_deref()
                    .zip(canonical_root.as_deref())
                    .is_some_and(|(path, root)| path.starts_with(root));
            inside.then(|| Breakpoint {
                path: canonical.unwrap_or_else(|| b.path.clone()),
                line: b.line,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_projects_breakpoints_are_set() {
        let bp = |path: &str| Breakpoint {
            path: path.into(),
            line: 1,
        };
        let all = [bp("/work/app/src/main.rs"), bp("/work/other/lib.rs")];
        let set = project_breakpoints(&all, Path::new("/work/app"));
        assert_eq!(set, [bp("/work/app/src/main.rs")]);
    }
}
