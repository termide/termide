//! Permission rules and the hook that enforces them.
//!
//! Rules live per tool as `pattern = decision` tables; among the rules that
//! match a call the strictest wins (`deny` over `ask` over `allow`). The mode
//! decides which rules count and what happens to calls none covers: only
//! `configured` takes the configured `allow` rules in full (`auto` takes the
//! narrow ones), every mode keeps their `deny` and `ask`, and answers given
//! "for this session" count everywhere but in `all`. In `auto` a reviewer
//! model decides what would otherwise be a prompt. Reading inside the project, loading a skill and a short list
//! of read-only shell commands never prompt, so long as those commands read
//! inside the project too.
//!
//! Shell commands are split on `&&`, `||`, `;`, `|`, a background `&` and
//! newlines; every part
//! must be allowed for the whole to pass, while a `deny` or `ask` on any part
//! applies to the whole. Command substitution (`$(...)`, backticks) is never
//! auto-allowed by an `allow` rule.
//!
//! A command is read in the language of the shell that runs it
//! ([`ShellSyntax`]): a fish command as the POSIX one it means, and one the
//! checks cannot read that way as a substitution, which nothing allows
//! without asking.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::{Hooks, ToolDecision};
use crate::cancel::CancelToken;
use crate::classifier::{Classifier, Verdict};
use crate::message::ToolCall;
use crate::refusals::Refusals;
use crate::tool::{ShellSyntax, ToolContext, SHELL_TOOL};

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
    Configured,
    /// The configured rules decide, less the broad `allow` ones that would
    /// let any program run; edits inside the project pass, and what none
    /// covers goes to a reviewer model instead of a prompt. The default.
    #[default]
    Auto,
    /// Allow everything the rules do not refuse or send to a prompt.
    All,
}

impl Mode {
    /// Every mode, in the order the UI lists and cycles through them.
    pub const ALL: [Mode; 6] = [
        Mode::Ask,
        Mode::Plan,
        Mode::Edit,
        Mode::Configured,
        Mode::Auto,
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
            Mode::Auto => "auto",
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

/// How a tool call came to run or not, kept with its result for the record:
/// the session log holds it and the transcript shows it, the model never
/// sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionNote {
    pub by: DecidedBy,
    pub allowed: bool,
    /// For an answer of the user's, how long it holds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lasting: Option<Lasting>,
    /// The reviewer's reason, or the words the user denied with.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

impl PermissionNote {
    #[must_use]
    pub fn new(by: DecidedBy, allowed: bool) -> Self {
        Self {
            by,
            allowed,
            lasting: None,
            reason: String::new(),
        }
    }
}

/// Who decided a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecidedBy {
    /// The rules, the mode or a safe default, without asking anyone.
    Rules,
    /// Plan mode, which refuses what could change something.
    Plan,
    /// A command hook, which approved or blocked the call.
    Hook,
    /// The `auto` mode reviewer.
    Reviewer,
    /// The user, answering a permission card.
    User,
    /// No one: the run had no one to ask (a subagent, a headless run), so
    /// what would have asked was refused.
    Unattended,
}

/// How long a user's answer holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lasting {
    Once,
    Session,
    /// A rule written to the project's configuration.
    Project,
    /// A rule written to the global configuration.
    Global,
}

/// Whether a call can change nothing: a read, a skill, or a shell command
/// made only of read-only parts without substitution, read as `shell` reads
/// it.
#[must_use]
pub fn is_read_only_call(call: &ToolCall, shell: ShellSyntax) -> bool {
    not_read_only(call, shell).is_none()
}

/// Why a call could change something, or `None` when it cannot: for a shell
/// command, what in it is not look-only, so a refusal can name it; empty for
/// any other tool.
fn not_read_only(call: &ToolCall, shell: ShellSyntax) -> Option<String> {
    match call.name.as_str() {
        // The web tools read the web; nothing on this machine changes. A
        // question to the user changes nothing either, nor does `recall`,
        // which only reads the project's history and code, or `agents`,
        // which lists the panels.
        "read" | "skill" | "fetch" | "web_search" | "question" | "recall" | "agents" => None,
        // Listing what the agent set going changes nothing; stopping it does.
        "jobs" if call.arguments.get("stop").is_none() => None,
        // Reading the debug session changes nothing; every other action runs
        // or stops the program.
        "debug" if call.arguments["action"] == "state" => None,
        // Looking at the agent's own terminals changes nothing; typing into
        // one, interrupting or closing it does.
        "terminals" if terminals_looks(call) => None,
        SHELL_TOOL => {
            command_not_read_only(call.arguments["command"].as_str().unwrap_or(""), shell)
        }
        _ => Some(String::new()),
    }
}

/// What in shell `command` is not look-only, as [`not_read_only`] tells it.
fn command_not_read_only(command: &str, shell: ShellSyntax) -> Option<String> {
    let Some((_, parsed)) = parse_in(command, shell) else {
        return Some("a command that cannot be read as a whole".to_string());
    };
    if parsed.has_substitution {
        return Some("a command substitution".to_string());
    }
    if parsed.parts.is_empty() {
        return Some("an empty command".to_string());
    }
    parsed
        .parts
        .iter()
        .find(|part| !is_read_only_command(part))
        .map(|part| format!("`{part}`"))
}

/// Whether a `terminals` call only looks: lists the agent's terminals,
/// reads one's output or its screen.
fn terminals_looks(call: &ToolCall) -> bool {
    matches!(
        call.arguments["action"].as_str(),
        None | Some("list" | "read" | "screen")
    )
}

/// Whether a call runs a shell command line: a `shell` call, or a line
/// typed into one of the agent's terminals at its prompt. Both are judged
/// part by part under the `shell` rules.
fn runs_command_line(call: &ToolCall, ctx: &ToolContext) -> bool {
    call.name == SHELL_TOOL || ctx.tool.typed_at_prompt
}

/// The calls no mode asks about, though a rule may still refuse one. Each
/// changes nothing; or it is put to the user already — a question, or a
/// card whose command, goal or loop runs only when the user picks it; or
/// it touches only what the agent itself set going: listing or stopping
/// its jobs, looking at, interrupting or closing its terminals. The plan
/// guard lets them through and the mode allows them, from this one list.
fn asks_no_one(call: &ToolCall) -> bool {
    match call.name.as_str() {
        // `recall` reads the project's own session logs (under the
        // configuration directory), its git history and its code; `agents`
        // lists the panels; a skill's text may live outside the project.
        "skill" | "question" | "recall" | "agents" | "jobs" | "suggest_command"
        | "suggest_goal" | "suggest_loop" => true,
        // Typing into a terminal is judged by what it types.
        "terminals" => call.arguments["action"].as_str() != Some("send"),
        "debug" => call.arguments["action"] == "state",
        _ => false,
    }
}

/// Whether a `debug` call only moves on or looks at a program already
/// launched: anything but `start` and `evaluate`.
fn debug_moves_only(call: &ToolCall) -> bool {
    matches!(
        call.arguments["action"].as_str(),
        Some(
            "state"
                | "breakpoint"
                | "continue"
                | "step_over"
                | "step_in"
                | "step_out"
                | "pause"
                | "stop"
                | "frame"
        )
    )
}

/// Refuses every call that could change something while the shared mode is
/// [`Mode::Plan`]. It sits first in the hook chain, ahead of command hooks,
/// so nothing — not even a hook's approval — lets a change through in plan
/// mode; in every other mode it does nothing. A delegation to another agent
/// goes through: the subagent runs in plan mode under a guard of its own. So
/// does an MCP tool: only its server and the user know what it changes, so
/// the permission rules decide it, and ask when they do not. And so does
/// `suggest_command`, which runs nothing itself: the panel shows its card
/// without `[Run]` in plan mode, so the user can still copy the command.
/// `suggest_goal` and `suggest_loop` start nothing themselves either: what
/// the user starts on their card runs in plan mode like a typed request.
/// `send_message` stays refused: the agent it reaches is not in plan mode,
/// and could make the change this one may not.
pub struct PlanGuard {
    mode: ModeHandle,
    /// What the model reads for a refusal.
    reason: String,
    /// Whether it refused the call judged last.
    refused: bool,
}

impl PlanGuard {
    #[must_use]
    pub fn new(mode: ModeHandle) -> Self {
        Self {
            mode,
            reason: Refusals::default().plan_mode,
            refused: false,
        }
    }

    /// Refuse with the texts of `system/permissions.md`.
    #[must_use]
    pub fn with_refusals(mut self, refusals: &Refusals) -> Self {
        self.reason = refusals.plan_mode.clone();
        self
    }
}

impl Hooks for PlanGuard {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        if self.mode.get() != Mode::Plan || call.name == "task" || asks_no_one(call) || ctx.tool.mcp
        {
            return ToolDecision::Allow;
        }
        let not_look_only = if ctx.tool.typed_at_prompt {
            command_not_read_only(&subject_of(call, ctx), ctx.tool.shell)
        } else {
            not_read_only(call, ctx.tool.shell)
        };
        let Some(what) = not_look_only else {
            return ToolDecision::Allow;
        };
        self.refused = true;
        // Naming the part that is not look-only lets the model take another
        // way at once instead of guessing.
        let reason = if what.is_empty() {
            self.reason.clone()
        } else {
            format!("{} (not look-only: {what})", self.reason)
        };
        ToolDecision::Block { reason }
    }

    fn take_permission(&mut self) -> Option<PermissionNote> {
        // Only a refusal is the guard's to record; a call it lets through is
        // the rest of the chain's to decide.
        std::mem::take(&mut self.refused).then(|| PermissionNote::new(DecidedBy::Plan, false))
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
    /// "Allow always" answers given for the project, kept in termide's own
    /// data rather than in the repository (whose config may not allow
    /// anything); they count as the configured rules do. Never written out
    /// with the config.
    #[serde(skip)]
    pub granted: RuleTables,
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
        let configured = evaluate_rules(&self.tools, tool, subject);
        let granted = evaluate_rules(&self.granted, tool, subject);
        configured.max(granted)
    }

    /// Strictest decision among this session's answers for `tool` that match
    /// `subject`.
    #[must_use]
    pub fn evaluate_session(&self, tool: &str, subject: &str) -> Option<Decision> {
        evaluate_rules(&self.session, tool, subject)
    }

    /// [`PermissionRules::evaluate`] without the broad `allow` rules `auto`
    /// mode sets aside.
    #[must_use]
    pub fn evaluate_narrow(&self, tool: &str, subject: &str) -> Option<Decision> {
        [&self.tools, &self.granted]
            .into_iter()
            .flat_map(|tables| tool_tables(tables, tool))
            .flatten()
            .filter(|(pattern, decision)| {
                **decision != Decision::Allow || !broad_allow(tool, pattern)
            })
            .filter(|(pattern, _)| wildcard_match(pattern, subject))
            .map(|(_, decision)| *decision)
            .max()
    }
}

/// Whether an `allow` pattern would let any program run, which `auto` mode
/// leaves to the reviewer instead: a whole tool, a delegation to another
/// agent, or a wildcard after an interpreter, a script runner or a wrapper
/// (`python*`, `npm run *`, `env *`). A pattern without a wildcard names
/// one command and stays.
#[must_use]
fn broad_allow(tool: &str, pattern: &str) -> bool {
    if tool == "task" {
        return true;
    }
    if !pattern.contains('*') {
        return false;
    }
    let fixed = pattern.trim().trim_end_matches('*').trim();
    if fixed.is_empty() {
        return true;
    }
    // What the pattern fixes, followed by an argument its wildcard would take.
    tool == SHELL_TOOL && delegating_command(&format!("{fixed} x"))
}

fn add_rule(tables: &mut RuleTables, tool: &str, pattern: &str, decision: Decision) {
    tables
        .entry(tool.to_string())
        .or_default()
        .insert(pattern.to_string(), decision);
}

/// The rule table of `tool`, when it has one.
fn tool_tables<'a>(
    tables: &'a RuleTables,
    tool: &'a str,
) -> impl Iterator<Item = &'a BTreeMap<String, Decision>> {
    tables.get(tool).into_iter()
}

fn evaluate_rules(tables: &RuleTables, tool: &str, subject: &str) -> Option<Decision> {
    tool_tables(tables, tool)
        .flatten()
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
    /// Whether "allow always" is on offer: in `configured` and `auto`, the
    /// modes the configured rules count in, with somewhere to write the rule,
    /// and only for the one command whose scope its pattern states — a bundle
    /// of parts and a command that destroys or runs some other program leave
    /// no rule behind.
    pub can_persist: bool,
    /// Whether "allow for the session" is on offer: not for a command that
    /// destroys, which is allowed once at a time. Denying for the session is
    /// always on offer, since it tightens rather than trusts.
    pub can_allow_session: bool,
    /// For a shell command, the parts the question is about — those no rule
    /// or safe default settles — each with the rule an answer can record for
    /// it; empty for other tools, whose `suggested_pattern` is the rule.
    pub parts: Vec<AskedPart>,
}

/// A part of a command line the user is asked about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskedPart {
    pub text: String,
    /// The rule an answer beyond this call records; `None` when none can
    /// stand for it (its program's directory is unknown, or it runs a
    /// command substitution), so it is answered for this call only.
    pub pattern: Option<String>,
}

impl PermissionRequest {
    /// The rules an answer beyond this call records, one per part for a
    /// shell command.
    #[must_use]
    pub fn patterns(&self) -> Vec<String> {
        if self.tool == SHELL_TOOL {
            self.parts
                .iter()
                .filter_map(|p| p.pattern.clone())
                .collect()
        } else {
            vec![self.suggested_pattern.clone()]
        }
    }

    /// Whether an answer can outlast this call: some rule can be recorded.
    #[must_use]
    pub fn can_remember(&self) -> bool {
        !self.patterns().is_empty()
    }
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

