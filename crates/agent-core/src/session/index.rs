//! The summary index of a session directory: each log's summary with the
//! size and modification time it was read at, kept in `summaries.json` next
//! to the logs, so a new run lists the directory without reading the logs
//! that have not changed since.
//!
//! The index is only a cache. A summary in it counts while its log keeps
//! that stamp; a missing, unreadable or older-format index is read as empty
//! and written again. Several termides may share a directory: each writes
//! the whole index atomically, so a reader sees one writer's index or
//! another's, never half of one, and a summary one of them lost is read
//! from its log again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{KeptSummaries, SessionSummary, SummaryStamp};

/// The index's file name in a session directory.
pub(super) const INDEX_FILE: &str = "summaries.json";

/// Raised whenever what a summary holds or how it is written changes, so
/// an index from before is read again rather than misread.
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Index {
    version: u32,
    /// By the log's file name, so a directory moved whole keeps its index.
    logs: HashMap<String, Indexed>,
}

#[derive(Serialize, Deserialize)]
struct Indexed {
    stamp: SummaryStamp,
    summary: SessionSummary,
}

/// The summaries the index of `dir` holds, by log path; none when there is
/// no index or it cannot be read.
pub(super) fn load(dir: &Path) -> Vec<(PathBuf, (SummaryStamp, SessionSummary))> {
    let Ok(text) = std::fs::read(dir.join(INDEX_FILE)) else {
        return Vec::new();
    };
    let index: Index = match serde_json::from_slice(&text) {
        Ok(index) => index,
        Err(error) => {
            log::warn!("ignoring {}: {error}", dir.join(INDEX_FILE).display());
            return Vec::new();
        }
    };
    if index.version != VERSION {
        return Vec::new();
    }
    index
        .logs
        .into_iter()
        .filter(|(name, _)| Path::new(name).file_name().and_then(|n| n.to_str()) == Some(name))
        .map(|(name, Indexed { stamp, mut summary })| {
            let path = dir.join(name);
            summary.path.clone_from(&path);
            (path, (stamp, summary))
        })
        .collect()
}

/// Write the index of `dir` from the summaries kept of its logs. A failure
/// costs only the next run's reading the logs again, so it is logged.
pub(super) fn save(dir: &Path, kept: &KeptSummaries) {
    let index = Index {
        version: VERSION,
        logs: kept
            .iter()
            .filter(|(path, _)| path.parent() == Some(dir))
            .filter_map(|(path, (stamp, summary))| {
                let name = path.file_name()?.to_str()?.to_string();
                Some((
                    name,
                    Indexed {
                        stamp: *stamp,
                        summary: summary.clone(),
                    },
                ))
            })
            .collect(),
    };
    let written = serde_json::to_vec(&index)
        .map_err(std::io::Error::other)
        .and_then(|bytes| termide_fsutil::write_atomic(&dir.join(INDEX_FILE), &bytes));
    if let Err(error) = written {
        log::warn!("cannot write {}: {error}", dir.join(INDEX_FILE).display());
    }
}
