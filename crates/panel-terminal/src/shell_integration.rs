//! Shell integration for the user's terminals: bash, zsh and fish are
//! started so that they read the user's own startup files as they would
//! anyway, then report each prompt and command with OSC 133 marks (`A` a
//! prompt starts, `C` a command's output starts, `D;code` it ended). Only
//! the marks are added: no option, binding or variable of the user's
//! changes. Other shells start untouched.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use portable_pty::CommandBuilder;

/// The `terminal.shell_integration` setting, published by the app.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Whether the terminals opened from now on get shell integration.
pub fn set_shell_integration(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// Read in place of `~/.bashrc`, which it reads first, as an interactive
/// bash would. Every hook keeps `$?` for what runs after it. The output
/// start comes from bash-preexec when the user has it, else from `PS0`
/// (bash 4.4 and later), which also notes that a command ran (an empty
/// line runs none and shows no `PS0`), else from a `DEBUG` trap when the
/// user has none (bash 3.2, the one macOS ships). The end comes first at
/// each prompt, the prompt mark last.
const BASH_RC: &str = r#"# termide: shell integration for a terminal panel.
if [ -f ~/.bashrc ]; then . ~/.bashrc; fi
if [ -z "$__termide_marks" ]; then
__termide_marks=1
__termide_ran=
__termide_ready=
__termide_preexec() {
  [ -n "$__termide_ready" ] || return 0
  [ "$BASH_COMMAND" = __termide_precmd ] && return 0
  __termide_ready=
  __termide_ran=1
  printf '\033]133;C\007'
}
__termide_precmd() {
  local code=$?
  __termide_ready=
  if [ -n "$__termide_ran" ]; then printf '\033]133;D;%s\007' "$code"; fi
  __termide_ran=
  return $code
}
__termide_prompt() {
  local code=$?
  printf '\033]133;A\007'
  __termide_ready=1
  return $code
}
if [ -n "$bash_preexec_imported" ] || [ -n "$__bp_imported" ]; then
  preexec_functions+=(__termide_preexec)
  precmd_functions=(__termide_precmd "${precmd_functions[@]}" __termide_prompt)
else
  if [ "${BASH_VERSINFO[0]}${BASH_VERSINFO[1]}" -ge 44 ] 2>/dev/null; then
    PS0=$'\033]133;C\007''${__termide_none[__termide_ran=1]}'"$PS0"
  elif [ -z "$(trap -p DEBUG)" ]; then
    trap '__termide_preexec' DEBUG
  fi
  if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
    PROMPT_COMMAND=(__termide_precmd "${PROMPT_COMMAND[@]}" __termide_prompt)
  else
    PROMPT_COMMAND="__termide_precmd${PROMPT_COMMAND:+;$PROMPT_COMMAND};__termide_prompt"
  fi
fi
fi
"#;

/// zsh reads these in place of the user's: each reads the user's own from
/// where the user's `ZDOTDIR` points (`TERMIDE_USER_ZDOTDIR`, the home
/// when unset) — which one of them may move — and points `ZDOTDIR` back
/// here for zsh to read the next. `.zshrc` leaves it the user's, so zsh
/// reads the user's `.zlogin` itself and shells started later find the
/// user's files.
const ZSH_ENV: &str = r#"# termide: shell integration for a terminal panel.
__termide_dotdir=$ZDOTDIR
[[ -n $TERMIDE_USER_ZDOTDIR ]] || __termide_no_zdotdir=1
ZDOTDIR=${TERMIDE_USER_ZDOTDIR:-$HOME}
[[ -f $ZDOTDIR/.zshenv ]] && . $ZDOTDIR/.zshenv
TERMIDE_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$__termide_dotdir
"#;

const ZSH_PROFILE: &str = r#"# termide: shell integration for a terminal panel.
ZDOTDIR=$TERMIDE_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zprofile ]] && . $ZDOTDIR/.zprofile
TERMIDE_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$__termide_dotdir
"#;