    /// [`Self::ask`] for a call that can be given up while the question is
    /// open: once `withdrawn` is cancelled the question is taken back and
    /// the call denied. A prompter that answers at once has nothing to take
    /// back.
    fn ask_for_call(
        &mut self,
        request: &PermissionRequest,
        withdrawn: Option<&CancelToken>,
    ) -> PermissionAnswer {
        let _ = withdrawn;
        self.ask(request)
    }

    /// Whether a person answers; a prompter that answers alone is recorded
    /// as such, not as the user.
    fn attended(&self) -> bool {
        true
    }
}

/// A prompter that answers every question with the same denial, for a run
/// with no one to ask — a headless run, or a subagent whose parent has no
/// panel to put a card in: anything the rules and mode do not already allow
/// is refused with a reason the model reads, rather than waiting on no one.
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

    fn attended(&self) -> bool {
        false
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
    /// Cancelled once no one waits for the answer any more — the call was
    /// given up — so the panel takes the card down.
    pub withdrawn: CancelToken,
    /// The delegate that asks, when it is not the panel's own agent: its
    /// card is headed with its name, and what it grants stays with its task.
    pub delegate: Option<String>,
    /// The id of the `task` call that handed the delegate its task, so a
    /// panel following that task can take the card.
    pub task: Option<String>,
}

/// Cloneable, so the hooks of calls that come in another way — tools served
/// to an external agent — ask on the same channel the panel answers.
#[derive(Clone)]
pub struct ChannelPrompter {
    tx: Sender<PermissionEnvelope>,
    cancel: CancelToken,
    next_id: u64,
    /// Who asks, when it is not the panel's own agent.
    delegate: Option<String>,
    /// The `task` call that handed it its task.
    task: Option<String>,
    /// Ends the wait besides `cancel`: a delegate's own stop, which the
    /// panel's run does not see.
    stop: Option<CancelToken>,
}

impl std::fmt::Debug for ChannelPrompter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelPrompter").finish_non_exhaustive()
    }
}

impl ChannelPrompter {
    /// This prompter for the delegate `agent`, whose run ends with `stop`:
    /// its requests reach the same user, on cards headed with its name.
    #[must_use]
    pub fn for_delegate(&self, agent: &str, stop: CancelToken) -> Self {
        Self {
            delegate: Some(agent.to_string()),
            stop: Some(stop),
            ..self.clone()
        }
    }

    /// This prompter for the task the `task` call `call` handed over: its
    /// cards say so, for a panel following the task to take them.
    #[must_use]
    pub fn for_task(&self, call: &str) -> Self {
        Self {
            task: Some(call.to_string()),
            ..self.clone()
        }
    }

    /// This prompter, no longer ending with the panel's run: for a delegate
    /// handed over in the background, which only its own stop ends.
    #[must_use]
    pub fn detached(&self) -> Self {
        Self {
            cancel: CancelToken::new(),
            ..self.clone()
        }
    }

