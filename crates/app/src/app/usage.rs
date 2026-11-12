//! The agents' usage window: its opening, the counting of the session logs
//! on a worker thread, and the lines of the view the window shows.
//!
//! The logs are counted into a [`UsageIndex`] kept in a cache file beside
//! them, so only the logs changed since are read again; the index stays in
//! memory between openings, shown at once while the logs are counted anew.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use termide_agent_core::usage::{UsageIndex, UsagePeriod, UsageReport, UsageScope};
use termide_modal::{ModalResult, UsageLine, UsageModal, UsageScopeTab, UsageView};

use crate::state::ActiveModal;

use super::App;

/// The cache of the counted logs, in the agents' directory.
const CACHE_FILE: &str = "usage-cache.json";

/// How often the counting reports its progress.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

/// What the worker counting the logs reports.
#[derive(Debug)]
pub(crate) enum UsageMessage {
    /// So many logs read of so many.
    Progress(usize, usize),
    /// The logs counted.
    Done(UsageIndex),
}

/// The usage window's state beside the window itself.
#[derive(Debug, Default)]
pub(crate) struct UsageState {
    /// The logs as last counted.
    index: Option<Arc<UsageIndex>>,
    /// The counting under way.
    receiver: Option<mpsc::Receiver<UsageMessage>>,
    /// The period last chosen, offered first the next time.
    period: usize,
    /// Whether a period was ever chosen.
    period_chosen: bool,
}

impl App {
    /// Open the usage window, on the session summarised in `session` when
    /// given (from an agent panel), else on the project.
    pub(in crate::app) fn open_usage(&mut self, session: Option<Vec<(String, String)>>) {
        let t = termide_i18n::t();
        let periods = vec![
            t.usage_period_today().to_string(),
            t.usage_period_week().to_string(),
            t.usage_period_month().to_string(),
            t.usage_period_all().to_string(),
        ];
        let period = if self.state.usage.period_chosen {
            self.state.usage.period
        } else {
            index_of(UsagePeriod::default())
        };
        let scope = if session.is_some() {
            UsageScopeTab::Session
        } else {
            UsageScopeTab::Project
        };
        let session = session.map(|rows| {
            rows.into_iter()
                .map(|(key, value)| UsageLine::new(key, value))
                .collect()
        });
        let modal = UsageModal::new(t.usage_title(), periods, period, session, scope);
        self.state
            .show_message_modal(ActiveModal::Usage(Box::new(modal)));
        self.start_usage_count();
        self.fill_usage();
        self.state.needs_redraw = true;
    }

    /// Count the logs anew on a worker thread, unless that is under way.
    fn start_usage_count(&mut self) {
        if self.state.usage.receiver.is_some() {
            return;
        }
        let Ok(config) = termide_config::get_config_dir() else {
            return;
        };
        let agents = config.join(termide_agent_core::GLOBAL_AGENT_DIR);
        let sessions = agents.join(termide_agent_core::SESSIONS_DIR);
        let cache = agents.join(CACHE_FILE);
        let (tx, rx) = mpsc::channel();
        self.state.usage.receiver = Some(rx);
        let spawned = std::thread::Builder::new()
            .name("usage-count".into())
            .spawn(move || {
                let mut index = UsageIndex::load(&cache);
                let mut reported: Option<Instant> = None;
                let changed = index.refresh(&sessions, |done, total| {
                    let due = reported.is_none_or(|at| at.elapsed() >= PROGRESS_EVERY);
                    if total > 0 && (due || done == total) {
                        reported = Some(Instant::now());
                        let _ = tx.send(UsageMessage::Progress(done, total));
                    }
                });
                if changed {
                    if let Err(error) = index.save(&cache) {
                        log::warn!("usage: cannot save {}: {error}", cache.display());
                    }
                }
                let _ = tx.send(UsageMessage::Done(index));
            });
        if let Err(error) = spawned {
            log::warn!("usage: cannot count the logs: {error}");
            self.state.usage.receiver = None;
        }
    }

