//! Staging, unstaging and discarding part of a file's changes.
//!
//! A part is one hunk of the file's diff, or a range of lines in it. The
//! patch for it is cut from a diff fetched afresh at the moment of the action,
//! never rebuilt from what a panel parsed and shows: that one may be stale,
//! and parsing drops what `git apply` needs to the byte (carriage returns,
//! `\ No newline at end of file`). The caller names the hunk by its index and
//! the header it saw, and a hunk that no longer reads so is refused as stale.

use std::path::Path;

use crate::command::{git_command, git_command_stdout_bytes, run_git_with_input};
use crate::commits::PLAIN_PATCH;

/// What to do with the chosen part of a file's changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchAction {
    /// Add the part of the working tree's changes to the index.
    Stage,
    /// Take the part of the index's changes out of it.
    Unstage,
    /// Undo the part of the working tree's changes in the file itself.
    Discard,
}

impl PatchAction {
    /// Whether the action works on the staged diff (`--cached`) rather than
    /// the working tree's.
    fn reads_staged(self) -> bool {
        self == Self::Unstage
    }

    /// Whether the patch goes in backwards (`git apply -R`).
    fn reverse(self) -> bool {
        self != Self::Stage
    }
}

/// The part of a file's changes an action takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPart {
    /// Index of the hunk in the file's diff.
    pub hunk: usize,
    /// The hunk's `@@` header as the caller saw it.
    pub header: String,
    /// First and last line of the hunk to take, counting its `+`, `-` and
    /// context lines from 0; `None` takes the whole hunk.
    pub lines: Option<(usize, usize)>,
}

/// Why an action on part of a file did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// The file's diff is no longer the one the caller saw.
    Stale,
    /// The chosen lines hold no change.
    NothingChosen,
    /// Git refused; its words.
    Git(String),
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => f.write_str("the diff changed meanwhile"),
            Self::NothingChosen => f.write_str("no change in the chosen lines"),
            Self::Git(words) => f.write_str(words),
        }
    }
}

/// Stage, unstage or discard `part` of the changes to `path` in `repo`.
pub fn apply_part(
    repo: &Path,
    path: &Path,
    action: PatchAction,
    part: &PatchPart,
) -> Result<(), PatchError> {
    let file = path.to_string_lossy();
    let mut args = vec!["diff"];
    if action.reads_staged() {
        args.push("--cached");
    }
    args.extend(PLAIN_PATCH);
    args.extend(["--", &file]);
    let diff = git_command_stdout_bytes(repo, &args)
        .ok_or_else(|| PatchError::Git(format!("git diff failed for {file}")))?;
    let patch = RawPatch::parse(&diff);
    let hunk = patch.hunks.get(part.hunk).ok_or(PatchError::Stale)?;
    if trim_cr(&hunk.header) != part.header.as_bytes() {
        return Err(PatchError::Stale);
    }
    let text = patch
        .cut(part.hunk, part.lines, action.reverse())
        .ok_or(PatchError::NothingChosen)?;

    let mut apply = vec!["apply", "--recount", "--whitespace=nowarn"];
    if action != PatchAction::Discard {
        apply.push("--cached");
    }
    if action.reverse() {
        apply.push("-R");
    }
    apply.push("-");
    run_git_with_input(repo, &apply, &text)
        .map(drop)
        .map_err(PatchError::Git)
}