    fn stopped(&self) -> bool {
        self.cancel.is_cancelled() || self.stop.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

/// Build a prompter and the receiver the panel polls from `tick()`.
pub fn permission_channel(cancel: CancelToken) -> (ChannelPrompter, Receiver<PermissionEnvelope>) {
    let (tx, rx) = mpsc::channel();
    (
        ChannelPrompter {
            tx,
            cancel,
            next_id: 0,
            delegate: None,
            task: None,
            stop: None,
        },
        rx,
    )
}

impl PermissionPrompter for ChannelPrompter {
    fn ask(&mut self, request: &PermissionRequest) -> PermissionAnswer {
        self.ask_for_call(request, None)
    }

    fn ask_for_call(
        &mut self,
        request: &PermissionRequest,
        withdrawn: Option<&CancelToken>,
    ) -> PermissionAnswer {
        let (reply, answer) = mpsc::channel();
        self.next_id += 1;
        // Cancelled here too when the run stops, so the card goes either way.
        let withdrawn = withdrawn.cloned().unwrap_or_default();
        let envelope = PermissionEnvelope {
            id: self.next_id,
            request: request.clone(),
            reply,
            withdrawn: withdrawn.clone(),
            delegate: self.delegate.clone(),
            task: self.task.clone(),
        };
        if self.tx.send(envelope).is_err() {
            // The panel is gone; nobody can approve anything.
            return PermissionAnswer::Deny;
        }
        loop {
            match answer.recv_timeout(Duration::from_millis(100)) {
                Ok(answer) => return answer,
                Err(RecvTimeoutError::Timeout) if self.stopped() || withdrawn.is_cancelled() => {
                    withdrawn.cancel();
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
    /// Who decides in place of the prompt in `auto` mode; without one, `auto`
    /// asks what it would review.
    classifier: Option<Box<dyn Classifier>>,
    breaker: Breaker,
    /// Who decided the call judged last, until the loop takes it.
    note: Option<PermissionNote>,
    /// What the model reads when a call is refused.
    refusals: Refusals,
}

/// How many blocks in a row, and in all, pause the reviewer and put the
/// questions back to the user: a run blocked over and over has likely lost
/// its way, or the reviewer lacks context the user has.
const BLOCKS_IN_A_ROW: u32 = 3;
const BLOCKS_IN_ALL: u32 = 20;

/// The reviewer's block count, and whether it is paused.
#[derive(Debug, Default, Clone, Copy)]
struct Breaker {
    consecutive: u32,
    total: u32,
    paused: bool,
}

impl Breaker {
    fn block(&mut self) {
        self.consecutive += 1;
        self.total += 1;
        if self.consecutive >= BLOCKS_IN_A_ROW || self.total >= BLOCKS_IN_ALL {
            self.paused = true;
        }
    }
}

/// A shell command's verdict: the decision, the parts a question would be
/// about, and whether an `ask` rule demands the question.
struct CommandVerdict {
    decision: Decision,
    asked: Vec<AskedPart>,
    forced: bool,
}

impl PermissionHooks {
    #[must_use]
    pub fn new(rules: PermissionRules, prompter: Box<dyn PermissionPrompter>) -> Self {
        Self {
            mode: ModeHandle::new(rules.mode),
            rules,
            prompter,
            persist: None,
            classifier: None,
            breaker: Breaker::default(),
            note: None,
            refusals: Refusals::default(),
        }
    }

    /// Refuse with the texts of `system/permissions.md`.
    #[must_use]
    pub fn with_refusals(mut self, refusals: Refusals) -> Self {
        self.refusals = refusals;
        self
    }

    /// Let `classifier` decide in `auto` mode what would otherwise be asked.
    #[must_use]
    pub fn with_classifier(mut self, classifier: Box<dyn Classifier>) -> Self {
        self.classifier = Some(classifier);
        self
    }

    /// Follow `mode` instead of a mode of its own: hooks that judge another
    /// source of calls under the same switch the panel flips.
    #[must_use]
    pub fn with_mode_handle(mut self, mode: ModeHandle) -> Self {
        self.mode = mode;
        self
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

    /// Whether `call` edits under `.git` in a mode that asks about it: every
    /// mode but `all`.
    fn guards_git_dir(&self, call: &ToolCall, subject: &str) -> bool {
        self.mode.get() != Mode::All && writes_git_dir(call, subject)
    }

    /// The verdict before any prompt: the rules that count in the mode
    /// (the strictest wins), then the mode's own answer and the built-in
    /// safe defaults.
    #[must_use]
    pub fn decide(&self, call: &ToolCall, ctx: &ToolContext) -> Decision {
        let subject = subject_of(call, ctx);
        let mode = self.mode.get();
        if runs_command_line(call, ctx) {
            return self.judge_command(&subject, ctx).decision;
        }
        // What git keeps under `.git` — its config, its hooks — names programs
        // that a look-only `git status` then runs, so no `allow` rule or
        // answer for the session lets an edit there through unasked. `all`
        // runs any shell command unasked anyway, so a prompt there guards
        // nothing the user has not chosen to give away.
        if self.guards_git_dir(call, &subject) {
            return match self.rule_for(call, &subject, ctx) {
                Some(Decision::Deny) => Decision::Deny,
                _ if mode == Mode::Plan => Decision::Deny,
                _ => Decision::Ask,
            };
        }

        if let Some(decision) = self.rule_for(call, &subject, ctx) {
            return decision;
        }
        let inside = inside_project(call, ctx);
        match (mode, call.name.as_str()) {
            (Mode::All, _) => Decision::Allow,
            (_, "read") if inside => Decision::Allow,
            // The panel withholds `[Run]` on a `suggest_command` card where
            // the mode or a rule forbids the command.
            _ if asks_no_one(call) => Decision::Allow,
            // Stepping, pausing or stopping a program, setting a breakpoint,
            // reading the session: the program's launch was the decision, and
            // these only move it on or look at it. `start`, and `evaluate`,
            // which can call the program's own functions, stay with the mode.
            (mode, "debug") if mode != Mode::Plan && debug_moves_only(call) => Decision::Allow,
            (Mode::Plan | Mode::Edit, "fetch" | "web_search") => Decision::Allow,
            // The subagent is held to plan mode too, so it only reads.
            (Mode::Plan, "task") => Decision::Allow,
            (Mode::Edit | Mode::Auto, "edit" | "write") if inside => Decision::Allow,
            // A query reads the web; a fetched URL can carry data out, so the
            // reviewer sees it.
            (Mode::Auto, "web_search") => Decision::Allow,
            // An MCP tool its server declares read-only passes plan mode; any
            // other asks, since only the user can tell whether it changes
            // something.
            // `edit` lets it through too, as it does the web.
            (Mode::Plan | Mode::Edit, _) if ctx.tool.mcp && ctx.tool.read_only => Decision::Allow,
            (Mode::Plan, _) if ctx.tool.mcp => Decision::Ask,
            // Plan mode asks before reading outside the project and refuses
            // what could change something.
            (Mode::Plan, "read") => Decision::Ask,
            (Mode::Plan, _) => Decision::Deny,
            _ => Decision::Ask,
        }
    }
}

impl PermissionHooks {
    /// The strictest decision among the rules that count in the mode for
    /// `call`, whose subject is `text`. An MCP tool in plan mode is held to
    /// the configured rules in full, its `allow`s included: plan mode lets
    /// it through only on the user's word, and a rule is that word given
    /// ahead of time.
    fn rule_for(&self, call: &ToolCall, text: &str, ctx: &ToolContext) -> Option<Decision> {
        if self.mode.get() == Mode::Plan && ctx.tool.mcp {
            let configured = self.rules.evaluate(&call.name, text);
            let session = self.rules.evaluate_session(&call.name, text);
            return configured.into_iter().chain(session).max();
        }
        self.rule(&call.name, text)
    }

    /// The strictest decision among the rules that count in the mode for
    /// `tool` and `text`.
    fn rule(&self, tool: &str, text: &str) -> Option<Decision> {
        let mode = self.mode.get();
        // The configured rules count in full only in `configured`, and less
        // their broad allows in `auto`; elsewhere they can only tighten. This
        // session's answers count everywhere but in `all`, which allows what
        // they would — in `auto` too: the user gave them for this session.
        if mode == Mode::Auto {
            let configured = self.rules.evaluate_narrow(tool, text);
            let session = self.rules.evaluate_session(tool, text);
            return configured.into_iter().chain(session).max();
        }
        let configured = self
            .rules
            .evaluate(tool, text)
            .filter(|d| mode == Mode::Configured || *d != Decision::Allow);
        let session = self
            .rules
            .evaluate_session(tool, text)
            .filter(|_| mode != Mode::All);
        configured.into_iter().chain(session).max()
    }

    /// A shell command's verdict, and the parts it asks about. Each part is
    /// judged on its own, as written and with its program's path resolved;
    /// a whole-command rule can tighten a multi-part command but never
    /// loosen it, since each part must earn its own allow.
    fn judge_command(&self, command: &str, ctx: &ToolContext) -> CommandVerdict {
        let mode = self.mode.get();
        let parts = shell_parts_in(command, ctx.tool.shell, &ctx.cwd);
        let mut verdict = Decision::Allow;
        let mut forced = false;
        if parts.len() > 1 {
            if let Some(whole) = self
                .rule(SHELL_TOOL, command)
                .filter(|d| *d != Decision::Allow)
            {
                forced |= whole == Decision::Ask;
                verdict = verdict.max(whole);
            }
        }
        let mut asked = Vec::new();
        for part in parts {
            let matched = self
                .rule(SHELL_TOOL, &part.text)
                .into_iter()
                .chain(self.rule(SHELL_TOOL, &part.resolved))
                .max();
            forced |= matched == Some(Decision::Ask);
            let mut reads_out = false;
            let decision = match matched {
                // A rule cannot vouch for a substitution, nor for a program
                // whose directory is unknown; the parts beside them, which a
                // rule does cover, are not dragged into it.
                Some(Decision::Allow) if part.substituted || !part.savable => Decision::Ask,
                Some(decision) => decision,
                None if mode == Mode::All => Decision::Allow,
                // A look-only command reading beyond the project asks, as
                // the `read` tool does there, in plan mode too.
                None if !part.substituted && is_read_only_command(&part.text) => {
                    if reads_outside(&part, &ctx.cwd) {
                        reads_out = true;
                        Decision::Ask
                    } else {
                        Decision::Allow
                    }
                }
                // Plan mode refuses a command that could change something.
                None if mode == Mode::Plan => Decision::Deny,
                None => Decision::Ask,
            };
            if decision == Decision::Ask {
                // A rule for a read beyond the project names that one
                // command: `cat *` would let every file on the machine out.
                asked.push(AskedPart {
                    pattern: part.savable.then(|| {
                        if reads_out {
                            part.resolved.clone()
                        } else {
                            suggested_pattern(SHELL_TOOL, &part.resolved)
                        }
                    }),
                    text: part.text,
                });
            }
            verdict = verdict.max(decision);
        }
        CommandVerdict {
            decision: verdict,
            asked,
            forced,
        }
    }

    /// Put a call the rules leave open to the reviewer, in `auto` mode:
    /// `None` when the user is to be asked instead — another mode, an `ask`
    /// rule that `forced` the question, a removal of a critical directory,
    /// a reviewer that paused after repeated blocks or gave no verdict.
    fn review(
        &mut self,
        call: &ToolCall,
        ctx: &ToolContext,
        forced: bool,
        parts: &[AskedPart],
    ) -> Option<(ToolDecision, PermissionNote)> {
        if self.mode.get() != Mode::Auto || forced || self.breaker.paused {
            return None;
        }
        if parts.iter().any(|p| critical_removal(&p.text, &ctx.cwd)) {
            return None;
        }
        let verdict = self.classifier.as_mut()?.classify(call, ctx);
        // The mode may have changed while the reviewer thought; its verdict
        // then answers a question the new mode does not ask.
        if self.mode.get() != Mode::Auto {
            return Some(self.judge(call, ctx));
        }
        let reviewed = |allowed: bool, reason: &str| PermissionNote {
            reason: reason.to_string(),
            ..PermissionNote::new(DecidedBy::Reviewer, allowed)
        };
        match verdict {
            Verdict::Allow { reason } => {
                log::info!("auto mode allowed {}: {reason}", call.name);
                self.breaker.consecutive = 0;
                Some((ToolDecision::Allow, reviewed(true, &reason)))
            }
            Verdict::Block { reason } => {
                log::info!("auto mode blocked {}: {reason}", call.name);
                self.breaker.block();
                Some((
                    ToolDecision::Block {
                        reason: Refusals::with_reason(&self.refusals.reviewer_blocked, &reason),
                    },
                    reviewed(false, &reason),
                ))
            }
            Verdict::Unavailable { reason } => {
                log::warn!("auto mode could not review {}: {reason}", call.name);
                None
            }
        }
    }

    /// Record `decision` for every rule `request`'s answer stands for, for
    /// the session or (`scope`) in the configuration.
    fn remember(
        &mut self,
        request: &PermissionRequest,
        decision: Decision,
        scope: Option<PersistScope>,
    ) {
        for pattern in request.patterns() {
            match scope {
                Some(scope) if request.can_persist => {
                    self.rules.add(&request.tool, &pattern, decision);
                    if let Some(persist) = &mut self.persist {
                        persist(&request.tool, &pattern, decision, scope);
                    }
                }
                // "Always" not on offer here: it lasts for the session.
                _ => self.rules.add_session(&request.tool, &pattern, decision),
            }
        }
    }
}

impl Hooks for PermissionHooks {
    fn before_tool_call(&mut self, call: &ToolCall, ctx: &ToolContext) -> ToolDecision {
        let (decision, note) = self.judge(call, ctx);
        self.note = Some(note);
        decision
    }

    fn take_permission(&mut self) -> Option<PermissionNote> {
        self.note.take()
    }
}

impl PermissionHooks {
    /// The decision on `call`, and who made it.
    fn judge(&mut self, call: &ToolCall, ctx: &ToolContext) -> (ToolDecision, PermissionNote) {
        match self.decide(call, ctx) {
            Decision::Allow => (
                ToolDecision::Allow,
                PermissionNote::new(DecidedBy::Rules, true),
            ),
            Decision::Deny if self.mode.get() == Mode::Plan => (
                ToolDecision::Block {
                    reason: self.refusals.plan_mode.clone(),
                },
                PermissionNote::new(DecidedBy::Plan, false),
            ),
            Decision::Deny => (
                ToolDecision::Block {
                    reason: self.refusals.rule_denied.clone(),
                },
                PermissionNote::new(DecidedBy::Rules, false),
            ),
            Decision::Ask => {
                let subject = subject_of(call, ctx);
                let command_line = runs_command_line(call, ctx);
                let (parts, forced) = if command_line {
                    let verdict = self.judge_command(&subject, ctx);
                    (verdict.asked, verdict.forced)
                } else {
                    (
                        Vec::new(),
                        self.rule_for(call, &subject, ctx) == Some(Decision::Ask)
                            || self.guards_git_dir(call, &subject),
                    )
                };
                if let Some(judged) = self.review(call, ctx, forced, &parts) {
                    return judged;
                }
                // A rule outliving this call is an answer given without seeing
                // the calls it will cover, so it is on offer only where it
                // states enough to trust: one command, not a bundle answered
                // without being picked apart, not one that destroys what it
                // touches, and not one that runs some other program a rule for
                // it would vouch for blindly.
                let bundle = parts.len() > 1;
                // An edit under `.git` asks every time, so no answer outlasts it.
                // Keys typed into a program the shell runs, or that edit the
                // line unseen, are read by no check: the same keys mean
                // something else each time, so each send is answered alone.
                let destructive = parts.iter().any(|p| destructive_command(&p.text))
                    || self.guards_git_dir(call, &subject)
                    || (call.name == "terminals" && !command_line);
                let delegating = parts.iter().any(|p| delegating_command(&p.text));
                // In plan mode an MCP tool counts its configured `allow`, so
                // a lasting answer about one is worth keeping there too.
                let mode = self.mode.get();
                let can_persist = (matches!(mode, Mode::Configured | Mode::Auto)
                    || (mode == Mode::Plan && ctx.tool.mcp))
                    && self.persist.is_some()
                    && !bundle
                    && !destructive
                    && !delegating
                    // A whole-command `ask` rule can ask about a command
                    // whose every part is settled, which leaves no part, and
                    // so no rule, to record.
                    && (!command_line || parts.first().is_some_and(|p| p.pattern.is_some()));
                // A destructive call is allowed once at a time; `all` is the
                // mode that lets a run through without asking. Denying for the
                // session stays on offer: it trusts nothing.
                let can_allow_session = !destructive;
                let suggested = if command_line {
                    parts
                        .iter()
                        .filter_map(|p| p.pattern.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    suggested_pattern(&call.name, &subject)
                };
                // A line typed at a prompt is a `shell` command: asked about
                // as one, and its answer recorded as a `shell` rule.
                let tool = if command_line {
                    SHELL_TOOL.to_string()
                } else {
                    call.name.clone()
                };
                let request = PermissionRequest {
                    tool,
                    suggested_pattern: suggested,
                    subject,
                    call: call.clone(),
                    can_persist,
                    can_allow_session,
                    parts,
                };
                let answer = self.prompter.ask_for_call(&request, ctx.withdrawn.as_ref());
                // Allowing what the paused reviewer sent to the user hands
                // the decisions back to it.
                if matches!(
                    answer,
                    PermissionAnswer::AllowOnce
                        | PermissionAnswer::AllowSession
                        | PermissionAnswer::AllowAlways
                        | PermissionAnswer::AllowAlwaysGlobal
                ) {
                    self.breaker = Breaker::default();
                }
                let attended = self.prompter.attended();
                let user = |allowed: bool, lasting: Lasting| {
                    if attended {
                        PermissionNote {
                            lasting: Some(lasting),
                            ..PermissionNote::new(DecidedBy::User, allowed)
                        }
                    } else {
                        PermissionNote::new(DecidedBy::Unattended, allowed)
                    }
                };
                match answer {
                    PermissionAnswer::AllowOnce => (ToolDecision::Allow, user(true, Lasting::Once)),
                    // An answer outlasting the call when none was on offer
                    // stands for this call only.
                    PermissionAnswer::AllowSession if !can_allow_session => {
                        (ToolDecision::Allow, user(true, Lasting::Once))
                    }
                    PermissionAnswer::AllowSession => {
                        self.remember(&request, Decision::Allow, None);
                        (ToolDecision::Allow, user(true, Lasting::Session))
                    }
                    PermissionAnswer::AllowAlways => {
                        self.remember(&request, Decision::Allow, Some(PersistScope::Project));
                        let lasting = if request.can_persist {
                            Lasting::Project
                        } else {
                            Lasting::Session
                        };
                        (ToolDecision::Allow, user(true, lasting))
                    }
                    PermissionAnswer::AllowAlwaysGlobal => {
                        self.remember(&request, Decision::Allow, Some(PersistScope::Global));
                        let lasting = if request.can_persist {
                            Lasting::Global
                        } else {
                            Lasting::Session
                        };
                        (ToolDecision::Allow, user(true, lasting))
                    }
                    PermissionAnswer::Deny => (
                        ToolDecision::Block {
                            reason: self.refusals.user_denied.clone(),
                        },
                        user(false, Lasting::Once),
                    ),
                    PermissionAnswer::DenySession => {
                        self.remember(&request, Decision::Deny, None);
                        (
                            ToolDecision::Block {
                                reason: self.refusals.user_denied_session.clone(),
                            },
                            user(false, Lasting::Session),
                        )
                    }
                    PermissionAnswer::DenyWithReason(reason) => (
                        ToolDecision::Block {
                            reason: Refusals::with_reason(
                                &self.refusals.user_denied_reason,
                                &reason,
                            ),
                        },
                        if attended {
                            PermissionNote {
                                reason,
                                ..user(false, Lasting::Once)
                            }
                        } else {
                            user(false, Lasting::Once)
                        },
                    ),
                }
            }
        }
    }
}

/// The text rules are matched against: the command for `shell`, the
/// project-relative path for file tools, empty otherwise.
#[must_use]
pub fn subject_of(call: &ToolCall, ctx: &ToolContext) -> String {
    match call.name.as_str() {
        SHELL_TOOL => call.arguments["command"].as_str().unwrap_or("").to_string(),
        // A line typed at a prompt is the command it runs; any other `send`
        // shows the terminal and all it types, keys too, since each such
        // call is answered on its own.
        "terminals" if ctx.tool.typed_at_prompt => call.arguments["input"]
            .as_str()
            .unwrap_or("")
            .trim_end_matches('\n')
            .to_string(),
        "terminals" => {
            let arg = |key: &str| call.arguments[key].as_str().unwrap_or("");
            let action = call.arguments["action"].as_str().unwrap_or("list");
            let mut subject = format!("{action} {}", arg("session").trim());
            if action == "send" {
                let keys = match &call.arguments["keys"] {
                    Value::Array(keys) => keys.iter().filter_map(Value::as_str).collect(),
                    Value::String(key) => vec![key.as_str()],
                    _ => Vec::new(),
                };
                subject.push_str(": ");
                subject.push_str(&arg("input").escape_debug().to_string());
                for key in keys {
                    subject.push_str(&format!(" [{key}]"));
                }
            }
            subject.trim_end().to_string()
        }
        "read" | "edit" | "write" => {
            let raw = call.arguments["path"].as_str().unwrap_or("");
            real_relative_to_project(raw, &ctx.cwd)
        }
        "fetch" => call.arguments["url"]
            .as_str()
            .unwrap_or("")
            .trim()
            .to_string(),
        "web_search" => call.arguments["query"].as_str().unwrap_or("").to_string(),
        // What runs: `start debug-server`, `start src/main.py` (the file as
        // the file tools give it), `evaluate len(items)`; the bare action
        // otherwise.
        "debug" => {
            let arg = |key: &str| call.arguments[key].as_str().unwrap_or("").trim();
            let action = arg("action");
            let what = match action {
                "start" if !arg("command").is_empty() => arg("command").to_string(),
                "start" if !arg("file").is_empty() => relative_to_project(arg("file"), &ctx.cwd),
                "evaluate" => arg("expression").replace('\n', " "),
                _ => String::new(),
            };
            if what.is_empty() {
                action.to_string()
            } else {
                format!("{action} {what}")
            }
        }
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

/// [`relative_to_project`] for the file a file tool acts on, with symlinks
/// resolved: the file the system opens, which a link in the project (`docs`
/// pointing at the home directory) may put outside it, or a link to `.env`
/// may hide from the rule that guards it. A path outside the project that
/// stays outside keeps the spelling it was given (`/etc/hosts`, not the
/// `/private/etc/hosts` macOS resolves it to).
fn real_relative_to_project(raw: &str, cwd: &Path) -> String {
    let real = real_path(&cwd.join(raw));
    match real.strip_prefix(real_path(cwd)) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.to_string_lossy().into_owned(),
        Ok(_) => ".".to_string(),
        Err(_) => {
            let lexical = relative_to_project(raw, cwd);
            if Path::new(&lexical).is_absolute() {
                lexical
            } else {
                real.to_string_lossy().into_owned()
            }
        }
    }
}

/// `path` as the file system resolves it, symlinks included, so far as it
/// exists; the part that does not yet exist (a file to create) is appended
/// as written. A `..` steps out of what the link before it points at, as
/// the system's own lookup does.
fn real_path(path: &Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            std::path::Component::Normal(name) => {
                out.push(name);
                // Each existing prefix is made real before the next step, so
                // a `..` after a link leaves its target, not the link.
                if let Ok(real) = dunce::canonicalize(&out) {
                    out = real;
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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

/// Whether `call` edits or writes a file under a `.git` directory, or the
/// `.git` file of a worktree, its `subject` being the real path. The match
/// ignores case, which file systems such as macOS's ignore too.
fn writes_git_dir(call: &ToolCall, subject: &str) -> bool {
    matches!(call.name.as_str(), "edit" | "write")
        && Path::new(subject)
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case(".git"))
}

fn inside_project(call: &ToolCall, ctx: &ToolContext) -> bool {
    let subject = subject_of(call, ctx);
    !Path::new(&subject).is_absolute() && !subject.starts_with("..")
}

/// Pattern offered when the user allows a call for longer than once: the
/// command's leading words for `shell`, the exact path for file tools.
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
    if tool != SHELL_TOOL {
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
    /// Whether the part at the same index expands: a substitution it carries
    /// or a here-document body feeding it. A substitution belongs to the
    /// command that carries it, so it marks that part and not the line, and
    /// a dropped word passes what it expanded to the command that follows.
    substituted: Vec<bool>,
    /// Whether any part expands.
    has_substitution: bool,
    /// The loops each part runs in, as indices into `loops`, outermost
    /// first.
    enclosing: Vec<Vec<usize>>,
    /// Every `for`, `select`, `while` and `until` loop of the line, each
    /// with the variable a `for NAME in WORDS` header binds and its words,
    /// their quotes taken off; `None` for the rest, and for a header whose
    /// words expand other than as paths.
    loops: Vec<Option<(String, Vec<ShellWord>)>>,
    /// The loops open where the reading has got to.
    open_loops: Vec<usize>,
}

/// `command` as the POSIX reading takes it — the text itself, or the POSIX a
/// fish command means — and what that reading made of it; `None` for a fish
/// command that cannot be read that way.
fn parse_in(command: &str, shell: ShellSyntax) -> Option<(Cow<'_, str>, ParsedShell)> {
    match shell {
        ShellSyntax::Posix => Some((Cow::Borrowed(command), split_posix(command, false))),
        ShellSyntax::Zsh => Some((Cow::Borrowed(command), split_posix(command, true))),
        ShellSyntax::Fish => {
            let posix = crate::fish::as_posix(command)?;
            let parsed = split_posix(&posix, false);
            Some((Cow::Owned(posix), parsed))
        }
    }
}

/// [`split_posix`] for bash and the POSIX shells.
fn split_shell(command: &str) -> ParsedShell {
    split_posix(command, false)
}

/// Split a command line into its simple commands, honouring quotes,
/// backslash escapes and here-documents: the text of a `<<EOF` body is data
/// for the command before it, not commands of its own, so it is skipped —
/// though an unquoted delimiter's body still expands substitutions. With
/// `zsh`, a parenthesis inside a command's words expands too: zsh runs code
/// from a glob qualifier `*(e:…:)`, an expansion flag `${(e)…}` and `=(…)`.
fn split_posix(command: &str, zsh: bool) -> ParsedShell {
    let mut parsed = ParsedShell::default();
    let mut current = String::new();
    // Whether the command being read expands; carried to the part it becomes,
    // or to the next one when this segment is not a command at all.
    let mut expands = false;
    let mut quote: Option<char> = None;
    // The substitutions open here, innermost last: `(` for `$(…)`, `` ` ``
    // for a backquoted one. Their operators belong to the substitution, so
    // they are not separators of the line around it. A backquote closes the
    // backquoted substitution it is in and opens one otherwise.
    let mut open: Vec<char> = Vec::new();
    // Here-documents opened on the current line: delimiter, whether tabs
    // are stripped (`<<-`), whether the body expands (unquoted delimiter).
    let mut heredocs: Vec<(String, bool, bool)> = Vec::new();
    let chars: Vec<char> = command.chars().collect();
    let flags = if zsh { "${(" } else { "" };
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == '\\' && q == '"' && i + 1 < chars.len() {
                // An escaped character inside double quotes, `\"` included.
                current.push(c);
                current.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == q {
                quote = None;
            } else if q == '"'
                && (c == '`'
                    || (c == '$' && opens_substitution(&chars, i))
                    || (zsh && c == '$' && chars.get(i + 1..i + 3) == Some(&['{', '('])))
            {
                // Double quotes still expand substitutions.
                expands = true;
            }
            current.push(c);
            i += 1;
            continue;
        }
        match c {
            '\\' if i + 1 < chars.len() => {
                // An escaped character stands for itself; an escaped newline
                // continues the line.
                if chars[i + 1] != '\n' {
                    current.push(c);
                    current.push(chars[i + 1]);
                }
                i += 1;
            }
            '\'' | '"' => {
                quote = Some(c);
                current.push(c);
            }
            '`' => {
                expands = true;
                if open.last() == Some(&'`') {
                    open.pop();
                } else {
                    open.push('`');
                }
                current.push(c);
            }
            '$' if opens_substitution(&chars, i) => {
                expands = true;
                open.push(if chars[i + 1] == '(' { '(' } else { '{' });
                current.push(c);
            }
            ')' if open.last() == Some(&'(') => {
                open.pop();
                current.push(c);
            }
            '}' if open.last() == Some(&'{') => {
                open.pop();
                current.push(c);
            }
            // A subshell opens a command; a parenthesis later in it is zsh's.
            '(' if zsh && !current.trim().is_empty() => {
                expands = true;
                open.push('(');
                current.push(c);
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) != Some(&'<') => {
                let (heredoc, end) = heredoc_at(&chars, i);
                current.extend(&chars[i..end]);
                heredocs.extend(heredoc);
                i = end;
                continue;
            }
            '&' if chars.get(i + 1) == Some(&'&') && open.is_empty() => {
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
                i += 1;
            }
            // A lone `&` sends the command before it to the background and
            // starts the next one, a separator like `;`. It is part of a
            // redirection in `&>`, `&>>`, `>&2`, `2>&1` and `<&0`.
            '&' if open.is_empty()
                && chars.get(i + 1) != Some(&'>')
                && !matches!(i.checked_sub(1).map(|p| chars[p]), Some('>' | '<')) =>
            {
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
            }
            '|' if open.is_empty() => {
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
                // `||`, and `|&`, which pipes standard error too.
                if matches!(chars.get(i + 1), Some('|' | '&')) {
                    i += 1;
                }
            }
            '\n' if !heredocs.is_empty() => {
                // The bodies feed the command just ended, so they are read
                // while its segment is still open and mark it when they expand.
                i = skip_heredocs(&chars, i + 1, &mut heredocs, flags, &mut expands);
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
                continue;
            }
            ';' | '\n' if open.is_empty() => {
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
            }
            _ => current.push(c),
        }
        i += 1;
    }
    push_part(&mut parsed, &mut current, expands);
    if parsed.parts.is_empty() {
        parsed.parts.push(command.trim().to_string());
        parsed.substituted.push(false);
    }
    parsed.has_substitution = parsed.substituted.iter().any(|e| *e);
    parsed
}

/// Whether the `$` at `at` starts a command substitution: `$(…)`, or the
/// `${ …; }` and `${|…; }` forms of bash 5.3, ksh93 and mksh, which run the
/// command in the shell itself. `${NAME}` is a parameter.
fn opens_substitution(chars: &[char], at: usize) -> bool {
    match chars.get(at + 1) {
        Some('(') => true,
        Some('{') => chars
            .get(at + 2)
            .is_some_and(|c| c.is_whitespace() || *c == '|'),
        _ => false,
    }
}

/// The here-document `<<` at `start` opens, and where its operator and
/// delimiter end: `(delimiter, strip tabs, expands)`, or `None` when no
/// delimiter follows.
fn heredoc_at(chars: &[char], start: usize) -> (Option<(String, bool, bool)>, usize) {
    let mut j = start + 2;
    let strip = chars.get(j) == Some(&'-');
    if strip {
        j += 1;
    }
    while chars.get(j).is_some_and(|c| *c == ' ' || *c == '\t') {
        j += 1;
    }
    let mut delimiter = String::new();
    let mut quoted = false;
    while let Some(&c) = chars.get(j) {
        match c {
            '\'' | '"' => {
                quoted = true;
                j += 1;
                while let Some(&inner) = chars.get(j) {
                    j += 1;
                    if inner == c {
                        break;
                    }
                    delimiter.push(inner);
                }
            }
            '\\' => {
                quoted = true;
                j += 1;
            }
            c if c.is_whitespace() || ";|&<>()".contains(c) => break,
            c => {
                delimiter.push(c);
                j += 1;
            }
        }
    }
    let heredoc = (!delimiter.is_empty()).then_some((delimiter, strip, !quoted));
    (heredoc, j)
}

/// Skip the bodies of the here-documents opened on the line just ended,
/// from `start`, each up to its delimiter line; returns where the commands
/// go on. An unquoted delimiter's body that substitutes marks `expands`, the
/// command being fed by them.
fn skip_heredocs(
    chars: &[char],
    start: usize,
    heredocs: &mut Vec<(String, bool, bool)>,
    flags: &str,
    expands: &mut bool,
) -> usize {
    let mut i = start;
    for (delimiter, strip, body_expands) in heredocs.drain(..) {
        while i < chars.len() {
            let end = chars[i..]
                .iter()
                .position(|c| *c == '\n')
                .map_or(chars.len(), |n| i + n);
            let line: String = chars[i..end].iter().collect();
            i = (end + 1).min(chars.len());
            let line = if strip {
                line.trim_start_matches('\t')
            } else {
                line.as_str()
            };
            if line == delimiter {
                break;
            }
            let line_chars: Vec<char> = line.chars().collect();
            let substitutes = line_chars
                .iter()
                .enumerate()
                .any(|(at, c)| *c == '`' || (*c == '$' && opens_substitution(&line_chars, at)));
            let flagged = !flags.is_empty() && line.contains(flags);
            if body_expands && (substitutes || flagged) {
                *expands = true;
            }
        }
    }
    i
}

/// One part of a command line as the rules see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellPart {
    /// As written.
    pub text: String,
    /// With the program's path resolved against the directory the earlier
    /// `cd`s left — project-relative inside the project, absolute outside —
    /// so a rule means the same program wherever the command started; the
    /// text itself for a program found on `PATH` or in an unknown directory.
    pub resolved: String,
    /// Whether a rule can be recorded for it: not when its program's
    /// directory is unknown, nor when it runs a command substitution.
    pub savable: bool,
    /// Whether the part expands a command substitution of its own. A
    /// substitution asks, but it asks about the part that carries it and not
    /// about the parts beside it, so a `cd && ls && echo $(date)` line asks
    /// about the one unknown command instead of listing all three.
    pub substituted: bool,
    /// The directory it runs in, as the `cd`s before it left it; `None` when
    /// that cannot be told.
    pub dir: Option<std::path::PathBuf>,
    /// The variables of the `for NAME in WORDS` loops it runs in, innermost
    /// last, with the words each takes in turn — only where nothing else
    /// on the line can give that variable another value or move the loop
    /// to another directory between its turns. A word that is a pattern
    /// takes, as in the shell, the paths it matches.
    pub loop_values: Vec<(String, Vec<ShellWord>)>,
}

/// [`shell_parts_in`] for bash and the POSIX shells.
#[must_use]
pub fn shell_parts(command: &str, cwd: &Path) -> Vec<ShellPart> {
    shell_parts_in(command, ShellSyntax::Posix, cwd)
}

/// Split `command`, in the language of `shell`, into its parts, following
/// `cd`, `pushd` and `popd` from `cwd` so each part's program resolves
/// against the directory it runs in. A `cd` that cannot be followed (a
/// variable, `-`, a substitution, a subshell) leaves the directory unknown
/// for the parts after it. A fish command's parts are those of the POSIX it
/// means, in POSIX words; one that cannot be read that way is one part, a
/// substitution no rule can stand for.
#[must_use]
pub fn shell_parts_in(command: &str, shell: ShellSyntax, cwd: &Path) -> Vec<ShellPart> {
    let Some((command, parsed)) = parse_in(command, shell) else {
        return vec![ShellPart {
            text: command.to_string(),
            resolved: command.to_string(),
            savable: false,
            substituted: true,
            dir: None,
            loop_values: Vec::new(),
        }];
    };
    let command = command.as_ref();
    let mut dir = Some(normalize(cwd));
    let mut stack: Vec<Option<std::path::PathBuf>> = Vec::new();
    let bindings = loop_bindings(command, &parsed);
    let moving = moving_loops(&parsed);
    parsed
        .parts
        .iter()
        .zip(parsed.substituted.iter())
        .zip(parsed.enclosing.iter())
        .map(|((text, &substituted), enclosing)| {
            let mut words = text.split_whitespace();
            let head = words.next().unwrap_or("");
            let rest = &text.trim_start()[head.len()..];
            // A loop that changes directory runs each turn but the first
            // somewhere else, and leaves after any number of turns: from
            // its first part on the directory, and every one `pushd` kept,
            // is unknown until an absolute `cd` after the loop.
            let in_moving_loop = enclosing.iter().any(|&l| moving[l]);
            if in_moving_loop {
                dir = None;
                stack.iter_mut().for_each(|kept| *kept = None);
            }
            let runs_in = dir.clone();
            let (resolved, known) = match resolve_program(head, dir.as_deref(), cwd) {
                Some(program) => (format!("{program}{rest}"), true),
                None if head.contains('/') => (text.clone(), false),
                None => (text.clone(), true),
            };
            match head {
                "cd" => dir = change_dir(dir.as_deref(), words.next()),
                "pushd" => {
                    stack.push(dir.clone());
                    dir = change_dir(dir.as_deref(), words.next());
                }
                "popd" => dir = stack.pop().flatten(),
                _ => {}
            }
            // A subshell's `cd` may or may not last; do not guess.
            if text.starts_with('(') || text.ends_with(')') || in_moving_loop {
                dir = None;
            }
            ShellPart {
                savable: known && !substituted,
                substituted,
                text: text.clone(),
                resolved,
                dir: runs_in,
                loop_values: enclosing
                    .iter()
                    .filter_map(|&l| bindings[l].clone())
                    .collect(),
            }
        })
        .collect()
}

/// Whether each loop of `parsed` runs a `cd`, `pushd` or `popd`, in its
/// header, its body or a loop nested in it.
fn moving_loops(parsed: &ParsedShell) -> Vec<bool> {
    let mut moving = vec![false; parsed.loops.len()];
    for (text, enclosing) in parsed.parts.iter().zip(&parsed.enclosing) {
        let head = text.split_whitespace().next().unwrap_or("");
        if matches!(head, "cd" | "pushd" | "popd") {
            for &l in enclosing {
                moving[l] = true;
            }
        }
    }
    moving
}

/// The binding of each loop of `parsed` that a part in it can rely on: its
/// header's, while every part the loop runs is a look-only command of its
/// own (no `cd`, no substitution, no subshell) and `command` names the
/// variable nowhere but in a `for` header and as a plain `$NAME` or
/// `${NAME}`, so no assignment, `read`, `${NAME:=…}` or function changes
/// it. A line that touches `IFS` binds nothing, its words splitting
/// differently.
fn loop_bindings(command: &str, parsed: &ParsedShell) -> Vec<Option<(String, Vec<ShellWord>)>> {
    parsed
        .loops
        .iter()
        .enumerate()
        .map(|(index, binding)| {
            let (name, _) = binding.as_ref()?;
            let body_looks_only = parsed
                .parts
                .iter()
                .zip(&parsed.substituted)
                .zip(&parsed.enclosing)
                .filter(|(_, enclosing)| enclosing.contains(&index))
                .all(|((text, &substituted), _)| {
                    let head = text.split_whitespace().next().unwrap_or("");
                    !substituted
                        && !matches!(head, "cd" | "pushd" | "popd")
                        && !text.starts_with('(')
                        && !text.ends_with(')')
                        && is_read_only_command(text)
                });
            (body_looks_only
                && !command.contains("IFS")
                && names_only_as_loop_variable(command, name))
            .then(|| binding.clone())
            .flatten()
        })
        .collect()
}

/// Whether `command` names the variable `name` only in a `for` header and
/// in plain expansions of it.
fn names_only_as_loop_variable(command: &str, name: &str) -> bool {
    let is_name_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    command.match_indices(name).all(|(at, _)| {
        let before = &command[..at];
        let after = &command[at + name.len()..];
        if before.ends_with(is_name_char) || after.starts_with(is_name_char) {
            return true; // part of a longer word
        }
        before.ends_with('$')
            || (before.ends_with("${") && after.starts_with('}'))
            || (before.ends_with(char::is_whitespace)
                && before
                    .trim_end()
                    .strip_suffix("for")
                    .is_some_and(|b| b.is_empty() || b.ends_with(|c: char| !is_name_char(c))))
    })
}

/// The look-only commands that read the files their arguments name.
const FILE_READERS: [&str; 20] = [
    "ls", "cat", "head", "tail", "wc", "grep", "egrep", "fgrep", "stat", "du", "cut", "sort",
    "tree", "uniq", "rg", "file", "find", "sed", "git", "date",
];

/// Whether a look-only `part` reads beyond `project`, so that it asks where
/// the `read` tool would: a path argument or an input redirection that leads
/// outside it (absolute, `~`, a `..` or a symlink stepping out, a glob that
/// could match `..` or matches such a path — or more paths than
/// [`MAX_GLOB_MATCHES`], or one taken for an option), a file reader run in
/// a directory outside it or in one that cannot be told, a variable whose
/// value cannot be seen, or the whole environment dumped (`env`,
/// `printenv`), secrets and all.
fn reads_outside(part: &ShellPart, project: &Path) -> bool {
    let text = without_harmless_redirections(&part.text);
    let head = text.split_whitespace().next().unwrap_or("");
    let reader = FILE_READERS.contains(&head);
    let project = real_path(project);
    let dir = part.dir.as_deref();
    let reads_files = reader || text.contains('<');
    // The globs take the paths they match, as the shell expands them, each
    // pattern once however many turns the loops take.
    let mut globs = GlobExpansion::new(dir);
    // A loop variable counts when each word it takes is one the command
    // could name itself: a path inside the project, no option, nothing the
    // shell would split. The command is then judged with every value in.
    let mut loop_values: Vec<(&str, Vec<String>)> = Vec::new();
    for (name, words) in &part.loop_values {
        let values: Option<Vec<String>> = words.iter().try_fold(Vec::new(), |mut all, word| {
            all.extend(globs.expand(word)?);
            Some(all)
        });
        let Some(values) = values else { continue };
        let usable = values.iter().all(|value| {
            !value.is_empty()
                && !value.starts_with('-')
                && !value.contains(char::is_whitespace)
                && !path_outside(value, dir, &project)
        });
        if usable {
            loop_values.push((name.as_str(), values));
        }
    }
    let turns: usize = loop_values.iter().map(|(_, v)| v.len()).product();
    if turns > MAX_LOOP_TURNS {
        return reads_files;
    }
    (0..turns).any(|turn| {
        let mut rest = turn;
        let values: Vec<(&str, &str)> = loop_values
            .iter()
            .map(|(name, values)| {
                let value = values[rest % values.len()].as_str();
                rest /= values.len();
                (*name, value)
            })
            .collect();
        // A variable expands to a path no one can see beforehand.
        let Some(words) = globbing_words(&text, &values) else {
            return reads_files;
        };
        let mut expanded = Vec::with_capacity(words.len());
        for (index, word) in words.iter().enumerate() {
            if index == 0 || !reads_files {
                expanded.push(word.text.clone());
                continue;
            }
            match globs.expand(word) {
                Some(paths) => expanded.extend(paths),
                None => return true,
            }
        }
        words_read_outside(head, reader, &expanded, dir, &project)
    })
}

/// The glob expansions of the words of one look-only command run in `dir`,
/// remembered by pattern and sharing one [`MAX_GLOB_MATCHES`] budget.
struct GlobExpansion<'d> {
    dir: Option<&'d Path>,
    budget: usize,
    seen: HashMap<String, Option<Vec<String>>>,
}

impl<'d> GlobExpansion<'d> {
    fn new(dir: Option<&'d Path>) -> Self {
        Self {
            dir,
            budget: MAX_GLOB_MATCHES,
            seen: HashMap::new(),
        }
    }

    /// The words `word` becomes (see [`expand_glob`]); `None` when the
    /// command should ask instead: the matches pass the budget, or one
    /// starts with `-` and would be taken for an option. An input
    /// redirection (`<*`, `0<src/*`) expands its target.
    fn expand(&mut self, word: &ShellWord) -> Option<Vec<String>> {
        let Some(pattern) = &word.glob else {
            return Some(vec![word.text.clone()]);
        };
        if let Some(cached) = self.seen.get(pattern) {
            return cached.clone();
        }
        let digits = word.text.len()
            - word
                .text
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .len();
        let redirect = word.text[digits..].starts_with('<')
            && !word.text[digits..].starts_with("<<")
            && !word.text[digits..].starts_with("<&");
        // The prefix holds no glob character, so it is spelled alike in the
        // text and the pattern.
        let prefix = if redirect { digits + 1 } else { 0 };
        let target = ShellWord {
            text: word.text[prefix..].to_string(),
            glob: pattern.get(prefix..).map(str::to_string),
        };
        let expanded = expand_glob(&target, self.dir, &mut self.budget).and_then(|paths| {
            let literal = paths.len() == 1 && paths[0] == target.text;
            if !literal && paths.iter().any(|p| p.starts_with('-')) {
                return None;
            }
            Some(
                paths
                    .into_iter()
                    .map(|path| format!("{}{path}", &word.text[..prefix]))
                    .collect(),
            )
        });
        self.seen.insert(pattern.clone(), expanded.clone());
        expanded
    }
}

/// The most turns of the loops around a command [`reads_outside`] judges it
/// for, one by one; beyond that it asks.
const MAX_LOOP_TURNS: usize = 256;

/// Whether the look-only command `head` with the shell `words` reads beyond
/// `project` (a real path), run in `dir`; `reader` when it is one of
/// [`FILE_READERS`].
fn words_read_outside(
    head: &str,
    reader: bool,
    words: &[String],
    dir: Option<&Path>,
    project: &Path,
) -> bool {
    if matches!(head, "env" | "printenv") && words.len() == 1 {
        return true;
    }
    let outside = |word: &str| path_outside(word, dir, project);
    // The first operand of these is a pattern or a script, not a file,
    // unless an option gives that.
    let patterned = matches!(head, "grep" | "egrep" | "fgrep" | "rg" | "sed");
    let args: Vec<&str> = words[1..].iter().map(String::as_str).collect();
    if follows_symlinks(head, &args) {
        return true;
    }
    let mut pattern_pending = patterned
        && !has_short_flag(&args, 'e')
        && !has_short_flag(&args, 'f')
        && !args.iter().any(|a| {
            ["--regexp", "--expression", "--file"]
                .iter()
                .any(|o| a.split_once('=').map_or(*a, |(name, _)| name) == *o)
        });
    // Options whose separate value is a pattern, never a file.
    let pattern_value: &[&str] = match head {
        "grep" | "egrep" | "fgrep" => &["-e", "--regexp"],
        "sed" => &["-e", "--expression"],
        "rg" => &[
            "-e",
            "--regexp",
            "-g",
            "--glob",
            "--iglob",
            "-r",
            "--replace",
            "-t",
            "--type",
            "-T",
            "--type-not",
            "--type-add",
        ],
        _ => &[],
    };
    let mut operands_only = false;
    let mut operands = 0;
    let mut i = 0;
    while i < args.len() {
        let word = args[i];
        i += 1;
        // An input redirection, which any command reads: `< file`, `<file`,
        // `0<file`; `<&3` and here-documents and -strings read no file.
        if let Some(target) = word
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .strip_prefix('<')
        {
            if target.starts_with(['<', '&']) {
                continue;
            }
            let target = if target.is_empty() {
                i += 1;
                args.get(i - 1).copied().unwrap_or("")
            } else {
                target
            };
            if outside(target) {
                return true;
            }
            continue;
        }
        if !reader {
            continue;
        }
        if !operands_only && word == "--" {
            operands_only = true;
            continue;
        }
        if !operands_only && word.starts_with('-') && word.len() > 1 {
            if pattern_value.contains(&word) {
                i += 1;
            } else if let Some((_, value)) = word.strip_prefix("--").and_then(|w| w.split_once('='))
            {
                if outside(value) {
                    return true;
                }
            }
            continue;
        }
        if pattern_pending {
            pattern_pending = false;
            continue;
        }
        if outside(word) {
            return true;
        }
        operands += 1;
    }
    // Without a file to read the others read standard input; these read
    // the directory they run in, and `git` that repository's config.
    let reads_dir = match head {
        "ls" | "du" | "tree" | "find" | "git" => true,
        "rg" => operands == 0,
        "grep" | "egrep" | "fgrep" => {
            operands == 0
                && (has_short_flag(&args, 'r')
                    || has_short_flag(&args, 'R')
                    || has_long_option(&args, "recursive")
                    || has_long_option(&args, "dereference-recursive"))
        }
        _ => false,
    };
    reads_dir && dir.is_none_or(|dir| !real_path(dir).starts_with(project))
}

/// Whether the look-only command `head` with `args` follows the symlinks it
/// meets while walking a tree: an in-project link can lead it anywhere, so
/// such a walk counts as a read beyond the project. A symlink named as an
/// operand is not meant here — [`path_outside`] already judges where it
/// leads.
fn follows_symlinks(head: &str, args: &[&str]) -> bool {
    match head {
        "rg" => has_short_flag(args, 'L') || has_long_option(args, "follow"),
        "grep" | "egrep" | "fgrep" => {
            has_short_flag(args, 'R') || has_long_option(args, "dereference-recursive")
        }
        "ls" | "du" => has_short_flag(args, 'L') || has_long_option(args, "dereference"),
        "tree" => has_short_flag(args, 'l'),
        // `-H` and `-L` lead the arguments, alone or together (`-HL`);
        // `-follow` is an expression.
        "find" => args.iter().any(|a| {
            *a == "-follow"
                || a.strip_prefix('-').is_some_and(|flags| {
                    !flags.is_empty()
                        && flags.chars().all(|c| matches!(c, 'H' | 'L' | 'P'))
                        && flags.contains(['H', 'L'])
                })
        }),
        _ => false,
    }
}

/// Whether the shell word `word`, a path run from `dir`, leads outside
/// `project` (a real path); `/dev/null` and `-` (standard input) do not.
fn path_outside(word: &str, dir: Option<&Path>, project: &Path) -> bool {
    if word.is_empty() || word == "-" || word == "/dev/null" {
        return false;
    }
    // `.*` and `.?` match `..` in some shells, `{..,x}` spells it out.
    let dotted_glob = word
        .split('/')
        .any(|c| c.starts_with('.') && c.contains(['*', '?', '[']));
    if dotted_glob || (word.contains('{') && word.contains("..")) {
        return true;
    }
    let path = if word == "~" || word.starts_with("~/") {
        match home() {
            Some(home) => home.join(word[1..].trim_start_matches('/')),
            None => return true,
        }
    } else if word.starts_with('~') {
        // Another user's home.
        return true;
    } else if Path::new(word).is_absolute() {
        std::path::PathBuf::from(word)
    } else {
        match dir {
            Some(dir) => dir.join(word),
            None => return true,
        }
    };
    !real_path(&path).starts_with(project)
}

/// The home directory, for `~`.
fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// Where `cd target` leads from `dir`; `None` when it cannot be told.
fn change_dir(dir: Option<&Path>, target: Option<&str>) -> Option<std::path::PathBuf> {
    let target = target.unwrap_or("~");
    let quoted = target.starts_with(['"', '\'']);
    if quoted && !(target.len() > 1 && target.ends_with(&target[..1])) {
        return None; // a quoted path with spaces, split apart
    }
    let target = target.trim_matches(['"', '\'']);
    if target == "-" || target.contains(['$', '`', '(', '*', '?']) {
        return None;
    }
    let path = if target == "~" {
        home()?
    } else if let Some(rest) = target.strip_prefix("~/") {
        home()?.join(rest)
    } else if Path::new(target).is_absolute() {
        std::path::PathBuf::from(target)
    } else {
        dir?.join(target)
    };
    Some(normalize(&path))
}

/// A program named by a path, resolved from `dir` and shown as file tools
/// show paths: project-relative inside `project`, absolute outside. `None`
/// for a bare name (found on `PATH`) or a relative path from an unknown
/// directory.
fn resolve_program(head: &str, dir: Option<&Path>, project: &Path) -> Option<String> {
    if !head.contains('/') {
        return None;
    }
    let head = head.trim_matches(['"', '\'']);
    let path = if Path::new(head).is_absolute() {
        std::path::PathBuf::from(head)
    } else if let Some(rest) = head.strip_prefix("~/") {
        home()?.join(rest)
    } else {
        dir?.join(head)
    };
    Some(relative_to_project(
        &normalize(&path).to_string_lossy(),
        project,
    ))
}

/// Push `current` as a part when it is a command, and say whether it was: a
/// dropped word — a `for … in` header in particular — is not a command, but
/// what it expanded is still run, so `expands` is left set for the command
/// that follows it to carry.
fn push_part(parsed: &mut ParsedShell, current: &mut String, expands: bool) -> bool {
    track_loops(parsed, current.trim());
    let kept = match without_shell_keywords(current.trim()) {
        Some(part) => {
            parsed.substituted.push(expands);
            parsed.parts.push(part.to_string());
            parsed.enclosing.push(parsed.open_loops.clone());
            true
        }
        None => false,
    };
    current.clear();
    kept
}

/// Open the loops the segment `part` starts — a `while` or `until` among its
/// leading reserved words, a `for` or `select` header — and close the one a
/// `done` ends.
fn track_loops(parsed: &mut ParsedShell, part: &str) {
    let words: Vec<&str> = part.split_whitespace().collect();
    let leading = words
        .iter()
        .take_while(|w| {
            matches!(
                **w,
                "if" | "then" | "else" | "elif" | "do" | "while" | "until" | "!" | "{" | "time"
            )
        })
        .count();
    for word in &words[..leading] {
        if matches!(*word, "while" | "until") {
            parsed.open_loops.push(parsed.loops.len());
            parsed.loops.push(None);
        }
    }
    match words.get(leading) {
        Some(&"done") => {
            parsed.open_loops.pop();
        }
        Some(&head @ ("for" | "select")) => {
            // The reserved words before the header are plain words, so the
            // header's words start where they end.
            let binding = (head == "for")
                .then(|| globbing_words(part, &[]))
                .flatten()
                .and_then(|words| match words.get(leading + 1..)? {
                    [name, keyword, values @ ..]
                        if keyword.text == "in"
                            && is_shell_name(&name.text)
                            && !values.is_empty() =>
                    {
                        Some((name.text.clone(), values.to_vec()))
                    }
                    _ => None,
                });
            parsed.open_loops.push(parsed.loops.len());
            parsed.loops.push(binding);
        }
        _ => {}
    }
}

/// Whether `word` can name a shell variable.
fn is_shell_name(word: &str) -> bool {
    word.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `part` without the shell's reserved words, which run nothing of their
/// own: after `if`, `then`, `do`, `!` and the like, the command that follows
/// is what runs, while a closing word (`fi`, `done`, `esac`, `}`) or a
/// `for`/`select`/`case` header runs nothing — a substitution in a header
/// still marks the whole line. `None` when nothing is left to judge.
fn without_shell_keywords(part: &str) -> Option<&str> {
    const LEADING: [&str; 10] = [
        "if", "then", "else", "elif", "do", "while", "until", "!", "{", "time",
    ];
    let mut rest = part;
    loop {
        let (word, tail) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(word, tail)| (word, tail.trim_start()));
        if !LEADING.contains(&word) {
            break;
        }
        rest = tail;
    }
    let head = rest.split_whitespace().next()?;
    let closes = matches!(head, "fi" | "done" | "esac" | "}")
        && without_harmless_redirections(rest).trim() == head;
    if closes || matches!(head, "for" | "select" | "case") {
        return None;
    }
    Some(rest)
}

/// `part` without the redirections that write no file: one stream pointed
/// at another (`2>&1`, `>&2`) or at `/dev/null`, either spelled in one word
/// or with the target apart (`2> /dev/null`).
fn without_harmless_redirections(part: &str) -> String {
    let words: Vec<&str> = part.split_whitespace().collect();
    let mut kept = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        let target = word
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '&')
            .trim_start_matches(">>")
            .trim_start_matches('>');
        let is_redirection = word
            .trim_start_matches(|c: char| c.is_ascii_digit() || c == '&')
            .starts_with('>');
        if is_redirection && target.is_empty() && words.get(i + 1) == Some(&"/dev/null") {
            i += 2;
            continue;
        }
        let harmless = target == "/dev/null"
            || target
                .strip_prefix('&')
                .is_some_and(|fd| !fd.is_empty() && fd.chars().all(|c| c.is_ascii_digit()));
        if !(is_redirection && harmless) {
            kept.push(word);
        }
        i += 1;
    }
    kept.join(" ")
}

/// Commands that only read state; a redirection into a file disqualifies a
/// command, while pointing one stream at another (`2>&1`) or at `/dev/null`
/// does not. A command that can write a file or run another program through
/// its arguments counts only when those arguments are absent.
#[must_use]
pub fn is_read_only_command(part: &str) -> bool {
    let part = without_harmless_redirections(part);
    if part.contains('>') || part.contains("<(") {
        return false;
    }
    // An array subscript is arithmetic in bash, and arithmetic runs the
    // substitutions in it however they were quoted: `test -v 'a[$(rm x)]'`.
    if part.contains('[') && (part.contains("$(") || part.contains('`')) {
        return false;
    }
    if evaluates_a_value(&part) {
        return false;
    }
    // Quotes and backslashes only join a word for the program: `-del''ete`
    // reaches `find` as `-delete`. Without them an option is seen as the
    // program gets it; a quoted space splitting a word in two can only make
    // a command look less read-only.
    let bare: String = part
        .chars()
        .filter(|c| !matches!(c, '\'' | '"' | '\\'))
        .collect();
    let words: Vec<&str> = bare.split_whitespace().collect();
    let Some((&head, args)) = words.split_first() else {
        return false;
    };
    // Commands no argument turns into a write. `cd`, `pushd` and `popd`
    // only move the rest of the one command line.
    const PLAIN: [&str; 29] = [
        "ls", "cat", "head", "tail", "wc", "pwd", "echo", "grep", "egrep", "fgrep", "which",
        "stat", "du", "cut", "tr", "basename", "dirname", "realpath", "printenv", "whoami",
        "uname", "true", "false", "cd", "pushd", "popd", "[", "[[", "test",
    ];
    if matches!(head, "[" | "[[" | "test") && evaluates_an_operand(args) {
        return false;
    }
    if PLAIN.contains(&head) {
        return true;
    }
    match head {
        // `env <command>` runs that command.
        "env" => args.is_empty(),
        "date" => !args.iter().any(|a| *a == "-s" || a.starts_with("--set")),
        // `--compress-program` runs a program on the temporary files.
        "sort" => {
            !has_short_flag(args, 'o')
                && !has_long_option(args, "output")
                && !has_long_option(args, "compress-program")
        }
        // `-o` writes the listing to a file, `-R` writes one per directory.
        "tree" => !has_short_flag(args, 'o') && !has_short_flag(args, 'R'),
        // A second operand is the file `uniq` writes to.
        "uniq" => args.iter().filter(|a| !a.starts_with('-')).count() <= 1,
        // `--pre` runs a program on every file searched (`--pre-glob` picks
        // the files), `--hostname-bin` one to name the host for hyperlinks.
        // ripgrep takes no abbreviated long options.
        "rg" => !args.iter().any(|a| {
            let name = a.split_once('=').map_or(*a, |(name, _)| name);
            matches!(name, "--pre" | "--pre-glob" | "--hostname-bin")
        }),
        // `-C` compiles a magic file next to the source.
        "file" => !args.iter().any(|a| *a == "-C" || *a == "--compile"),
        "find" => !["-delete", "-exec", "-ok", "-fprint", "-fls"]
            .iter()
            .any(|action| bare.contains(action)),
        "git" => is_read_only_git(args),
        "gh" | "glab" | "tea" => is_read_only_forge(head, args),
        "sed" => shell_words(&part).is_some_and(|words| is_print_only_sed(&words[1..])),
        _ => false,
    }
}

/// Whether a `${…}` in `part` takes a variable's value as code: bash
/// evaluates the value of a name in arithmetic — an array subscript
/// (`${a[i]}`) or a substring's offset and length (`${s:i:n}`) — as an
/// expression, running the substitutions in it, and runs them in what
/// `${!x}` names and in the prompt `${x@P}` expands. A session keeps the
/// variables an earlier command set, so the value cannot be told from the
/// command alone. Quotes are not looked at: asking about a `${…}` single
/// quotes keep literal costs little.
fn evaluates_a_value(part: &str) -> bool {
    let mut rest = part;
    while let Some(at) = rest.find("${") {
        rest = &rest[at + 2..];
        let body = &rest[..rest.find('}').unwrap_or(rest.len())];
        if body.starts_with('!') {
            return true;
        }
        let name = body.strip_prefix('#').unwrap_or(body);
        let name_end = name
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(name.len());
        let mut after = &name[name_end..];
        if let Some(index) = after.strip_prefix('[') {
            let end = index.find(']').unwrap_or(index.len());
            let index_is_code = !(index[..end] == *"@"
                || index[..end] == *"*"
                || index[..end].chars().all(|c| c.is_ascii_digit()));
            if index_is_code {
                return true;
            }
            after = index.get(end + 1..).unwrap_or("");
        }
        let offset_is_code = after.strip_prefix(':').is_some_and(|offset| {
            !offset.starts_with(['-', '=', '?', '+'])
                && !offset
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ':' | ' ' | '-'))
        });
        if after.starts_with('@') || offset_is_code {
            return true;
        }
    }
    false
}

/// Whether a test (`[`, `[[`, `test`) evaluates an operand as code: one
/// beside an arithmetic comparison that is not a number, which bash takes
/// as an expression — a name's value included, and the substitutions in
/// it — or the variable `-v` asks about, given by a value or with a
/// subscript.
fn evaluates_an_operand(args: &[&str]) -> bool {
    let number = |word: &str| {
        let digits = word.strip_prefix(['-', '+']).unwrap_or(word);
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
    };
    args.iter().enumerate().any(|(i, word)| match *word {
        "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => {
            let before = i.checked_sub(1).and_then(|i| args.get(i));
            let after = args.get(i + 1);
            !before.is_some_and(|w| number(w)) || !after.is_some_and(|w| number(w))
        }
        "-v" => args
            .get(i + 1)
            .is_some_and(|name| name.contains(['$', '[', '`'])),
        _ => false,
    })
}

/// The words of a simple command with their quotes taken off, or `None`
/// where a word would expand (a parameter, a substitution, a backquote; a
/// glob in double quotes is fine) or a quote is left open. A `$` that starts
/// no expansion, as in `rg "foo$"`, is the character itself.
fn shell_words(part: &str) -> Option<Vec<String>> {
    shell_words_with(part, &[])
}

/// [`shell_words`] with the variables in `values` expanded, `$NAME` and
/// `${NAME}` alike, to the value given (the last one for a name given
/// twice); any other expansion still gives `None`.
fn shell_words_with(part: &str, values: &[(&str, &str)]) -> Option<Vec<String>> {
    Some(
        globbing_words(part, values)?
            .into_iter()
            .map(|word| word.text)
            .collect(),
    )
}

/// A word of a simple command: its text with the quotes taken off and,
/// when an unquoted `*`, `?` or `[` makes the shell expand it into the
/// paths it matches, the pattern it is matched as — every quoted character
/// escaped, a run of `*` taken for one (no `globstar`), `[^…]` for `[!…]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellWord {
    pub text: String,
    pub glob: Option<String>,
}

impl ShellWord {
    /// A word the shell takes as written.
    #[must_use]
    pub fn literal(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            glob: None,
        }
    }
}

/// A [`ShellWord`] being read.
#[derive(Default)]
struct WordBuilder {
    text: String,
    pattern: String,
    globbed: bool,
    /// The last character, when it was unquoted.
    last_unquoted: Option<char>,
}

impl WordBuilder {
    fn quoted(&mut self, c: char) {
        self.text.push(c);
        self.pattern
            .push_str(&glob::Pattern::escape(c.encode_utf8(&mut [0; 4])));
        self.last_unquoted = None;
    }

