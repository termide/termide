use std::path::Path;

use chrono::FixedOffset;

use super::*;
use crate::agent::test_support::text_reply;
use crate::message::{Message, ToolCall, ToolResultMessage, SUBAGENT_SESSION_KEY};
use crate::spend::Spender;

fn spent(input: u64) -> Usage {
    Usage {
        input,
        ..Default::default()
    }
}

/// A session in project `project` of `sessions` whose one reply spent
/// `input` tokens.
fn session_with(sessions: &Path, project: &str, input: u64) -> Session {
    let dir = sessions.join(project);
    std::fs::create_dir_all(&dir).unwrap();
    let mut session = Session::create(&dir, Path::new("/work")).unwrap();
    let mut reply = text_reply("ok");
    reply.usage = spent(input);
    session.append_message(&Message::Assistant(reply)).unwrap();
    session
}

/// A `task` result that ran in delegate log `log` and says it spent `input`.
fn task_result(log: &Path, input: u64) -> Message {
    let call = ToolCall {
        id: "task-1".into(),
        name: "task".into(),
        arguments: serde_json::json!({}),
        extra_content: None,
    };
    Message::ToolResult(
        ToolResultMessage::text(&call, "done")
            .with_spent(spent(input))
            .with_details(serde_json::json!({ SUBAGENT_SESSION_KEY: log.to_string_lossy() })),
    )
}

fn now() -> u64 {
    crate::message::now_millis()
}

/// Every log under the sessions directory counts once: a delegate's inside
/// the session that took it in, an orphan on its own; a project's report
/// holds its own logs only.
#[test]
fn every_log_counts_once_in_its_project() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path();
    let delegate = session_with(sessions, "Users/me/a", 60);
    let mut parent = session_with(sessions, "Users/me/a", 100);
    parent
        .append_message(&task_result(delegate.path(), 60))
        .unwrap();
    let _orphan = session_with(sessions, "Users/me/a", 7);
    let _other = session_with(sessions, "Users/me/b", 1_000);

    let mut index = UsageIndex::default();
    let mut heard = Vec::new();
    assert!(index.refresh(sessions, |done, total| heard.push((done, total))));
    assert_eq!(heard.last(), Some(&(4, 4)));

    let tz = FixedOffset::east_opt(0).unwrap();
    let a = UsageScope::Project(PathBuf::from("Users/me/a"));
    let report = UsageReport::build(&index, &a, UsagePeriod::Week, now(), &tz);
    assert_eq!(report.spending.total().input, 167);
    assert_eq!(report.sessions, 2);
    assert_eq!(report.spending.by[&Spender::Subagent].input, 60);
    assert_eq!(report.top[0].path, parent.path());
    assert_eq!(report.top[0].usage.input, 160);
    assert_eq!(report.buckets.len(), 7);
    assert_eq!(report.buckets[6].1.input, 167);

    let all = UsageReport::build(&index, &UsageScope::All, UsagePeriod::Today, now(), &tz);
    assert_eq!(all.spending.total().input, 1_167);
    assert_eq!(all.projects.len(), 2);
    assert!(all.buckets.is_empty());
}

/// A refresh reads again only the logs that changed, a delegate's change
/// counting as its parent's, and drops the logs gone; a saved index loads
/// as it was.
#[test]
fn a_refresh_reads_only_what_changed() {
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path();
    let mut delegate = session_with(sessions, "p", 10);
    let mut parent = session_with(sessions, "p", 100);
    parent
        .append_message(&task_result(delegate.path(), 10))
        .unwrap();
    let gone = session_with(sessions, "p", 5);

    let mut index = UsageIndex::default();
    index.refresh(sessions, |_, _| {});
    let mut heard = Vec::new();
    assert!(!index.refresh(sessions, |done, total| heard.push((done, total))));
    assert_eq!(heard, [(0, 0)]);

    // The delegate spends more: it and its parent are read again.
    let mut reply = text_reply("more");
    reply.usage = spent(20);
    delegate.append_message(&Message::Assistant(reply)).unwrap();
    std::fs::remove_file(gone.path()).unwrap();
    let mut heard = Vec::new();
    assert!(index.refresh(sessions, |done, total| heard.push((done, total))));
    assert_eq!(heard.last(), Some(&(2, 2)));
    assert_eq!(index.records.len(), 2);
    let tz = FixedOffset::east_opt(0).unwrap();
    let report = UsageReport::build(&index, &UsageScope::All, UsagePeriod::All, now(), &tz);
    assert_eq!(report.spending.total().input, 130);
    assert_eq!(report.buckets.len(), 1);

    let cache = root.path().join("usage.json");
    index.save(&cache).unwrap();
    let loaded = UsageIndex::load(&cache);
    assert_eq!(loaded.records, index.records);
    // A cache of another version, or none, is counted from scratch.
    assert!(UsageIndex::load(&root.path().join("none.json"))
        .records
        .is_empty());
    std::fs::write(&cache, br#"{"version":0,"records":{}}"#).unwrap();
    assert_eq!(UsageIndex::load(&cache).version, 0);
}

/// A record spent at the given hours (millis), each `input` tokens.
fn record(project: &str, spent_at: &[(u64, u64)]) -> UsageRecord {
    let mut timeline = Timeline::default();
    for (at, input) in spent_at {
        timeline.add(*at, Spender::Main, "m", &spent(*input));
    }
    UsageRecord {
        stamp: Stamp {
            size: 0,
            modified: 0,
        },
        project: PathBuf::from(project),
        label: project.into(),
        agent: Some("default".into()),
        modified: 0,
        delegates: Vec::new(),
        timeline,
    }
}

/// Periods start at local midnight, and the days and months of the span
/// are all listed, those that spent nothing too.
#[test]
fn periods_are_cut_at_local_midnight() {
    let hour = MILLIS_PER_HOUR;
    // 2026-11-11 12:00 UTC, in UTC+3 15:00 the same day.
    let now = 1_794_398_400_000;
    let tz = FixedOffset::east_opt(3 * 3600).unwrap();
    let mut index = UsageIndex::default();
    index.records.insert(
        PathBuf::from("/s/1.jsonl"),
        record(
            "p",
            &[
                // 2026-11-10 22:00 UTC: 01:00 local on the 11th, today.
                (now - 14 * hour, 1),
                // 2026-11-10 20:00 UTC: 23:00 local on the 10th.
                (now - 16 * hour, 2),
                // Forty days before.
                (now - 40 * 24 * hour, 4),
            ],
        ),
    );
    let all = UsageScope::All;
    let today = UsageReport::build(&index, &all, UsagePeriod::Today, now, &tz);
    assert_eq!(today.spending.total().input, 1);
    let week = UsageReport::build(&index, &all, UsagePeriod::Week, now, &tz);
    assert_eq!(week.spending.total().input, 3);
    assert_eq!(
        week.buckets.first().map(|(day, _)| day.as_str()),
        Some("2026-11-05")
    );
    assert_eq!(
        &week.buckets[5..],
        [
            ("2026-11-10".to_string(), spent(2)),
            ("2026-11-11".to_string(), spent(1)),
        ]
    );
    let ever = UsageReport::build(&index, &all, UsagePeriod::All, now, &tz);
    assert_eq!(ever.spending.total().input, 7);
    let months: Vec<&str> = ever.buckets.iter().map(|(m, _)| m.as_str()).collect();
    assert_eq!(months, ["2026-10", "2026-11"]);
}
