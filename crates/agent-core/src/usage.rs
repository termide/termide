//! What the agents spent across sessions: every session log under the
//! sessions directory, a project's or all of them, added up over a period.
//!
//! Reading every log takes seconds once there are hundreds of megabytes of
//! them, so each log's [`Timeline`] is kept in a cache file and read again
//! only when the log, or a delegate's log it took in, has changed.
//!
//! A delegate's session counts inside the session that delegated it, as
//! [`Spending::with_subagents`] counts it there; its own log counts on its
//! own only when no other log took it in (its parent's log is gone).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{Datelike, Duration, NaiveDate, TimeZone};

/// The local time zone, for [`UsageReport::build`].
pub use chrono::Local;
use serde::{Deserialize, Serialize};

use crate::message::Usage;
use crate::session::{Session, SessionSummary};
use crate::spend::{Spending, Timeline, MILLIS_PER_HOUR};

/// Bumped when what a [`UsageRecord`] holds changes, so an older cache is
/// counted again rather than misread.
const CACHE_VERSION: u32 = 1;

/// How many sessions a report names, the costliest first.
pub const TOP_SESSIONS: usize = 10;

/// A file's size and last change, to tell it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Stamp {
    pub size: u64,
    /// Nanoseconds since the epoch.
    pub modified: u64,
}

impl Stamp {
    /// The stamp of the file at `path`; `None` when it is gone.
    #[must_use]
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
        Some(Self {
            size: meta.len(),
            modified,
        })
    }
}

/// What one session log spent, as the cache keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    /// The log as it was read.
    pub stamp: Stamp,
    /// The project's key: the log's directory under the sessions
    /// directory, as `termide_project::project_key` makes it.
    pub project: PathBuf,
    /// What a session picker shows for it.
    pub label: String,
    /// The agent it ran on last.
    pub agent: Option<String>,
    /// When it last changed (millis).
    pub modified: u64,
    /// The delegates' logs taken in, each as it was read; `None` for one
    /// gone.
    pub delegates: Vec<(PathBuf, Option<Stamp>)>,
    pub timeline: Timeline,
}

impl UsageRecord {
    /// Read the log at `path` in project `project`.
    fn read(path: &Path, project: PathBuf, stamp: Stamp) -> std::io::Result<Self> {
        let session = Session::open_read_only(path)?;
        let summary = SessionSummary::from(&session);
        let (timeline, seen) = session.timeline_with_delegates();
        let mut delegates: Vec<(PathBuf, Option<Stamp>)> = seen
            .into_iter()
            .map(|log| {
                let stamp = Stamp::of(&log);
                (log, stamp)
            })
            .collect();
        delegates.sort();
        Ok(Self {
            stamp,
            project,
            label: summary.display_label(),
            agent: summary.agent,
            modified: summary.modified,
            delegates,
            timeline,
        })
    }

    /// Whether this record no longer tells what the log holds: the log or
    /// one of its delegates' changed.
    fn is_stale(&self, stamp: Stamp) -> bool {
        self.stamp != stamp
            || self
                .delegates
                .iter()
                .any(|(log, seen)| Stamp::of(log) != *seen)
    }
}

/// Every session log's [`UsageRecord`], by the log's path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageIndex {
    version: u32,
    records: BTreeMap<PathBuf, UsageRecord>,
}

