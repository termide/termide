//! A local Chrome (or another Chromium) launched for the agent: finding the
//! executable, starting it on a profile of its own with the DevTools protocol
//! on a pipe, and the handful of page operations the web tools need.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use termide_agent_core::CancelToken;

use crate::cdp::Cdp;

/// A protocol call that should answer at once.
const QUICK: Duration = Duration::from_secs(10);
/// How long a page may take to load before what it has is taken.
pub const LOAD_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a loaded page may keep changing before it is taken as it is.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a document may stay `interactive` before it counts as loaded.
const INTERACTIVE_GRACE: Duration = Duration::from_secs(5);
/// How long a loaded page without any text may take to render some.
const EMPTY_GRACE: Duration = Duration::from_secs(10);
/// A navigation answers once the response starts; a slow server or a cold
/// browser can take a while.
const NAVIGATE_TIMEOUT: Duration = Duration::from_secs(30);
/// Interval of the readiness polls.
const POLL: Duration = Duration::from_millis(250);

/// Look for a Chrome-family browser in the usual places. The first match in
/// this order wins: Chrome, Chromium, then other Chromium browsers.
#[must_use]
pub fn find_chrome() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let apps = [
            ("Google Chrome", "Google Chrome"),
            ("Chromium", "Chromium"),
            ("Microsoft Edge", "Microsoft Edge"),
            ("Brave Browser", "Brave Browser"),
        ];
        let mut roots = vec![PathBuf::from("/Applications")];
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(Path::new(&home).join("Applications"));
        }
        for (app, binary) in apps {
            for root in &roots {
                let path = root.join(format!("{app}.app/Contents/MacOS/{binary}"));
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "macos"))]
    {
        let names = [
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "brave-browser",
        ];
        let path = std::env::var_os("PATH")?;
        names.iter().find_map(|name| {
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|candidate| candidate.is_file())
        })
    }
}

/// The user agent the same browser sends with a window. Headless Chrome
/// differs from it by one word, `HeadlessChrome` for `Chrome`; everything
/// else in the string is frozen by Chrome's user-agent reduction (the
/// platform token per OS, the version as `<major>.0.0.0`), so only the
/// major version has to be learned, from `--version`. `None` when it cannot
/// be (the browser then sends its own).
fn windowed_user_agent(executable: &Path) -> Option<String> {
    let output = std::process::Command::new(executable)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let major = major_version(&String::from_utf8_lossy(&output.stdout))?;
    Some(reduced_user_agent(major))
}

/// The major version in `--version` output such as `Google Chrome
/// 154.0.8037.58` or `Chromium 154.0.8037.58 built on Debian`.
fn major_version(text: &str) -> Option<u32> {
    text.split_whitespace()
        .find(|word| word.contains('.') && word.starts_with(|c: char| c.is_ascii_digit()))?
        .split('.')
        .next()?
        .parse()
        .ok()
}

fn reduced_user_agent(major: u32) -> String {
    let platform = if cfg!(target_os = "macos") {
        "Macintosh; Intel Mac OS X 10_15_7"
    } else if cfg!(target_os = "windows") {
        "Windows NT 10.0; Win64; x64"
    } else {
        "X11; Linux x86_64"
    };
    format!(
        "Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36"
    )
}

/// Whether another browser process holds `profile`. Chrome leaves a
/// `SingletonLock` symlink whose target ends in `-<pid>`; a live pid means a
/// second launch would hand its work to that process and exit.
#[must_use]
pub fn profile_in_use(profile: &Path) -> bool {
    let Ok(target) = std::fs::read_link(profile.join("SingletonLock")) else {
        return false;
    };
    let target = target.to_string_lossy();
    let Some(pid) = target
        .rsplit('-')
        .next()
        .and_then(|pid| pid.parse::<i32>().ok())
    else {
        return false;
    };
    process_alive(pid)
}

#[cfg(unix)]
fn process_alive(pid: i32) -> bool {
    // Signal 0 checks existence; EPERM still means the process is there.
    // 0 and negative pids name process groups, not one browser.
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill(2) takes plain integers and touches no memory of ours.
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    alive || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn process_alive(_pid: i32) -> bool {
    false
}

/// How the browser shows itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    /// No window at all, presenting the user agent the same browser has with
    /// one. A challenge relaunches the browser with a window for the user.
    Headless,
    /// A real window, minimized as soon as a tab opens, restored when the
    /// user has to step in.
    Minimized,
    /// A real window left on screen.
    Visible,
}

