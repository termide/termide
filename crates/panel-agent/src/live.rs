//! Delegated tasks running out of sight, for the panels to show and follow.
//!
//! A delegate registers here while it runs, under the panel and the `task`
//! call that handed it over. The panel that delegated it lists the ones it
//! handed over in the background in its state strip, since their calls have
//! returned and no running block stands for them; a click on a row, or `o`
//! on the block, shows a view following the task's session log (see
//! [`crate::Watch`]); closing the panel that delegated them stops them.
//! While a view follows a task, its cards go there. The registration ends
//! with the task, which marks it over for the views following it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use crate::watch::{DelegateControl, Watch};

/// A task running out of sight.
struct Live {
    /// The delegating agent.
    from: String,
    /// The delegate.
    agent: String,
    task: String,
    session: Option<PathBuf>,
    control: DelegateControl,
    /// Handed over in the background: no call waits for it.
    background: bool,
    started: Instant,
    /// What it did last, for its row.
    status: Arc<Mutex<String>>,
    /// A panel follows it: its cards go there rather than to the panel that
    /// delegated it.
    viewed: bool,
    /// Whether a pause can hold it at all: an external agent runs its own
    /// program, so pausing the panel leaves that going.
    pausable: bool,
}

/// The tasks running out of sight, by the panel and the `task` call that
/// handed each over.
fn registry() -> &'static Mutex<HashMap<(u64, String), Live>> {
    static LIVE: OnceLock<Mutex<HashMap<(u64, String), Live>>> = OnceLock::new();
    LIVE.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, HashMap<(u64, String), Live>> {
    registry().lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the task is, as the panel that handed it over registers it.
pub struct LiveTask<'a> {
    /// The panel that handed it over; a task from no panel is not listed.
    pub panel: Option<u64>,
    /// The id of the `task` call.
    pub call: &'a str,
    /// The delegating agent.
    pub from: &'a str,
    /// The delegate.
    pub agent: &'a str,
    pub task: &'a str,
    /// Its session log, which a panel follows.
    pub session: Option<PathBuf>,
    /// Handed over in the background.
    pub background: bool,
    /// Whether a pause can hold it: `false` for an external agent, which
    /// runs its own program and only stops.
    pub pausable: bool,
}

/// A task's place in the registry, for as long as it runs: dropped, the
/// task is over.
pub struct Registration {
    key: Option<(u64, String)>,
    control: DelegateControl,
    status: Arc<Mutex<String>>,
}

impl Registration {
    /// What the user does to the task, from a panel following it or from
    /// its row.
    #[must_use]
    pub fn control(&self) -> &DelegateControl {
        &self.control
    }

    /// Note what the task did last, for its row.
    pub fn set_status(&self, status: &str) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status.to_string();
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.control.finish();
        if let Some(key) = &self.key {
            lock().remove(key);
        }
    }
}

/// Register `task` for as long as the returned registration lives.
#[must_use]
pub fn register(task: LiveTask<'_>) -> Registration {
    let control = DelegateControl::new();
    let status = Arc::new(Mutex::new(String::new()));
    let key = task.panel.map(|panel| {
        let key = (panel, task.call.to_string());
        lock().insert(
            key.clone(),
            Live {
                from: task.from.to_string(),
                agent: task.agent.to_string(),
                task: task.task.to_string(),
                session: task.session,
                control: control.clone(),
                background: task.background,
                started: Instant::now(),
                status: Arc::clone(&status),
                viewed: false,
                pausable: task.pausable,
            },
        );
        key
    });
    Registration {
        key,
        control,
        status,
    }
}

/// What a panel needs to follow the task the `task` call `call` of panel
/// `panel` handed over, while it runs and has a log.
#[must_use]
pub fn watch(panel: u64, call: &str) -> Option<Watch> {
    let registry = lock();
    let live = registry.get(&(panel, call.to_string()))?;
    Some(Watch::new(
        live.from.clone(),
        &live.task,
        live.session.clone()?,
        live.control.clone(),
        panel,
        call,
    ))
}

/// Whether the task the `task` call `call` of panel `panel` handed over
/// still runs out of sight.
#[must_use]
pub fn is_running(panel: u64, call: &str) -> bool {
    lock().contains_key(&(panel, call.to_string()))
}

/// A task handed over in the background, as its row shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackgroundTask {
    pub call: String,
    pub agent: String,
    pub task: String,
    pub status: String,
    pub started: Instant,
    /// Held at the panel's pause.
    pub paused: bool,
    /// A pause cannot hold it: an external agent runs its own program.
    pub pausable: bool,
}

/// The tasks panel `panel` handed over in the background that still run,
/// oldest first.
pub(crate) fn background_of(panel: u64) -> Vec<BackgroundTask> {
    let mut tasks: Vec<BackgroundTask> = lock()
        .iter()
        .filter(|((owner, _), live)| *owner == panel && live.background)
        .map(|((_, call), live)| BackgroundTask {
            call: call.clone(),
            agent: live.agent.clone(),
            task: live.task.clone(),
            status: live
                .status
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone(),
            started: live.started,
            paused: live.control.is_paused(),
            pausable: live.pausable,
        })
        .collect();
    tasks.sort_by_key(|task| task.started);
    tasks
}

/// Note whether a panel follows the task the `task` call `call` of panel
/// `panel` handed over: while one does, the task's cards go there.
pub(crate) fn set_viewed(panel: u64, call: &str, viewed: bool) {
    if let Some(live) = lock().get_mut(&(panel, call.to_string())) {
        live.viewed = viewed;
    }
}