impl UsageIndex {
    /// The cache at `path`; an empty index when there is none yet, or one
    /// unreadable or of another version.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Self>(&bytes).ok())
            .filter(|index| index.version == CACHE_VERSION)
            .unwrap_or_default()
    }

    /// Write the cache to `path`, whole or not at all.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let mut index = self.clone();
        index.version = CACHE_VERSION;
        let bytes = serde_json::to_vec(&index)?;
        termide_fsutil::write_atomic(path, &bytes)
    }

    /// Bring the index up to the logs under `sessions`: read the logs new
    /// or changed since, drop those gone. `progress` hears how many of the
    /// logs to read are read, and of how many, before each and at the end.
    /// Returns whether anything changed.
    pub fn refresh(&mut self, sessions: &Path, mut progress: impl FnMut(usize, usize)) -> bool {
        let logs = session_logs(sessions);
        let found: HashSet<&Path> = logs.iter().map(|(path, _)| path.as_path()).collect();
        let before = self.records.len();
        self.records
            .retain(|path, _| found.contains(path.as_path()));
        let mut changed = self.records.len() != before;
        let stale: Vec<(&PathBuf, &PathBuf, Stamp)> = logs
            .iter()
            .filter_map(|(path, project)| {
                let stamp = Stamp::of(path)?;
                self.records
                    .get(path)
                    .is_none_or(|record| record.is_stale(stamp))
                    .then_some((path, project, stamp))
            })
            .collect();
        let total = stale.len();
        for (done, (path, project, stamp)) in stale.into_iter().enumerate() {
            progress(done, total);
            changed = true;
            match UsageRecord::read(path, project.clone(), stamp) {
                Ok(record) => {
                    self.records.insert(path.clone(), record);
                }
                Err(error) => {
                    log::warn!("usage: skipping {}: {error}", path.display());
                    self.records.remove(path);
                }
            }
        }
        progress(total, total);
        changed
    }

    /// The records that count on their own: every log but those another
    /// log took in as its delegate's.
    fn counted(&self) -> impl Iterator<Item = (&PathBuf, &UsageRecord)> {
        let taken: HashSet<&Path> = self
            .records
            .values()
            .flat_map(|record| record.delegates.iter().map(|(log, _)| log.as_path()))
            .collect();
        self.records
            .iter()
            .filter(move |(path, _)| !taken.contains(path.as_path()))
    }
}

/// Every session log under `sessions`, with its project's key.
fn session_logs(sessions: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut logs = Vec::new();
    let mut dirs = vec![sessions.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                dirs.push(path);
            } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                let project = dir.strip_prefix(sessions).unwrap_or(&dir).to_path_buf();
                logs.push((path, project));
            }
        }
    }
    logs.sort();
    logs
}

/// Whose spending a report adds up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageScope {
    /// One project's, by its key.
    Project(PathBuf),
    /// Every project's.
    All,
}

/// The span a report covers, ending now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsagePeriod {
    /// Since local midnight.
    Today,
    /// Today and the six days before.
    #[default]
    Week,
    /// Today and the 29 days before.
    Month,
    /// Every log.
    All,
}

impl UsagePeriod {
    /// The periods in the order they are offered.
    pub const ALL: [Self; 4] = [Self::Today, Self::Week, Self::Month, Self::All];

    /// How many days back from today it reaches, today counted; `None`
    /// for all time.
    #[must_use]
    pub fn days(self) -> Option<i64> {
        match self {
            Self::Today => Some(1),
            Self::Week => Some(7),
            Self::Month => Some(30),
            Self::All => None,
        }
    }
}

/// A session in a report.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionUsage {
    pub path: PathBuf,
    /// Its project's key.
    pub project: PathBuf,
    pub label: String,
    pub agent: Option<String>,
    /// When it last changed (millis).
    pub modified: u64,
    /// What it spent in the period.
    pub usage: Usage,
}

/// What a scope spent over a period.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UsageReport {
    /// By who spent it and on which model.
    pub spending: Spending,
    /// How many sessions spent anything.
    pub sessions: usize,
    /// By local day (`YYYY-MM-DD`) for a period of days, by month
    /// (`YYYY-MM`) for all time; every day or month of the span, those that
    /// spent nothing too. Empty for today alone.
    pub buckets: Vec<(String, Usage)>,
    /// By agent; a log that names none under the empty name.
    pub agents: BTreeMap<String, Usage>,
    /// By project key.
    pub projects: BTreeMap<PathBuf, Usage>,
    /// The sessions that spent the most, at most [`TOP_SESSIONS`].
    pub top: Vec<SessionUsage>,
}