    fn unquoted(&mut self, c: char) {
        self.text.push(c);
        match c {
            '*' if self.last_unquoted == Some('*') => {}
            '^' if self.last_unquoted == Some('[') => self.pattern.push('!'),
            c => self.pattern.push(c),
        }
        self.globbed |= matches!(c, '*' | '?' | '[');
        self.last_unquoted = Some(c);
    }

    fn finish(self) -> ShellWord {
        ShellWord {
            text: self.text,
            glob: self.globbed.then_some(self.pattern),
        }
    }
}

/// [`shell_words_with`] keeping, for each word, the pattern it expands as.
/// A variable's value counts as unquoted where the `$` is, as the shell
/// expands it.
fn globbing_words(part: &str, values: &[(&str, &str)]) -> Option<Vec<ShellWord>> {
    let mut words = Vec::new();
    let mut word: Option<WordBuilder> = None;
    let mut chars = part.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => words.extend(word.take().map(WordBuilder::finish)),
            '\'' => {
                let current = word.get_or_insert_with(WordBuilder::default);
                loop {
                    match chars.next()? {
                        '\'' => break,
                        c => current.quoted(c),
                    }
                }
            }
            '"' => {
                let current = word.get_or_insert_with(WordBuilder::default);
                loop {
                    match chars.next()? {
                        '"' => break,
                        '$' if starts_expansion(chars.peek().copied(), true) => {
                            expand_variable(&mut chars, values)?
                                .chars()
                                .for_each(|c| current.quoted(c));
                        }
                        '`' => return None,
                        '\\' => current.quoted(chars.next()?),
                        c => current.quoted(c),
                    }
                }
            }
            '$' if starts_expansion(chars.peek().copied(), false) => {
                let current = word.get_or_insert_with(WordBuilder::default);
                expand_variable(&mut chars, values)?
                    .chars()
                    .for_each(|c| current.unquoted(c));
            }
            '`' => return None,
            '\\' => word
                .get_or_insert_with(WordBuilder::default)
                .quoted(chars.next()?),
            c => word.get_or_insert_with(WordBuilder::default).unquoted(c),
        }
    }
    words.extend(word.map(WordBuilder::finish));
    Some(words)
}

