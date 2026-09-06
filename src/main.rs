mod ui;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;

use termide_app::App;
use termide_config::Config;
use termide_core::{init_icon_mode, init_terminal_caps};
use termide_git::is_available as check_git_available;
use termide_i18n::init_with_language;
use termide_theme::{set_ansi16_mode, set_themes_dir};

#[derive(Parser)]
#[command(name = "termide", version = termide_core::VERSION, about = "Terminal IDE")]
struct Cli {
    /// Override minimum log level (trace, debug, info, warn, error)
    #[arg(long)]
    log_level: Option<String>,

    /// Disable LSP support
    #[arg(long)]
    no_lsp: bool,

    /// Path to config file (default: ~/.config/termide/config.toml)
    #[arg(long, value_name = "PATH")]
    config: Option<std::path::PathBuf>,

    /// Run pre-flight diagnostics (config / paths / git) and exit
    /// without starting the UI. Exit code 0 if everything is OK,
    /// non-zero if any check failed.
    #[arg(long)]
    diagnostics: bool,

    /// File(s) to open. Given a path, termide starts in a clean editor view
    /// (no session is restored or saved), so it works as $EDITOR for tools
    /// like git, crontab and visudo: `EDITOR=termide git commit`.
    #[arg(value_name = "FILE")]
    files: Vec<std::path::PathBuf>,
}

/// Print a diagnostics report to stdout and return whether everything
/// passed. Called when the user runs `termide --diagnostics`; never
/// touches raw mode / alternate screen, so the output is safely
/// captured by shell redirects.
fn run_diagnostics(custom_config: Option<&std::path::Path>) -> bool {
    use termide_config::{get_config_dir, get_data_dir};

    let mut ok = true;
    let mut check = |label: &str, status: Result<String, String>| match status {
        Ok(msg) => println!("  \u{2713} {}: {}", label, msg),
        Err(msg) => {
            println!("  \u{2717} {}: {}", label, msg);
            ok = false;
        }
    };

    println!("termide diagnostics");
    println!("===================\n");

    println!("Config:");
    let project_root = std::env::current_dir().ok();
    let config_result = if let Some(path) = custom_config {
        termide_config::Config::load_from(path).map(|_| path.display().to_string())
    } else if let Some(root) = project_root.as_ref() {
        termide_config::Config::load_layered(None, root)
            .map(|_| "layered (defaults + global + project) parses OK".to_string())
    } else {
        Err(anyhow::anyhow!("cannot resolve current directory"))
    };
    check("load", config_result.map_err(|e| format!("{e}")));

    println!("\nDirectories:");
    check(
        "config dir",
        get_config_dir()
            .map(|p| p.display().to_string())
            .map_err(|e| format!("{e}")),
    );
    check(
        "data dir",
        get_data_dir()
            .map(|p| p.display().to_string())
            .map_err(|e| format!("{e}")),
    );
    check(
        "themes dir",
        termide_config::Config::get_themes_dir()
            .map(|p| {
                if p.exists() {
                    p.display().to_string()
                } else {
                    format!("{} (will be created on first save)", p.display())
                }
            })
            .map_err(|e| format!("{e}")),
    );
    if let Some(ref root) = project_root {
        check(
            "session dir",
            termide_session::Session::get_session_dir(root)
                .map(|p| p.display().to_string())
                .map_err(|e| format!("{e}")),
        );
    }

    println!("\nGit:");
    if check_git_available() {
        check("git", Ok("found in PATH".to_string()));
    } else {
        // Not an error — git is optional — but flag it as a warning so
        // users know why git panels stay empty.
        println!("  \u{26A0}  git: not found in PATH (git panels will be disabled)");
    }

    println!();
    if ok {
        println!("All checks passed.");
    } else {
        println!("One or more checks failed.");
    }
    ok
}

/// Restore terminal to a usable state (raw mode off, alternate screen off, etc.).
/// Called both on normal exit and from the panic handler.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste,
        SetTitle("")
    );
    let _ = execute!(io::stdout(), crossterm::cursor::Show);
}

