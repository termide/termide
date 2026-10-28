//! Shell detection and configuration utilities.
//!
//! This module provides functions for detecting available shells
//! and determining appropriate arguments for launching them.

/// Information about an available shell.
#[derive(Debug, Clone)]
pub struct ShellInfo {
    /// Friendly display name (e.g. "Git Bash", "PowerShell Core")
    pub name: String,
    /// Full path to shell binary (or launch command for WSL)
    pub path: String,
}

/// Discover all available shells on the system.
///
/// Returns a list of shells with friendly names, ordered by preference.
pub fn discover_shells() -> Vec<ShellInfo> {
    let mut shells = Vec::new();

    #[cfg(windows)]
    {
        let mut seen_paths = std::collections::HashSet::new();
        // Git Bash at standard install locations
        let git_bash_paths = [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ];
        for path in &git_bash_paths {
            if std::path::Path::new(path).exists() && !seen_contains(&seen_paths, path) {
                shells.push(ShellInfo {
                    name: "Git Bash".to_string(),
                    path: path.to_string(),
                });
                seen_paths.insert(path.to_lowercase());
                break; // Only add one Git Bash
            }
        }

        // Bash on PATH (MSYS2, custom installs — skip if already found as Git Bash)
        if let Some(path) = where_first("bash.exe") {
            if !seen_contains(&seen_paths, &path) {
                shells.push(ShellInfo {
                    name: "Bash".to_string(),
                    path: path.clone(),
                });
                seen_paths.insert(path.to_lowercase());
            }
        }

        // PowerShell Core (pwsh)
        if let Some(path) = where_first("pwsh.exe") {
            if !seen_contains(&seen_paths, &path) {
                shells.push(ShellInfo {
                    name: "PowerShell Core".to_string(),
                    path: path.clone(),
                });
                seen_paths.insert(path.to_lowercase());
            }
        }

        // Windows PowerShell
        if let Some(path) = where_first("powershell.exe") {
            if !seen_contains(&seen_paths, &path) {
                shells.push(ShellInfo {
                    name: "Windows PowerShell".to_string(),
                    path: path.clone(),
                });
                seen_paths.insert(path.to_lowercase());
            }
        }

        // Command Prompt
        let cmd = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        if !seen_contains(&seen_paths, &cmd) {
            shells.push(ShellInfo {
                name: "Command Prompt".to_string(),
                path: cmd.clone(),
            });
            seen_paths.insert(cmd.to_lowercase());
        }

        // WSL distributions
        if let Ok(output) = std::process::Command::new("wsl")
            .args(["--list", "--quiet"])
            .output()
        {
            if output.status.success() {
                // WSL output may be UTF-16LE on some Windows versions
                let text = match String::from_utf8(output.stdout) {
                    Ok(s) => s,
                    Err(e) => {
                        let bytes = e.into_bytes();
                        // Try UTF-16LE decoding; truncate trailing odd byte if present
                        let len = bytes.len() & !1;
                        let u16s: Vec<u16> = bytes[..len]
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        String::from_utf16(&u16s).unwrap_or_default()
                    }
                };

                for line in text.lines() {
                    let distro = line.trim().trim_start_matches('\u{feff}'); // strip BOM
                    if !distro.is_empty() {
                        shells.push(ShellInfo {
                            name: format!("WSL: {}", distro),
                            path: format!("wsl -d {}", distro),
                        });
                    }
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        // Deduplicate by (canonical path + binary name) so that symlinks
        // like /bin/bash → /usr/bin/bash collapse, but sh and bash remain
        // separate even when they share a binary (bash runs in POSIX mode as sh).
        let mut seen = std::collections::HashSet::new();

        let mut try_add = |shells: &mut Vec<ShellInfo>, path: &str| {
            let p = std::path::Path::new(path);
            if !p.exists() {
                return;
            }
            let canon = dunce::canonicalize(p)
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .to_string();
            let basename = p
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path)
                .to_string();
            if seen.insert((canon, basename)) {
                shells.push(ShellInfo {
                    name: shell_display_name(path),
                    path: path.to_string(),
                });
            }
        };

        // Parse /etc/shells for all valid login shells
        if let Ok(content) = std::fs::read_to_string("/etc/shells") {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.is_empty() {
                    continue;
                }
                try_add(&mut shells, line);
            }
        }

        // Also check NixOS paths and common paths not in /etc/shells
        let extra_paths = [
            "/run/current-system/sw/bin/fish",
            "/run/current-system/sw/bin/zsh",
            "/run/current-system/sw/bin/bash",
            "/usr/bin/fish",
            "/usr/bin/zsh",
            "/bin/bash",
            "/bin/sh",
        ];
        for path in extra_paths {
            try_add(&mut shells, path);
        }
    }

    shells
}

/// Get a friendly display name for a shell path.
#[cfg(not(windows))]
fn shell_display_name(path: &str) -> String {
    let name = std::path::Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path);

    // Capitalize first letter
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
        None => name.to_string(),
    }
}

