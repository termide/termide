//! Delegated tasks running out of sight, for a panel to follow.
//!
//! A hidden delegate registers here while it runs, under the panel and the
//! `task` call that handed it over: a click on its row in that panel's state
//! strip, or `o` on its block, finds it and opens a panel following its
//! session log (see `termide_panel_agent::Watch`). Following takes nothing
//! from the task: the panel reads the log, and closing it leaves the task
//! running. The registration ends with the task, which marks it over for
//! the panels following it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock, PoisonError};

use termide_panel_agent::{AgentPanel, DelegateControl, Watch};

use super::{agent_setup, DEFAULT_AGENT};
use crate::app::App;

/// A task running out of sight.
struct Live {
    /// The delegating agent.
    from: String,
    task: String,
    session: PathBuf,
    control: DelegateControl,
}

/// The tasks running out of sight, by the panel and the `task` call that
/// handed each over.
fn registry() -> &'static Mutex<HashMap<(u64, String), Live>> {
    static LIVE: OnceLock<Mutex<HashMap<(u64, String), Live>>> = OnceLock::new();
    LIVE.get_or_init(Mutex::default)
}

/// A task's place in the registry, for as long as it runs: dropped, the
/// task is over.
pub(super) struct Registration {
    key: Option<(u64, String)>,
    control: DelegateControl,
}

impl Registration {
    /// What the user does to the task from a panel following it.
    pub(super) fn control(&self) -> &DelegateControl {
        &self.control
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.control.finish();
        if let Some(key) = &self.key {
            registry()
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(key);
        }
    }
}

/// Register the task the `task` call `call` of panel `panel`, the agent
/// `from`'s, handed over, logged at `session`. Only a task from a panel,
/// with a log, can be followed; the others get a registration all the same,
/// for its control.
pub(super) fn register(
    panel: Option<u64>,
    call: &str,
    from: &str,
    task: &str,
    session: Option<PathBuf>,
) -> Registration {
    let control = DelegateControl::new();
    let key = panel.zip(session).map(|(panel, session)| {
        let key = (panel, call.to_string());
        registry()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                key.clone(),
                Live {
                    from: from.to_string(),
                    task: task.to_string(),
                    session,
                    control: control.clone(),
                },
            );
        key
    });
    Registration { key, control }
}

/// What a panel needs to follow the task the `task` call `call` of panel
/// `panel` handed over, while it runs.
fn watch(panel: u64, call: &str) -> Option<Watch> {
    let registry = registry().lock().unwrap_or_else(PoisonError::into_inner);
    let live = registry.get(&(panel, call.to_string()))?;
    Some(Watch::new(
        live.from.clone(),
        &live.task,
        live.session.clone(),
        live.control.clone(),
        panel,
        call,
    ))
}

impl App {
    /// Open a panel following the task the `task` call `call` of panel
    /// `panel` handed over, while it runs out of sight; the panel takes the
    /// focus. `false` when no such task runs.
    pub(in crate::app) fn watch_delegate(&mut self, panel: u64, call: &str) -> bool {
        let Some(watch) = watch(panel, call) else {
            return false;
        };
        // On the default agent until the task is over: the panel only reads
        // the log, and the delegate's own agent — an external one would
        // start a process of its own — comes with its session once it opens.
        let settings = self.project_ai_settings();
        let setup = agent_setup(
            &settings,
            self.project_root.clone(),
            &self.project_root,
            DEFAULT_AGENT,
            None,
        );
        let mut follower = AgentPanel::new(setup);
        follower.start_watch(watch);
        self.add_panel(Box::new(follower));
        self.auto_save_layout();
        self.state.needs_redraw = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_can_be_followed_while_it_runs() {
        let registration = register(
            Some(7),
            "call-1",
            "default",
            "find it",
            Some(PathBuf::from("/tmp/x.jsonl")),
        );
        assert!(watch(7, "call-1").is_some());
        assert!(watch(7, "call-2").is_none());
        let control = registration.control().clone();
        assert!(!control.is_done());
        drop(registration);
        // Over: gone from the registry, and the panels following it know.
        assert!(watch(7, "call-1").is_none());
        assert!(control.is_done());
    }

    #[test]
    fn a_task_outside_a_panel_cannot_be_followed() {
        let registration = register(None, "call-3", "default", "x", None);
        assert!(watch(0, "call-3").is_none());
        drop(registration);
    }
}