fn main() -> Result<()> {
    // SSH_ASKPASS mode: when termide is set as ssh's askpass helper for a git
    // network operation, ssh re-executes this binary to obtain the SSH key
    // passphrase. We detect that purely by the presence of TERMIDE_ASKPASS_FILE
    // (ssh passes the prompt as argv, which must NOT be treated as a file to
    // open), hand the passphrase termide stored there back to ssh, and exit.
    // No TUI, no clap.
    if let Ok(secret_file) = std::env::var("TERMIDE_ASKPASS_FILE") {
        if let Ok(secret) = std::fs::read(&secret_file) {
            // This is the SSH_ASKPASS contract, not logging: ssh reads the
            // passphrase from our stdout (a pipe it owns), so it never reaches
            // a terminal or log. Write the raw bytes straight through.
            use std::io::Write;
            let _ = std::io::stdout().write_all(&secret);
        }
        return Ok(());
    }

    // Parse CLI arguments
    let cli = Cli::parse();

    // --diagnostics short-circuits before terminal init so output
    // is plain stdout, capturable by scripts and visible if termide
    // is launched without a TTY.
    if cli.diagnostics {
        let ok = run_diagnostics(cli.config.as_deref());
        std::process::exit(if ok { 0 } else { 1 });
    }

    // Install panic handler that restores terminal before printing the panic.
    // Without this, a panic leaves the terminal in raw mode + alternate screen,
    // which looks like a frozen blank screen (especially over SSH).
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    // Detect terminal capabilities first (before loading themes)
    let caps = init_terminal_caps();

    // Enable ANSI-16 color adaptation for limited color terminals (Linux TTY)
    if caps.needs_color_adaptation() {
        set_ansi16_mode(true);
    }

    // Resolve project root early so the layered config loader can pick up
    // a `<project>/.termide/config.toml` overlay.
    let project_root = std::env::current_dir()
        .unwrap_or_else(|_| dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/")));

    // Layered load: defaults → global file → project overlay (if any).
    // `--config <PATH>` bypasses layering and treats the file as the whole
    // effective config (historical semantics). The second tuple element is
    // the `defaults + global` snapshot used later as the diff baseline for
    // the per-project override file.
    // Capture any config-load failure so we can re-emit it as
    // `log::warn!` after the logger comes up below. eprintln before
    // raw mode prints to a soon-to-be-overwritten terminal scrollback;
    // the Journal panel is where the user will actually look.
    let mut config_load_warning: Option<String> = None;
    let (mut config, mut global_baseline) = if let Some(ref path) = cli.config {
        let cfg = Config::load_from(path)?;
        (cfg.clone(), cfg)
    } else {
        Config::load_layered(None, &project_root).unwrap_or_else(|e| {
            config_load_warning = Some(format!("Could not load config: {e}. Using defaults."));
            (Config::default(), Config::default())
        })
    };

    // Apply CLI overrides on top of the layered config. These are runtime-only
    // — they do NOT propagate into the diff-against-baseline saves.
    if let Some(ref level) = cli.log_level {
        config.logging.min_level = level.clone();
        global_baseline.logging.min_level = config.logging.min_level.clone();
    }
    if cli.no_lsp {
        config.lsp.enabled = false;
        global_baseline.lsp.enabled = false;
    }

    // On Linux VT, use norton-commander theme by default (better for 16-color)
    if caps.is_linux_console && config.general.theme == "default" {
        config.general.theme = "norton-commander".to_string();
    }

    // Initialize icon mode based on config + terminal capabilities
    init_icon_mode(config.general.icon_mode);

    // Initialize theme system with themes directory from config
    if let Ok(themes_dir) = Config::get_themes_dir() {
        set_themes_dir(themes_dir);
    }

    // Initialize translation system with language from config
    init_with_language(&config.general.language)?;

    // Check for git on the system
    let git_available = check_git_available();

    // Initialize terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();

    // Check if terminal supports enhanced keyboard protocol (kitty protocol).
    // This enables proper Alt+Cyrillic handling in modern terminals like Ghostty, Kitty, WezTerm.
    // Skip on SSH: the detection sends escape sequences and waits for a response,
    // which can hang indefinitely if the SSH terminal doesn't reply.
    let keyboard_caps = termide_keyboard::KeyboardCaps::detect(config.general.report_all_keys);
    let keyboard_enhanced = keyboard_caps.kitty_full;

    let title = format!(
        "Termide: {}",
        termide_core::util::shorten_home_path(
            &std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )
    );

    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange,
        EnableBracketedPaste,
        SetTitle(title)
    )?;

    if keyboard_enhanced {
        // REPORT_EVENT_TYPES exposes `KeyEventState::CAPS_LOCK` on every key
        // event, which the hotkey matcher uses to ignore the spurious Shift
        // modifier that Caps Lock attaches to letters. `EventHandler` drops
        // Release events so the rest of the app keeps its press-only
        // assumption (Repeat is kept, for held-key auto-repeat).
        //
        // REPORT_ALTERNATE_KEYS is what makes shifted characters typable.
        // Under REPORT_ALL_KEYS_AS_ESCAPE_CODES every key — including plain
        // text — arrives as CSI-u, and the protocol's primary codepoint is the
        // key *without* modifiers. Without the alternate codepoint crossterm
        // reports `Shift+6` as `Char('6') + Shift`, so a Russian layout types
        // `6` where the user pressed a comma, and a US layout types `1` for
        // `!`. With the flag crossterm substitutes the shifted codepoint and
        // clears the Shift bit, which is the correct character to insert.
        //
        // The cost is that a chord carrying Shift reaches the matcher in that
        // same substituted shape — `Ctrl+Shift+S` as `Ctrl+Char('S')`.
        // `ParsedKeyBinding::matches` accepts it; see the note there.
        let mut flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_EVENT_TYPES;

        // REPORT_ALL_KEYS_AS_ESCAPE_CODES is what makes macOS `Option+<letter>`
        // reach us as `Alt+<letter>`: without it the terminal composes text and
        // sends the glyph (`Option+F` → `ƒ`) with no ALT bit, so ~25 `Alt+…`
        // defaults are unreachable there. It also makes the terminal report
        // standalone modifier-key presses, which `EventHandler` drops — that
        // side effect, not the flag itself, is what once "broke Shift+Home".
        // `KeyboardCaps::detect` restricts the flag to macOS and honours
        // `general.report_all_keys`, because the flag also stops dead-key and
        // IME composition from reaching the app.
        if keyboard_caps.all_keys {
            flags |= KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        }

        execute!(stdout, PushKeyboardEnhancementFlags(flags))?;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Get terminal size and use it to initialize app with correct dimensions.
    // Guard against 0x0 (can happen on SSH before PTY size negotiation completes).
    let size = terminal.size()?;
    let width = size.width.max(20);
    let height = size.height.max(5);

    // Create application with pre-loaded config (avoids double config loading)
    let mut app = App::new_with_config(config, global_baseline, width, height, keyboard_caps);

    // Re-emit any deferred startup warnings now that the logger is up
    // — these end up in the Journal panel where users actually look.
    if let Some(msg) = config_load_warning {
        log::warn!("{}", msg);
    }

    // Log git availability to journal (not to stderr)
    app.log_git_status(git_available);

    // With explicit file arguments, behave like a plain $EDITOR invocation:
    // open just those files in a clean view and don't touch the project's
    // session (restoring or overwriting it when editing e.g. a commit message
    // would be surprising and could clobber the real session).
    if cli.files.is_empty() {
        // Try to load session, fallback to default layout on error
        if let Err(e) = app.load_session() {
            // Session file doesn't exist or is corrupted - use default layout.
            // Surface the reason in the Journal so a corrupted session is
            // diagnosable instead of silently snapping to defaults.
            log::warn!("Could not load session ({e}); starting with the default layout.");
            app.setup_default_layout();
        }
    } else {
        app.set_session_persistence(false);
        for path in cli.files {
            if let Err(e) = app.open_path_in_editor(path.clone()) {
                log::error!("Failed to open '{}' from CLI: {e}", path.display());
            }
        }
    }

    // Run application
    let result = app.run(&mut terminal, |frame, state, layout_manager| {
        ui::render_layout_with_accordion(frame, state, layout_manager);
    });

    // Restore terminal
    if keyboard_enhanced {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    restore_terminal();

    // Print error if there was one
    if let Err(err) = result {
        log::error!("Error: {:?}", err);
    }

    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::Cli;
    use clap::Parser;
    use std::path::PathBuf;

    // Regression for #24: a bare file path must parse as a positional argument
    // (clap previously rejected it as "unexpected argument"), so termide can be
    // used as $EDITOR — e.g. `EDITOR=termide crontab -e`.
    #[test]
    fn accepts_a_file_path_argument() {
        let cli = Cli::try_parse_from(["termide", "/tmp/crontab.kIwZUa/crontab"]).unwrap();
        assert_eq!(
            cli.files,
            vec![PathBuf::from("/tmp/crontab.kIwZUa/crontab")]
        );
    }

    #[test]
    fn no_arguments_means_no_files() {
        let cli = Cli::try_parse_from(["termide"]).unwrap();
        assert!(cli.files.is_empty());
    }

    #[test]
    fn flags_and_multiple_files_coexist() {
        let cli = Cli::try_parse_from(["termide", "--no-lsp", "a.rs", "b.rs"]).unwrap();
        assert!(cli.no_lsp);
        assert_eq!(
            cli.files,
            vec![PathBuf::from("a.rs"), PathBuf::from("b.rs")]
        );
    }
}
