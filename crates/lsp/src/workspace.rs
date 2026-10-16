//! Where a language server's workspace starts.
//!
//! The nearest root marker (`Cargo.toml`, `go.mod`) names the package a file
//! belongs to, not always the workspace: a crate can be a member of a cargo
//! workspace higher up, a Go module can be listed in a `go.work`. Starting
//! the server at the package would still work — cargo and go find the
//! workspace themselves — but every package would get a server of its own,
//! each loading the same workspace. Whether a package belongs to the
//! workspace above it is decided by rules (member globs, `exclude`,
//! `package.workspace`, `use` directives) only the toolchain implements
//! faithfully, so we ask it rather than read manifests ourselves. Any
//! failure — tool missing, an error, no answer in time — keeps the package
//! root, which is what we used before.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// How long a toolchain may take to name the workspace. `cargo
/// locate-project` answers in tens of milliseconds; the limit only guards
/// against a toolchain manager that decides to download something.
const TOOL_TIMEOUT: Duration = Duration::from_secs(3);

/// The workspace root for a package whose nearest marker is in
/// `package_root`, or `None` to keep `package_root`.
pub(crate) fn workspace_root(lang: &str, package_root: &Path) -> Option<PathBuf> {
    match lang {
        "rust" => cargo_workspace_root(package_root),
        "go" => go_workspace_root(package_root),
        _ => None,
    }
}

/// The directory of the workspace manifest cargo assigns the package to.
/// For a crate outside any workspace that is its own directory; for a crate
/// that sits under a workspace without being a member cargo errors out and
/// we keep the crate.
fn cargo_workspace_root(package_root: &Path) -> Option<PathBuf> {
    let mut command = Command::new("cargo");
    command
        .args(["locate-project", "--workspace", "--message-format", "plain"])
        // A `rust-toolchain.toml` naming a missing toolchain must not
        // start a download on file open.
        .env("RUSTUP_AUTO_INSTALL", "0");
    let stdout = run(command, package_root)?;
    let manifest = PathBuf::from(stdout.trim());
    manifest.parent().map(Path::to_path_buf)
}

/// The directory of the `go.work` in effect for the module, if that file
/// lists the module in a `use` directive. `go env GOWORK` names the file the
/// go command found walking up, whether or not the module is in it; `go
/// work edit -json` parses it with go's own parser.
fn go_workspace_root(package_root: &Path) -> Option<PathBuf> {
    let gowork = run(go_command(&["env", "GOWORK"]), package_root)?;
    let gowork = gowork.trim();
    if gowork.is_empty() || gowork == "off" {
        return None;
    }
    let gowork = Path::new(gowork);
    let json = run(
        go_command(&["work", "edit", "-json", &gowork.to_string_lossy()]),
        package_root,
    )?;
    let work_dir = gowork.parent()?;
    go_work_uses(&json, work_dir, package_root).then(|| work_dir.to_path_buf())
}

fn go_command(args: &[&str]) -> Command {
    let mut command = Command::new("go");
    command
        .args(args)
        // A `toolchain` line in go.mod or go.work must not start a
        // download on file open.
        .env("GOTOOLCHAIN", "local");
    command
}

#[derive(Deserialize)]
struct GoWork {
    #[serde(rename = "Use", default)]
    uses: Option<Vec<GoWorkUse>>,
}

#[derive(Deserialize)]
struct GoWorkUse {
    #[serde(rename = "DiskPath")]
    disk_path: String,
}

/// Whether the `go work edit -json` output lists `module_dir`. `use` paths
/// are relative to the `go.work` directory unless absolute.
fn go_work_uses(json: &str, work_dir: &Path, module_dir: &Path) -> bool {
    let Ok(work) = serde_json::from_str::<GoWork>(json) else {
        return false;
    };
    let Some(module_dir) = canonical(module_dir) else {
        return false;
    };
    work.uses
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| canonical(&work_dir.join(&entry.disk_path)))
        .any(|dir| dir == module_dir)
}