pub struct Browser {
    child: Child,
    cdp: Cdp,
    display: Display,
    /// A throwaway profile to delete on close, when the agent's own was busy.
    temp_profile: Option<PathBuf>,
    /// With a visible window, the one tab every page opens in, kept on the
    /// last page so the user can see what the agent read: target and session.
    watched: Mutex<Option<(String, String)>>,
}

impl Browser {
    /// Start `executable` on `profile`. When the profile is held by another
    /// browser, a throwaway profile is used instead and deleted on close.
    pub fn launch(executable: &Path, profile: &Path, display: Display) -> Result<Self, String> {
        let (profile, temp_profile) = if profile_in_use(profile) {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos());
            let temp = std::env::temp_dir()
                .join(format!("termide-browser-{}-{stamp}", std::process::id()));
            log::info!(
                "browser profile {} is in use; using {}",
                profile.display(),
                temp.display()
            );
            (temp.clone(), Some(temp))
        } else {
            (profile.to_path_buf(), None)
        };
        std::fs::create_dir_all(&profile)
            .map_err(|error| format!("cannot create {}: {error}", profile.display()))?;

        let mut args = vec![
            "--remote-debugging-pipe".to_string(),
            format!("--user-data-dir={}", profile.display()),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--window-size=1280,900".into(),
        ];
        // Keep the agent's browser away from the user's keychain: Chrome
        // otherwise encrypts cookies with a key it stores there, prompting
        // for access (or to create a keychain) and binding the agent's
        // profile to the user's secrets. A fixed key still keeps cookies.
        if cfg!(target_os = "macos") {
            args.push("--use-mock-keychain".into());
        } else if cfg!(target_os = "linux") {
            args.push("--password-store=basic".into());
        }
        if display == Display::Headless {
            if let Some(user_agent) = windowed_user_agent(executable) {
                args.push(format!("--user-agent={user_agent}"));
            }
        }
        match display {
            Display::Headless => args.extend(["--headless".into(), "about:blank".into()]),
            // Tabs open in windows of their own, created minimized in `open`.
            Display::Minimized => args.push("--no-startup-window".into()),
            Display::Visible => args.push("about:blank".into()),
        }

