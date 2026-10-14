//! The context windows external agents reported, remembered across runs.
//!
//! An agent tells its window (`usage_update`'s `size`) only once a turn is
//! over, yet a conversation handed over to a new session of it is sized
//! before the first one. So the last window each agent reported for a model
//! is kept in a small file, and read when the agent next joins a
//! conversation; for a model never seen, Claude Code's window is told from
//! the model's name the way its adapter does.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use termide_agent_core::AcpFlavor;

/// Where the windows are kept, set once by the application.
static FILE: OnceLock<PathBuf> = OnceLock::new();

/// Keep the windows agents report in `path` from now on. Without it they
/// are not remembered past the agent's own run.
pub fn remember_windows_in(path: PathBuf) {
    let _ = FILE.set(path);
}

/// The file set by [`remember_windows_in`], if any.
pub(crate) fn windows_file() -> Option<PathBuf> {
    FILE.get().cloned()
}

/// Claude Code's window for a model it reports none for: its adapter's
/// default.
const CLAUDE_DEFAULT_WINDOW: u64 = 200_000;
/// …and for a model whose name says it has the long context.
const CLAUDE_LONG_WINDOW: u64 = 1_000_000;

/// The key a window is kept under: the adapter and the model.
pub(crate) fn key(flavor: AcpFlavor, agent: &str, model: &str) -> String {
    let adapter = match flavor {
        AcpFlavor::ClaudeCode => "claude_code",
        AcpFlavor::Codex => "codex",
        AcpFlavor::GeminiCli => "gemini_cli",
        AcpFlavor::Generic => agent,
    };
    format!("{adapter}/{model}")
}

fn load(path: &Path) -> BTreeMap<String, u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// The window kept in `path` under `key`.
pub(crate) fn remembered(path: &Path, key: &str) -> Option<u64> {
    load(path).get(key).copied().filter(|size| *size > 0)
}

/// Keep `size` in `path` under `key`; nothing is written when it is
/// already there. A failure is only logged: the window is a hint.
pub(crate) fn remember(path: &Path, key: &str, size: u64) {
    if size == 0 {
        return;
    }
    let mut windows = load(path);
    if windows.get(key) == Some(&size) {
        return;
    }
    windows.insert(key.to_string(), size);
    let written = serde_json::to_string_pretty(&windows)
        .map_err(std::io::Error::other)
        .and_then(|text| {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Written aside and moved in, so a reader never sees half.
            let aside = path.with_extension("json.tmp");
            std::fs::write(&aside, text)?;
            std::fs::rename(&aside, path)
        });
    if let Err(error) = written {
        log::warn!(
            "cannot keep the agent's window in {}: {error}",
            path.display()
        );
    }
}

/// The window an agent that reported none is taken to have: Claude Code's
/// from the model's name (`1m` in it is the long context), none for others.
pub(crate) fn inferred(flavor: AcpFlavor, model: Option<&str>) -> Option<u64> {
    (flavor == AcpFlavor::ClaudeCode).then(|| {
        let long = model.is_some_and(|model| {
            model
                .to_lowercase()
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|word| word == "1m")
        });
        if long {
            CLAUDE_LONG_WINDOW
        } else {
            CLAUDE_DEFAULT_WINDOW
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_kept_per_adapter_and_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ai").join("acp-windows.json");
        let opus = key(AcpFlavor::ClaudeCode, "experimental", "opus");
        assert_eq!(remembered(&path, &opus), None);
        remember(&path, &opus, 1_000_000);
        remember(&path, &key(AcpFlavor::Codex, "x", "gpt"), 272_000);
        assert_eq!(remembered(&path, &opus), Some(1_000_000));
        assert_eq!(
            remembered(&path, &key(AcpFlavor::ClaudeCode, "other", "opus")),
            Some(1_000_000),
            "the agent's name does not matter for a known adapter"
        );
        remember(&path, &opus, 200_000);
        assert_eq!(remembered(&path, &opus), Some(200_000));
        assert_eq!(
            remembered(&path, &key(AcpFlavor::Generic, "mine", "opus")),
            None
        );
    }

    #[test]
    fn claude_codes_window_is_told_from_the_models_name() {
        assert_eq!(
            inferred(AcpFlavor::ClaudeCode, Some("opus[1m]")),
            Some(1_000_000)
        );
        assert_eq!(
            inferred(AcpFlavor::ClaudeCode, Some("Opus 4.5 (1M context)")),
            Some(1_000_000)
        );
        assert_eq!(inferred(AcpFlavor::ClaudeCode, Some("opus")), Some(200_000));
        assert_eq!(inferred(AcpFlavor::ClaudeCode, None), Some(200_000));
        assert_eq!(inferred(AcpFlavor::Codex, Some("gpt-5")), None);
    }
}
