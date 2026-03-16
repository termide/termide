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
    let mut seen_paths = std::collections::HashSet::new();

    #[cfg(windows)]
    {
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
                let text = String::from_utf8(output.stdout)
                    .or_else(|e| {
                        let bytes = e.into_bytes();
                        // Try UTF-16LE decoding; truncate trailing odd byte if present
                        let len = bytes.len() & !1;
                        let u16s: Vec<u16> = bytes[..len]
                            .chunks_exact(2)
                            .map(|c| u16::from_le_bytes([c[0], c[1]]))
                            .collect();
                        String::from_utf16(&u16s).map_err(|_| ())
                    })
                    .unwrap_or_default();

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
        // Parse /etc/shells for all valid login shells
        if let Ok(content) = std::fs::read_to_string("/etc/shells") {
            for line in content.lines() {
                let line = line.trim();
                if line.starts_with('#') || line.is_empty() {
                    continue;
                }
                if std::path::Path::new(line).exists() && !seen_contains(&seen_paths, line) {
                    shells.push(ShellInfo {
                        name: shell_display_name(line),
                        path: line.to_string(),
                    });
                    seen_paths.insert(line.to_string());
                }
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
            if std::path::Path::new(path).exists() && !seen_contains(&seen_paths, path) {
                shells.push(ShellInfo {
                    name: shell_display_name(path),
                    path: path.to_string(),
                });
                seen_paths.insert(path.to_string());
            }
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
    let shell_name = std::path::Path::new(shell_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Strip .exe suffix for matching on Windows
    let shell_name = shell_name.strip_suffix(".exe").unwrap_or(&shell_name);

    match shell_name {
        "fish" => vec!["-l"],      // login shell
        "zsh" => vec!["-l", "-i"], // login + interactive
        "bash" => vec![],          // PTY will make it interactive automatically
        "pwsh" | "powershell" => vec!["-NoLogo"],
        "cmd" => vec![],
        _ => vec![],
    }
}
