//! Permission rules and the hook that enforces them.
//!
//! Rules live per tool as `pattern = decision` tables; among the rules that
//! match a call the strictest wins (`deny` over `ask` over `allow`). The mode
//! decides which rules count and what happens to calls none covers: only
//! `configured` takes the configured `allow` rules, every mode keeps their
//! `deny` and `ask`, and answers given "for this session" count everywhere
//! but in `all`. Reading inside the project, loading a skill and a short list
//! of read-only shell commands never prompt.
//!
//! Shell commands are split on `&&`, `||`, `;`, `|` and newlines; every part
//! must be allowed for the whole to pass, while a `deny` or `ask` on any part
//! applies to the whole. Command substitution (`$(...)`, backticks) is never
//! auto-allowed by an `allow` rule.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::{Hooks, ToolDecision};
use crate::cancel::CancelToken;
use crate::message::ToolCall;
use crate::tool::ToolContext;

/// Which rules count and what happens to calls none covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Ask about everything but project reads and read-only commands; the
    /// configured `allow` rules do not count.
    Ask,
    /// Read only, the web included: the agent explores and answers with a
    /// plan; every tool that could change something is refused.
    Plan,
    /// Also edit and write inside the project, and use the web, without a
    /// prompt; commands still ask. The configured `allow` rules do not count.
    #[serde(alias = "accept-edits")]
    Edit,
    /// The configured rules decide, and "allow always" adds to them; what
    /// none covers asks.
    #[default]
    Configured,
    /// Allow everything the rules do not refuse or send to a prompt.
    #[serde(alias = "auto")]
    All,
}

impl Mode {
    /// Every mode, in the order the UI lists and cycles through them.
    pub const ALL: [Mode; 5] = [
        Mode::Ask,
        Mode::Plan,
        Mode::Edit,
        Mode::Configured,
        Mode::All,
    ];

    /// The kebab-case spelling used in configuration and the status bar.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Plan => "plan",
            Mode::Edit => "edit",
            Mode::Configured => "configured",
            Mode::All => "all",
        }
    }

    /// The mode after this one, wrapping from `all` back to `ask`.
    #[must_use]
    pub fn next(self) -> Mode {
        let index = Mode::ALL.iter().position(|m| *m == self).unwrap_or(0);
        Mode::ALL[(index + 1) % Mode::ALL.len()]
    }
}

/// Why a call was refused in plan mode; the model reads it as the result.
pub const PLAN_MODE_REASON: &str =
    "plan mode: only reading is allowed; describe the change in the plan and wait for the user to leave plan mode";

/// Whether a call can change nothing: a read, a skill, or a shell command
/// made only of read-only parts without substitution.
#[must_use]
pub fn is_read_only_call(call: &ToolCall) -> bool {
    match call.name.as_str() {
        // The web tools read the web; nothing on this machine changes.
        "read" | "skill" | "fetch" | "web_search" => true,
        "bash" => {
            let parsed = split_shell(call.arguments["command"].as_str().unwrap_or(""));
            !parsed.has_substitution
                && !parsed.parts.is_empty()
                && parsed.parts.iter().all(|part| is_read_only_command(part))
        }
        _ => false,
    }
}

/// Refuses every call that could change something while the shared mode is
/// [`Mode::Plan`]. It sits first in the hook chain, ahead of command hooks,
/// so nothing — not even a hook's approval — lets a change through in plan
/// mode; in every other mode it does nothing.
pub struct PlanGuard {
    mode: ModeHandle,
}

impl PlanGuard {
    #[must_use]
    pub fn new(mode: ModeHandle) -> Self {
        Self { mode }
    }
}

impl Hooks for PlanGuard {
    fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
        if self.mode.get() == Mode::Plan && !is_read_only_call(call) {
            ToolDecision::Block {
                reason: PLAN_MODE_REASON.into(),
            }
        } else {
            ToolDecision::Allow
        }
    }
}

/// The permission mode shared between the UI and the agent thread.
///
/// Like [`crate::CancelToken`], a cloneable atomic: the panel flips it and
/// the hooks read it on the next tool call, so a switch made during a run
/// applies to that run without a channel round-trip.
#[derive(Debug, Clone)]
pub struct ModeHandle {
    mode: Arc<AtomicU8>,
}

impl ModeHandle {
    #[must_use]
    pub fn new(mode: Mode) -> Self {
        let handle = Self {
            mode: Arc::new(AtomicU8::new(0)),
        };
        handle.set(mode);
        handle
    }

    #[must_use]
    pub fn get(&self) -> Mode {
        let index = self.mode.load(Ordering::Acquire) as usize;
        Mode::ALL.get(index).copied().unwrap_or_default()
    }

    pub fn set(&self, mode: Mode) {
        let index = Mode::ALL.iter().position(|m| *m == mode).unwrap_or(0);
        self.mode.store(index as u8, Ordering::Release);
    }
}

/// Ordered from most to least permissive so `max` yields the strictest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

/// One `pattern = decision` table per tool.
pub type RuleTables = BTreeMap<String, BTreeMap<String, Decision>>;

/// `[ai.permissions]`: a mode plus one `pattern = decision` table per tool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionRules {
    #[serde(default)]
    pub mode: Mode,
    #[serde(flatten, default)]
    pub tools: RuleTables,
    /// Answers given "for this session", kept apart from the configured
    /// rules because they count in modes those do not; never written out.
    #[serde(skip)]
    pub session: RuleTables,
}

