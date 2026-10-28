//! Git file status types and functions.
//!
//! Provides types and functions for working with staged and unstaged files.

use std::path::{Path, PathBuf};

use crate::command::{git_command_stdout_bytes, nul_fields, path_from_git_bytes};

/// Staged file information
#[derive(Debug, Clone)]
pub struct StagedFile {
    /// Path relative to repo root
    pub path: PathBuf,
    /// Status code (M=modified, A=added, D=deleted, R=renamed)
    pub status: char,
}

/// Unstaged file information
#[derive(Debug, Clone)]
pub struct UnstagedFile {
    /// Path relative to repo root
    pub path: PathBuf,
    /// Status code (M=modified, D=deleted)
    pub status: char,
    /// Is this an untracked file
    pub untracked: bool,
}

/// Parse `git diff --name-status -z`: a status field, then one path, or
/// two (source, destination) for a rename or copy, which is listed under
/// its destination.
fn parse_name_status_z(stdout: &[u8]) -> Vec<(char, PathBuf)> {
    let mut fields = nul_fields(stdout);
    let mut result = Vec::new();
    while let Some(status) = fields.next() {
        let Some(&code) = status.first() else {
            continue;
        };
        let mut path = fields.next();
        if matches!(code, b'R' | b'C') {
            path = fields.next();
        }
        if let Some(path) = path {
            result.push((char::from(code), path_from_git_bytes(path)));
        }
    }
    result
}

/// Get staged files (files in index ready for commit)
pub fn get_staged_files(repo: &Path) -> Vec<StagedFile> {
    git_command_stdout_bytes(repo, &["diff", "--cached", "--name-status", "-z"])
        .map(|stdout| parse_name_status_z(&stdout))
        .unwrap_or_default()
        .into_iter()
        .map(|(status, path)| StagedFile { path, status })
        .collect()
}

/// Get unstaged files (modified files not in index) and untracked files
pub fn get_unstaged_files(repo: &Path) -> Vec<UnstagedFile> {
    let mut result: Vec<UnstagedFile> =
        git_command_stdout_bytes(repo, &["diff", "--name-status", "-z"])
            .map(|stdout| parse_name_status_z(&stdout))
            .unwrap_or_default()
            .into_iter()
            .map(|(status, path)| UnstagedFile {
                path,
                status,
                untracked: false,
            })
            .collect();

    if let Some(stdout) =
        git_command_stdout_bytes(repo, &["ls-files", "--others", "--exclude-standard", "-z"])
    {
        result.extend(nul_fields(&stdout).map(|path| UnstagedFile {
            path: path_from_git_bytes(path),
            status: '?',
            untracked: true,
        }));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_lists_renames_under_their_destination() {
        let out = b"M\0src/a.rs\0R087\0old name.rs\0new\tname.rs\0C100\0x\0y\0D\0gone\0";
        assert_eq!(
            parse_name_status_z(out),
            [
                ('M', PathBuf::from("src/a.rs")),
                ('R', PathBuf::from("new\tname.rs")),
                ('C', PathBuf::from("y")),
                ('D', PathBuf::from("gone")),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn name_status_keeps_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt;
        let out = b"M\0ok.rs\0A\0a\xff\0";
        let parsed = parse_name_status_z(out);
        assert_eq!(parsed[0].1, PathBuf::from("ok.rs"));
        assert_eq!(parsed[1].1.as_os_str().as_bytes(), b"a\xff");
    }
}