/// The most paths the globs of a look-only command may match before it
/// asks instead of judging each.
const MAX_GLOB_MATCHES: usize = 1024;

/// The words the shell turns `word` into in `dir`: the paths its pattern
/// matches, sorted and spelled as the shell spells them (relative to `dir`
/// for a relative pattern), or the word itself when it has no pattern or
/// nothing matches, as `shell` leaves it by default. A dotfile matches only
/// a pattern that names its leading `.`. A pattern left as it is: a
/// relative one when `dir` is unknown, one with a component like `.*`
/// (which some shells let match `..`), `~user/…` — [`path_outside`] asks
/// about each. `None` once the matches would pass `budget`, which they
/// take from.
fn expand_glob(word: &ShellWord, dir: Option<&Path>, budget: &mut usize) -> Option<Vec<String>> {
    let literal = || Some(vec![word.text.clone()]);
    let Some(pattern) = &word.glob else {
        return literal();
    };
    let dotted = word
        .text
        .split('/')
        .any(|c| c.starts_with('.') && c.contains(['*', '?', '[']));
    if dotted {
        return literal();
    }
    let escaped = |path: &Path| path.to_str().map(glob::Pattern::escape);
    let (full, strip) = if Path::new(pattern).is_absolute() {
        (pattern.clone(), None)
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        match home() {
            Some(home) => (format!("{}/{rest}", escaped(&home)?), None),
            None => return literal(),
        }
    } else if pattern.starts_with('~') {
        return literal();
    } else {
        match dir {
            Some(dir) => (format!("{}/{pattern}", escaped(dir)?), Some(dir)),
            None => return literal(),
        }
    };
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: true,
    };
    let Ok(paths) = glob::glob_with(&full, options) else {
        return literal(); // an unclosed `[` is a plain character
    };
    let mut matches = Vec::new();
    for path in paths.flatten() {
        *budget = budget.checked_sub(1)?;
        let shown = strip
            .and_then(|dir| path.strip_prefix(dir).ok())
            .unwrap_or(&path);
        matches.push(shown.to_str()?.to_string());
    }
    if matches.is_empty() {
        return literal();
    }
    matches.sort();
    Some(matches)
}