impl PermissionRules {
    pub fn add(&mut self, tool: &str, pattern: &str, decision: Decision) {
        add_rule(&mut self.tools, tool, pattern, decision);
    }

    /// Record an answer given for this session.
    pub fn add_session(&mut self, tool: &str, pattern: &str, decision: Decision) {
        add_rule(&mut self.session, tool, pattern, decision);
    }

    /// Strictest decision among the configured rules of `tool` that match
    /// `subject`.
    #[must_use]
    pub fn evaluate(&self, tool: &str, subject: &str) -> Option<Decision> {
        evaluate_rules(&self.tools, tool, subject)
    }

    /// Strictest decision among this session's answers for `tool` that match
    /// `subject`.
    #[must_use]
    pub fn evaluate_session(&self, tool: &str, subject: &str) -> Option<Decision> {
        evaluate_rules(&self.session, tool, subject)
    }
}

fn add_rule(tables: &mut RuleTables, tool: &str, pattern: &str, decision: Decision) {
    tables
        .entry(tool.to_string())
        .or_default()
        .insert(pattern.to_string(), decision);
}

fn evaluate_rules(tables: &RuleTables, tool: &str, subject: &str) -> Option<Decision> {
    tables
        .get(tool)?
        .iter()
        .filter(|(pattern, _)| wildcard_match(pattern, subject))
        .map(|(_, decision)| *decision)
        .max()
}