/// Check if a path is already in the seen set (case-insensitive on Windows).
#[cfg(windows)]
fn seen_contains(seen: &std::collections::HashSet<String>, path: &str) -> bool {
    #[cfg(windows)]
    {
        seen.contains(&path.to_lowercase())
    }
    #[cfg(not(windows))]
    {
        seen.contains(path)
    }
}

/// Run `where` on Windows and return the first result.
#[cfg(windows)]
fn where_first(binary: &str) -> Option<String> {
    let output = std::process::Command::new("where")
        .arg(binary)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let path = text.trim();
    if path.is_empty() {
        return None;
    }
    Some(path.lines().next().unwrap_or(path).to_string())
}

/// Detect available shell on the system.
///
/// Checks in order:
/// - Windows: $SHELL (Git Bash), pwsh.exe, powershell.exe, cmd.exe ($COMSPEC)
/// - Unix: NixOS system shells, $SHELL, common shell paths
pub fn detect_shell() -> String {
    #[cfg(windows)]
    {
        // Check $SHELL first (set by Git Bash, MSYS2, Cygwin, etc.)
        if let Ok(shell) = std::env::var("SHELL") {
            if std::path::Path::new(&shell).exists() {
                return shell;
            }
        }
        // Check for Git Bash at standard install locations
        let git_bash_paths = [
            r"C:\Program Files\Git\bin\bash.exe",
            r"C:\Program Files (x86)\Git\bin\bash.exe",
        ];
        for path in &git_bash_paths {
            if std::path::Path::new(path).exists() {
                return path.to_string();
            }
        }
        // Also check if bash is on PATH (e.g. MSYS2, custom Git install)
        if let Ok(output) = std::process::Command::new("where").arg("bash.exe").output() {
            if output.status.success() {
                if let Ok(path) = String::from_utf8(output.stdout) {
                    let path = path.trim();
                    if !path.is_empty() {
                        return path.lines().next().unwrap_or(path).to_string();
                    }
                }
            }
        }
        // Check for PowerShell Core (pwsh)
        if let Ok(output) = std::process::Command::new("where").arg("pwsh.exe").output() {
            if output.status.success() {
                if let Ok(path) = String::from_utf8(output.stdout) {
                    let path = path.trim();
                    if !path.is_empty() {
                        return path.lines().next().unwrap_or(path).to_string();
                    }
                }
            }
        }
        // Check for Windows PowerShell
        if let Ok(output) = std::process::Command::new("where")
            .arg("powershell.exe")
            .output()
        {
            if output.status.success() {
                if let Ok(path) = String::from_utf8(output.stdout) {
                    let path = path.trim();
                    if !path.is_empty() {
                        return path.lines().next().unwrap_or(path).to_string();
                    }
                }
            }
        }
        // Fallback to cmd.exe
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }

    #[cfg(not(windows))]
    {
        // On NixOS first check bash-interactive in system profile
        // (regular bash in nix store might be without readline)
        let nixos_shells = [
            "/run/current-system/sw/bin/fish",
            "/run/current-system/sw/bin/zsh",
            "/run/current-system/sw/bin/bash",
        ];
        for shell in nixos_shells {
            if std::path::Path::new(shell).exists() {
                return shell.to_string();
            }
        }

        // Then check $SHELL
        if let Ok(shell) = std::env::var("SHELL") {
            if std::path::Path::new(&shell).exists() {
                return shell;
            }
        }

        // Check popular shells on regular systems
        let shells = ["/usr/bin/fish", "/usr/bin/zsh", "/bin/bash", "/bin/sh"];
        for shell in shells {
            if std::path::Path::new(shell).exists() {
                return shell.to_string();
            }
        }

        "/bin/sh".to_string()
    }
}

