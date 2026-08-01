//! Git command execution utilities.
//!
//! Internal helpers for executing git commands with proper error handling.

use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Build a `git` command hardened so it can never block on or corrupt the
/// controlling terminal: git's interactive credential prompt is disabled and
/// stdin is detached. Used for local, non-network git invocations (status, log,
/// diff, blame, …).
fn hardened_git(dir: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    cmd
}

/// Authentication mode for a network git command.
pub enum SshAuth<'a> {
    /// Non-interactive: agent only, ssh runs in `BatchMode`. A missing
    /// passphrase fails cleanly instead of prompting on the terminal.
    Batch,
    /// Supply a known SSH key passphrase via an `SSH_ASKPASS` helper. `helper`
    /// is the askpass program (the termide binary in askpass mode); it reads
    /// the secret from `secret_file`. Used to retry after termide collected the
    /// passphrase in a modal.
    Askpass {
        helper: &'a Path,
        secret_file: &'a Path,
    },
}

/// Build a hardened `git` command for a NETWORK operation (fetch/pull/push).
///
/// Network operations can trigger ssh / credential prompts. Without hardening,
/// ssh opens `/dev/tty` directly to ask for a key passphrase and writes the
/// prompt straight over the TUI. Here stdin is detached and git's own prompt is
/// disabled. With [`SshAuth::Batch`] ssh runs in `BatchMode` (no prompts; keys
/// in ssh-agent still work); with [`SshAuth::Askpass`] ssh is pointed at a
/// non-interactive askpass helper that supplies a passphrase termide already
/// holds. `stdout`/`stderr` are piped for the caller to capture; the command is
/// returned ready to `.spawn()`.
pub fn network_command(repo: &Path, args: &[&str], auth: SshAuth) -> Command {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match auth {
        SshAuth::Batch => {
            cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
        }
        SshAuth::Askpass {
            helper,
            secret_file,
        } => {
            // Force ssh to use our askpass helper instead of /dev/tty, and feed
            // it the passphrase via the secret file. DISPLAY is a fallback for
            // ssh older than the SSH_ASKPASS_REQUIRE support (8.4).
            cmd.env("SSH_ASKPASS", helper)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("GIT_ASKPASS", helper)
                .env("TERMIDE_ASKPASS_FILE", secret_file)
                .env("DISPLAY", ":0");
        }
    }
    cmd
}

/// Execute a git command in the specified directory.
/// Returns None if the command fails or git is not available.
pub(crate) fn git_command(dir: &Path, args: &[&str]) -> Option<Output> {
    hardened_git(dir, args)
        .output()
        .ok()
        .filter(|output| output.status.success())
}

/// Execute a git command and return stdout as String.
pub(crate) fn git_command_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    git_command(dir, args).and_then(|output| String::from_utf8(output.stdout).ok())
}

/// Read a file's content as of `HEAD`, using hardened git so a credential or
/// ssh prompt can never reach the TUI.
///
/// Returns:
/// - `Some(content)` for a file tracked in `HEAD`,
/// - `Some(String::new())` for an untracked, non-ignored file (its lines all
///   show as added against an empty original),
/// - `None` when the path is outside a repo, gitignored, or unreadable.
///
/// The path is canonicalized first so `strip_prefix` matches git's canonical
/// repository root even when the caller holds a symlinked path.
pub(crate) fn head_file_content(file_path: &Path) -> Option<String> {
    let file_path = std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let dir = file_path.parent().unwrap_or_else(|| Path::new("/"));

    let root = git_command_stdout(dir, &["rev-parse", "--show-toplevel"])?;
    let root = Path::new(root.trim());
    let relative = file_path.strip_prefix(root).ok()?;
    let head_ref = format!("HEAD:{}", relative.display());

    let show = hardened_git(root, &["show", &head_ref]).output().ok()?;
    if show.status.success() {
        return String::from_utf8(show.stdout).ok();
    }

    // Not in HEAD: gitignored → no diff; otherwise a new file → empty original.
    let ignored = hardened_git(root, &["check-ignore", "-q"])
        .arg(&file_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if ignored {
        None
    } else {
        Some(String::new())
    }
}

/// Run a simple git operation, returning Ok(()) on success or error message on failure.
pub(crate) fn run_git_simple(repo: &Path, args: &[&str], error_msg: &str) -> Result<(), String> {
    match git_command(repo, args) {
        Some(_) => Ok(()),
        None => Err(error_msg.to_string()),
    }
}

/// Run a git command capturing stderr for detailed error messages.
pub(crate) fn run_git_with_stderr(repo: &Path, args: &[&str], op_name: &str) -> Result<(), String> {
    let output = hardened_git(repo, args)
        .output()
        .map_err(|e| format!("Failed to run git {}: {}", op_name, e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{} failed: {}", op_name, stderr.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::{network_command, SshAuth};
    use std::ffi::OsStr;
    use std::path::Path;

    // A network git command must never be able to prompt on the controlling
    // terminal (issue: SSH passphrase prompt drawn over the TUI). Verify the
    // hardening env is in place.
    #[test]
    fn network_command_disables_interactive_prompts() {
        let cmd = network_command(Path::new("/tmp"), &["fetch", "origin"], SshAuth::Batch);

        let mut terminal_prompt = None;
        let mut ssh_command = None;
        for (k, v) in cmd.get_envs() {
            if k == OsStr::new("GIT_TERMINAL_PROMPT") {
                terminal_prompt = v.map(|s| s.to_string_lossy().into_owned());
            } else if k == OsStr::new("GIT_SSH_COMMAND") {
                ssh_command = v.map(|s| s.to_string_lossy().into_owned());
            }
        }

        assert_eq!(terminal_prompt.as_deref(), Some("0"));
        assert!(
            ssh_command
                .as_deref()
                .unwrap_or("")
                .contains("BatchMode=yes"),
            "GIT_SSH_COMMAND must run ssh in BatchMode, got {ssh_command:?}"
        );

        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["fetch", "origin"]);
    }

    // Askpass mode must point ssh at the helper (not /dev/tty) and not run in
    // BatchMode, so the supplied passphrase is actually used.
    #[test]
    fn network_command_askpass_uses_helper() {
        let cmd = network_command(
            Path::new("/tmp"),
            &["fetch"],
            SshAuth::Askpass {
                helper: Path::new("/usr/bin/termide"),
                secret_file: Path::new("/run/secret"),
            },
        );
        let mut askpass = None;
        let mut require = None;
        let mut secret = None;
        let mut ssh_cmd = None;
        for (k, v) in cmd.get_envs() {
            match k.to_string_lossy().as_ref() {
                "SSH_ASKPASS" => askpass = v.map(|s| s.to_string_lossy().into_owned()),
                "SSH_ASKPASS_REQUIRE" => require = v.map(|s| s.to_string_lossy().into_owned()),
                "TERMIDE_ASKPASS_FILE" => secret = v.map(|s| s.to_string_lossy().into_owned()),
                "GIT_SSH_COMMAND" => ssh_cmd = v.map(|s| s.to_string_lossy().into_owned()),
                _ => {}
            }
        }
        assert_eq!(askpass.as_deref(), Some("/usr/bin/termide"));
        assert_eq!(require.as_deref(), Some("force"));
        assert_eq!(secret.as_deref(), Some("/run/secret"));
        // Must NOT force BatchMode in askpass mode (that would suppress the helper).
        assert!(
            ssh_cmd.is_none(),
            "askpass mode must not set BatchMode ssh command"
        );
    }
}