/// Glob-style match: `*` spans any text (slashes included), `?` one
/// character, a leading `**/` is optional so `**/.env` also matches `.env`.
#[must_use]
pub fn wildcard_match(pattern: &str, text: &str) -> bool {
    if let Some(rest) = pattern.strip_prefix("**/") {
        if wildcard_match(rest, text) {
            return true;
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            while p < pattern.len() && pattern[p] == '*' {
                p += 1;
            }
            star = Some((p, t));
        } else if let Some((sp, st)) = star {
            p = sp;
            t = st + 1;
            star = Some((sp, t));
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

/// What the user is asked about.
#[derive(Debug, Clone, PartialEq)]
pub struct PermissionRequest {
    pub tool: String,
    /// The command line or the project-relative path being acted on.
    pub subject: String,
    pub call: ToolCall,
    /// Rule pattern offered for the answers that last beyond this call.
    pub suggested_pattern: String,
    /// Whether "allow always" is on offer: only in `configured` mode, which
    /// is the one the configured rules count in, and only with somewhere to
    /// write the rule.
    pub can_persist: bool,
}

/// The answers of a permission prompt: once, for the session or always
/// (in the project's configuration or the global one), a denial for now or
/// for the session, and a denial that tells the model what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionAnswer {
    AllowOnce,
    AllowSession,
    /// Allowed, and the rule written to the project's configuration.
    AllowAlways,
    /// Allowed, and the rule written to the global configuration.
    AllowAlwaysGlobal,
    Deny,
    DenySession,
    /// Denied, with the user's words returned to the model as the reason.
    DenyWithReason(String),
}

/// Where an "allow always" rule is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistScope {
    /// The project's `.termide/config.toml`.
    Project,
    /// The global configuration.
    Global,
}

/// Blocks on the agent thread until the user answers.
pub trait PermissionPrompter: Send {
    fn ask(&mut self, request: &PermissionRequest) -> PermissionAnswer;
}

/// A prompter that answers every question with the same denial. A subagent
/// runs with no one to ask, so anything the rules and mode do not already
/// allow is refused with a reason the model reads, rather than blocking a
/// user who is not watching this nested run.
pub struct AutoDenyPrompter {
    reason: String,
}

impl AutoDenyPrompter {
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl PermissionPrompter for AutoDenyPrompter {
    fn ask(&mut self, _request: &PermissionRequest) -> PermissionAnswer {
        PermissionAnswer::DenyWithReason(self.reason.clone())
    }
}

// Permission prompts across a thread boundary: the agent thread blocks
// inside `before_tool_call` until the user answers, so the request travels
// over a channel and the wait wakes regularly to notice an abort.

/// One outstanding prompt: the request and the channel for its answer.
pub struct PermissionEnvelope {
    pub id: u64,
    pub request: PermissionRequest,
    pub reply: Sender<PermissionAnswer>,
}

pub struct ChannelPrompter {
    tx: Sender<PermissionEnvelope>,
    cancel: CancelToken,
    next_id: u64,
}

/// Build a prompter and the receiver the panel polls from `tick()`.
pub fn permission_channel(cancel: CancelToken) -> (ChannelPrompter, Receiver<PermissionEnvelope>) {
    let (tx, rx) = mpsc::channel();
    (
        ChannelPrompter {
            tx,
            cancel,
            next_id: 0,
        },
        rx,
    )
}

impl PermissionPrompter for ChannelPrompter {
    fn ask(&mut self, request: &PermissionRequest) -> PermissionAnswer {
        let (reply, answer) = mpsc::channel();
        self.next_id += 1;
        let envelope = PermissionEnvelope {
            id: self.next_id,
            request: request.clone(),
            reply,
        };
        if self.tx.send(envelope).is_err() {
            // The panel is gone; nobody can approve anything.
            return PermissionAnswer::Deny;
        }
        loop {
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) if self.cancel.is_cancelled() => {
                    return PermissionAnswer::Deny;
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return PermissionAnswer::Deny,
            }
        }
    }
}

/// Called when the user chose "allow always", so the host can persist the
/// rule to the configuration `scope` names.
pub type PersistRule = Box<dyn FnMut(&str, &str, Decision, PersistScope) + Send>;

/// [`Hooks`] implementation that evaluates rules and prompts through a
/// [`PermissionPrompter`].
pub struct PermissionHooks {
    /// The configured rules and this session's answers.
    rules: PermissionRules,
    /// The live mode; `rules.mode` is only its initial value.
    mode: ModeHandle,
    prompter: Box<dyn PermissionPrompter>,
    persist: Option<PersistRule>,
}

impl PermissionHooks {
    #[must_use]
    pub fn new(rules: PermissionRules, prompter: Box<dyn PermissionPrompter>) -> Self {
        Self {
            mode: ModeHandle::new(rules.mode),
            rules,
            prompter,
            persist: None,
        }
    }

    #[must_use]
    pub fn with_persist(mut self, persist: PersistRule) -> Self {
        self.persist = Some(persist);
        self
    }

    /// The configured rules plus any "allow always" grants, and this
    /// session's answers. The mode in them is the starting one;
    /// [`PermissionHooks::mode`] is the live one.
    #[must_use]
    pub fn rules(&self) -> &PermissionRules {
        &self.rules
    }

    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode.get()
    }

    pub fn set_mode(&self, mode: Mode) {
        self.mode.set(mode);
    }

    /// A handle the UI keeps to switch the mode while the hooks run on the
    /// agent thread.
    #[must_use]
    pub fn mode_handle(&self) -> ModeHandle {
        self.mode.clone()
    }

    /// The verdict before any prompt: the rules that count in the mode
    /// (the strictest wins), then the mode's own answer and the built-in
    /// safe defaults.
    #[must_use]
    pub fn decide(&self, call: &ToolCall, ctx: &ToolContext) -> Decision {
        let subject = subject_of(call, ctx);
        let mode = self.mode.get();
        let rule = |text: &str| {
            // The configured rules count in full only in `configured`;
            // elsewhere they can only tighten. This session's answers count
            // everywhere but in `all`, which allows what they would.
            let configured = self
                .rules
                .evaluate(&call.name, text)
                .filter(|d| mode == Mode::Configured || *d != Decision::Allow);
            let session = self
                .rules
                .evaluate_session(&call.name, text)
                .filter(|_| mode != Mode::All);
            configured.into_iter().chain(session).max()
        };

        if call.name == "bash" {
            let parsed = split_shell(&subject);
            let mut verdict = Decision::Allow;
            // Whole-command rules can tighten but never loosen a multi-part
            // command: each part must earn its own allow.
            if parsed.parts.len() > 1 {
                if let Some(whole) = rule(&subject).filter(|d| *d != Decision::Allow) {
                    verdict = verdict.max(whole);
                }
            }
            for part in &parsed.parts {
                let decision = match rule(part) {
                    Some(Decision::Allow) if parsed.has_substitution => Decision::Ask,
                    Some(decision) => decision,
                    None if mode == Mode::All => Decision::Allow,
                    None if !parsed.has_substitution && is_read_only_command(part) => {
                        Decision::Allow
                    }
                    // Plan mode refuses a command that could change something.
                    None if mode == Mode::Plan => Decision::Deny,
                    None => Decision::Ask,
                };
                verdict = verdict.max(decision);
            }
            return verdict;
        }

        if let Some(decision) = rule(&subject) {
            return decision;
        }
        let inside = inside_project(call, ctx);
        match (mode, call.name.as_str()) {
            (Mode::All, _) => Decision::Allow,
            (_, "read") if inside => Decision::Allow,
            // Loads a skill's own text, which may live outside the project.
            (_, "skill") => Decision::Allow,
            (Mode::Plan | Mode::Edit, "fetch" | "web_search") => Decision::Allow,
            (Mode::Edit, "edit" | "write") if inside => Decision::Allow,
            // Plan mode asks before reading outside the project and refuses
            // what could change something.
            (Mode::Plan, "read") => Decision::Ask,
            (Mode::Plan, _) => Decision::Deny,
            _ => Decision::Ask,
        }
    }
}

impl Hooks for PermissionHooks {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        match self.decide(call, ctx) {
            Decision::Allow => ToolDecision::Allow,
            Decision::Deny if self.mode.get() == Mode::Plan => ToolDecision::Block {
                reason: PLAN_MODE_REASON.into(),
            },
            Decision::Deny => ToolDecision::Block {
                reason: "denied by the permission rules".into(),
            },
            Decision::Ask => {
                let subject = subject_of(call, ctx);
                let can_persist = self.mode.get() == Mode::Configured && self.persist.is_some();
                let request = PermissionRequest {
                    tool: call.name.clone(),
                    suggested_pattern: suggested_pattern(&call.name, &subject),
                    subject,
                    call: call.clone(),
                    can_persist,
                };
                let persist = |hooks: &mut Self, scope: PersistScope| {
                    if !can_persist {
                        // Not on offer here: it lasts for the session.
                        hooks.rules.add_session(
                            &request.tool,
                            &request.suggested_pattern,
                            Decision::Allow,
                        );
                        return;
                    }
                    hooks
                        .rules
                        .add(&request.tool, &request.suggested_pattern, Decision::Allow);
                    if let Some(persist) = &mut hooks.persist {
                        persist(
                            &request.tool,
                            &request.suggested_pattern,
                            Decision::Allow,
                            scope,
                        );
                    }
                };
                match self.prompter.ask(&request) {
                    PermissionAnswer::AllowOnce => ToolDecision::Allow,
                    PermissionAnswer::AllowSession => {
                        self.rules.add_session(
                            &request.tool,
                            &request.suggested_pattern,
                            Decision::Allow,
                        );
                        ToolDecision::Allow
                    }
                    PermissionAnswer::AllowAlways => {
                        persist(self, PersistScope::Project);
                        ToolDecision::Allow
                    }
                    PermissionAnswer::AllowAlwaysGlobal => {
                        persist(self, PersistScope::Global);
                        ToolDecision::Allow
                    }
                    PermissionAnswer::Deny => ToolDecision::Block {
                        reason: "denied by the user".into(),
                    },
                    PermissionAnswer::DenySession => {
                        self.rules.add_session(
                            &request.tool,
                            &request.suggested_pattern,
                            Decision::Deny,
                        );
                        ToolDecision::Block {
                            reason: "denied by the user for this session".into(),
                        }
                    }
                    PermissionAnswer::DenyWithReason(reason) => ToolDecision::Block {
                        reason: format!("denied by the user: {reason}"),
                    },
                }
            }
        }
    }
}

