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
//!
//! A task stopped by a limit that passes on its own is not over: it is
//! [`Parked`](self::Parked) here, waiting for the reset — or for the panel to
//! move to another connection, which wakes it at once. Its row stays, and its
//! answer is owed to the panel that delegated it when it is really done.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Instant;

use termide_agent_core::Failure;

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

/// Stop every task panel `panel` handed over: it is closing. A pause that
/// holds them stays in their logs, with the panel's own.
pub fn stop_all(panel: u64) {
    for ((owner, _), live) in lock().iter() {
        if *owner == panel {
            live.control.close();
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

/// Hold every task panel `panel` handed over in the background at its next
/// step, as the panel paused its own run. A task the run waits on is the
/// call in progress, which the panel's pause waits for: held, it would never
/// return, and the pause never come.
pub fn pause_all(panel: u64) -> PauseState {
    let mut state = PauseState::default();
    for ((owner, _), live) in lock().iter() {
        if *owner != panel || !live.background {
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
        if *owner != panel || !live.background {
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

/// A delegated task stopped by a limit that passes on its own: it answers
/// nothing, it waits. The wait ends at the reset the limit named (or after a
/// backoff that grows with each try), when the panel moves to another
/// connection, or when the user says so from its row or its card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parked {
    /// The `task` call that handed the task over.
    pub call: String,
    /// The delegate that ran it.
    pub agent: String,
    /// The task as it was handed over, for the row and the answer.
    pub task: String,
    /// Its own session log, where it stopped and where it goes on from.
    pub session: Option<PathBuf>,
    /// What stopped it, with the reset it named.
    pub failure: Failure,
    /// When it goes again on its own, Unix seconds.
    pub wake_at: u64,
    /// The connection it stopped on: a switch off it is the way past a spent
    /// subscription, so it wakes the task at once.
    pub connection: String,
    /// How many tries it has been through; the backoff grows with them.
    pub attempts: u32,
    /// When it was parked, Unix seconds, for the row's clock.
    pub since: u64,
    /// The user asked it to go now.
    pub asked: bool,
    /// A try of it is under way: it is not to be started twice.
    pub launching: bool,
}

/// The tasks parked at a limit, by the panel and the `task` call that handed
/// each over.
fn parked_registry() -> &'static Mutex<HashMap<(u64, String), Parked>> {
    static PARKED: OnceLock<Mutex<HashMap<(u64, String), Parked>>> = OnceLock::new();
    PARKED.get_or_init(Mutex::default)
}

fn parked_lock() -> std::sync::MutexGuard<'static, HashMap<(u64, String), Parked>> {
    parked_registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Park `task` until `wake_at`: it answers its parent nothing now, and goes
/// again on its own at that moment — or before it, once the panel is on
/// another connection or the user asks. `attempts` counts the failures it has
/// been through, so the wait it sleeps by grows with them.
pub fn park(panel: u64, parked: Parked) {
    parked_lock().insert((panel, parked.call.clone()), parked);
}

/// Take a parked task back: it is over, answered or given up for good.
pub fn unpark(panel: u64, call: &str) -> Option<Parked> {
    parked_lock().remove(&(panel, call.to_string()))
}

/// Start a parked task: `Some` when no try of it is under way, and the entry
/// marked as launching so the tick does not start it twice.
pub fn launch(panel: u64, call: &str) -> Option<Parked> {
    let mut parked = parked_lock();
    let entry = parked.get_mut(&(panel, call.to_string()))?;
    if entry.launching {
        return None;
    }
    entry.launching = true;
    entry.asked = false;
    Some(entry.clone())
}

/// The try of a parked task could not be started: it waits again, until
/// `wake_at`, for the tick to try once more.
pub fn sleep_until(panel: u64, call: &str, wake_at: u64) {
    if let Some(parked) = parked_lock().get_mut(&(panel, call.to_string())) {
        parked.launching = false;
        parked.wake_at = wake_at;
    }
}

/// The tasks panel `panel` has parked, oldest first.
pub fn parked_of(panel: u64) -> Vec<Parked> {
    let mut parked: Vec<Parked> = parked_lock()
        .iter()
        .filter(|((owner, _), _)| *owner == panel)
        .map(|(_, parked)| parked.clone())
        .collect();
    parked.sort_by_key(|parked| (parked.since, parked.call.clone()));
    parked
}

/// Whether any task is parked at all: the app looks for wakes only then.
#[must_use]
pub fn any_parked() -> bool {
    !parked_lock().is_empty()
}

/// Ask the parked task to go now, from its row's `↻` or the card's Try again.
pub fn ask_now(panel: u64, call: &str) {
    if let Some(parked) = parked_lock().get_mut(&(panel, call.to_string())) {
        parked.asked = true;
    }
}

/// The user gave the parked task up: it is no longer run again.
pub fn abandon(panel: u64, call: &str) -> Option<Parked> {
    unpark(panel, call)
}

/// Park nothing any longer: the panel that delegated these tasks is closing.
/// Their own logs stay where they stopped, so a reopen still offers them.
pub fn clear_parked_of(panel: u64) -> Vec<Parked> {
    let mut parked = parked_lock();
    let calls: Vec<(u64, String)> = parked
        .keys()
        .filter(|(owner, _)| *owner == panel)
        .cloned()
        .collect();
    calls
        .into_iter()
        .filter_map(|key| parked.remove(&key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use termide_agent_core::FailureKind;

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
        // The call the run waits on: the pause comes once it returns.
        let sync = register(task(Some(7004), "d", false));

        assert_eq!(
            pause_all(7004),
            PauseState {
                held: 1,
                running: 1
            },
            "the external task keeps going"
        );
        // Stopped from its row, a held task is over, not paused; with its
        // panel closing, it keeps the pause the panel's log keeps too.
        let stopped = register(task(Some(7006), "e", true));
        let closed = register(task(Some(7007), "f", true));
        pause_all(7006);
        pause_all(7007);
        stop(7006, "e");
        stop_all(7007);
        assert!(stopped.control().stop_token().is_cancelled());
        assert!(!stopped.control().is_paused());
        assert!(closed.control().stop_token().is_cancelled());
        assert!(closed.control().is_paused());
        assert!(held.control().is_paused());
        assert!(!running.control().is_paused());
        assert!(is_paused(7004, "a"));
        assert!(!is_paused(7004, "b"));
        assert!(!other.control().is_paused(), "another panel is its own");
        assert!(!sync.control().is_paused(), "the call in progress runs on");

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

    /// A task parked at a usage limit resetting at `wake_at`.
    fn parked(call: &str, since: u64, wake_at: u64) -> Parked {
        Parked {
            call: call.into(),
            agent: "methodical".into(),
            task: "cut the release".into(),
            session: Some(PathBuf::from("/tmp/delegate.jsonl")),
            failure: Failure::new(FailureKind::QuotaExhausted).with_reset(Some(wake_at)),
            wake_at,
            connection: "claude-code".into(),
            attempts: 0,
            since,
            asked: false,
            launching: false,
        }
    }

    #[test]
    fn a_parked_task_is_listed_asked_and_taken_back() {
        park(7010, parked("a", 100, 400));
        park(7010, parked("b", 90, 500));
        park(7011, parked("c", 95, 500));
        let listed = parked_of(7010);
        assert_eq!(
            listed.iter().map(|p| p.call.as_str()).collect::<Vec<_>>(),
            ["b", "a"],
            "oldest first, this panel's only"
        );
        assert!(any_parked());

        ask_now(7010, "a");
        assert!(parked_of(7010)[1].asked);
        assert!(!parked_of(7010)[0].asked);

        let taken = unpark(7010, "a").expect("parked");
        assert_eq!(taken.connection, "claude-code");
        assert_eq!(parked_of(7010).len(), 1);
        assert_eq!(abandon(7010, "b").expect("parked").call, "b");
        assert!(parked_of(7010).is_empty());
        assert!(any_parked(), "another panel still parks");
    }

    /// A parked task is started once, not twice; a try that could not be
    /// started waits again, and a task parked again counts that try.
    #[test]
    fn a_parked_task_is_started_once_and_retried_by_the_tick() {
        park(7012, parked("a", 100, 400));
        let started = launch(7012, "a").expect("not launched yet");
        assert_eq!(started.attempts, 0);
        assert!(launch(7012, "a").is_none(), "one try at a time");
        // The process would not start: the tick tries again later.
        sleep_until(7012, "a", 700);
        assert_eq!(parked_of(7012)[0].wake_at, 700);
        assert!(launch(7012, "a").is_some());
        // It ran and stopped on the limit again: parked once more, one try
        // older, so the wait it sleeps by has grown.
        let mut again = parked("a", 420, 900);
        again.attempts = 1;
        park(7012, again);
        let listed = parked_of(7012);
        assert_eq!(listed.len(), 1, "one park per task");
        assert_eq!((listed[0].attempts, listed[0].wake_at), (1, 900));
        assert!(!listed[0].asked, "a new park asks nothing");
        // Answered at last: nothing waits any longer.
        assert!(unpark(7012, "a").is_some());
        assert!(parked_of(7012).is_empty());
    }

    #[test]
    fn a_closing_panel_leaves_nothing_parked() {
        park(7013, parked("a", 100, 400));
        park(7014, parked("b", 100, 400));
        let cleared = clear_parked_of(7013);
        assert_eq!(cleared.len(), 1);
        assert!(parked_of(7013).is_empty());
        assert_eq!(parked_of(7014).len(), 1);
    }
}