/// Get arguments for launching the shell.
///
/// Different shells require different arguments for proper interactive mode:
/// - fish: `-l` (login shell)
/// - zsh: `-l -i` (login + interactive)
/// - bash: no args (PTY makes it interactive automatically)
/// - pwsh / powershell: `-NoLogo`
/// - cmd: no args
pub fn get_shell_args(shell_path: &str) -> Vec<&'static str> {
    match shell_name(shell_path).as_str() {
        "fish" => vec!["-l"],      // login shell
        "zsh" => vec!["-l", "-i"], // login + interactive
        "bash" => vec![],          // PTY will make it interactive automatically
        "pwsh" | "powershell" => vec!["-NoLogo"],
        "cmd" => vec![],
        _ => vec![],
    }
}

/// Lowercased executable name without `.exe`.
fn shell_name(shell_path: &str) -> String {
    let shell_name = std::path::Path::new(shell_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    match shell_name.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => shell_name,
    }
}

/// Command syntax family of a shell, as far as typing a command into it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellKind {
    /// sh, dash and anything unrecognised: no way to write a control
    /// character other than as the raw byte, which the line editor would
    /// take for a key, so a line holding one is not typed at all.
    #[default]
    Posix,
    /// bash (Git Bash included), zsh and ksh: POSIX, plus `$'…'` escapes.
    Bash,
    /// fish: POSIX-like, but its single quotes take escapes.
    Fish,
    /// cmd.exe.
    Cmd,
    /// Windows PowerShell and PowerShell Core.
    PowerShell,
}

impl ShellKind {
    pub fn from_shell_path(shell_path: &str) -> Self {
        match shell_name(shell_path).as_str() {
            "cmd" => Self::Cmd,
            "pwsh" | "powershell" => Self::PowerShell,
            "fish" => Self::Fish,
            "bash" | "zsh" | "ksh" | "ksh93" | "mksh" => Self::Bash,
            _ => Self::Posix,
        }
    }

    /// A command line changing to `path`, quoted for this shell. cmd needs
    /// `/d` to switch drives; neither cmd nor PowerShell understands POSIX
    /// quoting.
    pub fn cd_command(self, path: &std::path::Path) -> String {
        let path = self.quote(&path.to_string_lossy());
        match self {
            Self::Posix | Self::Bash | Self::Fish => format!("cd {path}"),
            Self::Cmd => format!("cd /d {path}"),
            Self::PowerShell => format!("Set-Location -LiteralPath {path}"),
        }
    }

    /// A command line running the program at `path`, quoted for this shell.
    /// PowerShell takes a quoted word for a string to print, so it needs
    /// its call operator.
    pub fn run_command(self, path: &std::path::Path) -> String {
        let path = self.quote(&path.to_string_lossy());
        match self {
            Self::PowerShell => format!("& {path}"),
            _ => path,
        }
    }

