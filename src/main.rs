#[cfg(unix)]
mod completions;
mod ui;

use anyhow::Result;
use clap::{ArgGroup, Parser};
use crossterm::{event::PopKeyboardEnhancementFlags, execute, terminal::enable_raw_mode};
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
#[command(group(ArgGroup::new("headless").args(["prompt", "recall"])))]
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

    /// Start a detached instance and print its id. The instance keeps
    /// running — with every shell, LSP server and job inside it — after the
    /// terminal that started it is closed.
    #[cfg(unix)]
    #[arg(long)]
    detached: bool,

    /// Attach to a detached instance. Without an id, the most recent one.
    #[cfg(unix)]
    #[arg(
        long,
        value_name = "ID",
        num_args = 0..=1,
        default_missing_value = "",
        conflicts_with = "restore"
    )]
    attach: Option<String>,

    /// With `--attach`: take the instance over from a client that is already
    /// attached, instead of being refused. That client is detached.
    #[cfg(unix)]
    #[arg(short, long, requires = "attach")]
    force: bool,

    /// End a detached instance, with every shell and job inside it, and exit.
    /// Unsaved changes in it are lost.
    #[cfg(unix)]
    #[arg(long, value_name = "ID")]
    kill: Option<String>,

    /// List detached instances and exit.
    #[cfg(unix)]
    #[arg(long)]
    list_instances: bool,

    /// Print a completion script for the given shell and exit. Load it with
    /// `eval "$(termide --completions bash)"` in ~/.bashrc, or write it into
    /// the shell's completions directory; `--attach` then completes instance
    /// ids from `--list-instances`.
    #[cfg(unix)]
    #[arg(long, value_name = "SHELL", value_parser = completions::SHELLS)]
    completions: Option<String>,

    /// Write the completion script where the shell loads it from and exit:
    /// bash and fish pick it up on their own, for zsh the line to add to
    /// ~/.zshrc is printed. Without a shell name, `$SHELL` decides.
    #[cfg(unix)]
    #[arg(long, value_name = "SHELL", num_args = 0..=1, value_parser = completions::SHELLS)]
    install_completions: Option<Option<String>>,

    /// Run one agent task without the UI and print the answer to stdout,
    /// then exit: `termide --prompt "summarise src/main.rs"`. With `-` the
    /// prompt is read from stdin. Tool activity and errors go to stderr.
    /// The agent runs under the configured permission rules and mode with
    /// anything else refused, since nothing can prompt; set `mode = "auto"`
    /// or add allow rules for unattended use.
    #[arg(long, value_name = "PROMPT")]
    prompt: Option<String>,

    /// Search the project's earlier agent sessions, git history and code the
    /// way the agent's `recall` tool does, print the ranked results (or, with
    /// `[ai.recall] solver = true`, the answer drawn from them) and exit:
    /// `termide --recall "why no tokio"`. Exits 1 when nothing is found.
    #[arg(long, value_name = "QUERY", conflicts_with = "prompt")]
    recall: Option<String>,

    /// Which agent definition the `--prompt` run uses; the default agent
    /// otherwise.
    #[arg(
        long,
        value_name = "NAME",
        requires = "prompt",
        conflicts_with = "recall"
    )]
    agent: Option<String>,

    /// How a `--prompt` run reports: `text` (the answer on stdout, the
    /// default), `json` (one object with the answer, token usage, the tool
    /// calls and the status) or `stream-json` (one JSON object per event as
    /// it happens). A `--recall` run takes `text` or `json`.
    #[arg(long, value_name = "FORMAT", requires = "headless", value_parser = ["text", "json", "stream-json"], default_value = "text")]
    output: String,

    /// Reopen the projects of the last run. The current project is the one
    /// for the working directory; the others open in the background and load
    /// when first entered. Without a saved set, an ordinary launch.
    #[arg(short, long, conflicts_with_all = ["files", "headless"])]
    restore: bool,

    /// File(s) or directories to open. Given a path, termide starts in a
    /// clean view (no project layout is restored or saved). Text opens in the
    /// editor, so it works as $EDITOR for tools like git, crontab and visudo:
    /// `EDITOR=termide git commit`. Images, SQLite files, other binary files
    /// and directories open in their viewer, the hex editor or a file manager.
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
        termide_config::Config::load_layered(None, root).and_then(|loaded| {
            if loaded.warnings.is_empty() {
                Ok("layered (defaults + global + project) parses OK".to_string())
            } else {
                Err(anyhow::anyhow!(loaded.warnings.join("\n    ")))
            }
        })
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
            "project dir",
            termide_project::ProjectLayout::get_project_dir(root)
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
    termide_core::leave_terminal_modes();
}