/// The text rules are matched against: the command for `bash`, the
/// project-relative path for file tools, empty otherwise.
#[must_use]
pub fn subject_of(call: &ToolCall, ctx: &ToolContext) -> String {
    match call.name.as_str() {
        "bash" => call.arguments["command"].as_str().unwrap_or("").to_string(),
        "read" | "edit" | "write" => {
            let raw = call.arguments["path"].as_str().unwrap_or("");
            relative_to_project(raw, &ctx.cwd)
        }
        "fetch" => call.arguments["url"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string(),
        "web_search" => call.arguments["query"].as_str().unwrap_or("").to_string(),
        _ => match &call.arguments {
            Value::Object(map) => map
                .values()
                .find_map(Value::as_str)
                .unwrap_or("")
                .to_string(),
            _ => String::new(),
        },
    }
}

fn relative_to_project(raw: &str, cwd: &Path) -> String {
    let path = Path::new(raw);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let normalized = normalize(&absolute);
    match normalized.strip_prefix(normalize(cwd)) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_string_lossy().into_owned(),
        Ok(_) => ".".to_string(),
        Err(_) => normalized.to_string_lossy().into_owned(),
    }
}

/// Resolve `.` and `..` lexically; the file need not exist.
fn normalize(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn inside_project(call: &ToolCall, ctx: &ToolContext) -> bool {
    let subject = subject_of(call, ctx);
    !Path::new(&subject).is_absolute() && !subject.starts_with("..")
}

/// Pattern offered when the user allows a call for longer than once: the
/// command's leading words for `bash`, the exact path for file tools.
#[must_use]
pub fn suggested_pattern(tool: &str, subject: &str) -> String {
    match tool {
        // Trust a site, not one page of it.
        "fetch" => {
            return match subject.split_once("://") {
                Some((scheme, rest)) => {
                    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
                    format!("{scheme}://{host}/*")
                }
                None => "*".to_string(),
            };
        }
        // A query is never repeated word for word.
        "web_search" => return "*".to_string(),
        _ => {}
    }
    if tool != "bash" {
        return if subject.is_empty() {
            "*".to_string()
        } else {
            subject.to_string()
        };
    }
    let parts = split_shell(subject);
    let first = parts.parts.first().map_or(subject, |p| p.as_str());
    let mut words = first.split_whitespace();
    let Some(head) = words.next() else {
        return "*".to_string();
    };
    const SUBCOMMAND_TOOLS: [&str; 14] = [
        "git", "cargo", "npm", "pnpm", "yarn", "go", "docker", "kubectl", "make", "python", "pip",
        "uv", "brew", "gh",
    ];
    match words.next() {
        Some(sub) if SUBCOMMAND_TOOLS.contains(&head) && !sub.starts_with('-') => {
            format!("{head} {sub} *")
        }
        _ => format!("{head} *"),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedShell {
    parts: Vec<String>,
    has_substitution: bool,
}

/// Split a command line into its simple commands, honouring quotes.
fn split_shell(command: &str) -> ParsedShell {
    let mut parsed = ParsedShell::default();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if q == '"' && (c == '`' || (c == '$' && chars.get(i + 1) == Some(&'('))) {
                // Double quotes still expand substitutions.
                parsed.has_substitution = true;
            }
            current.push(c);
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                current.push(c);
            }
            '`' => {
                parsed.has_substitution = true;
                current.push(c);
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                parsed.has_substitution = true;
                current.push(c);
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                push_part(&mut parsed.parts, &mut current);
                i += 1;
            }
            '|' => {
                push_part(&mut parsed.parts, &mut current);
                if chars.get(i + 1) == Some(&'|') {
                    i += 1;
                }
            }
            ';' | '\n' => push_part(&mut parsed.parts, &mut current),
            _ => current.push(c),
        }
        i += 1;
    }
    push_part(&mut parsed.parts, &mut current);
    if parsed.parts.is_empty() {
        parsed.parts.push(command.trim().to_string());
    }
    parsed
}

fn push_part(parts: &mut Vec<String>, current: &mut String) {
    let part = current.trim().to_string();
    if !part.is_empty() {
        parts.push(part);
    }
    current.clear();
}

