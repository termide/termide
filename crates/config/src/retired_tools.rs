//! Rewriting the shell tool's old name, `bash`, out of the files termide
//! keeps: `[ai.permissions.bash]` in the global config and in the rules
//! granted for a project. A project's own config is the repository's and is
//! left as it is.
//!
//! Transitional: it ships for one release after the rename, then goes
//! together with the old name's other readers.

use std::path::Path;

use termide_agent_core::{LEGACY_SHELL_TOOL, SHELL_TOOL};
use toml_edit::DocumentMut;

/// Rename the `[ai.permissions.bash]` table of the file at `path` to
/// `[ai.permissions.shell]`, in place, keeping its comments and layout; the
/// rules of a `shell` table already there win over the old ones for the same
/// pattern. A file without the old table, or one that does not parse, is
/// left alone — the loader reports what it cannot read.
pub(crate) fn rename_in_file(path: &Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Some(renamed) = rename(&text) else {
        return;
    };
    let tmp = path.with_extension("toml.tmp");
    let written = std::fs::write(&tmp, renamed).and_then(|()| std::fs::rename(&tmp, path));
    match written {
        Ok(()) => log::info!(
            "Renamed [ai.permissions.{LEGACY_SHELL_TOOL}] to [ai.permissions.{SHELL_TOOL}] in {}",
            path.display()
        ),
        Err(error) => {
            let _ = std::fs::remove_file(&tmp);
            log::warn!("Could not rewrite {}: {error}", path.display());
        }
    }
}

/// `text` with the old table renamed; `None` when there is nothing to do.
fn rename(text: &str) -> Option<String> {
    let mut doc: DocumentMut = text.parse().ok()?;
    let permissions = doc
        .get_mut("ai")?
        .as_table_like_mut()?
        .get_mut("permissions")?
        .as_table_like_mut()?;
    let old = permissions.remove(LEGACY_SHELL_TOOL)?;
    match permissions.get_mut(SHELL_TOOL) {
        Some(current) => {
            if let (Some(current), Some(old)) = (current.as_table_like_mut(), old.as_table_like()) {
                for (pattern, decision) in old.iter() {
                    if current.get(pattern).is_none() {
                        current.insert(pattern, decision.clone());
                    }
                }
            }
        }
        None => {
            permissions.insert(SHELL_TOOL, old);
        }
    }
    Some(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_table_takes_the_new_name_in_place() {
        let text = "[ai]\nmodel = \"m\"  # keep me\n\n\
                    [ai.permissions.bash]\n\"rm *\" = \"deny\" # never\n\n\
                    [ai.permissions.read]\n\"**/.env\" = \"deny\"\n";
        let renamed = rename(text).unwrap();
        assert_eq!(
            renamed,
            "[ai]\nmodel = \"m\"  # keep me\n\n\
             [ai.permissions.shell]\n\"rm *\" = \"deny\" # never\n\n\
             [ai.permissions.read]\n\"**/.env\" = \"deny\"\n"
        );
        assert_eq!(rename(&renamed), None);
    }

    #[test]
    fn rules_already_under_the_new_name_win_and_the_rest_join_them() {
        let text = "[ai.permissions.shell]\n\"cargo *\" = \"ask\"\n\n\
                    [ai.permissions.bash]\n\"cargo *\" = \"allow\"\n\"rm *\" = \"deny\"\n";
        let renamed = rename(text).unwrap();
        let value: toml::Value = toml::from_str(&renamed).unwrap();
        let shell = &value["ai"]["permissions"]["shell"];
        assert_eq!(shell["cargo *"].as_str(), Some("ask"));
        assert_eq!(shell["rm *"].as_str(), Some("deny"));
        assert!(value["ai"]["permissions"].get("bash").is_none());
    }

    #[test]
    fn a_file_is_rewritten_once_and_an_unrelated_one_never() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ai.permissions.bash]\n\"ls *\" = \"allow\"\n").unwrap();
        rename_in_file(&path);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "[ai.permissions.shell]\n\"ls *\" = \"allow\"\n"
        );
        let other = dir.path().join("other.toml");
        std::fs::write(&other, "[general]\nx = 1 # as it is\n").unwrap();
        rename_in_file(&other);
        assert_eq!(
            std::fs::read_to_string(&other).unwrap(),
            "[general]\nx = 1 # as it is\n"
        );
        rename_in_file(&dir.path().join("missing.toml"));
    }
}