/// The arguments a detached instance's own termide starts with: the files
/// to open and `--restore`, which the hosted process acts on itself.
#[cfg(unix)]
fn hosted_args(cli: &Cli) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = Vec::new();
    if cli.restore {
        args.push("--restore".into());
    }
    args.extend(cli.files.iter().map(|f| f.clone().into_os_string()));
    args
}

/// Handle `--list-instances`, `--kill`, `--attach` and `--detached`.
///
/// Returns `Some(exit_code)` when one of them ran and the process should stop,
/// `None` when this is an ordinary launch.
#[cfg(unix)]
fn handle_detached_instance_cli(cli: &Cli) -> Result<Option<i32>> {
    if cli.list_instances {
        print!("{}", termide_detach::format_instance_list()?);
        return Ok(Some(0));
    }

    if let Some(id) = &cli.kill {
        return match termide_detach::client::kill(id) {
            Ok(()) => Ok(Some(0)),
            Err(e) => {
                eprintln!("termide: {e:#}");
                Ok(Some(1))
            }
        };
    }

    if let Some(id) = &cli.attach {
        // `--attach` with no value means "the most recent instance".
        let id = if id.is_empty() {
            None
        } else {
            Some(id.clone())
        };
        return match termide_detach::client::attach(id, cli.force) {
            Ok(code) => Ok(Some(code)),
            Err(e) => {
                eprintln!("termide: {e:#}");
                Ok(Some(1))
            }
        };
    }

    if cli.detached {
        let project_root = std::env::current_dir()?;
        let id = termide_detach::spawn_detached(&project_root, &hosted_args(cli))?;
        println!("Detached instance '{id}' started.");
        println!("Attach with: termide --attach {id}");
        return Ok(Some(0));
    }

    Ok(None)
}