/// Keep the file's current content in the object store, byte for byte, and
/// return its id: what [`restore_blob`] brings back after a discard.
pub fn save_blob(repo: &Path, path: &Path) -> Result<String, String> {
    let file = path.to_string_lossy();
    let output = git_command(repo, &["hash-object", "-w", "--no-filters", "--", &file])
        .ok_or_else(|| format!("could not keep {file} before discarding"))?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Write the blob `id` that [`save_blob`] kept back to `path`.
pub fn restore_blob(repo: &Path, path: &Path, id: &str) -> Result<(), String> {
    let content = git_command_stdout_bytes(repo, &["cat-file", "blob", id])
        .ok_or_else(|| format!("the kept copy {id} is gone"))?;
    termide_fsutil::write_atomic(&repo.join(path), &content).map_err(|e| e.to_string())
}

/// One file's diff as git wrote it, split into lines that keep their bytes.
struct RawPatch {
    /// `diff --git`, `index`, `---`, `+++` and the like, before the hunks.
    header: Vec<Vec<u8>>,
    hunks: Vec<RawHunk>,
}

struct RawHunk {
    /// The `@@` line.
    header: Vec<u8>,
    lines: Vec<RawLine>,
}

struct RawLine {
    /// The line with its `+`, `-` or space, without the `\n`.
    bytes: Vec<u8>,
    /// A `\ No newline at end of file` follows it.
    no_newline: bool,
}

const NO_NEWLINE: &[u8] = b"\\ No newline at end of file";

impl RawPatch {
    fn parse(diff: &[u8]) -> Self {
        let mut patch = RawPatch {
            header: Vec::new(),
            hunks: Vec::new(),
        };
        let body = diff.strip_suffix(b"\n").unwrap_or(diff);
        for line in body.split(|&b| b == b'\n') {
            if line.starts_with(b"@@") {
                patch.hunks.push(RawHunk {
                    header: line.to_vec(),
                    lines: Vec::new(),
                });
            } else if let Some(hunk) = patch.hunks.last_mut() {
                if line.starts_with(b"\\") {
                    if let Some(last) = hunk.lines.last_mut() {
                        last.no_newline = true;
                    }
                } else if matches!(line.first(), Some(b' ' | b'+' | b'-')) {
                    hunk.lines.push(RawLine {
                        bytes: line.to_vec(),
                        no_newline: false,
                    });
                }
            } else {
                patch.header.push(line.to_vec());
            }
        }
        patch
    }

    /// The patch of hunk `index` alone, or of its lines `lines` (inclusive),
    /// to apply forwards or, with `reverse`, backwards. A change left out
    /// stays as it is on the side the patch goes onto: applied forwards
    /// that is the old side, so a left-out removal becomes context and a
    /// left-out addition goes; backwards it is the new side, the other way
    /// round. `None` when no change is chosen.
    fn cut(&self, index: usize, lines: Option<(usize, usize)>, reverse: bool) -> Option<Vec<u8>> {
        let hunk = self.hunks.get(index)?;
        let (old_start, new_start, tail) = split_hunk_header(&hunk.header)?;
        let mut body: Vec<u8> = Vec::new();
        let (mut old, mut new, mut changed) = (0, 0, false);
        for (i, line) in hunk.lines.iter().enumerate() {
            let chosen = lines.is_none_or(|(first, last)| (first..=last).contains(&i));
            let kept_as = match (line.bytes[0], chosen) {
                (b' ', _) => Some(b' '),
                (sign, true) => {
                    changed = true;
                    Some(sign)
                }
                (b'-', false) if !reverse => Some(b' '),
                (b'+', false) if reverse => Some(b' '),
                _ => None,
            };
            let Some(sign) = kept_as else { continue };
            match sign {
                b' ' => (old, new) = (old + 1, new + 1),
                b'-' => old += 1,
                _ => new += 1,
            }
            body.push(sign);
            body.extend_from_slice(&line.bytes[1..]);
            body.push(b'\n');
            if line.no_newline {
                body.extend_from_slice(NO_NEWLINE);
                body.push(b'\n');
            }
        }
        if !changed {
            return None;
        }
        let mut out = Vec::new();
        for line in &self.header {
            out.extend_from_slice(line);
            out.push(b'\n');
        }
        out.extend_from_slice(format!("@@ -{old_start},{old} +{new_start},{new} @@").as_bytes());
        out.extend_from_slice(tail);
        out.push(b'\n');
        out.extend_from_slice(&body);
        Some(out)
    }
}

/// `@@ -a[,b] +c[,d] @@ tail` → `(a, c, " tail")`.
fn split_hunk_header(header: &[u8]) -> Option<(usize, usize, &[u8])> {
    let rest = header.strip_prefix(b"@@ -")?;
    let end = rest.windows(3).position(|w| w == b" @@")?;
    let ranges = std::str::from_utf8(&rest[..end]).ok()?;
    let (old, new) = ranges.split_once(" +")?;
    let start = |range: &str| range.split(',').next()?.parse().ok();
    Some((start(old)?, start(new)?, &rest[end + 3..]))
}

fn trim_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

#[cfg(test)]
mod tests;