/// The value in `values` of the variable `chars` names just after a `$`,
/// as `NAME` or `{NAME}`; `None` for any other expansion or a name without
/// a value given.
fn expand_variable<'v>(
    chars: &mut std::iter::Peekable<std::str::Chars>,
    values: &[(&str, &'v str)],
) -> Option<&'v str> {
    let braced = chars.next_if_eq(&'{').is_some();
    let mut name = String::new();
    while let Some(c) = chars.next_if(|c| c.is_ascii_alphanumeric() || *c == '_') {
        name.push(c);
    }
    if !is_shell_name(&name) || (braced && chars.next() != Some('}')) {
        return None;
    }
    values
        .iter()
        .rev()
        .find(|(n, _)| *n == name)
        .map(|(_, value)| *value)
}

/// Whether a `$` followed by `next` expands: a name, a positional or special
/// parameter, `${…}`, `$(…)` or `$[…]`. Outside double quotes `$'…'` and
/// `$"…"` are quotes of their own whose escapes can spell any text.
fn starts_expansion(next: Option<char>, in_double_quotes: bool) -> bool {
    next.is_some_and(|c| {
        c.is_ascii_alphanumeric()
            || "_@*#?$!-{([".contains(c)
            || (!in_double_quotes && (c == '\'' || c == '"'))
    })
}