        let (child, cdp) = spawn_with_pipe(executable, &args)?;
        let browser = Self {
            child,
            cdp,
            display,
            temp_profile,
            watched: Mutex::new(None),
        };
        browser
            .cdp
            .call("Browser.getVersion", json!({}), None, QUICK, None)
            .map_err(|error| format!("{} did not start: {error}", executable.display()))?;
        Ok(browser)
    }

    #[must_use]
    pub fn display(&self) -> Display {
        self.display
    }

    /// Whether the browser is still there to talk to.
    pub fn is_alive(&mut self) -> bool {
        !self.cdp.is_closed() && matches!(self.child.try_wait(), Ok(None))
    }

    /// Open `url` in a new tab and wait until it has loaded and settled.
    /// Whatever arrived by the deadline is kept: a page that never goes quiet
    /// is still worth reading.
    pub fn open(&self, url: &str, cancel: &CancelToken) -> Result<Page<'_>, String> {
        let page = self.navigate(url, cancel)?;
        page.wait_until_settled(cancel)?;
        Ok(page)
    }

    /// Open `url` in a new tab and wait until the page-side expression
    /// `ready` is true, up to `limit`. For pages whose loading never
    /// finishes but whose content is known by its markup.
    pub fn open_until(
        &self,
        url: &str,
        ready: &str,
        limit: Duration,
        cancel: &CancelToken,
    ) -> Result<Page<'_>, String> {
        let page = self.navigate(url, cancel)?;
        page.wait_for(ready, limit, cancel);
        if cancel.is_cancelled() {
            return Err("cancelled".into());
        }
        Ok(page)
    }

    fn navigate(&self, url: &str, cancel: &CancelToken) -> Result<Page<'_>, String> {
        if self.display != Display::Visible {
            let page = self.new_tab(cancel)?;
            page.go(url, cancel)?;
            return Ok(page);
        }
        let page = self.watched_tab(cancel)?;
        match page.go(url, cancel) {
            // The user closed the watched tab: open the page in a new one.
            Err(_) if !cancel.is_cancelled() && !self.target_exists(&page.target_id, cancel) => {
                *self.watched.lock().unwrap_or_else(PoisonError::into_inner) = None;
                let page = self.watched_tab(cancel)?;
                page.go(url, cancel)?;
                Ok(page)
            }
            result => result.map(|()| page),
        }
    }

    fn target_exists(&self, target_id: &str, cancel: &CancelToken) -> bool {
        self.cdp
            .call("Target.getTargets", json!({}), None, QUICK, Some(cancel))
            .map(|targets| {
                targets["targetInfos"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|info| info["targetId"] == target_id)
            })
            .unwrap_or(false)
    }

    /// A fresh tab, closed when the page is dropped.
    fn new_tab(&self, cancel: &CancelToken) -> Result<Page<'_>, String> {
        let target = self.cdp.call(
            "Target.createTarget",
            json!({
                "url": "about:blank",
                "newWindow": self.display == Display::Minimized,
                "background": self.display == Display::Minimized,
            }),
            None,
            QUICK,
            Some(cancel),
        )?;
        let target_id = target["targetId"]
            .as_str()
            .ok_or("no target id")?
            .to_string();
        let attached = self.attach(&target_id, cancel);
        // From here on the page closes its tab when dropped, on every path.
        let mut page = Page {
            browser: self,
            target_id,
            session_id: String::new(),
            keep: false,
        };
        page.session_id = attached?;
        if self.display == Display::Minimized {
            page.set_window_state("minimized", cancel)?;
        }
        Ok(page)
    }

    /// The tab of a visible window that every page opens in and that stays:
    /// the one the browser started with, or a new one once that is gone.
    fn watched_tab(&self, cancel: &CancelToken) -> Result<Page<'_>, String> {
        let mut watched = self.watched.lock().unwrap_or_else(PoisonError::into_inner);
        if watched.is_none() {
            let targets =
                self.cdp
                    .call("Target.getTargets", json!({}), None, QUICK, Some(cancel))?;
            let existing = targets["targetInfos"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|info| info["type"] == "page")
                .and_then(|info| info["targetId"].as_str())
                .map(str::to_string);
            let target_id = match existing {
                Some(id) => id,
                None => self.cdp.call(
                    "Target.createTarget",
                    json!({ "url": "about:blank" }),
                    None,
                    QUICK,
                    Some(cancel),
                )?["targetId"]
                    .as_str()
                    .ok_or("no target id")?
                    .to_string(),
            };
            let session_id = self.attach(&target_id, cancel)?;
            *watched = Some((target_id, session_id));
        }
        let (target_id, session_id) = watched.clone().expect("set above");
        Ok(Page {
            browser: self,
            target_id,
            session_id,
            keep: true,
        })
    }

    fn attach(&self, target_id: &str, cancel: &CancelToken) -> Result<String, String> {
        let attached = self.cdp.call(
            "Target.attachToTarget",
            json!({ "targetId": target_id, "flatten": true }),
            None,
            QUICK,
            Some(cancel),
        )?;
        attached["sessionId"]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| "no session id".to_string())
    }

    fn set_window_state(
        &self,
        target_id: &str,
        state: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let window = self.cdp.call(
            "Browser.getWindowForTarget",
            json!({ "targetId": target_id }),
            None,
            QUICK,
            Some(cancel),
        )?;
        self.cdp
            .call(
                "Browser.setWindowBounds",
                json!({ "windowId": window["windowId"], "bounds": { "windowState": state } }),
                None,
                QUICK,
                Some(cancel),
            )
            .map(drop)
    }

    /// Ask the browser to quit, so the profile is written out, then make sure.
    pub fn close(mut self) {
        let _ = self.cdp.call(
            "Browser.close",
            json!({}),
            None,
            Duration::from_secs(3),
            None,
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // Drop does the rest.
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(temp) = &self.temp_profile {
            let _ = std::fs::remove_dir_all(temp);
        }
    }
}

/// A tab opened by [`Browser::open`]; closed when dropped, except the
/// watched tab of a visible window.
pub struct Page<'a> {
    browser: &'a Browser,
    target_id: String,
    session_id: String,
    /// The watched tab of a visible window: left open when dropped.
    keep: bool,
}

