//! The binaries of a Cargo project, found with `cargo metadata`, so that a
//! Rust file can be debugged without a command in `commands.toml`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// A binary target of a Cargo workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CargoBin {
    /// The target's name, which `--bin` takes.
    pub name: String,
    /// The package it belongs to, which `-p` takes.
    pub package: String,
    /// What `cargo build` writes: `<target dir>/debug/<name>`.
    pub program: PathBuf,
    /// Where cargo runs.
    pub workspace_root: PathBuf,
}

impl CargoBin {
    /// The command that builds this binary alone, run in `cwd`: outside the
    /// workspace root it names the workspace's manifest.
    pub fn build_command(&self, cwd: &Path) -> String {
        let manifest = if same_file(cwd, &self.workspace_root) {
            String::new()
        } else {
            let path = self.workspace_root.join("Cargo.toml");
            let path = path.strip_prefix(cwd).unwrap_or(&path);
            format!(
                " --manifest-path {}",
                crate::shell_quote(&path.to_string_lossy())
            )
        };
        format!(
            "cargo build{manifest} -p {} --bin {}",
            self.package, self.name
        )
    }
}

/// The `Cargo.toml` nearest above `file`, if it is in a Cargo project.
pub fn manifest_for(file: &Path) -> Option<PathBuf> {
    file.ancestors()
        .skip(1)
        .map(|dir| dir.join("Cargo.toml"))
        .find(|manifest| manifest.is_file())
}

/// The binaries `file` belongs to: the one whose main file it is, or else
/// those of its package, or else every binary of the workspace.
pub fn bins_for(file: &Path) -> Result<Vec<CargoBin>> {
    let manifest = manifest_for(file).context("not in a Cargo project")?;
    let dir = manifest.parent().unwrap_or(Path::new("."));
    let out = crate::detach(&mut Command::new("cargo"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .context("cannot run cargo")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!(
            "cargo metadata failed: {}",
            stderr.lines().last().unwrap_or("")
        );
    }
    let metadata: Value =
        serde_json::from_slice(&out.stdout).context("cargo metadata printed no JSON")?;
    Ok(bins_in(&metadata, file))
}

/// [`bins_for`] on the output of `cargo metadata`.
fn bins_in(metadata: &Value, file: &Path) -> Vec<CargoBin> {
    let target_dir = PathBuf::from(metadata["target_directory"].as_str().unwrap_or("target"));
    let workspace_root = PathBuf::from(metadata["workspace_root"].as_str().unwrap_or("."));
    let exe = if cfg!(windows) { ".exe" } else { "" };

    // (package directory, bin, its main file)
    let mut all: Vec<(PathBuf, CargoBin, PathBuf)> = Vec::new();
    let mut package_dirs: Vec<PathBuf> = Vec::new();
    for package in metadata["packages"].as_array().into_iter().flatten() {
        let Some(package_name) = package["name"].as_str() else {
            continue;
        };
        let package_dir = package["manifest_path"]
            .as_str()
            .and_then(|m| Path::new(m).parent())
            .map(Path::to_path_buf)
            .unwrap_or_default();
        package_dirs.push(package_dir.clone());
        for target in package["targets"].as_array().into_iter().flatten() {
            let is_bin = target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|k| k == "bin"));
            let (true, Some(name)) = (is_bin, target["name"].as_str()) else {
                continue;
            };
            all.push((
                package_dir.clone(),
                CargoBin {
                    name: name.to_string(),
                    package: package_name.to_string(),
                    program: target_dir.join("debug").join(format!("{name}{exe}")),
                    workspace_root: workspace_root.clone(),
                },
                PathBuf::from(target["src_path"].as_str().unwrap_or_default()),
            ));
        }
    }

    if let Some((_, bin, _)) = all.iter().find(|(_, _, src)| same_file(src, file)) {
        return vec![bin.clone()];
    }
    // The package whose directory is the nearest above the file.
    let package_dir = package_dirs
        .iter()
        .filter(|dir| file.starts_with(dir))
        .max_by_key(|dir| dir.components().count())
        .cloned();
    let own: Vec<CargoBin> = all
        .iter()
        .filter(|(dir, _, _)| Some(dir) == package_dir.as_ref())
        .map(|(_, bin, _)| bin.clone())
        .collect();
    if !own.is_empty() {
        return own;
    }
    all.into_iter().map(|(_, bin, _)| bin).collect()
}

fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || dunce::canonicalize(a)
            .ok()
            .is_some_and(|a| dunce::canonicalize(b).ok() == Some(a))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Value {
        json!({
            "target_directory": "/w/target",
            "workspace_root": "/w",
            "packages": [
                {
                    "name": "app",
                    "manifest_path": "/w/Cargo.toml",
                    "targets": [
                        {"name": "app", "kind": ["bin"], "src_path": "/w/src/main.rs"},
                        {"name": "tool", "kind": ["bin"], "src_path": "/w/src/bin/tool.rs"},
                        {"name": "app", "kind": ["lib"], "src_path": "/w/src/lib.rs"}
                    ]
                },
                {
                    "name": "core",
                    "manifest_path": "/w/crates/core/Cargo.toml",
                    "targets": [
                        {"name": "core", "kind": ["lib"], "src_path": "/w/crates/core/src/lib.rs"}
                    ]
                }
            ]
        })
    }

    fn names(bins: &[CargoBin]) -> Vec<&str> {
        bins.iter().map(|b| b.name.as_str()).collect()
    }

    #[test]
    fn a_bins_main_file_is_that_bin_alone() {
        let bins = bins_in(&metadata(), Path::new("/w/src/bin/tool.rs"));
        assert_eq!(names(&bins), ["tool"]);
        assert_eq!(bins[0].program, Path::new("/w/target/debug/tool"));
        assert_eq!(
            bins[0].build_command(Path::new("/w")),
            "cargo build -p app --bin tool"
        );
    }

    #[test]
    fn a_build_outside_the_workspace_names_its_manifest() {
        let bins = bins_in(&metadata(), Path::new("/w/src/bin/tool.rs"));
        assert_eq!(
            bins[0].build_command(Path::new("/")),
            "cargo build --manifest-path w/Cargo.toml -p app --bin tool"
        );
    }

    #[test]
    fn another_file_of_a_package_offers_its_bins() {
        let bins = bins_in(&metadata(), Path::new("/w/src/ui.rs"));
        assert_eq!(names(&bins), ["app", "tool"]);
    }

    #[test]
    fn a_library_without_bins_offers_the_workspaces() {
        let bins = bins_in(&metadata(), Path::new("/w/crates/core/src/lib.rs"));
        assert_eq!(names(&bins), ["app", "tool"]);
    }
}
