//! A panel following a delegated task that runs out of sight.
//!
//! The tasks register in `termide_panel_agent::live` while they run; a
//! click on a task's row, or `o` on its block, asks the app for a panel on
//! it (`PanelEvent::FocusDelegate`), opened here when no panel runs or
//! follows it already.

use termide_panel_agent::AgentPanel;

use super::{agent_setup, DEFAULT_AGENT};
use crate::app::App;

impl App {
    /// Open a panel following the task the `task` call `call` of panel
    /// `panel` handed over, while it runs out of sight; the panel takes the
    /// focus. `false` when no such task runs.
    pub(in crate::app) fn watch_delegate(&mut self, panel: u64, call: &str) -> bool {
        let Some(watch) = termide_panel_agent::live::watch(panel, call) else {
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