impl Page<'_> {
    /// Load `url` in this tab; waiting for it is the caller's business.
    fn go(&self, url: &str, cancel: &CancelToken) -> Result<(), String> {
        let navigated = self.call(
            "Page.navigate",
            json!({ "url": url }),
            NAVIGATE_TIMEOUT,
            cancel,
        )?;
        match navigated["errorText"].as_str().filter(|e| !e.is_empty()) {
            Some(error) => Err(format!("cannot open {url}: {error}")),
            None => Ok(()),
        }
    }

    fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancelToken,
    ) -> Result<Value, String> {
        self.browser.cdp.call(
            method,
            params,
            Some(&self.session_id),
            timeout,
            Some(cancel),
        )
    }

    /// Evaluate `expression` in the page and return its value as JSON.
    pub fn eval(&self, expression: &str, cancel: &CancelToken) -> Result<Value, String> {
        let reply = self.call(
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            QUICK,
            cancel,
        )?;
        if let Some(details) = reply.get("exceptionDetails") {
            let text = details["exception"]["description"]
                .as_str()
                .or_else(|| details["text"].as_str())
                .unwrap_or("script error");
            return Err(text.to_string());
        }
        Ok(reply["result"]["value"].clone())
    }

    /// Poll until the document has loaded, shows text, and the length of
    /// that text has held still for two polls (late scripts filling the page
    /// in). A document still `interactive` after [`INTERACTIVE_GRACE`] counts
    /// as loaded: some pages keep a request open forever. A loaded page with
    /// no text yet is an application still fetching its content, given up to
    /// [`EMPTY_GRACE`] to render. Loading may take up to [`LOAD_TIMEOUT`];
    /// settling after the text appears at most [`SETTLE_TIMEOUT`].
    fn wait_until_settled(&self, cancel: &CancelToken) -> Result<(), String> {
        let started = Instant::now();
        let mut deadline = started + LOAD_TIMEOUT;
        let mut loaded_at: Option<Instant> = None;
        let mut settling = false;
        let mut last_length = None;
        let mut steady = 0;
        while Instant::now() < deadline {
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            // Mid-navigation the context can vanish under the call; that is
            // just "not ready yet".
            let state = self
                .eval(
                    "[document.readyState, document.body ? document.body.innerText.length : 0]",
                    cancel,
                )
                .unwrap_or(Value::Null);
            let ready = state[0] == "complete"
                || (state[0] == "interactive" && started.elapsed() >= INTERACTIVE_GRACE);
            let length = state[1].as_u64().unwrap_or(0);
            if ready {
                let loaded = *loaded_at.get_or_insert_with(Instant::now);
                if length == 0 && loaded.elapsed() < EMPTY_GRACE {
                    std::thread::sleep(POLL);
                    continue;
                }
                if !settling {
                    settling = true;
                    deadline = deadline.min(Instant::now() + SETTLE_TIMEOUT);
                }
                if Some(length) == last_length {
                    steady += 1;
                    if steady >= 2 {
                        return Ok(());
                    }
                } else {
                    steady = 0;
                    last_length = Some(length);
                }
            }
            std::thread::sleep(POLL);
        }
        Ok(())
    }

    /// Poll the page-side expression `ready` until it is true or `limit`
    /// passes; returns whether it came true.
    pub fn wait_for(&self, ready: &str, limit: Duration, cancel: &CancelToken) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline && !cancel.is_cancelled() {
            if self.eval(ready, cancel).ok().and_then(|v| v.as_bool()) == Some(true) {
                return true;
            }
            std::thread::sleep(POLL);
        }
        false
    }

    /// Poll the page-side expression `measure` until it gives the same value
    /// three times in a row (a list that has stopped growing) or `limit`
    /// passes.
    pub fn wait_until_steady(&self, measure: &str, limit: Duration, cancel: &CancelToken) {
        let deadline = Instant::now() + limit;
        let mut last = None;
        let mut steady = 0;
        while Instant::now() < deadline && !cancel.is_cancelled() {
            let value = self.eval(measure, cancel).ok();
            if value.is_some() && value == last {
                steady += 1;
                if steady >= 2 {
                    return;
                }
            } else {
                steady = 0;
                last = value;
            }
            std::thread::sleep(POLL);
        }
    }

    /// Wait while `blocked` holds, up to `limit`. Returns whether the page
    /// got through.
    pub fn wait_while(
        &self,
        limit: Duration,
        cancel: &CancelToken,
        mut blocked: impl FnMut(&Self) -> bool,
    ) -> Result<bool, String> {
        let deadline = Instant::now() + limit;
        while blocked(self) {
            if Instant::now() >= deadline {
                return Ok(false);
            }
            if cancel.is_cancelled() {
                return Err("cancelled".into());
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        Ok(true)
    }

    /// Bring the tab to the front, so a visible window shows it.
    pub fn bring_to_front(&self, cancel: &CancelToken) {
        let _ = self.call("Page.bringToFront", json!({}), QUICK, cancel);
    }

    /// Set the state of the window holding this tab: `normal`, `minimized`,
    /// `maximized` or `fullscreen`. A headless browser has no window to set.
    pub fn set_window_state(&self, state: &str, cancel: &CancelToken) -> Result<(), String> {
        self.browser
            .set_window_state(&self.target_id, state, cancel)
    }
}

impl Drop for Page<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        let _ = self.browser.cdp.call(
            "Target.closeTarget",
            json!({ "targetId": self.target_id }),
            None,
            Duration::from_secs(2),
            None,
        );
    }
}