    /// Take what the counting reported: its progress into the window while
    /// there is nothing to show yet, the counted logs into the view shown.
    pub(super) fn poll_usage(&mut self) {
        let Some(receiver) = &self.state.usage.receiver else {
            return;
        };
        let mut progress = None;
        let mut done = None;
        loop {
            match receiver.try_recv() {
                Ok(UsageMessage::Progress(read, total)) => progress = Some((read, total)),
                Ok(UsageMessage::Done(index)) => {
                    done = Some(index);
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state.usage.receiver = None;
                    break;
                }
            }
        }
        if let Some(index) = done {
            self.state.usage.receiver = None;
            self.state.usage.index = Some(Arc::new(index));
            // The view shown is drawn again from the logs as they are now.
            if let Some(ActiveModal::Usage(modal)) = &mut self.state.active_modal {
                let view = modal.view();
                if view.scope != UsageScopeTab::Session {
                    let lines = self.usage_lines(view);
                    if let Some(ActiveModal::Usage(modal)) = &mut self.state.active_modal {
                        modal.set_lines(view, lines);
                    }
                }
            }
            self.state.needs_redraw = true;
        } else if let Some((read, total)) = progress {
            if self.state.usage.index.is_none() {
                if let Some(ActiveModal::Usage(modal)) = &mut self.state.active_modal {
                    modal.set_counting(read, total);
                    self.state.needs_redraw = true;
                }
            }
        }
    }

    /// Hand the open usage window the lines of the view it waits for, once
    /// the logs are counted; remember the period chosen.
    pub(super) fn fill_usage(&mut self) {
        let Some(ActiveModal::Usage(modal)) = &self.state.active_modal else {
            return;
        };
        let view = modal.view();
        if view.scope != UsageScopeTab::Session {
            self.state.usage.period = view.period;
            self.state.usage.period_chosen = true;
        }
        let Some(wanted) = modal.wants() else {
            return;
        };
        if self.state.usage.index.is_none() {
            return;
        }
        let lines = self.usage_lines(wanted);
        if let Some(ActiveModal::Usage(modal)) = &mut self.state.active_modal {
            modal.set_lines(wanted, lines);
        }
        self.state.needs_redraw = true;
    }

    /// The window's lines for `view`, from the logs as last counted.
    fn usage_lines(&self, view: UsageView) -> Vec<UsageLine> {
        let Some(index) = self.state.usage.index.as_deref() else {
            return Vec::new();
        };
        let here = termide_project::project_key(&self.project_root);
        let all = view.scope == UsageScopeTab::All;
        let scope = if all {
            UsageScope::All
        } else {
            UsageScope::Project(here.clone())
        };
        let period = UsagePeriod::ALL
            .get(view.period)
            .copied()
            .unwrap_or_default();
        let now = termide_agent_core::message::now_millis();
        let report = UsageReport::build(
            index,
            &scope,
            period,
            now,
            &termide_agent_core::usage::Local,
        );
        // Canonical, as the project keys are: `/tmp` is `/private/tmp` on
        // macOS.
        let home = dirs::home_dir().map(|home| dunce::canonicalize(&home).unwrap_or(home));
        termide_panel_agent::usage_report_lines(&report, period, all, &here, home.as_deref())
            .into_iter()
            .map(|(key, value, link)| UsageLine::new(key, value).with_link(link))
            .collect()
    }

    /// The usage window closed with `result`: open the session picked.
    pub(super) fn finish_usage(
        &mut self,
        result: ModalResult<termide_modal::ModalOutput>,
    ) -> Result<()> {
        self.state.close_modal();
        self.state.needs_redraw = true;
        if let ModalResult::Confirmed(value) = result {
            if let Some(path) = value.get::<PathBuf>() {
                self.ai_open_session(path.clone())?;
            }
        }
        Ok(())
    }
}

/// Where `period` stands among the periods offered.
fn index_of(period: UsagePeriod) -> usize {
    UsagePeriod::ALL
        .iter()
        .position(|p| *p == period)
        .unwrap_or(0)
}