/// `sed` that only prints: no in-place edit, no script file, and every
/// script made of addresses and the commands that print, number, stop or
/// skip lines (`p`, `l`, `=`, `q`, `Q`, `d`). Substitution, `w`, `r`, `e`
/// and the rest are left out, so nothing is written or run.
fn is_print_only_sed(args: &[String]) -> bool {
    let mut scripts: Vec<&str> = Vec::new();
    let mut operands: Vec<&str> = Vec::new();
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        match arg {
            "-n" | "--quiet" | "--silent" | "-E" | "-r" | "--regexp-extended" | "-u"
            | "--unbuffered" | "-z" | "--null-data" | "-s" | "--separate" | "--posix"
            | "--sandbox" => {}
            "-e" | "--expression" => match args.next() {
                Some(script) => scripts.push(script),
                None => return false,
            },
            _ if arg.starts_with("--expression=") => scripts.push(&arg["--expression=".len()..]),
            _ if arg.starts_with("-e") => scripts.push(&arg[2..]),
            // Bundled short flags, `-nE`; `-i` (in place) and `-f` (a script
            // file) are not among them.
            _ if arg.starts_with('-') && arg.len() > 1 => {
                if arg.starts_with("--") || !arg[1..].chars().all(|c| "nEruzs".contains(c)) {
                    return false;
                }
            }
            _ => operands.push(arg),
        }
    }
    if scripts.is_empty() {
        match operands.first() {
            Some(script) => scripts.push(script),
            None => return false,
        }
    }
    scripts.iter().all(|script| print_only_script(script))
}

/// Whether a `sed` script is only addresses with `p`, `l`, `=`, `q`, `Q` or
/// `d`, separated by `;` or new lines.
fn print_only_script(script: &str) -> bool {
    let chars: Vec<char> = script.chars().collect();
    let mut i = 0;
    let skip_blank = |i: &mut usize| {
        while *i < chars.len() && chars[*i].is_whitespace() {
            *i += 1;
        }
    };
    loop {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ';') {
            i += 1;
        }
        if i == chars.len() {
            return true;
        }
        if sed_address(&chars, &mut i) {
            skip_blank(&mut i);
            if chars.get(i) == Some(&',') {
                i += 1;
                skip_blank(&mut i);
                if !sed_address(&chars, &mut i) {
                    return false;
                }
            }
        }
        skip_blank(&mut i);
        if chars.get(i) == Some(&'!') {
            i += 1;
            skip_blank(&mut i);
        }
        match chars.get(i) {
            Some('p' | 'l' | '=' | 'd') => i += 1,
            Some('q' | 'Q') => {
                i += 1;
                skip_blank(&mut i);
                while chars.get(i).is_some_and(char::is_ascii_digit) {
                    i += 1;
                }
            }
            _ => return false,
        }
        skip_blank(&mut i);
        if !matches!(chars.get(i), None | Some(';')) {
            return false;
        }
    }
}

/// Reads one `sed` address at `i` (a line number, `first~step`, `$`,
/// `+N`, `~N`, or a `/regex/` with an optional `I` or `M`), moving past it;
/// `false`, unmoved, when there is none.
fn sed_address(chars: &[char], i: &mut usize) -> bool {
    let start = *i;
    match chars.get(*i) {
        Some(c) if c.is_ascii_digit() => {
            while chars.get(*i).is_some_and(char::is_ascii_digit) {
                *i += 1;
            }
            if chars.get(*i) == Some(&'~') {
                *i += 1;
                while chars.get(*i).is_some_and(char::is_ascii_digit) {
                    *i += 1;
                }
            }
            true
        }
        Some('$') => {
            *i += 1;
            true
        }
        // GNU's `addr,+N` and `addr,~N`, as the end of a range.
        Some('+' | '~') if chars.get(*i + 1).is_some_and(char::is_ascii_digit) => {
            *i += 1;
            while chars.get(*i).is_some_and(char::is_ascii_digit) {
                *i += 1;
            }
            true
        }
        Some('/') => {
            *i += 1;
            loop {
                match chars.get(*i) {
                    None => {
                        *i = start;
                        return false;
                    }
                    Some('\\') => *i += 2,
                    Some('/') => {
                        *i += 1;
                        break;
                    }
                    Some(_) => *i += 1,
                }
            }
            if matches!(chars.get(*i), Some('I' | 'M')) {
                *i += 1;
            }
            true
        }
        _ => false,
    }
}

/// Whether the command runs a program its own name does not tell: a wrapper
/// taking the program as an argument, an inline script, a script named by a
/// path, or a manager pulling an image or a target. A rule for such a command
/// would vouch for every program it might ever run.
#[must_use]
fn delegating_command(part: &str) -> bool {
    let words: Vec<&str> = part.split_whitespace().collect();
    let Some((&head, args)) = words.split_first() else {
        return false;
    };
    if matches!(
        head,
        "env"
            | "xargs"
            | "parallel"
            | "timeout"
            | "watch"
            | "nohup"
            | "eval"
            | "nice"
            | "time"
            | "arch"
            | "script"
    ) {
        return true;
    }
    // A script or an interpreter running a snippet: what runs is in the file
    // or the argument, not in the name.
    if matches!(head, "sh" | "bash" | "zsh" | "fish" | "csh" | "dash") {
        return true;
    }
    if head.contains('/') {
        return true;
    }
    if matches!(
        head,
        "python" | "python2" | "python3" | "ruby" | "perl" | "node" | "php"
    ) {
        // `python -c`, and a bare script path, run code the rule cannot show.
        // `python --version` and the like do not.
        return args.iter().any(|a| *a == "-c" || !a.starts_with('-'));
    }
    // `make` runs the targets its Makefile names, `nix`/`orb` a program pulled
    // from outside; both are code the pattern does not name.
    if matches!(head, "make" | "gmake" | "nix" | "orb") {
        return true;
    }
    // `docker run`, `kubectl run`: the image and its command are the payload.
    matches!(
        head,
        "docker" | "podman" | "kubectl" | "npm" | "pnpm" | "yarn" | "bun" | "uv"
    ) && args.iter().any(|a| matches!(*a, "run" | "exec" | "start"))
}