    /// The quoting rules of this shell, for a line typed into it.
    pub fn quoting(self) -> termide_config::Quoting {
        match self {
            Self::Posix => termide_config::Quoting::Posix,
            Self::Bash => termide_config::Quoting::Bash,
            Self::Fish => termide_config::Quoting::Fish,
            Self::Cmd => termide_config::Quoting::Cmd,
            Self::PowerShell => termide_config::Quoting::PowerShell,
        }
    }

    /// `value` as one literal word of this shell's command line.
    pub fn quote(self, value: &str) -> String {
        self.quoting().quote(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn shell_kind_from_executable_name() {
        assert_eq!(ShellKind::from_shell_path("cmd.exe"), ShellKind::Cmd);
        assert_eq!(
            ShellKind::from_shell_path("PWSH.EXE"),
            ShellKind::PowerShell
        );
        assert_eq!(
            ShellKind::from_shell_path("powershell"),
            ShellKind::PowerShell
        );
        assert_eq!(ShellKind::from_shell_path("/bin/zsh"), ShellKind::Bash);
        assert_eq!(ShellKind::from_shell_path("bash.exe"), ShellKind::Bash);
        assert_eq!(ShellKind::from_shell_path("/bin/sh"), ShellKind::Posix);
        assert_eq!(
            ShellKind::from_shell_path("/opt/homebrew/bin/fish"),
            ShellKind::Fish
        );
        assert_eq!(
            ShellKind::from_shell_path("wsl -d Ubuntu"),
            ShellKind::Posix
        );
    }

    #[test]
    fn cd_command_quotes_for_each_shell() {
        let path = Path::new(r"D:\it's here");
        assert_eq!(ShellKind::Posix.cd_command(path), r"cd 'D:\it'\''s here'");
        assert_eq!(ShellKind::Bash.cd_command(path), r"cd 'D:\it'\''s here'");
        assert_eq!(ShellKind::Cmd.cd_command(path), r#"cd /d "D:\it's here""#);
        assert_eq!(
            ShellKind::PowerShell.cd_command(path),
            r"Set-Location -LiteralPath 'D:\it''s here'"
        );
    }

    /// A directory named to type keys into the shell (Ctrl+U to clear the
    /// line, a command, CR to run it) stays a name for shells that can
    /// escape control characters, and keeps the raw bytes for the others —
    /// which `Terminal::send_command` then refuses to type.
    #[test]
    fn cd_command_never_types_control_characters_where_they_can_be_escaped() {
        let path = Path::new("/tmp/x\u{15}touch /tmp/pwned\r");
        let bash = ShellKind::Bash.cd_command(path);
        assert_eq!(bash, r"cd '/tmp/x'$'\025''touch /tmp/pwned'$'\015'");
        let fish = ShellKind::Fish.cd_command(path);
        assert_eq!(fish, r"cd '/tmp/x'\x15'touch /tmp/pwned'\x0d");
        for kind in [ShellKind::Posix, ShellKind::Cmd, ShellKind::PowerShell] {
            assert!(kind.cd_command(path).contains('\u{15}'), "{kind:?}");
        }
    }

    #[test]
    fn run_command_quotes_the_program() {
        let path = Path::new("/p/my app;rm -rf ~");
        assert_eq!(ShellKind::Bash.run_command(path), r"'/p/my app;rm -rf ~'");
        assert_eq!(
            ShellKind::PowerShell.run_command(Path::new(r"C:\my app.exe")),
            r"& 'C:\my app.exe'"
        );
        assert_eq!(
            ShellKind::Cmd.run_command(Path::new(r"C:\my app.exe")),
            r#""C:\my app.exe""#
        );
    }

    #[test]
    fn quote_makes_one_literal_word() {
        assert_eq!(ShellKind::Posix.quote("a b'c $x"), r"'a b'\''c $x'");
        assert_eq!(ShellKind::Cmd.quote(r#"a b"c"#), r#""a b""c""#);
        assert_eq!(
            ShellKind::PowerShell.quote("a b'c\u{2019}d $x"),
            "'a b''c\u{2019}\u{2019}d $x'"
        );
    }
}
