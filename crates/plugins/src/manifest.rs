//! Installed plugins: the directories under the global plugins directory and
//! the `plugin.toml` each holds.
//!
//! Only this directory is ever read for plugin code. The program a manifest
//! names is resolved against the plugin's own directory, or, for a bare
//! name, against the absolute entries of `PATH` — never against a working
//! directory, so nothing planted in an opened project can stand in for it.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The manifest file in each plugin directory.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// What `plugin.toml` says.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// One line on what the plugin does.
    #[serde(default)]
    pub description: String,
    /// The program: absolute, relative to the plugin directory (with a
    /// `/`), or a bare name looked up in the absolute entries of `PATH`.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Added to the environment termide passes on.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// A plugin found in the plugins directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    /// The directory's name, which is the plugin's name.
    pub name: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Installed {
    /// The program to start, resolved as [`resolve_program`] does with the
    /// current `PATH`.
    ///
    /// # Errors
    ///
    /// When the program cannot be found.
    pub fn program(&self) -> Result<PathBuf, String> {
        resolve_program(
            &self.dir,
            &self.manifest.command,
            std::env::var_os("PATH").as_deref(),
        )
    }
}

/// Whether `name` may name a plugin: ASCII letters, digits, `-` and `_`.
#[must_use]
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Every plugin in `plugins_dir`, sorted by name, with one line for each
/// directory that could not be read as one. A missing directory has none.
#[must_use]
pub fn discover(plugins_dir: &Path) -> (Vec<Installed>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(plugins_dir) else {
        return (found, problems);
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let Some(name) = dir.file_name().and_then(OsStr::to_str).map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if !valid_name(&name) {
            problems.push(format!(
                "plugin {}: the name may hold only letters, digits, '-' and '_'",
                dir.display()
            ));
            continue;
        }
        let manifest_path = dir.join(MANIFEST_FILE);
        let parsed = std::fs::read_to_string(&manifest_path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                toml::from_str::<Manifest>(&text).map_err(|e| e.message().to_string())
            });
        match parsed {
            Ok(manifest) => found.push(Installed {
                name,
                dir,
                manifest,
            }),
            Err(e) => problems.push(format!("plugin {name}: {}: {e}", manifest_path.display())),
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    (found, problems)
}

/// The program `command` names for the plugin in `dir`.
///
/// - An absolute path is taken as it is.
/// - A path with a separator is relative to `dir`.
/// - A bare name is looked up in the entries of `path_var` that are
///   absolute; a relative entry (`.`, an empty one) would resolve against
///   whatever directory termide was started in, which may be a project's.
///
/// # Errors
///
/// When no such file exists.
pub fn resolve_program(
    dir: &Path,
    command: &str,
    path_var: Option<&OsStr>,
) -> Result<PathBuf, String> {
    if command.is_empty() {
        return Err("the manifest names no command".to_string());
    }
    let as_path = Path::new(command);
    let candidate = if as_path.is_absolute() {
        Some(as_path.to_path_buf())
    } else if as_path.components().count() > 1 {
        Some(dir.join(as_path))
    } else {
        path_var.and_then(|paths| {
            std::env::split_paths(paths)
                .filter(|entry| entry.is_absolute())
                .flat_map(|entry| executable_names(command).map(move |name| entry.join(name)))
                .find(|path| path.is_file())
        })
    };
    match candidate {
        Some(path) if path.is_file() => Ok(path),
        Some(path) => Err(format!("{} does not exist", path.display())),
        None => Err(format!("{command} is not found in PATH")),
    }
}

/// The file names a bare command may have: itself, and on Windows with the
/// usual executable extensions.
fn executable_names(command: &str) -> impl Iterator<Item = String> + '_ {
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    extensions.iter().map(move |ext| format!("{command}{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_plugin(root: &Path, name: &str, manifest: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST_FILE), manifest).unwrap();
        dir
    }

    #[test]
    fn discovers_plugins_and_reports_broken_ones() {
        let root = tempfile::tempdir().unwrap();
        write_plugin(
            root.path(),
            "b-two",
            "command = \"./run\"\nargs = [\"-v\"]\n",
        );
        write_plugin(
            root.path(),
            "a_one",
            "command = \"/bin/sh\"\ndescription = \"x\"\n",
        );
        write_plugin(root.path(), "broken", "commnd = \"typo\"\n");
        write_plugin(root.path(), "bad name", "command = \"/bin/sh\"\n");
        std::fs::create_dir(root.path().join(".hidden")).unwrap();
        std::fs::write(root.path().join("stray-file"), "").unwrap();

        let (found, problems) = discover(root.path());
        let names: Vec<_> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a_one", "b-two"]);
        assert_eq!(found[1].manifest.args, ["-v"]);
        assert_eq!(problems.len(), 2, "{problems:?}");
    }

    #[test]
    fn a_missing_directory_holds_no_plugins() {
        let (found, problems) = discover(Path::new("/nonexistent/termide/plugins"));
        assert!(found.is_empty() && problems.is_empty());
    }

    #[test]
    fn a_relative_command_resolves_inside_the_plugin_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = write_plugin(root.path(), "p", "");
        std::fs::create_dir(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/run"), "").unwrap();
        assert_eq!(
            resolve_program(&dir, "bin/run", None).unwrap(),
            dir.join("bin/run")
        );
        assert!(resolve_program(&dir, "./missing", None).is_err());
    }

    #[test]
    fn a_bare_name_skips_relative_path_entries() {
        let root = tempfile::tempdir().unwrap();
        let plugin_dir = write_plugin(root.path(), "p", "");
        // A "project" with a planted program, and a trusted bin directory.
        let project = root.path().join("project");
        let trusted = root.path().join("trusted");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&trusted).unwrap();
        std::fs::write(project.join("tool"), "").unwrap();

        let relative = std::env::join_paths([Path::new("."), Path::new("project")]).unwrap();
        assert!(resolve_program(&plugin_dir, "tool", Some(&relative)).is_err());

        std::fs::write(trusted.join("tool"), "").unwrap();
        let mixed = std::env::join_paths([Path::new("project"), trusted.as_path()]).unwrap();
        assert_eq!(
            resolve_program(&plugin_dir, "tool", Some(&mixed)).unwrap(),
            trusted.join("tool")
        );
    }

    #[test]
    fn names_are_restricted() {
        assert!(valid_name("my-plugin_2"));
        assert!(!valid_name(""));
        assert!(!valid_name("../x"));
        assert!(!valid_name("a b"));
    }
}