/// After the user's `.zshrc`: `preexec` marks the output start, the first
/// `precmd` the end, and the last puts the prompt mark at the head of the
/// prompt — again whenever a theme rebuilt it — so that it is drawn with
/// the prompt, after zsh's mark of a last line left open, and again when
/// the prompt is redrawn.
const ZSH_RC: &str = r#"# termide: shell integration for a terminal panel.
ZDOTDIR=$TERMIDE_USER_ZDOTDIR
[[ -f $ZDOTDIR/.zshrc ]] && . $ZDOTDIR/.zshrc
if [[ -n $__termide_no_zdotdir && $ZDOTDIR == $HOME ]]; then unset ZDOTDIR; fi
unset __termide_dotdir __termide_no_zdotdir TERMIDE_USER_ZDOTDIR
if [[ -z $__termide_marks ]]; then
  __termide_marks=1
  __termide_ran=
  __termide_preexec() {
    __termide_ran=1
    printf '\033]133;C\007'
  }
  __termide_precmd() {
    local code=$?
    if [[ -n $__termide_ran ]]; then printf '\033]133;D;%s\007' $code; fi
    __termide_ran=
    return $code
  }
  __termide_prompt() {
    local code=$?
    [[ $PS1 == *$'\033]133;A\007'* ]] || PS1=$'%{\033]133;A\007%}'$PS1
    return $code
  }
  preexec_functions+=(__termide_preexec)
  precmd_functions=(__termide_precmd $precmd_functions __termide_prompt)
fi
"#;

/// fish runs it after the user's configuration. fish 4 marks its prompts
/// and commands itself; an older one is given the events that do.
const FISH_INIT: &str = r#"# termide: shell integration for a terminal panel.
if not set -q __termide_marks; and string match -qr '^[0-3]\.' -- $version
  set -g __termide_marks 1
  function __termide_preexec --on-event fish_preexec
    set -g __termide_ran 1
    printf '\e]133;C\a'
  end
  function __termide_postexec --on-event fish_postexec
    set -l code $status
    if set -q __termide_ran
      set -e __termide_ran
      printf '\e]133;D;%s\a' $code
    end
  end
  function __termide_prompt --on-event fish_prompt
    printf '\e]133;A\a'
  end
end
"#;

/// The directory of the scripts, written once per process: one of a name
/// no one can guess, that only the user can write to, since a shell runs
/// what is in it.
fn init_dir() -> std::io::Result<&'static Path> {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    if let Some(dir) = DIR.get() {
        return Ok(dir);
    }
    let dir = tempfile::Builder::new()
        .prefix("termide-shell-")
        .tempdir()?
        .keep();
    for (name, text) in [
        ("bashrc", BASH_RC),
        (".zshenv", ZSH_ENV),
        (".zprofile", ZSH_PROFILE),
        (".zshrc", ZSH_RC),
        ("init.fish", FISH_INIT),
    ] {
        std::fs::write(dir.join(name), text)?;
    }
    Ok(DIR.get_or_init(|| dir))
}