fn main() -> Result<()> {
    // Askpass mode: when termide is set as ssh's or git's askpass helper for a
    // git network operation, ssh / git re-execute this binary to obtain the
    // SSH key passphrase or the HTTPS credentials. We detect that purely by
    // the presence of TERMIDE_ASKPASS_FILE (the prompt comes as argv, which
    // must NOT be treated as a file to open), hand back what termide stored —
    // the user name for git's "Username for ..." prompt, the secret for any
    // other — and exit. No TUI, no clap.
    if let Ok(secret_file) = std::env::var("TERMIDE_ASKPASS_FILE") {
        let prompt = std::env::args().nth(1).unwrap_or_default();
        if let (true, Ok(user)) = (
            prompt.starts_with("Username"),
            std::env::var("TERMIDE_ASKPASS_USER"),
        ) {
            println!("{user}");
            return Ok(());
        }
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

    // `--completions` only prints a script; like the other pre-UI options
    // it must stay plain stdout so `eval "$(termide --completions bash)"`
    // and shell redirects capture it.
    #[cfg(unix)]
    if let Some(shell) = &cli.completions {
        print!("{}", completions::script(shell));
        return Ok(());
    }
    #[cfg(unix)]
    if let Some(shell) = &cli.install_completions {
        let env = completions::Env::from_process()?;
        match completions::install(shell.as_deref(), &env) {
            Ok(report) => print!("{report}"),
            Err(e) => {
                eprintln!("termide: {e:#}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }

    // --diagnostics short-circuits before terminal init so output
    // is plain stdout, capturable by scripts and visible if termide
    // is launched without a TTY.
    if cli.diagnostics {
        let ok = run_diagnostics(cli.config.as_deref());
        std::process::exit(if ok { 0 } else { 1 });
    }

    // Detached-instance handling runs before anything else touches the
    // terminal, the config or the logger. `--detached` forks, and fork only
    // carries the calling thread into the child: a lock held by a thread that
    // no longer exists would deadlock the daemon, so no thread may exist yet.
    #[cfg(unix)]
    if let Some(code) = handle_detached_instance_cli(&cli)? {
        std::process::exit(code);
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
    // Capture config-load problems so we can re-emit them as
    // `log::warn!` after the logger comes up below. eprintln before
    // raw mode prints to a soon-to-be-overwritten terminal scrollback;
    // the Journal panel is where the user will actually look.
    let mut config_load_warnings: Vec<String> = Vec::new();
    let (mut config, mut global_baseline) = if let Some(ref path) = cli.config {
        let cfg = Config::load_from(path)?;
        (cfg.clone(), cfg)
    } else {
        match Config::load_layered(None, &project_root) {
            Ok(loaded) => {
                config_load_warnings = loaded.warnings;
                (loaded.effective, loaded.global_layer)
            }
            Err(e) => {
                config_load_warnings.push(format!("Could not load config: {e}. Using defaults."));
                (Config::default(), Config::default())
            }
        }
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

    // `general.always_detachable`: put this instance in a host of its own and
    // attach to it, so that it is the host — not this process — that dies with
    // the terminal. Must happen before anything spawns a thread, because the
    // host is created by fork.
    //
    // Skipped with file arguments: `git commit` and friends wait for the editor
    // to exit, and a detach would tell them the edit finished when it had not.
    // Skipped inside a hosted instance for the obvious reason.
    #[cfg(unix)]
    if config.general.always_detachable
        && cli.files.is_empty()
        && std::env::var_os(termide_detach::SOCKET_ENV).is_none()
    {
        let id = termide_detach::spawn_detached(&project_root, &hosted_args(&cli))?;
        let code = termide_detach::client::attach(Some(id), false)?;
        std::process::exit(code);
    }

    // Initialize icon mode based on config + terminal capabilities
    init_icon_mode(config.general.icon_mode);

    // Initialize theme system with themes directory from config
    if let Ok(themes_dir) = Config::get_themes_dir() {
        set_themes_dir(themes_dir);
    }

    // Initialize translation system with language from config
    init_with_language(&config.general.language)?;

    // Headless agent run: no UI, plain stdout, like --diagnostics. Config and
    // translations are up; the terminal is still untouched.
    if let Some(query) = cli.recall.clone() {
        if cli.output == "stream-json" {
            eprintln!("termide: --recall reports as text or json");
            std::process::exit(2);
        }
        let code = termide_app::run_recall(
            &config.ai,
            &project_root,
            query.trim(),
            cli.output == "json",
        );
        std::process::exit(code);
    }
    if let Some(prompt) = cli.prompt.clone() {
        let prompt = if prompt == "-" {
            use std::io::Read;
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer)?;
            buffer
        } else {
            prompt
        };
        let cwd = std::env::current_dir().unwrap_or_else(|_| project_root.clone());
        let output = match cli.output.as_str() {
            "json" => termide_app::HeadlessOutput::Json,
            "stream-json" => termide_app::HeadlessOutput::StreamJson,
            _ => termide_app::HeadlessOutput::Text,
        };
        let code = termide_app::run_agent_headless(
            &config.ai,
            &cwd,
            &project_root,
            cli.agent.as_deref(),
            prompt.trim(),
            output,
        );
        std::process::exit(code);
    }

    // Saved layouts of a termide older than the projects rename. Only the
    // process that goes on to run the UI does this, so its report reaches
    // the journal with the config warnings below.
    let migration_notes = termide_project::migrate_legacy_layouts();

    // Check for git on the system
    let git_available = check_git_available();

    // Initialize terminal
    enable_raw_mode()?;
    let stdout = io::stdout();

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

    // Alternate screen, mouse, focus and paste reporting, plus the keyboard
    // enhancement flags. Shared with the reattach path so a client that
    // connects to a detached instance gets exactly these modes and no other.
    termide_core::enter_terminal_modes(&keyboard_caps, Some(&title))?;

    // Whether `⏱️`-style emoji take one column or two is the host terminal's
    // call, and the frame diff must agree with it. A hosted instance has the
    // daemon at the other end of its PTY, which answers no query: there the
    // attach client probes its own terminal and hands the answer over.
    #[cfg(unix)]
    let hosted = std::env::var_os(termide_detach::SOCKET_ENV).is_some();
    #[cfg(not(unix))]
    let hosted = false;
    let vs16_probe = if hosted {
        None
    } else {
        Some(termide_core::probe_variation_selector_width())
    };
    let vs16_wide = termide_core::adopt_variation_selector_width(vs16_probe.flatten());

    // In a detached instance, the daemon signals us when a client attaches.
    // A no-op otherwise.
    #[cfg(unix)]
    if let Err(e) = termide_detach::install_reattach_handler() {
        log::warn!("Could not install the reattach handler: {e:#}");
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
    match vs16_probe {
        Some(Some(answer)) => log::info!(
            "Host terminal makes emoji + VS16 (⏱️) {} wide; frame diff follows it",
            if answer { "two columns" } else { "one column" }
        ),
        Some(None) => log::warn!(
            "Host terminal did not answer the VS16 width probe; assuming {} (set {}=0|1 to override)",
            if vs16_wide { "two columns" } else { "one column" },
            termide_core::VS16_WIDTH_ENV
        ),
        None => log::info!("Hosted instance: VS16 width comes from the attach client"),
    }
    // — these end up in the Journal panel where users actually look.
    for msg in config_load_warnings {
        log::warn!("{}", msg);
    }
    for (level, msg) in migration_notes {
        log::log!(level, "{}", msg);
    }

    // Log git availability to journal (not to stderr)
    app.log_git_status(git_available);

    // With explicit file arguments, behave like a plain $EDITOR invocation:
    // open just those files in a clean view and don't touch the project's
    // layout (restoring or overwriting it when editing e.g. a commit message
    // would be surprising and could clobber the real layout).
    if cli.files.is_empty() {
        // Try to load the project layout, fallback to default layout on error
        if let Err(e) = app.load_layout() {
            // Layout file doesn't exist or is corrupted - use default layout.
            // Surface the reason in the Journal so a corrupted layout is
            // diagnosable instead of silently snapping to defaults.
            log::warn!(
                "Could not load the project layout ({e}); starting with the default layout."
            );
            app.setup_default_layout();
        }
        if cli.restore {
            app.restore_projects_on_start();
        }
    } else {
        app.set_layout_persistence(false);
        for path in cli.files {
            if let Err(e) = app.open_cli_path(path.clone()) {
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

    #[test]
    fn restore_parses_alone_and_with_detached() {
        assert!(!Cli::try_parse_from(["termide"]).unwrap().restore);
        assert!(Cli::try_parse_from(["termide", "-r"]).unwrap().restore);
        assert!(
            Cli::try_parse_from(["termide", "--restore"])
                .unwrap()
                .restore
        );
        #[cfg(unix)]
        {
            let cli = Cli::try_parse_from(["termide", "--detached", "-r"]).unwrap();
            assert!(cli.detached && cli.restore);
            assert_eq!(super::hosted_args(&cli), vec!["--restore"]);
        }
    }

    /// `--restore` sets up the full workspace, which the clean file view,
    /// the headless runs and attaching to a running instance all exclude.
    #[test]
    fn restore_conflicts_with_files_headless_and_attach() {
        assert!(Cli::try_parse_from(["termide", "-r", "notes.md"]).is_err());
        assert!(Cli::try_parse_from(["termide", "-r", "--prompt", "x"]).is_err());
        assert!(Cli::try_parse_from(["termide", "-r", "--recall", "x"]).is_err());
        #[cfg(unix)]
        assert!(Cli::try_parse_from(["termide", "-r", "--attach"]).is_err());
    }

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
    fn prompt_flag_carries_the_text_and_optional_agent() {
        let cli = Cli::try_parse_from(["termide", "--prompt", "do a thing"]).unwrap();
        assert_eq!(cli.prompt.as_deref(), Some("do a thing"));
        assert!(cli.agent.is_none());
        let named =
            Cli::try_parse_from(["termide", "--prompt", "review", "--agent", "reviewer"]).unwrap();
        assert_eq!(named.agent.as_deref(), Some("reviewer"));
        // --agent without --prompt is rejected.
        assert!(Cli::try_parse_from(["termide", "--agent", "reviewer"]).is_err());
        // --output defaults to text, accepts json, rejects other values and
        // requires --prompt.
        assert_eq!(
            Cli::try_parse_from(["termide", "--prompt", "x"])
                .unwrap()
                .output,
            "text"
        );
        assert_eq!(
            Cli::try_parse_from(["termide", "--prompt", "x", "--output", "json"])
                .unwrap()
                .output,
            "json"
        );
        assert_eq!(
            Cli::try_parse_from(["termide", "--prompt", "x", "--output", "stream-json"])
                .unwrap()
                .output,
            "stream-json"
        );
        assert!(Cli::try_parse_from(["termide", "--prompt", "x", "--output", "yaml"]).is_err());
        assert!(Cli::try_parse_from(["termide", "--output", "json"]).is_err());
    }

    #[test]
    fn recall_flag_takes_a_query_and_an_output_but_not_a_prompt() {
        let cli = Cli::try_parse_from(["termide", "--recall", "why no tokio"]).unwrap();
        assert_eq!(cli.recall.as_deref(), Some("why no tokio"));
        assert_eq!(cli.output, "text");
        let json = Cli::try_parse_from(["termide", "--recall", "x", "--output", "json"]).unwrap();
        assert_eq!(json.output, "json");
        assert!(Cli::try_parse_from(["termide", "--recall", "x", "--prompt", "y"]).is_err());
        assert!(Cli::try_parse_from(["termide", "--recall", "x", "--agent", "a"]).is_err());
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

#[cfg(all(test, unix))]
mod completion_tests {
    use super::*;
    use clap::CommandFactory;

    const SCRIPTS: [(&str, &str); 3] = [
        ("bash", include_str!("../completions/termide.bash")),
        ("zsh", include_str!("../completions/_termide")),
        ("fish", include_str!("../completions/termide.fish")),
    ];

    /// `--install-completions [SHELL]` takes its value optionally: bare, it
    /// must parse as "use $SHELL" rather than swallow the next word.
    #[test]
    fn install_completions_value_is_optional() {
        let cli = Cli::try_parse_from(["termide", "--install-completions"]).unwrap();
        assert_eq!(cli.install_completions, Some(None));
        let cli = Cli::try_parse_from(["termide", "--install-completions", "fish"]).unwrap();
        assert_eq!(cli.install_completions, Some(Some("fish".to_string())));
        assert!(Cli::try_parse_from(["termide", "--install-completions", "tcsh"]).is_err());
        let cli =
            Cli::try_parse_from(["termide", "--install-completions", "--", "notes.md"]).unwrap();
        assert_eq!(cli.install_completions, Some(None));
        assert_eq!(cli.files, vec![std::path::PathBuf::from("notes.md")]);
    }

    /// The completion scripts spell out option names by hand, so a new clap
    /// option has to be added to each of them; this is what notices when it
    /// is not.
    #[test]
    fn completion_scripts_cover_every_option() {
        let mut cmd = Cli::command();
        cmd.build();
        let longs: Vec<String> = cmd
            .get_arguments()
            .filter_map(|arg| arg.get_long())
            .map(|long| format!("--{long}"))
            .collect();
        assert!(
            longs.contains(&"--help".to_string()),
            "clap's built-ins must be part of the check"
        );

        for (shell, script) in SCRIPTS {
            // bash and zsh name options as `--long`; fish declares them as
            // `-l long`.
            let spelled = |long: &str| match shell {
                "fish" => format!("-l {}", long.trim_start_matches("--")),
                _ => long.to_string(),
            };
            let missing: Vec<&str> = longs
                .iter()
                .filter(|long| !script.contains(&spelled(long)))
                .map(String::as_str)
                .collect();
            assert!(missing.is_empty(), "{shell} completion lacks {missing:?}");
        }
    }

    /// Every script accepts the shells `--completions` accepts, and each one
    /// asks termide for the instance list rather than guessing ids.
    #[test]
    fn completion_scripts_agree_with_the_cli() {
        for (shell, script) in SCRIPTS {
            for name in completions::SHELLS {
                assert!(
                    script.contains(name),
                    "{shell} completion does not offer {name} for --completions"
                );
            }
            assert!(
                script.contains("--list-instances 2>/dev/null"),
                "{shell} completion does not query --list-instances"
            );
            assert_eq!(completions::script(shell), script);
        }
    }
}