fn canonical(path: &Path) -> Option<PathBuf> {
    dunce::canonicalize(path).ok()
}

/// Run `command` in `dir` and return its stdout if it exits successfully
/// within [`TOOL_TIMEOUT`].
fn run(mut command: Command, dir: &Path) -> Option<String> {
    let mut child = command
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < TOOL_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(5));
            }
            _ => {
                log::warn!("LSP: {:?} gave no answer in {:?}", command, TOOL_TIMEOUT);
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut stdout = String::new();
    child.stdout.take()?.read_to_string(&mut stdout).ok()?;
    Some(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn crate_manifest(name: &str) -> String {
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
    }

    fn has(tool: &str) -> bool {
        Command::new(tool)
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    fn standalone_crate_is_its_own_root() {
        let dir = tempfile::tempdir().unwrap();
        let krate = dir.path().join("solo");
        write(&krate.join("Cargo.toml"), &crate_manifest("solo"));
        write(&krate.join("src/lib.rs"), "");

        let root = workspace_root("rust", &krate).expect("cargo answers");
        assert_eq!(canonical(&root), canonical(&krate));
    }

    #[test]
    fn workspace_member_resolves_to_workspace() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\nresolver = \"2\"\n",
        );
        let member = dir.path().join("crates/a");
        write(&member.join("Cargo.toml"), &crate_manifest("a"));
        write(&member.join("src/lib.rs"), "");

        let root = workspace_root("rust", &member).expect("cargo answers");
        assert_eq!(canonical(&root), canonical(dir.path()));
    }

    #[test]
    fn crate_under_a_workspace_it_does_not_belong_to_keeps_its_root() {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = []\nexclude = [\"tool\"]\n",
        );
        let tool = dir.path().join("tool");
        write(&tool.join("Cargo.toml"), &crate_manifest("tool"));
        write(&tool.join("src/main.rs"), "fn main() {}");
        assert_eq!(
            canonical(&workspace_root("rust", &tool).unwrap()),
            canonical(&tool)
        );

        // Neither a member nor excluded: cargo refuses, we keep the crate.
        let stray = dir.path().join("stray");
        write(&stray.join("Cargo.toml"), &crate_manifest("stray"));
        write(&stray.join("src/lib.rs"), "");
        assert_eq!(workspace_root("rust", &stray), None);
    }

    #[test]
    fn other_languages_keep_the_marker_root() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(workspace_root("python", dir.path()), None);
    }

    #[test]
    fn go_work_lists_relative_and_absolute_uses() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            dir.path().join("a"),
            dir.path().join("b"),
            dir.path().join("c"),
        );
        for module in [&a, &b, &c] {
            fs::create_dir_all(module).unwrap();
        }
        let json = format!(
            r#"{{"Go":"1.22","Use":[{{"DiskPath":"./a"}},{{"DiskPath":"{}"}}]}}"#,
            b.display()
        );
        assert!(go_work_uses(&json, dir.path(), &a));
        assert!(go_work_uses(&json, dir.path(), &b));
        assert!(!go_work_uses(&json, dir.path(), &c));
        assert!(!go_work_uses(r#"{"Go":"1.22"}"#, dir.path(), &a));
        assert!(!go_work_uses("not json", dir.path(), &a));
    }

    #[test]
    fn go_module_in_go_work_resolves_to_it() {
        if !has("go") {
            eprintln!("go not installed; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("a/go.mod"),
            "module example.com/a\n\ngo 1.21\n",
        );
        write(
            &dir.path().join("b/go.mod"),
            "module example.com/b\n\ngo 1.21\n",
        );
        assert_eq!(workspace_root("go", &dir.path().join("a")), None);

        write(&dir.path().join("go.work"), "go 1.21\n\nuse ./a\n");
        let root = workspace_root("go", &dir.path().join("a")).expect("listed in go.work");
        assert_eq!(canonical(&root), canonical(dir.path()));
        // Under the go.work but not listed in it.
        assert_eq!(workspace_root("go", &dir.path().join("b")), None);
    }
}