impl UsageReport {
    /// What `scope` spent over `period` up to `now` (millis), its days cut
    /// at midnight in `tz`.
    #[must_use]
    pub fn build<Tz: TimeZone>(
        index: &UsageIndex,
        scope: &UsageScope,
        period: UsagePeriod,
        now: u64,
        tz: &Tz,
    ) -> Self {
        let today = local_date(now, tz);
        let from = period.days().map_or(0, |days| {
            local_midnight(today - Duration::days(days - 1), tz)
        });
        let mut report = Self::default();
        let mut buckets: BTreeMap<String, Usage> = BTreeMap::new();
        let mut first: Option<NaiveDate> = None;
        let mut sessions = Vec::new();
        for (path, record) in index.counted() {
            if let UsageScope::Project(key) = scope {
                if record.project != *key {
                    continue;
                }
            }
            let mut usage = Usage::default();
            for (hour, spending) in record.timeline.hours.range(from / MILLIS_PER_HOUR..) {
                let spent = spending.total();
                if spent.total() == 0 {
                    continue;
                }
                usage.add(&spent);
                report.spending.merge(spending);
                if period != UsagePeriod::Today {
                    let date = local_date(hour * MILLIS_PER_HOUR, tz);
                    first = Some(first.map_or(date, |f| f.min(date)));
                    buckets
                        .entry(bucket_label(date, period))
                        .or_default()
                        .add(&spent);
                }
            }
            if usage.total() == 0 {
                continue;
            }
            report
                .agents
                .entry(record.agent.clone().unwrap_or_default())
                .or_default()
                .add(&usage);
            report
                .projects
                .entry(record.project.clone())
                .or_default()
                .add(&usage);
            sessions.push(SessionUsage {
                path: path.clone(),
                project: record.project.clone(),
                label: record.label.clone(),
                agent: record.agent.clone(),
                modified: record.modified,
                usage,
            });
        }
        report.sessions = sessions.len();
        sessions.sort_by(|a, b| {
            b.usage
                .total()
                .cmp(&a.usage.total())
                .then(b.modified.cmp(&a.modified))
        });
        sessions.truncate(TOP_SESSIONS);
        report.top = sessions;
        if period != UsagePeriod::Today {
            let start = match period.days() {
                Some(days) => Some(today - Duration::days(days - 1)),
                None => first,
            };
            if let Some(start) = start {
                report.buckets = span(start, today, period)
                    .into_iter()
                    .map(|label| {
                        let usage = buckets.remove(&label).unwrap_or_default();
                        (label, usage)
                    })
                    .collect();
            }
        }
        report
    }
}

/// The local date of `millis` in `tz`.
fn local_date<Tz: TimeZone>(millis: u64, tz: &Tz) -> NaiveDate {
    let millis = i64::try_from(millis).unwrap_or(i64::MAX);
    tz.timestamp_millis_opt(millis)
        .earliest()
        .map_or(NaiveDate::MIN, |at| at.date_naive())
}

/// Local midnight starting `date` in `tz`, in millis since the epoch.
fn local_midnight<Tz: TimeZone>(date: NaiveDate, tz: &Tz) -> u64 {
    date.and_hms_opt(0, 0, 0)
        .and_then(|midnight| tz.from_local_datetime(&midnight).earliest())
        .map_or(0, |at| u64::try_from(at.timestamp_millis()).unwrap_or(0))
}

/// The bucket `date` falls in for `period`: its day, or its month for all
/// time.
fn bucket_label(date: NaiveDate, period: UsagePeriod) -> String {
    match period {
        UsagePeriod::All => date.format("%Y-%m").to_string(),
        _ => date.format("%Y-%m-%d").to_string(),
    }
}

/// Every bucket from `start` to `end`, both in.
fn span(start: NaiveDate, end: NaiveDate, period: UsagePeriod) -> Vec<String> {
    let mut labels = Vec::new();
    let mut date = start;
    while date <= end {
        labels.push(bucket_label(date, period));
        date = match period {
            UsagePeriod::All => {
                let (year, month) = if date.month() == 12 {
                    (date.year() + 1, 1)
                } else {
                    (date.year(), date.month() + 1)
                };
                NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(NaiveDate::MAX)
            }
            _ => date + Duration::days(1),
        };
    }
    labels
}

#[cfg(test)]
mod tests;