/// Whether a panel follows the task the `task` call `call` of panel
/// `panel` handed over.
pub(crate) fn is_viewed(panel: u64, call: &str) -> bool {
    lock()
        .get(&(panel, call.to_string()))
        .is_some_and(|live| live.viewed)
}

/// Ask before stopping the task the `task` call `call` of panel `panel`
/// handed over, naming its delegate; `action` stops it once confirmed. Its
/// row, the view following it and any other place ask alike.
pub(crate) fn ask_stop(panel: u64, call: &str, action: String) -> termide_core::PanelEvent {
    let agent = lock()
        .get(&(panel, call.to_string()))
        .map(|live| live.agent.clone())
        .unwrap_or_default();
    termide_core::PanelEvent::ShowConfirm {
        message: termide_i18n::t().agent_stop_task_confirm_fmt(&agent),
        on_confirm: termide_core::ConfirmAction::Custom(action),
    }
}

/// Stop the task the `task` call `call` of panel `panel` handed over.
pub(crate) fn stop(panel: u64, call: &str) {
    if let Some(live) = lock().get(&(panel, call.to_string())) {
        live.control.stop();
    }
}

/// Stop every task panel `panel` handed over: it is closing.
pub fn stop_all(panel: u64) {
    for ((owner, _), live) in lock().iter() {
        if *owner == panel {
            live.control.stop();
        }
    }
}

/// How the tasks of one panel stand at a pause.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseState {
    /// Held at the pause, ready to go on where each stood.
    pub held: usize,
    /// Still going, because a pause cannot hold them: an external agent runs
    /// its own program and only stops.
    pub running: usize,
}

/// Hold every task panel `panel` handed over at its next step, as the panel
/// paused its own run.
pub fn pause_all(panel: u64) -> PauseState {
    let mut state = PauseState::default();
    for ((owner, _), live) in lock().iter() {
        if *owner != panel {
            continue;
        }
        if live.pausable {
            live.control.pause();
            state.held += 1;
        } else {
            state.running += 1;
        }
    }
    state
}

/// Lift the pause on every task panel `panel` handed over: none waits any
/// longer, each goes on where it stood.
pub fn resume_all(panel: u64) {
    for ((owner, _), live) in lock().iter() {
        if *owner == panel {
            live.control.resume();
        }
    }
}

/// How the tasks panel `panel` handed over stand now.
pub fn state_of(panel: u64) -> PauseState {
    let mut state = PauseState::default();
    for ((owner, _), live) in lock().iter() {
        if *owner != panel {
            continue;
        }
        if live.control.is_paused() {
            state.held += 1;
        } else if !live.pausable {
            state.running += 1;
        }
    }
    state
}

/// Whether the task the `task` call `call` of panel `panel` handed over is
/// held at a pause.
pub fn is_paused(panel: u64, call: &str) -> bool {
    lock()
        .get(&(panel, call.to_string()))
        .is_some_and(|live| live.control.is_paused())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(panel: Option<u64>, call: &str, background: bool) -> LiveTask<'_> {
        LiveTask {
            panel,
            call,
            from: "default",
            agent: "search",
            task: "find it",
            session: Some(PathBuf::from("/tmp/x.jsonl")),
            background,
            pausable: true,
        }
    }

    #[test]
    fn a_task_can_be_followed_while_it_runs() {
        let registration = register(task(Some(7001), "call-1", false));
        assert!(watch(7001, "call-1").is_some());
        assert!(watch(7001, "call-2").is_none());
        let control = registration.control().clone();
        assert!(!control.is_done());
        drop(registration);
        // Over: gone from the registry, and the panels following it know.
        assert!(watch(7001, "call-1").is_none());
        assert!(control.is_done());
    }

    #[test]
    fn a_task_outside_a_panel_is_not_listed() {
        let registration = register(task(None, "call-3", true));
        assert!(!is_running(0, "call-3"));
        drop(registration);
    }

    #[test]
    fn a_panels_background_tasks_are_listed_and_stopped_with_it() {
        let sync = register(task(Some(7002), "a", false));
        let first = register(task(Some(7002), "b", true));
        let other = register(task(Some(7003), "c", true));
        first.set_status("· read src/main.rs");
        let listed = background_of(7002);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].call, "b");
        assert_eq!(listed[0].status, "· read src/main.rs");

        stop(7002, "b");
        assert!(first.control().ended_by_user().is_some());
        stop_all(7002);
        assert!(sync.control().ended_by_user().is_some());
        assert!(other.control().ended_by_user().is_none());
    }

    /// The panel's pause holds the tasks it handed over, and its resume lets
    /// them go; an external agent's task is left going and counted, so the
    /// panel can say so.
    #[test]
    fn a_panels_pause_holds_its_tasks_and_lets_them_go() {
        let held = register(task(Some(7004), "a", true));
        let mut external = task(Some(7004), "b", true);
        external.pausable = false;
        let running = register(external);
        let other = register(task(Some(7005), "c", true));

        assert_eq!(
            pause_all(7004),
            PauseState {
                held: 1,
                running: 1
            },
            "the external task keeps going"
        );
        assert!(held.control().is_paused());
        assert!(!running.control().is_paused());
        assert!(is_paused(7004, "a"));
        assert!(!is_paused(7004, "b"));
        assert!(!other.control().is_paused(), "another panel is its own");

        resume_all(7004);
        assert!(!held.control().is_paused());
        assert_eq!(
            state_of(7004),
            PauseState {
                held: 0,
                running: 1
            }
        );
    }
}