/// Start the browser with its descriptors 3 (commands in) and 4 (replies out)
/// on two fresh pipes.
#[cfg(unix)]
fn spawn_with_pipe(executable: &Path, args: &[String]) -> Result<(Child, Cdp), String> {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    fn pipe() -> Result<(i32, i32), String> {
        let mut fds = [0; 2];
        // SAFETY: `fds` is a live array of the two ints pipe(2) writes.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(format!("pipe: {}", std::io::Error::last_os_error()));
        }
        // Close-on-exec on both ends, so no other child inherits them; the
        // browser gets its copies through dup2, which clears the flag.
        for fd in fds {
            // SAFETY: fcntl(2) on a descriptor pipe(2) just returned to us.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        Ok((fds[0], fds[1]))
    }

    let (browser_in, commands) = pipe()?;
    let (replies, browser_out) = pipe()?;

    let mut command = Command::new(executable);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the hook runs in the forked child before exec and calls only
    // fcntl and dup2, which are async-signal-safe; it allocates nothing.
    unsafe {
        command.pre_exec(move || {
            // Move both ends above 3 and 4 first, so neither dup2 clobbers
            // the other when a pipe already sits on 3 or 4.
            let high_in = libc::fcntl(browser_in, libc::F_DUPFD, 10);
            let high_out = libc::fcntl(browser_out, libc::F_DUPFD, 10);
            if high_in < 0
                || high_out < 0
                || libc::dup2(high_in, 3) < 0
                || libc::dup2(high_out, 4) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let spawned = command.spawn();
    // The browser's ends belong to the child now.
    // SAFETY: both are descriptors of ours from pipe(), closed once here.
    unsafe {
        libc::close(browser_in);
        libc::close(browser_out);
    }
    // SAFETY: our ends of the pipes, owned by nothing else; the Files take
    // them over and close them.
    let (commands, replies) = unsafe { (File::from_raw_fd(commands), File::from_raw_fd(replies)) };
    let child =
        spawned.map_err(|error| format!("cannot start {}: {error}", executable.display()))?;
    Ok((child, Cdp::new(commands, replies)))
}

#[cfg(not(unix))]
fn spawn_with_pipe(_executable: &Path, _args: &[String]) -> Result<(Child, Cdp), String> {
    Err("driving Chrome is not supported on this platform yet".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_windowed_user_agent_comes_from_the_major_version() {
        assert_eq!(major_version("Google Chrome 154.0.8037.58\n"), Some(154));
        assert_eq!(
            major_version("Chromium 139.0.7258.5 built on Debian GNU/Linux 12"),
            Some(139)
        );
        assert_eq!(major_version("Microsoft Edge 150.0.1.2 "), Some(150));
        assert_eq!(major_version("no version here"), None);
        let agent = reduced_user_agent(154);
        assert!(agent.contains(" Chrome/154.0.0.0 "), "{agent}");
        assert!(!agent.contains("Headless"));
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_singleton_lock_does_not_hold_the_profile() {
        let tmp = std::env::temp_dir().join(format!("termide-lock-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let lock = tmp.join("SingletonLock");
        let _ = std::fs::remove_file(&lock);
        assert!(!profile_in_use(&tmp));

        std::os::unix::fs::symlink(format!("host-{}", std::process::id()), &lock).unwrap();
        assert!(profile_in_use(&tmp), "our own pid is alive");

        std::fs::remove_file(&lock).unwrap();
        // Pid 0x7fff_fff0 is not a live process.
        std::os::unix::fs::symlink("host-2147483632", &lock).unwrap();
        assert!(!profile_in_use(&tmp));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