/// Commands that only read state and cannot write files even with unusual
/// flags; redirections disqualify a command.
#[must_use]
pub fn is_read_only_command(part: &str) -> bool {
    if part.contains('>') || part.contains("<(") {
        return false;
    }
    let mut words = part.split_whitespace();
    let Some(head) = words.next() else {
        return false;
    };
    const PLAIN: [&str; 30] = [
        "ls", "cat", "head", "tail", "wc", "pwd", "echo", "rg", "grep", "egrep", "fgrep", "which",
        "file", "stat", "tree", "du", "sort", "uniq", "cut", "tr", "basename", "dirname",
        "realpath", "env", "printenv", "date", "whoami", "uname", "true", "false",
    ];
    if PLAIN.contains(&head) {
        return true;
    }
    match head {
        "git" => matches!(
            words.next(),
            Some(
                "status"
                    | "diff"
                    | "log"
                    | "show"
                    | "branch"
                    | "blame"
                    | "remote"
                    | "rev-parse"
                    | "ls-files"
            )
        ),
        "find" => !part.contains("-delete") && !part.contains("-exec") && !part.contains("-ok"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn ctx() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("/proj"),
        }
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
        }
    }

    fn bash(command: &str) -> ToolCall {
        call("bash", json!({ "command": command }))
    }

    fn rules(toml_text: &str) -> PermissionRules {
        toml::from_str(toml_text).unwrap()
    }

    /// Records requests and replays scripted answers.
    struct Scripted {
        answers: Vec<PermissionAnswer>,
        asked: Arc<Mutex<Vec<PermissionRequest>>>,
    }

    impl PermissionPrompter for Scripted {
        fn ask(&mut self, request: &PermissionRequest) -> PermissionAnswer {
            self.asked.lock().unwrap().push(request.clone());
            if self.answers.is_empty() {
                PermissionAnswer::Deny
            } else {
                self.answers.remove(0)
            }
        }
    }

    fn hooks(
        rules: PermissionRules,
        answers: Vec<PermissionAnswer>,
    ) -> (PermissionHooks, Arc<Mutex<Vec<PermissionRequest>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let hooks = PermissionHooks::new(
            rules,
            Box::new(Scripted {
                answers,
                asked: asked.clone(),
            }),
        );
        (hooks, asked)
    }

    #[test]
    fn toml_shape_round_trips() {
        let rules = rules(
            r#"
            mode = "edit"
            [bash]
            "git status*" = "allow"
            "git push*" = "ask"
            "rm -rf *" = "deny"
            [edit]
            "src/**" = "allow"
            ".env" = "deny"
            "#,
        );
        assert_eq!(rules.mode, Mode::Edit);
        assert_eq!(
            rules.evaluate("bash", "git push origin main"),
            Some(Decision::Ask)
        );
        assert_eq!(rules.evaluate("edit", "src/lib.rs"), Some(Decision::Allow));
        assert_eq!(rules.evaluate("edit", "README.md"), None);
        let text = toml::to_string(&rules).unwrap();
        assert_eq!(toml::from_str::<PermissionRules>(&text).unwrap(), rules);
        assert_eq!(PermissionRules::default().mode, Mode::Configured);
        // The earlier names still read.
        assert_eq!(rules_of_mode("accept-edits"), Mode::Edit);
        assert_eq!(rules_of_mode("auto"), Mode::All);
    }

    fn rules_of_mode(mode: &str) -> Mode {
        rules(&format!("mode = \"{mode}\"")).mode
    }

    #[test]
    fn strictest_matching_rule_wins() {
        let mut rules = PermissionRules::default();
        rules.add("bash", "git *", Decision::Allow);
        rules.add("bash", "git push*", Decision::Ask);
        rules.add("bash", "*--force*", Decision::Deny);
        assert_eq!(rules.evaluate("bash", "git status"), Some(Decision::Allow));
        assert_eq!(
            rules.evaluate("bash", "git push origin"),
            Some(Decision::Ask)
        );
        assert_eq!(
            rules.evaluate("bash", "git push --force"),
            Some(Decision::Deny)
        );
    }

    #[test]
    fn wildcard_semantics() {
        assert!(wildcard_match("src/**", "src/a/b.rs"));
        assert!(wildcard_match("**/.env*", ".env"));
        assert!(wildcard_match("**/.env*", "config/.env.local"));
        assert!(!wildcard_match("**/.env*", "environment.rs"));
        assert!(wildcard_match("cargo *", "cargo build --release"));
        assert!(!wildcard_match("cargo *", "cargo"));
        assert!(wildcard_match("cargo*", "cargo"));
        assert!(wildcard_match("?.rs", "a.rs"));
        assert!(wildcard_match("*", ""));
    }

    /// Every mode against every kind of call, with configured rules that
    /// allow a command and an edit, and deny a secret: the table the
    /// documentation gives.
    #[test]
    fn each_mode_decides_by_its_table() {
        use Decision::{Allow, Ask, Deny};
        let read_in = call("read", json!({ "path": "src/a.rs" }));
        let read_out = call("read", json!({ "path": "/etc/passwd" }));
        let web = call("fetch", json!({ "url": "https://docs.rs/x" }));
        let edit_in = call("edit", json!({ "path": "src/a.rs" }));
        let edit_out = call("write", json!({ "path": "/tmp/x" }));
        let secret = call("edit", json!({ "path": ".env" }));
        let look = bash("git status");
        let build = bash("cargo build");
        let other = bash("make install");
        let mcp = call("fs__search", json!({ "query": "x" }));
        let calls = [
            &read_in, &read_out, &web, &edit_in, &edit_out, &secret, &look, &build, &other, &mcp,
        ];
        let table = [
            (
                Mode::Ask,
                [Allow, Ask, Ask, Ask, Ask, Deny, Allow, Ask, Ask, Ask],
            ),
            (
                Mode::Plan,
                [Allow, Ask, Allow, Deny, Deny, Deny, Allow, Deny, Deny, Deny],
            ),
            (
                Mode::Edit,
                [Allow, Ask, Allow, Allow, Ask, Deny, Allow, Ask, Ask, Ask],
            ),
            (
                Mode::Configured,
                [Allow, Ask, Ask, Allow, Ask, Deny, Allow, Allow, Ask, Ask],
            ),
            (
                Mode::All,
                [
                    Allow, Allow, Allow, Allow, Allow, Deny, Allow, Allow, Allow, Allow,
                ],
            ),
        ];
        for (mode, expected) in table {
            let mut rules = PermissionRules {
                mode,
                ..Default::default()
            };
            rules.add("bash", "cargo *", Decision::Allow);
            rules.add("edit", "src/**", Decision::Allow);
            rules.add("edit", "**/.env*", Decision::Deny);
            let (hooks, _) = hooks(rules, vec![]);
            let got: Vec<Decision> = calls.iter().map(|c| hooks.decide(c, &ctx())).collect();
            assert_eq!(got, expected, "{mode:?}");
        }
    }

    #[test]
    fn session_answers_count_in_every_mode_but_all() {
        let mut rules = PermissionRules {
            mode: Mode::Ask,
            ..Default::default()
        };
        rules.add_session("bash", "make *", Decision::Allow);
        rules.add_session("bash", "rm *", Decision::Deny);
        let (hooks, _) = hooks(rules, vec![]);
        let handle = hooks.mode_handle();
        for mode in [Mode::Ask, Mode::Edit, Mode::Configured] {
            handle.set(mode);
            assert_eq!(hooks.decide(&bash("make all"), &ctx()), Decision::Allow);
            assert_eq!(hooks.decide(&bash("rm x"), &ctx()), Decision::Deny);
        }
        // Everything is allowed in `all` anyway; a session refusal is not a
        // configured one.
        handle.set(Mode::All);
        assert_eq!(hooks.decide(&bash("rm x"), &ctx()), Decision::Allow);
    }

    #[test]
    fn shell_commands_are_split_and_each_part_judged() {
        let mut rules = PermissionRules::default();
        rules.add("bash", "cargo *", Decision::Allow);
        rules.add("bash", "git push*", Decision::Ask);
        let (hooks, _) = hooks(rules, vec![]);
        let decide = |cmd: &str| hooks.decide(&bash(cmd), &ctx());

        assert_eq!(decide("cargo build"), Decision::Allow);
        assert_eq!(decide("cargo build && cargo test"), Decision::Allow);
        assert_eq!(
            decide("cargo build && git status"),
            Decision::Allow,
            "git status is read-only"
        );
        assert_eq!(decide("cargo build && rm -rf target"), Decision::Ask);
        assert_eq!(decide("cargo build; git push"), Decision::Ask);
        assert_eq!(
            decide("cargo run -- $(cat cmd)"),
            Decision::Ask,
            "substitution"
        );
        assert_eq!(decide("cargo run -- `cat cmd`"), Decision::Ask);
        assert_eq!(
            decide("echo 'a && b'"),
            Decision::Allow,
            "quotes are not separators"
        );
        assert_eq!(
            decide("ls > out.txt"),
            Decision::Ask,
            "redirection is a write"
        );
        assert_eq!(decide("find . -name '*.rs'"), Decision::Allow);
        assert_eq!(decide("find . -delete"), Decision::Ask);
        assert_eq!(decide("git log | head"), Decision::Allow);
    }

    #[test]
    fn split_shell_details() {
        let parsed = split_shell("a && b || c | d; e\nf");
        assert_eq!(parsed.parts, vec!["a", "b", "c", "d", "e", "f"]);
        assert!(!parsed.has_substitution);
        let quoted = split_shell("echo \"x; $(id)\" && ls");
        assert_eq!(quoted.parts, vec!["echo \"x; $(id)\"", "ls"]);
        assert!(quoted.has_substitution);
        assert_eq!(split_shell("   ").parts, vec![""]);
    }

    #[test]
    fn prompt_answers_drive_grants_and_persistence() {
        let persisted = Arc::new(Mutex::new(Vec::new()));
        let sink = persisted.clone();
        let (hooks, asked) = hooks(
            PermissionRules::default(),
            vec![
                PermissionAnswer::AllowOnce,
                PermissionAnswer::AllowSession,
                PermissionAnswer::AllowAlways,
                PermissionAnswer::Deny,
            ],
        );
        let mut hooks = hooks.with_persist(Box::new(move |tool, pattern, decision, scope| {
            sink.lock()
                .unwrap()
                .push((tool.to_string(), pattern.to_string(), decision, scope));
        }));

        // 1. allow once: asked again next time
        assert_eq!(
            hooks.before_tool_call(&bash("cargo build"), &ctx()),
            ToolDecision::Allow
        );
        // 2. allow for the session: "cargo build *" is granted in memory
        assert_eq!(
            hooks.before_tool_call(&bash("cargo build"), &ctx()),
            ToolDecision::Allow
        );
        assert_eq!(
            hooks.decide(&bash("cargo build --release"), &ctx()),
            Decision::Allow
        );
        assert!(persisted.lock().unwrap().is_empty());
        // 3. allow always: persisted through the callback
        assert_eq!(
            hooks.before_tool_call(&bash("npm test"), &ctx()),
            ToolDecision::Allow
        );
        assert_eq!(
            *persisted.lock().unwrap(),
            vec![(
                "bash".to_string(),
                "npm test *".to_string(),
                Decision::Allow,
                PersistScope::Project
            )]
        );
        assert_eq!(
            hooks.rules().evaluate("bash", "npm test -- x"),
            Some(Decision::Allow)
        );
        // 4. deny
        let blocked = hooks.before_tool_call(&call("write", json!({ "path": "a" })), &ctx());
        assert!(matches!(blocked, ToolDecision::Block { reason } if reason.contains("user")));

        let asked = asked.lock().unwrap();
        assert_eq!(asked.len(), 4);
        assert_eq!(asked[0].suggested_pattern, "cargo build *");
        assert_eq!(asked[3].subject, "a");
        assert_eq!(asked[3].suggested_pattern, "a");
        assert!(asked.iter().all(|request| request.can_persist));
    }

    #[test]
    fn always_is_offered_only_in_configured_mode_and_goes_where_asked() {
        let persisted = Arc::new(Mutex::new(Vec::new()));
        let sink = persisted.clone();
        let (hooks, asked) = hooks(
            PermissionRules::default(),
            vec![
                PermissionAnswer::AllowAlwaysGlobal,
                PermissionAnswer::DenySession,
                PermissionAnswer::AllowAlways,
            ],
        );
        let mut hooks = hooks.with_persist(Box::new(move |_, pattern, _, scope| {
            sink.lock().unwrap().push((pattern.to_string(), scope));
        }));
        hooks.before_tool_call(&bash("npm test"), &ctx());
        let refused = hooks.before_tool_call(&bash("make install"), &ctx());
        assert!(matches!(refused, ToolDecision::Block { reason } if reason.contains("session")));
        // Refused for the session: not asked again.
        assert_eq!(
            hooks.decide(&bash("make install DESTDIR=x"), &ctx()),
            Decision::Deny
        );
        assert_eq!(
            *persisted.lock().unwrap(),
            vec![("npm test *".to_string(), PersistScope::Global)]
        );
        // Outside `configured`, "always" is not on offer, and an answer that
        // says it anyway lasts for the session only.
        hooks.set_mode(Mode::Ask);
        hooks.before_tool_call(&bash("cargo build"), &ctx());
        assert!(!asked.lock().unwrap()[2].can_persist);
        assert_eq!(persisted.lock().unwrap().len(), 1);
        assert_eq!(
            hooks.decide(&bash("cargo build --release"), &ctx()),
            Decision::Allow
        );
        assert_eq!(hooks.rules().evaluate("bash", "cargo build"), None);
    }

    #[test]
    fn rule_denials_block_without_prompting() {
        let mut rules = PermissionRules::default();
        rules.add("read", "**/.env*", Decision::Deny);
        let (mut hooks, asked) = hooks(rules, vec![]);
        let blocked = hooks.before_tool_call(
            &call("read", json!({ "path": "/proj/config/.env" })),
            &ctx(),
        );
        assert!(matches!(blocked, ToolDecision::Block { reason } if reason.contains("rules")));
        assert!(asked.lock().unwrap().is_empty());
    }

    #[test]
    fn suggested_patterns() {
        assert_eq!(
            suggested_pattern("bash", "git push origin main"),
            "git push *"
        );
        assert_eq!(suggested_pattern("bash", "ls -la"), "ls *");
        assert_eq!(suggested_pattern("bash", "git -C x status"), "git *");
        assert_eq!(
            suggested_pattern("bash", "cargo test && cargo fmt"),
            "cargo test *"
        );
        assert_eq!(suggested_pattern("edit", "src/lib.rs"), "src/lib.rs");
        assert_eq!(suggested_pattern("other", ""), "*");
        assert_eq!(
            suggested_pattern("fetch", "https://docs.rs/ratatui/latest/?x=1"),
            "https://docs.rs/*"
        );
        assert_eq!(
            suggested_pattern("fetch", "http://127.0.0.1:8080"),
            "http://127.0.0.1:8080/*"
        );
        assert_eq!(suggested_pattern("web_search", "rust tui"), "*");
    }

    #[test]
    fn web_tools_are_judged_by_url_and_query() {
        let fetch = call("fetch", json!({ "url": " https://docs.rs/a " }));
        assert_eq!(subject_of(&fetch, &ctx()), "https://docs.rs/a");
        let search = call("web_search", json!({ "query": "rust tui", "limit": 3 }));
        assert_eq!(subject_of(&search, &ctx()), "rust tui");
        assert!(is_read_only_call(&fetch));
        assert!(is_read_only_call(&search));
    }

    #[test]
    fn subjects_are_project_relative() {
        assert_eq!(
            subject_of(
                &call("edit", json!({ "path": "/proj/src/../a.rs" })),
                &ctx()
            ),
            "a.rs"
        );
        assert_eq!(
            subject_of(&call("edit", json!({ "path": "./b.rs" })), &ctx()),
            "b.rs"
        );
        assert_eq!(
            subject_of(&call("edit", json!({ "path": "/etc/x" })), &ctx()),
            "/etc/x"
        );
        assert_eq!(
            subject_of(&call("read", json!({ "path": "/proj" })), &ctx()),
            "."
        );
        assert_eq!(
            subject_of(&call("mcp_tool", json!({ "query": "q" })), &ctx()),
            "q"
        );
    }
    #[test]
    fn mode_handle_switches_the_live_mode() {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let mut hooks = PermissionHooks::new(
            PermissionRules::default(),
            Box::new(Scripted {
                answers: vec![],
                asked: asked.clone(),
            }),
        );
        let handle = hooks.mode_handle();
        assert_eq!(handle.get(), Mode::Configured);
        assert_eq!(
            hooks.decide(&call("write", json!({ "path": "a" })), &ctx()),
            Decision::Ask
        );

        handle.set(Mode::Edit);
        assert_eq!(hooks.mode(), Mode::Edit);
        assert_eq!(
            hooks.decide(&call("write", json!({ "path": "a" })), &ctx()),
            Decision::Allow
        );
        assert_eq!(hooks.decide(&bash("cargo build"), &ctx()), Decision::Ask);

        handle.set(Mode::All);
        assert_eq!(hooks.decide(&bash("cargo build"), &ctx()), Decision::Allow);
        assert_eq!(
            hooks.before_tool_call(&bash("cargo build"), &ctx()),
            ToolDecision::Allow
        );
        assert!(asked.lock().unwrap().is_empty());

        assert_eq!(Mode::Ask.next(), Mode::Plan);
        assert_eq!(Mode::Configured.next(), Mode::All);
        assert_eq!(Mode::All.next(), Mode::Ask);
        assert_eq!(Mode::Edit.label(), "edit");
        assert_eq!(Mode::Configured.label(), "configured");
    }
    #[test]
    fn loading_a_skill_never_asks() {
        let hooks = PermissionHooks::new(
            PermissionRules::default(),
            Box::new(Scripted {
                answers: vec![],
                asked: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        assert_eq!(
            hooks.decide(&call("skill", json!({ "name": "deploy" })), &ctx()),
            Decision::Allow
        );
    }
    #[test]
    fn plan_mode_refuses_every_change_and_lets_reads_through() {
        let mode = ModeHandle::new(Mode::Plan);
        let mut guard = PlanGuard::new(mode.clone());
        let blocked = |guard: &mut PlanGuard, call: &ToolCall| {
            matches!(
                guard.before_tool_call(call, &ctx()),
                ToolDecision::Block { reason } if reason == PLAN_MODE_REASON
            )
        };
        assert!(!blocked(
            &mut guard,
            &call("read", json!({ "path": "src/main.rs" }))
        ));
        assert!(!blocked(
            &mut guard,
            &call("skill", json!({ "name": "deploy" }))
        ));
        assert!(!blocked(
            &mut guard,
            &call("bash", json!({ "command": "git status && rg TODO src" }))
        ));
        assert!(blocked(
            &mut guard,
            &call("bash", json!({ "command": "cat $(ls)" }))
        ));
        assert!(blocked(
            &mut guard,
            &call("bash", json!({ "command": "ls > out" }))
        ));
        assert!(blocked(
            &mut guard,
            &call("bash", json!({ "command": "git status; cargo build" }))
        ));
        assert!(blocked(&mut guard, &call("bash", json!({ "command": "" }))));
        assert!(blocked(
            &mut guard,
            &call("edit", json!({ "path": "src/main.rs" }))
        ));
        assert!(blocked(
            &mut guard,
            &call("write", json!({ "path": "new.rs" }))
        ));
        assert!(blocked(
            &mut guard,
            &call("fs__search", json!({ "query": "x" }))
        ));

        // Any other mode: the guard steps aside, the rules decide.
        mode.set(Mode::All);
        assert!(!blocked(
            &mut guard,
            &call("edit", json!({ "path": "src/main.rs" }))
        ));
    }
}

#[cfg(test)]
mod prompter_tests {
    use super::*;
    use crate::message::ToolCall;
    use serde_json::json;

    fn request() -> PermissionRequest {
        PermissionRequest {
            tool: "bash".into(),
            subject: "git push".into(),
            call: ToolCall {
                id: "c".into(),
                name: "bash".into(),
                arguments: json!({ "command": "git push" }),
            },
            suggested_pattern: "git push *".into(),
            can_persist: true,
        }
    }

    #[test]
    fn answer_travels_back_and_abort_denies() {
        let cancel = CancelToken::new();
        let (mut prompter, rx) = permission_channel(cancel.clone());

        let worker = std::thread::spawn({
            let request = request();
            move || prompter.ask(&request)
        });
        let envelope = rx.recv().unwrap();
        assert_eq!(envelope.id, 1);
        assert_eq!(envelope.request.subject, "git push");
        envelope.reply.send(PermissionAnswer::AllowSession).unwrap();
        assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowSession);

        let (mut prompter, rx) = permission_channel(cancel.clone());
        let worker = std::thread::spawn({
            let request = request();
            move || prompter.ask(&request)
        });
        let _pending = rx.recv().unwrap();
        cancel.cancel();
        assert_eq!(worker.join().unwrap(), PermissionAnswer::Deny);
    }

    #[test]
    fn dropped_panel_denies() {
        let (mut prompter, rx) = permission_channel(CancelToken::new());
        drop(rx);
        assert_eq!(prompter.ask(&request()), PermissionAnswer::Deny);
    }
}