/// Set `cmd`, which starts the shell `shell_path`, up for shell
/// integration when it is on and the shell is one that takes it; `env` is
/// what the caller gave the shell's environment, already in `cmd`.
pub(crate) fn integrate(cmd: &mut CommandBuilder, shell_path: &str, env: &[(String, String)]) {
    if !cfg!(unix) || !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let name = crate::shell_utils::shell_name(shell_path);
    if !matches!(name.as_str(), "bash" | "zsh" | "fish") {
        return;
    }
    let dir = match init_dir() {
        Ok(dir) => dir,
        Err(e) => {
            log::warn!("shell integration unavailable: {e}");
            return;
        }
    };
    match name.as_str() {
        "bash" => {
            cmd.arg("--rcfile");
            cmd.arg(dir.join("bashrc"));
        }
        "zsh" => {
            let user = env
                .iter()
                .rev()
                .find(|(key, _)| key == "ZDOTDIR")
                .map(|(_, value)| value.clone())
                .or_else(|| std::env::var("ZDOTDIR").ok());
            match user {
                Some(user) => cmd.env("TERMIDE_USER_ZDOTDIR", user),
                None => cmd.env_remove("TERMIDE_USER_ZDOTDIR"),
            }
            cmd.env("ZDOTDIR", dir);
        }
        _ => {
            let init =
                crate::shell_utils::ShellKind::Fish.quote(&dir.join("init.fish").to_string_lossy());
            cmd.arg("--init-command");
            cmd.arg(format!("source {init}"));
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::Path;
    use std::time::{Duration, Instant};

    use crate::terminal::marks::ShellCommand;
    use crate::Terminal;

    /// A terminal of `program` in a scratch home with `files` written there;
    /// `None` when the shell is not installed.
    fn shell_in(program: &str, home: &Path, files: &[(&str, &str)]) -> Option<Terminal> {
        if std::process::Command::new(program)
            .args(["-c", "true"])
            .status()
            .is_err()
        {
            return None;
        }
        for (name, text) in files {
            let path = home.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let home_s = home.display().to_string();
        let env = [
            ("HOME".to_string(), home_s.clone()),
            ("ZDOTDIR".to_string(), home_s.clone()),
            ("XDG_CONFIG_HOME".to_string(), format!("{home_s}/.config")),
            (
                "XDG_DATA_HOME".to_string(),
                format!("{home_s}/.local/share"),
            ),
        ];
        Terminal::spawn_shell(24, 100, program, Some(home.to_path_buf()), &env).ok()
    }

    fn commands(term: &Terminal) -> Vec<ShellCommand> {
        term.read_screen().marks.commands().copied().collect()
    }

    /// Wait until the terminal's commands pass `test`, or fail with them.
    fn wait_for(term: &Terminal, what: &str, test: impl Fn(&[ShellCommand]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let seen = commands(term);
            if test(&seen) {
                return;
            }
            assert!(Instant::now() < deadline, "{what}: {seen:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn screen_text(term: &Terminal) -> String {
        crate::clipboard::tail_text(&term.screen, 100).0
    }

    /// Each shell reads the user's startup files, keeps the user's `$?`
    /// for the prompt, and marks a prompt, a command with its output and
    /// exit code, and the next prompt; an empty line runs no command.
    #[test]
    fn every_shell_marks_its_prompts_and_commands() {
        for (program, files) in [
            ("/bin/bash", &[(".bashrc", "RC=ran\nPS1='[$?] $ '\n")][..]),
            ("bash", &[(".bashrc", "RC=ran\nPS1='[$?] $ '\n")][..]),
            (
                "zsh",
                &[
                    (".zshenv", "ENV=ran\n"),
                    (".zprofile", "PROFILE=ran\n"),
                    (".zshrc", "RC=ran\nPS1='[%?] $ '\n"),
                ][..],
            ),
            (
                "fish",
                &[(
                    ".config/fish/config.fish",
                    "set -g RC ran\nfunction fish_prompt; printf '[%s] $ ' $status; end\n",
                )][..],
            ),
        ] {
            let home = tempfile::tempdir().unwrap();
            let Some(mut term) = shell_in(program, home.path(), files) else {
                continue;
            };
            wait_for(&term, program, |c| c.len() == 1 && c[0].is_pending());
            term.send_command("").unwrap();
            wait_for(&term, program, |c| c.len() == 1 && c[0].prompt > 0);
            term.send_command("echo \"$RC $ENV $PROFILE\"; false")
                .unwrap();
            wait_for(&term, program, |c| c.len() == 2 && c[0].end.is_some());
            let c = commands(&term);
            assert_eq!(c[0].exit, Some(1), "{program}");
            assert_eq!(c[0].output, Some(c[0].prompt + 1), "{program}");
            assert_eq!(c[0].end, Some(c[0].prompt + 2), "{program}");
            assert_eq!(c[1].prompt, c[0].prompt + 2, "{program}");
            let text = screen_text(&term);
            assert!(
                text.lines().any(|l| l.starts_with("ran")),
                "{program}: {text}"
            );
            assert!(text.ends_with("[1] $"), "{program}: {text}");
            if program == "zsh" {
                assert!(text.contains("\nran ran ran\n"), "{program}: {text}");
            }
        }
    }
}