/// Whether a command removes a directory no reviewer should be trusted with:
/// the filesystem root, the home directory, or the working directory or one
/// above it (its contents by a trailing `*` included). `auto` mode always
/// asks the user about these.
#[must_use]
fn critical_removal(part: &str, cwd: &Path) -> bool {
    let words: Vec<&str> = part.split_whitespace().collect();
    let Some((&head, args)) = words.split_first() else {
        return false;
    };
    let program = head.rsplit('/').next().unwrap_or(head);
    if !matches!(program, "rm" | "rmdir") {
        return false;
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let cwd = normalize(cwd);
    args.iter()
        .map(|arg| arg.trim_matches(['"', '\'']))
        .filter(|arg| !arg.starts_with('-'))
        .any(|arg| {
            // `dir/*` empties `dir`; a bare `*` empties the working directory.
            let arg = arg.trim_end_matches('*');
            let home_rest = ["~", "$HOME", "${HOME}"].iter().find_map(|name| {
                let rest = arg.strip_prefix(name)?;
                (rest.is_empty() || rest.starts_with('/')).then(|| rest.trim_start_matches('/'))
            });
            let path = match (home_rest, &home) {
                (Some(rest), Some(home)) => home.join(rest),
                _ if Path::new(arg).is_absolute() => std::path::PathBuf::from(arg),
                _ => cwd.join(arg),
            };
            let path = normalize(&path);
            path.parent().is_none()
                || home.as_deref() == Some(path.as_path())
                || cwd.starts_with(&path)
        })
}

/// Whether a command destroys what it cannot recover or reaches beyond this
/// machine: the question that decides whether "allow always" is on offer. A
/// rule for a build, an edit or a `git checkout` states its scope and what it
/// changes can be got back; a rule for `rm -rf`, a `git clean`, a force push
/// or a publish cannot be undone or says nothing about what it destroys.
#[must_use]
fn destructive_command(part: &str) -> bool {
    let words: Vec<&str> = part.split_whitespace().collect();
    let Some((&head, args)) = words.split_first() else {
        return false;
    };
    // Deleting or overwriting what no undo reaches, or acting as another
    // account. `chmod`/`chown` are recoverable but open or lock files down.
    if matches!(
        head,
        "rm" | "rmdir"
            | "unlink"
            | "shred"
            | "dd"
            | "mkfs"
            | "sudo"
            | "doas"
            | "truncate"
            | "chmod"
            | "chown"
    ) {
        return true;
    }
    let sub = args.first().copied().unwrap_or("");
    match head {
        "git" => match sub {
            // A push leaves the machine; a clean, a stash drop or a gc deletes
            // what the repository cannot bring back.
            "push" | "clean" | "gc" | "filter-branch" | "restore" => true,
            "rebase" | "reset" => {
                args[1..].iter().any(|a| {
                    *a == "--hard"
                        || *a == "-f"
                        || *a == "--force"
                        || *a == "--force-rebase"
                        || *a == "--no-ff"
                }) || matches!(args.get(1).copied(), Some("hard") | Some("--hard"))
            }
            // Listing looks; deleting and rewiring do not.
            "branch" | "tag" => args[1..]
                .iter()
                .any(|a| matches!(*a, "-d" | "-D" | "--delete")),
            "worktree" => !matches!(args.get(1).copied(), None | Some("list")),
            "stash" => matches!(args.get(1).copied(), Some("drop" | "clear")),
            "remote" => !matches!(
                args.get(1).copied(),
                None | Some("show") | Some("get-url") | Some("-v") | Some("--verbose")
            ),
            _ => false,
        },
        // Installing, removing and publishing change what is on the machine
        // or put something on it from outside.
        "cargo" | "npm" | "pnpm" | "yarn" | "bun" | "uv" | "pip" | "pip3" => matches!(
            sub,
            "install"
                | "uninstall"
                | "remove"
                | "rm"
                | "publish"
                | "unpublish"
                | "yank"
                | "add"
                | "vendor"
        ),
        // `kubectl delete`, `docker rm`, `gh issue close`, `terraform destroy`.
        "kubectl" | "docker" | "podman" | "gh" | "terraform" | "brew" | "apt" | "yum" => {
            words.iter().skip(1).any(|w| {
                matches!(
                    *w,
                    "delete"
                        | "destroy"
                        | "remove"
                        | "rm"
                        | "down"
                        | "purge"
                        | "uninstall"
                        | "close"
                        | "merge"
                        | "kill"
                )
            })
        }
        _ => false,
    }
}

/// Whether a long option `--name`, or an abbreviation of it, is among
/// `args`, alone or with `=value`: GNU tools and git take any unambiguous
/// prefix of a long option's name (`--out=x` for `--output=x`).
fn has_long_option(args: &[&str], name: &str) -> bool {
    args.iter().any(|a| {
        a.strip_prefix("--")
            .map(|a| a.split_once('=').map_or(a, |(key, _)| key))
            .is_some_and(|key| !key.is_empty() && name.starts_with(key))
    })
}

/// Whether a short-option word such as `-ro` carries `flag`.
fn has_short_flag(args: &[&str], flag: char) -> bool {
    args.iter()
        .any(|a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains(flag))
}

/// Forge CLIs — `gh` (GitHub), `glab` (GitLab), `tea` (Gitea) — in the
/// forms that only read. `--web` is left out, since it starts a browser, and
/// so is each `api`, which sends any request it is given.
fn is_read_only_forge(cli: &str, args: &[&str]) -> bool {
    if args
        .iter()
        .any(|a| *a == "--web" || *a == "-w" || *a == "--show-token")
    {
        return false;
    }
    let words = forge_words(args);
    let (group, action, then) = (
        words.first().copied(),
        words.get(1).copied(),
        words.get(2).copied(),
    );
    let listing = |action: Option<&str>| matches!(action, None | Some("list" | "ls"));
    let index = |action: Option<&str>| action.is_some_and(|a| a.parse::<u64>().is_ok());
    match (cli, group) {
        (_, None) => false,
        ("gh", Some(group)) => match group {
            "issue" => matches!(action, Some("list" | "view" | "status")),
            "pr" => matches!(action, Some("list" | "view" | "status" | "diff" | "checks")),
            "run" | "workflow" | "release" | "repo" => matches!(action, Some("list" | "view")),
            "label" => action == Some("list"),
            "search" => true,
            _ => false,
        },
        // `ci view` is an interactive view that also runs, retries and
        // cancels jobs, and `ci status` offers to retry from its prompt.
        ("glab", Some(group)) => match group {
            "issue" | "release" => matches!(action, Some("list" | "view")),
            "mr" => matches!(
                action,
                Some("list" | "view" | "diff" | "approvers" | "issues")
            ),
            "ci" => matches!(action, Some("list" | "get" | "trace")),
            "repo" => matches!(action, Some("list" | "view" | "search" | "contributors")),
            "label" | "milestone" => matches!(action, Some("list" | "get")),
            _ => false,
        },
        // A bare `tea issues` lists and `tea issues 5` shows one; a bare
        // `tea comments 5 text`, by contrast, posts the text.
        ("tea", Some(group)) => match group {
            "issues" | "issue" | "i" => listing(action) || index(action),
            "pulls" | "pull" | "pr" => {
                listing(action) || index(action) || matches!(action, Some("review-comments" | "rc"))
            }
            "repos" | "repo" => {
                listing(action)
                    || matches!(action, Some("search" | "s"))
                    || action.is_some_and(|a| a.contains('/'))
            }
            "releases" | "release" | "r" | "labels" | "label" | "milestones" | "milestone"
            | "ms" | "branches" | "branch" | "b" | "notifications" | "notification" | "n"
            | "times" | "time" | "t" => listing(action),
            "comments" | "comment" | "c" => matches!(action, Some("list" | "ls")),
            "actions" | "action" => match action {
                Some("runs" | "run") => {
                    matches!(
                        then,
                        Some("list" | "ls" | "view" | "show" | "get" | "logs" | "log")
                    )
                }
                Some("workflows" | "workflow") => {
                    matches!(then, Some("list" | "ls" | "view" | "show" | "get"))
                }
                _ => false,
            },
            "whoami" => true,
            _ => false,
        },
        _ => false,
    }
}

/// The positional words of a forge CLI call: options are dropped, and so is
/// the value of an option that takes one as a word of its own (`-R owner/repo`,
/// `--state closed`), so the value is not taken for a subcommand.
fn forge_words<'a>(args: &[&'a str]) -> Vec<&'a str> {
    // Options that take a value in every CLI that has them.
    const TAKES_VALUE: [&str; 19] = [
        "-R",
        "--repo",
        "-r",
        "-o",
        "--output",
        "-F",
        "-l",
        "--login",
        "--label",
        "--state",
        "-L",
        "--limit",
        "-p",
        "--page",
        "-P",
        "--per-page",
        "-q",
        "--jq",
        "--json",
    ];
    let mut words = Vec::with_capacity(args.len());
    let mut iter = args.iter();
    while let Some(&arg) = iter.next() {
        if TAKES_VALUE.contains(&arg) {
            iter.next();
        } else if !arg.starts_with('-') {
            words.push(arg);
        }
    }
    words
}

/// The words of a `git branch` or `git tag` listing with the value of each
/// listing option given as a word of its own (`--merged main`,
/// `--sort -committerdate`) left out, so the value is not taken for the name
/// of a branch or tag to create. The options are kept, so they still have to
/// be listing ones.
fn without_flag_values<'a>(args: &[&'a str]) -> Vec<&'a str> {
    const TAKES_VALUE: [&str; 7] = [
        "--contains",
        "--no-contains",
        "--merged",
        "--no-merged",
        "--points-at",
        "--sort",
        "--format",
    ];
    let mut kept = Vec::with_capacity(args.len());
    let mut iter = args.iter().peekable();
    while let Some(&arg) = iter.next() {
        kept.push(arg);
        if TAKES_VALUE.contains(&arg) {
            // `--sort` takes `-committerdate`; the others take a commit.
            iter.next_if(|next| arg == "--sort" || !next.starts_with('-'));
        }
    }
    kept
}

/// `git` subcommands that only look: `branch`, `tag`, `remote`, `stash`,
/// `worktree`, `reflog` and `config` count only when they list or get, since
/// the same subcommands also delete, rename and rewire.
fn is_read_only_git(mut args: &[&str]) -> bool {
    // Global options that only choose how to look. `-c` is left out, since
    // it can set a pager or an alias that runs a program, and so is `-C`:
    // another repository's config (`core.fsmonitor`, `diff.external`) runs
    // programs of its choosing even on `git status`.
    while let ["--no-pager" | "-P" | "--no-optional-locks", rest @ ..] = args {
        args = rest;
    }
    let Some((&sub, rest)) = args.split_first() else {
        return false;
    };
    match sub {
        "status" | "blame" | "rev-parse" | "ls-files" | "ls-tree" | "cat-file" | "describe"
        | "merge-base" | "for-each-ref" | "show-ref" | "name-rev" | "count-objects" => true,
        // `-O` opens the matches in a pager program of its choosing.
        "grep" => !has_short_flag(rest, 'O') && !has_long_option(rest, "open-files-in-pager"),
        "stash" => matches!(rest.first(), Some(&("list" | "show"))),
        "worktree" => matches!(rest.first(), Some(&"list")),
        // `expire` and `delete` rewrite the log; a ref name shows it, but is
        // left to ask rather than told apart from a future subcommand.
        "reflog" => match rest.first() {
            None => true,
            Some(&first) => matches!(first, "show" | "list" | "exists") || first.starts_with('-'),
        },
        "config" => {
            const READ: [&str; 6] = [
                "-l",
                "--list",
                "--get",
                "--get-all",
                "--get-regexp",
                "--get-urlmatch",
            ];
            const WRITE: [&str; 9] = [
                "--add",
                "--unset",
                "--unset-all",
                "--replace-all",
                "--rename-section",
                "--remove-section",
                "-e",
                "--edit",
                "--set",
            ];
            // `git config user.email` reads the key; a second word would set
            // it, and a subcommand's name is not a key.
            let positional: Vec<&&str> = rest.iter().filter(|a| !a.starts_with('-')).collect();
            let bare_key = matches!(positional.as_slice(), [key] if !matches!(
                **key,
                "set" | "unset" | "edit" | "rename-section" | "remove-section"
            ));
            let reads = matches!(rest.first(), Some(&("get" | "list")))
                || rest.iter().any(|a| READ.contains(a))
                || bare_key;
            reads && !rest.iter().any(|a| WRITE.contains(a))
        }
        "diff" | "log" | "show" | "rev-list" | "shortlog" | "cherry" | "range-diff" => {
            !has_long_option(rest, "output")
        }
        // Without `--list`, a bare name creates a tag; `-d`, `-a`, `-s`,
        // `-f` and the rest are left out.
        "tag" => {
            const LISTING: [&str; 16] = [
                "-l",
                "--list",
                "-n",
                "--column",
                "--no-column",
                "-i",
                "--ignore-case",
                "--color",
                "--no-color",
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--points-at",
                "--sort",
                "--format",
            ];
            const LISTING_WITH_VALUE: [&str; 9] = [
                "--sort=",
                "--format=",
                "--color=",
                "--column=",
                "--contains=",
                "--no-contains=",
                "--merged=",
                "--no-merged=",
                "--points-at=",
            ];
            let list = rest.iter().any(|a| *a == "-l" || *a == "--list");
            without_flag_values(rest).iter().all(|a| {
                LISTING.contains(a)
                    || LISTING_WITH_VALUE.iter().any(|p| a.starts_with(p))
                    || a.strip_prefix("-n")
                        .is_some_and(|n| n.chars().all(|c| c.is_ascii_digit()))
                    || (list && !a.starts_with('-'))
            })
        }
        "branch" => {
            const LISTING: [&str; 22] = [
                "-a",
                "--all",
                "-r",
                "--remotes",
                "-v",
                "-vv",
                "--verbose",
                "--show-current",
                "--color",
                "--no-color",
                "--column",
                "--no-column",
                "-i",
                "--ignore-case",
                "--contains",
                "--no-contains",
                "--merged",
                "--no-merged",
                "--omit-empty",
                "--points-at",
                "--sort",
                "--format",
            ];
            const LISTING_WITH_VALUE: [&str; 10] = [
                "--sort=",
                "--format=",
                "--color=",
                "--column=",
                "--contains=",
                "--no-contains=",
                "--merged=",
                "--no-merged=",
                "--points-at=",
                "--abbrev=",
            ];
            // Without `--list`, a bare name creates a branch; with it, a
            // name is a pattern to list.
            let list = rest.iter().any(|a| *a == "-l" || *a == "--list");
            without_flag_values(rest).iter().all(|a| {
                *a == "-l"
                    || *a == "--list"
                    || LISTING.contains(a)
                    || LISTING_WITH_VALUE.iter().any(|p| a.starts_with(p))
                    || (list && !a.starts_with('-'))
            })
        }
        "remote" => matches!(rest, [] | ["-v" | "--verbose"] | ["show" | "get-url", ..]),
        _ => false,
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod prompter_tests {
    use super::*;
    use crate::message::ToolCall;
    use serde_json::json;

    fn request() -> PermissionRequest {
        PermissionRequest {
            tool: "shell".into(),
            subject: "git push".into(),
            call: ToolCall {
                id: "c".into(),
                name: "shell".into(),
                arguments: json!({ "command": "git push" }),
                extra_content: None,
            },
            suggested_pattern: "git push *".into(),
            can_persist: true,
            can_allow_session: true,
            parts: Vec::new(),
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

    #[test]
    fn a_delegates_request_names_it_and_ends_with_its_task() {
        let (panel, rx) = permission_channel(CancelToken::new());
        let mut delegate = panel.for_delegate("search", CancelToken::new());
        let worker = std::thread::spawn(move || delegate.ask(&request()));
        let envelope = rx.recv().unwrap();
        assert_eq!(envelope.delegate.as_deref(), Some("search"));
        envelope.reply.send(PermissionAnswer::AllowOnce).unwrap();
        assert_eq!(worker.join().unwrap(), PermissionAnswer::AllowOnce);

        // The task stopped, the panel's run did not: the card is withdrawn.
        let stop = CancelToken::new();
        let mut delegate = panel.for_delegate("search", stop.clone());
        let worker = std::thread::spawn(move || delegate.ask(&request()));
        let envelope = rx.recv().unwrap();
        stop.cancel();
        assert_eq!(worker.join().unwrap(), PermissionAnswer::Deny);
        assert!(envelope.withdrawn.is_cancelled());
    }
}

#[cfg(test)]
mod granted_tests {
    use super::*;

    #[test]
    fn granted_rules_count_beside_the_configured_ones_and_stay_out_of_the_file() {
        let mut rules = PermissionRules::default();
        rules.add("shell", "rm *", Decision::Deny);
        add_rule(&mut rules.granted, "shell", "cargo *", Decision::Allow);
        add_rule(&mut rules.granted, "shell", "*", Decision::Allow);
        assert_eq!(rules.evaluate("shell", "cargo test"), Some(Decision::Allow));
        // The strictest still wins.
        assert_eq!(rules.evaluate("shell", "rm -rf x"), Some(Decision::Deny));
        assert_eq!(
            rules.evaluate_narrow("shell", "cargo test"),
            Some(Decision::Allow)
        );
        let text = toml::to_string(&rules).unwrap();
        assert!(!text.contains("cargo"), "{text}");
    }
}
