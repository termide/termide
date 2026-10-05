//! Permission rules and the hook that enforces them.
//!
//! Rules live per tool as `pattern = decision` tables; among the rules that
//! match a call the strictest wins (`deny` over `ask` over `allow`). The mode
//! decides which rules count and what happens to calls none covers: only
//! `configured` takes the configured `allow` rules in full (`auto` takes the
//! narrow ones), every mode keeps their `deny` and `ask`, and answers given
//! "for this session" count everywhere but in `all`. In `auto` a reviewer
//! model decides what would otherwise be a prompt. Reading inside the project, loading a skill and a short list
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
use crate::classifier::{Classifier, Verdict};
use crate::message::ToolCall;
use crate::refusals::Refusals;
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
/// made only of read-only parts without substitution.
#[must_use]
pub fn is_read_only_call(call: &ToolCall) -> bool {
    match call.name.as_str() {
        // The web tools read the web; nothing on this machine changes. A
        // question to the user changes nothing either, nor does `recall`,
        // which only reads the project's history and code.
        "read" | "skill" | "fetch" | "web_search" | "question" | "recall" => true,
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
    fn before_tool_call(&mut self, call: &ToolCall, _ctx: &ToolContext) -> ToolDecision {
        if self.mode.get() == Mode::Plan && !is_read_only_call(call) {
            self.refused = true;
            ToolDecision::Block {
                reason: self.reason.clone(),
            }
        } else {
            ToolDecision::Allow
        }
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

    /// [`PermissionRules::evaluate`] without the broad `allow` rules `auto`
    /// mode sets aside.
    #[must_use]
    pub fn evaluate_narrow(&self, tool: &str, subject: &str) -> Option<Decision> {
        self.tools
            .get(tool)?
            .iter()
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
    tool == "bash" && delegating_command(&format!("{fixed} x"))
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
        if self.tool == "bash" {
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

    /// Whether a person answers; a prompter that answers alone is recorded
    /// as such, not as the user.
    fn attended(&self) -> bool {
        true
    }
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
}

/// Cloneable, so the hooks of calls that come in another way — tools served
/// to an external agent — ask on the same channel the panel answers.
#[derive(Clone)]
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

    /// The verdict before any prompt: the rules that count in the mode
    /// (the strictest wins), then the mode's own answer and the built-in
    /// safe defaults.
    #[must_use]
    pub fn decide(&self, call: &ToolCall, ctx: &ToolContext) -> Decision {
        let subject = subject_of(call, ctx);
        let mode = self.mode.get();
        if call.name == "bash" {
            return self.judge_command(&subject, ctx).decision;
        }

        if let Some(decision) = self.rule(&call.name, &subject) {
            return decision;
        }
        let inside = inside_project(call, ctx);
        match (mode, call.name.as_str()) {
            (Mode::All, _) => Decision::Allow,
            (_, "read") if inside => Decision::Allow,
            // Loads a skill's own text, which may live outside the project;
            // a question is already put to the user, so is never asked about.
            // `suggest_command` changes nothing itself — it shows a card and
            // the user runs what is on it or does not — so it asks no one
            // twice; the panel withholds `[Run]` where the mode or a rule
            // forbids the command. `recall` reads the project's own session
            // logs (under the configuration directory), its git history and
            // its code, and writes nothing.
            (_, "skill" | "question" | "suggest_command" | "recall") => Decision::Allow,
            (Mode::Plan | Mode::Edit, "fetch" | "web_search") => Decision::Allow,
            (Mode::Edit | Mode::Auto, "edit" | "write") if inside => Decision::Allow,
            // A query reads the web; a fetched URL can carry data out, so the
            // reviewer sees it.
            (Mode::Auto, "web_search") => Decision::Allow,
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
        let parts = shell_parts(command, &ctx.cwd);
        let mut verdict = Decision::Allow;
        let mut forced = false;
        if parts.len() > 1 {
            if let Some(whole) = self.rule("bash", command).filter(|d| *d != Decision::Allow) {
                forced |= whole == Decision::Ask;
                verdict = verdict.max(whole);
            }
        }
        let mut asked = Vec::new();
        for part in parts {
            let matched = self
                .rule("bash", &part.text)
                .into_iter()
                .chain(self.rule("bash", &part.resolved))
                .max();
            forced |= matched == Some(Decision::Ask);
            let decision = match matched {
                // A rule cannot vouch for a substitution, nor for a program
                // whose directory is unknown; the parts beside them, which a
                // rule does cover, are not dragged into it.
                Some(Decision::Allow) if part.substituted || !part.savable => Decision::Ask,
                Some(decision) => decision,
                None if mode == Mode::All => Decision::Allow,
                None if !part.substituted && is_read_only_command(&part.text) => Decision::Allow,
                // Plan mode refuses a command that could change something.
                None if mode == Mode::Plan => Decision::Deny,
                None => Decision::Ask,
            };
            if decision == Decision::Ask {
                asked.push(AskedPart {
                    pattern: part
                        .savable
                        .then(|| suggested_pattern("bash", &part.resolved)),
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
                let (parts, forced) = if call.name == "bash" {
                    let verdict = self.judge_command(&subject, ctx);
                    (verdict.asked, verdict.forced)
                } else {
                    (
                        Vec::new(),
                        self.rule(&call.name, &subject) == Some(Decision::Ask),
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
                let destructive = parts.iter().any(|p| destructive_command(&p.text));
                let delegating = parts.iter().any(|p| delegating_command(&p.text));
                let can_persist = matches!(self.mode.get(), Mode::Configured | Mode::Auto)
                    && self.persist.is_some()
                    && !bundle
                    && !destructive
                    && !delegating
                    && (call.name != "bash" || parts[0].pattern.is_some());
                // A destructive call is allowed once at a time; `all` is the
                // mode that lets a run through without asking. Denying for the
                // session stays on offer: it trusts nothing.
                let can_allow_session = !destructive;
                let suggested = if call.name == "bash" {
                    parts
                        .iter()
                        .filter_map(|p| p.pattern.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                } else {
                    suggested_pattern(&call.name, &subject)
                };
                let request = PermissionRequest {
                    tool: call.name.clone(),
                    suggested_pattern: suggested,
                    subject,
                    call: call.clone(),
                    can_persist,
                    can_allow_session,
                    parts,
                };
                let answer = self.prompter.ask(&request);
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
    /// Whether the part at the same index expands: a substitution it carries
    /// or a here-document body feeding it. A substitution belongs to the
    /// command that carries it, so it marks that part and not the line, and
    /// a dropped word passes what it expanded to the command that follows.
    substituted: Vec<bool>,
    /// Whether any part expands.
    has_substitution: bool,
}

/// Split a command line into its simple commands, honouring quotes,
/// backslash escapes and here-documents: the text of a `<<EOF` body is data
/// for the command before it, not commands of its own, so it is skipped —
/// though an unquoted delimiter's body still expands substitutions.
fn split_shell(command: &str) -> ParsedShell {
    let mut parsed = ParsedShell::default();
    let mut current = String::new();
    // Whether the command being read expands; carried to the part it becomes,
    // or to the next one when this segment is not a command at all.
    let mut expands = false;
    let mut quote: Option<char> = None;
    // An open `$(…)` or `` `…` ``: its operators belong to the substitution,
    // so they are not separators of the line around it.
    let mut depth = 0usize;
    // Here-documents opened on the current line: delimiter, whether tabs
    // are stripped (`<<-`), whether the body expands (unquoted delimiter).
    let mut heredocs: Vec<(String, bool, bool)> = Vec::new();
    let chars: Vec<char> = command.chars().collect();
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
            } else if q == '"' && (c == '`' || (c == '$' && chars.get(i + 1) == Some(&'('))) {
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
                depth += 1;
                current.push(c);
            }
            '$' if chars.get(i + 1) == Some(&'(') => {
                expands = true;
                depth += 1;
                current.push(c);
            }
            ')' if depth > 0 => {
                depth -= 1;
                current.push(c);
            }
            '<' if chars.get(i + 1) == Some(&'<') && chars.get(i + 2) != Some(&'<') => {
                let (heredoc, end) = heredoc_at(&chars, i);
                current.extend(&chars[i..end]);
                heredocs.extend(heredoc);
                i = end;
                continue;
            }
            '&' if chars.get(i + 1) == Some(&'&') && depth == 0 => {
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
                i += 1;
            }
            '|' if depth == 0 => {
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
                if chars.get(i + 1) == Some(&'|') {
                    i += 1;
                }
            }
            '\n' if !heredocs.is_empty() => {
                // The bodies feed the command just ended, so they are read
                // while its segment is still open and mark it when they expand.
                i = skip_heredocs(&chars, i + 1, &mut heredocs, &mut expands);
                expands = !push_part(&mut parsed, &mut current, expands) && expands;
                continue;
            }
            ';' | '\n' if depth == 0 => {
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
            if body_expands && (line.contains("$(") || line.contains('`')) {
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
}

/// Split `command` into its parts, following `cd`, `pushd` and `popd` from
/// `cwd` so each part's program resolves against the directory it runs in.
/// A `cd` that cannot be followed (a variable, `-`, a substitution, a
/// subshell) leaves the directory unknown for the parts after it.
#[must_use]
pub fn shell_parts(command: &str, cwd: &Path) -> Vec<ShellPart> {
    let mut dir = Some(normalize(cwd));
    let mut stack: Vec<Option<std::path::PathBuf>> = Vec::new();
    let parsed = split_shell(command);
    parsed
        .parts
        .iter()
        .zip(parsed.substituted.iter())
        .map(|(text, &substituted)| {
            let mut words = text.split_whitespace();
            let head = words.next().unwrap_or("");
            let rest = &text.trim_start()[head.len()..];
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
            if text.starts_with('(') || text.ends_with(')') {
                dir = None;
            }
            ShellPart {
                savable: known && !substituted,
                substituted,
                text: text.clone(),
                resolved,
            }
        })
        .collect()
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
    let kept = match without_shell_keywords(current.trim()) {
        Some(part) => {
            parsed.substituted.push(expands);
            parsed.parts.push(part.to_string());
            true
        }
        None => false,
    };
    current.clear();
    kept
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
    let words: Vec<&str> = part.split_whitespace().collect();
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
    if PLAIN.contains(&head) {
        return true;
    }
    match head {
        // `env <command>` runs that command.
        "env" => args.is_empty(),
        "date" => !args.iter().any(|a| *a == "-s" || a.starts_with("--set")),
        "sort" => !has_short_flag(args, 'o') && !args.iter().any(|a| a.starts_with("--output")),
        // `-o` writes the listing to a file, `-R` writes one per directory.
        "tree" => !has_short_flag(args, 'o') && !has_short_flag(args, 'R'),
        // A second operand is the file `uniq` writes to.
        "uniq" => args.iter().filter(|a| !a.starts_with('-')).count() <= 1,
        // `--pre` runs a program on every file searched.
        "rg" => !args
            .iter()
            .any(|a| *a == "--pre" || a.starts_with("--pre=")),
        // `-C` compiles a magic file next to the source.
        "file" => !args.iter().any(|a| *a == "-C" || *a == "--compile"),
        "find" => !["-delete", "-exec", "-ok", "-fprint", "-fls"]
            .iter()
            .any(|action| part.contains(action)),
        "git" => is_read_only_git(args),
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

/// Whether a short-option word such as `-ro` carries `flag`.
fn has_short_flag(args: &[&str], flag: char) -> bool {
    args.iter()
        .any(|a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains(flag))
}

/// `git` subcommands that only look: `branch` and `remote` count only when
/// they list, since the same subcommands also delete, rename and rewire.
fn is_read_only_git(args: &[&str]) -> bool {
    let Some((&sub, rest)) = args.split_first() else {
        return false;
    };
    match sub {
        "status" | "blame" | "rev-parse" | "ls-files" => true,
        "diff" | "log" | "show" => !rest.iter().any(|a| a.starts_with("--output")),
        "branch" => {
            const LISTING: [&str; 19] = [
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
            rest.iter().all(|a| {
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
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn ctx() -> ToolContext {
        ToolContext::new(PathBuf::from("/proj"))
    }

    fn call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "c".into(),
            name: name.into(),
            arguments: args,
            extra_content: None,
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
        assert_eq!(PermissionRules::default().mode, Mode::Auto);
        // The earlier names still read.
        assert_eq!(rules_of_mode("accept-edits"), Mode::Edit);
        // `auto` is its own mode now, not a spelling of `all`.
        assert_eq!(rules_of_mode("auto"), Mode::Auto);
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
        // `make *` is a broad pattern, yet in `auto` too an answer the user
        // gave for the session holds.
        for mode in [Mode::Ask, Mode::Edit, Mode::Configured, Mode::Auto] {
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
        assert_eq!(
            decide("env rm -rf ."),
            Decision::Ask,
            "env runs its command"
        );
        assert_eq!(decide("git branch -D main"), Decision::Ask);
        assert_eq!(decide("git branch -vv"), Decision::Allow);
        assert_eq!(
            decide("if [ -f Cargo.toml ]; then cargo build; fi"),
            Decision::Allow
        );
        assert_eq!(decide("[ -d target ] && cargo test"), Decision::Allow);
        assert_eq!(decide("for f in a b; do cat $f; done"), Decision::Allow);
        assert_eq!(decide("if true; then rm -rf x; fi"), Decision::Ask);
        assert_eq!(
            decide("for f in $(ls); do cat $f; done"),
            Decision::Ask,
            "substitution in a loop header"
        );
    }

    #[test]
    fn parts_resolve_their_program_from_the_directory_cd_left() {
        let resolved = |command: &str| -> Vec<(String, bool)> {
            shell_parts(command, Path::new("/proj"))
                .into_iter()
                .map(|p| (p.resolved, p.savable))
                .collect()
        };
        // Outside the project: absolute; inside: project-relative.
        assert_eq!(
            resolved("cd /tmp && ./astro/bin/pip install x && cd /proj/src && ./tool -v"),
            vec![
                ("cd /tmp".to_string(), true),
                ("/tmp/astro/bin/pip install x".to_string(), true),
                ("cd /proj/src".to_string(), true),
                ("src/tool -v".to_string(), true),
            ]
        );
        // `pushd` and `popd` are followed too; a bare name stays as written.
        assert_eq!(
            resolved("pushd /opt && popd && ./x; make"),
            vec![
                ("pushd /opt".to_string(), true),
                ("popd".to_string(), true),
                ("x".to_string(), true),
                ("make".to_string(), true),
            ]
        );
        // A `cd` that cannot be followed leaves the directory unknown, and a
        // relative program after it has no rule to be saved under.
        for unknown in [
            "cd $DIR && ./run",
            "cd - && ./run",
            "cd \"my dir\" && ./run",
        ] {
            let parts = resolved(unknown);
            assert_eq!(parts[1], ("./run".to_string(), false), "{unknown}");
        }
        // A substitution is never saved either.
        assert!(!resolved("echo $(date)")[0].1);
    }

    /// The command from a real session: it asked about `cd *`, the first
    /// part, and asked again next time, since the rest stayed unanswered.
    #[test]
    fn a_compound_command_asks_about_each_part_left_and_remembers_each() {
        let command = "cd /tmp && python3 -m venv astro && ./astro/bin/pip install -q pyswisseph 2>&1 | tail -3; ./astro/bin/python -c \"import swisseph;print(swisseph.version)\"";
        let (mut hooks, asked) = hooks(
            PermissionRules::default(),
            vec![PermissionAnswer::AllowSession],
        );
        assert_eq!(
            hooks.before_tool_call(&bash(command), &ctx()),
            ToolDecision::Allow
        );
        let request = asked.lock().unwrap()[0].clone();
        // `cd` and `tail` only look; the rest is asked about, each with its
        // own rule, the programs resolved.
        let patterns: Vec<String> = request
            .parts
            .iter()
            .filter_map(|p| p.pattern.clone())
            .collect();
        assert_eq!(
            patterns,
            [
                "python3 *",
                "/tmp/astro/bin/pip *",
                "/tmp/astro/bin/python *"
            ]
        );
        assert_eq!(request.parts[0].text, "python3 -m venv astro");
        assert!(request.can_remember());
        // The same command, and another with the same programs, pass now.
        assert_eq!(hooks.decide(&bash(command), &ctx()), Decision::Allow);
        assert_eq!(
            hooks.decide(&bash("cd /tmp/astro && ./bin/pip list"), &ctx()),
            Decision::Allow
        );
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_part_from_an_unknown_directory_is_answered_for_this_call_only() {
        let (mut hooks, asked) = hooks(
            PermissionRules::default(),
            vec![PermissionAnswer::AllowSession],
        );
        hooks.before_tool_call(&bash("cd $BUILD && ./run"), &ctx());
        let request = asked.lock().unwrap()[0].clone();
        assert_eq!(request.parts.len(), 1);
        assert_eq!(request.parts[0].pattern, None);
        assert!(!request.can_remember());
        // Nothing was recorded, so it asks again.
        assert_eq!(
            hooks.decide(&bash("cd $BUILD && ./run"), &ctx()),
            Decision::Ask
        );
        // And a rule written for the text cannot vouch for it.
        let mut rules = PermissionRules::default();
        rules.add("bash", "./run*", Decision::Allow);
        let (hooks, _) = super::tests::hooks(rules, vec![]);
        assert_eq!(
            hooks.decide(&bash("cd $BUILD && ./run"), &ctx()),
            Decision::Ask
        );
    }

    #[test]
    fn pointing_a_stream_elsewhere_is_not_writing_a_file() {
        for look in [
            "ls 2>&1",
            "rg TODO 2>/dev/null",
            "ls 2> /dev/null",
            "cat x &>/dev/null",
            "echo hi >&2",
            "cd /tmp",
        ] {
            assert!(is_read_only_command(look), "{look}");
        }
        for write in ["ls > out", "ls 2>err.txt", "echo x >> log", "cat x 2> err"] {
            assert!(!is_read_only_command(write), "{write}");
        }
    }

    /// The look-only list must not let through a command that writes or runs
    /// another program through its arguments.
    #[test]
    fn arguments_that_write_or_run_disqualify_a_look_only_command() {
        for look in [
            "env",
            "date",
            "date +%F",
            "sort -rn counts.txt",
            "tree -L 2",
            "uniq -c words.txt",
            "rg TODO src",
            "rg --pretty TODO",
            "file README.md",
            "find . -name '*.rs' -print",
            "git status",
            "git diff --stat",
            "git log --oneline -5",
            "git branch",
            "git branch -a -vv",
            "git branch --list 'feat/*'",
            "git branch --merged",
            "git branch --sort=-committerdate --format=%(refname:short)",
            "git remote",
            "git remote -v",
            "git remote show origin",
            "git remote get-url origin",
        ] {
            assert!(is_read_only_command(look), "{look}");
        }
        for write in [
            "env rm -rf .",
            "env FOO=1 sh -c 'rm -rf .'",
            "date -s 12:00",
            "date --set=12:00",
            "sort -o out.txt in.txt",
            "sort -ro out.txt in.txt",
            "sort --output=out.txt in.txt",
            "tree -o listing.txt",
            "tree -R -H .",
            "uniq in.txt out.txt",
            "rg --pre ./run.sh TODO",
            "rg --pre=./run.sh TODO",
            "file -C -m magic",
            "find . -fprint list.txt",
            "find . -fls list.txt",
            "find . -execdir rm {} +",
            "git diff --output=patch.diff",
            "git log --output=log.txt",
            "git branch feature",
            "git branch -D feature",
            "git branch --delete feature",
            "git branch -m old new",
            "git branch --set-upstream-to=origin/main",
            "git remote add fork https://example.com/fork.git",
            "git remote remove origin",
            "git remote set-url origin https://example.com/x.git",
            "git remote prune origin",
            "git -C /tmp status",
        ] {
            assert!(!is_read_only_command(write), "{write}");
        }
    }

    /// An inline script is one command, not the shell commands its lines
    /// would be.
    #[test]
    fn an_inline_script_is_not_split_into_commands() {
        let parts = |command: &str| split_shell(command).parts;
        // A here-document's body is skipped, whatever it holds.
        assert_eq!(
            parts("python3 - <<'EOF'\nimport os; print(os.getcwd())\nif x | y:\n    pass\nEOF"),
            vec!["python3 - <<'EOF'"]
        );
        // The commands after it are commands again.
        assert_eq!(
            parts("cat <<EOF > notes.txt\nsome; text\nEOF\nls -la"),
            vec!["cat <<EOF > notes.txt", "ls -la"]
        );
        assert_eq!(
            parts("cat <<-EOF\n\tbody\n\tEOF\npwd"),
            vec!["cat <<-EOF", "pwd"]
        );
        // An escaped quote does not end a quoted script.
        assert_eq!(
            parts("python3 -c \"print(\\\"a; b\\\")\nx = 1 | 2\""),
            vec!["python3 -c \"print(\\\"a; b\\\")\nx = 1 | 2\""]
        );
        assert_eq!(parts("echo a\\;b && ls"), vec!["echo a\\;b", "ls"]);
        // A here-string is one word.
        assert_eq!(parts("cat <<< \"a;b\""), vec!["cat <<< \"a;b\""]);
        // An unquoted delimiter's body still expands a substitution; a quoted
        // one's does not.
        assert!(split_shell("cat <<EOF\n$(rm -rf x)\nEOF").has_substitution);
        assert!(!split_shell("cat <<'EOF'\n$(rm -rf x)\nEOF").has_substitution);
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

    /// The shell's reserved words are not commands: the part is the command
    /// after them, and closing words and loop headers run nothing.
    #[test]
    fn shell_keywords_are_not_commands() {
        let parts = |command: &str| split_shell(command).parts;
        assert_eq!(
            parts("if [ -f Cargo.toml ]; then cargo build; else echo no; fi"),
            vec!["[ -f Cargo.toml ]", "cargo build", "echo no"]
        );
        assert_eq!(
            parts("for f in *.rs; do\n  wc -l \"$f\"\ndone"),
            vec!["wc -l \"$f\""]
        );
        assert_eq!(
            parts("while ! grep -q ready log; do sleep 1; done"),
            vec!["grep -q ready log", "sleep 1"]
        );
        assert_eq!(parts("{ ls; } 2>&1"), vec!["ls"]);
        assert_eq!(parts("time cargo test"), vec!["cargo test"]);
        // A closing word that writes stays to be judged.
        assert_eq!(
            parts("for f in a; do cat $f; done > out"),
            vec!["cat $f", "done > out"]
        );
        // A substitution in a header still marks the line.
        assert!(split_shell("for f in $(rm -rf x); do echo $f; done").has_substitution);
    }

    /// A substitution asks about the part that carries it, not about the
    /// parts beside it: a session of read commands should not be listed as
    /// seven grants because one of them expanded `$(…)`.
    #[test]
    fn a_substitution_asks_about_its_own_part_only() {
        let mut rules = PermissionRules::default();
        rules.add("bash", "cd *", Decision::Allow);
        rules.add("bash", "ls *", Decision::Allow);
        rules.add("bash", "echo *", Decision::Allow);
        rules.add("bash", "head *", Decision::Allow);
        let (hooks, _) = hooks(rules, vec![]);

        let CommandVerdict {
            decision: verdict,
            asked,
            ..
        } = hooks.judge_command("cd x && ls && echo hi && tail -1 f", &ctx());
        assert_eq!(verdict, Decision::Allow, "no part expands");
        assert!(asked.is_empty());

        let CommandVerdict {
            decision: verdict,
            asked,
            ..
        } = hooks.judge_command("cd x && ls && echo $(date) && head -1 f", &ctx());
        assert_eq!(verdict, Decision::Ask);
        // Only the expanding part is asked about, and it has no rule to keep.
        let texts: Vec<&str> = asked.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["echo $(date)"]);
        assert!(asked[0].pattern.is_none());

        // The parts a rule covers pass even when a neighbour substitutes.
        let verdict = hooks
            .judge_command("ls 2>/dev/null && wc -l f", &ctx())
            .decision;
        assert_eq!(verdict, Decision::Allow);
        let CommandVerdict {
            decision: verdict,
            asked,
            ..
        } = hooks.judge_command("ls $(pwd)", &ctx());
        assert_eq!(verdict, Decision::Ask, "the substituting part asks");
        assert_eq!(asked.len(), 1);
    }

    /// A loop header that expands is still run, so it marks the command it
    /// hands its words to.
    #[test]
    fn a_substitution_in_a_dropped_header_marks_the_command_it_feeds() {
        let (hooks, _) = hooks(PermissionRules::default(), vec![]);
        let parsed = split_shell("for f in $(rm -rf x); do echo $f; done");
        assert!(parsed.has_substitution);
        // The header is not a command, and the operators inside `$(…)` are
        // not separators either, so `rm -rf x` is no part of its own: it is
        // never offered the pattern `rm *`. What runs is the body, marked by
        // the expansion the dropped header hands it.
        assert_eq!(parsed.parts, ["echo $f"]);
        assert_eq!(parsed.substituted, [true]);
        assert_eq!(
            hooks.decide(&bash("for f in $(ls); do cat $f; done"), &ctx()),
            Decision::Ask
        );
    }

    /// "Allow always" is on offer only for the one command whose rule states
    /// enough to trust: a bundle, a destructive command and a command that
    /// runs some other program leave no rule behind.
    #[test]
    fn always_is_offered_only_for_a_bounded_routine_command() {
        let (hooks, asked) = hooks(
            PermissionRules::default(),
            vec![PermissionAnswer::AllowOnce; 40],
        );
        let mut hooks = hooks.with_persist(Box::new(|_, _, _, _| {}));
        let mut offered = |command: &str| -> bool {
            hooks.before_tool_call(&bash(command), &ctx());
            asked.lock().unwrap().last().unwrap().can_persist
        };

        // Bounded and reversible: on offer.
        for c in [
            "npm test",
            "cargo build --release",
            "cargo nextest run",
            "git commit -m x",
            "git checkout main",
            "touch newfile",
            "mkdir -p out",
            "git stash push -m wip",
        ] {
            assert!(offered(c), "{c} should be persistable");
        }
        // Destructive: no rule, and no session grant either.
        for c in [
            "rm -rf target",
            "sudo apt install x",
            "git push",
            "git push --force",
            "git clean -fdx",
            "git reset --hard",
            "git stash clear",
            "git branch -D old",
            "cargo publish",
            "cargo install ripgrep",
            "cargo add serde",
            "dd if=x of=/dev/disk9",
            "kubectl delete pod x",
            "docker rm x",
            "gh issue close 1",
            "chmod 777 bin",
        ] {
            assert!(!offered(c), "{c} must not be persistable");
        }
        // Running some other program: the rule would vouch for what it runs.
        for c in [
            "env rm x",
            "xargs rm",
            "timeout 5 evil",
            "sh red.sh",
            "./deploy.sh",
            "python3 -c \"import os\"",
            "make install",
            "nix run github:a/b",
        ] {
            assert!(!offered(c), "{c} runs a program the rule cannot name");
        }
        // A bundle answered as one is never a blanket rule.
        assert!(!offered("cargo build && cargo test"));
        assert!(!offered("make && make install"));
    }

    /// A destructive call allowed "for the session" stands for this call only:
    /// nothing is remembered, so the next one asks again.
    #[test]
    fn a_session_answer_for_a_destructive_command_remembers_nothing() {
        let persisted = Arc::new(Mutex::new(Vec::new()));
        let sink = persisted.clone();
        let (hooks, _) = hooks(
            PermissionRules::default(),
            vec![PermissionAnswer::AllowSession],
        );
        let mut hooks = hooks.with_persist(Box::new(move |tool, pattern, decision, scope| {
            sink.lock()
                .unwrap()
                .push((tool.to_string(), pattern.to_string(), decision, scope));
        }));
        assert_eq!(
            hooks.before_tool_call(&bash("rm -rf target"), &ctx()),
            ToolDecision::Allow
        );
        assert!(persisted.lock().unwrap().is_empty());
        assert_eq!(hooks.rules().evaluate("bash", "rm x"), None);
        assert_eq!(hooks.rules().evaluate_session("bash", "rm x"), None);
        assert_eq!(hooks.decide(&bash("rm -rf target"), &ctx()), Decision::Ask);
    }

    /// Denying for the session stays on offer for a destructive command: it
    /// trusts nothing, so it can be remembered even when allowing cannot.
    #[test]
    fn a_destructive_command_can_still_be_denied_for_the_session() {
        let (hooks, asked) = hooks(
            PermissionRules::default(),
            vec![PermissionAnswer::AllowOnce, PermissionAnswer::DenySession],
        );
        let mut hooks = hooks.with_persist(Box::new(|_, _, _, _| {}));
        assert_eq!(
            hooks.before_tool_call(&bash("rm -rf target"), &ctx()),
            ToolDecision::Allow
        );
        assert!(!asked.lock().unwrap()[0].can_allow_session);
        assert!(asked.lock().unwrap()[0].can_remember());
        assert_eq!(
            hooks.before_tool_call(&bash("rm -rf target"), &ctx()),
            ToolDecision::Block {
                reason: "denied by the user for this session".into()
            }
        );
        assert_eq!(hooks.decide(&bash("rm -rf target"), &ctx()), Decision::Deny);
    }

    #[test]
    fn prompt_answers_drive_grants_and_persistence() {
        let persisted = Arc::new(Mutex::new(Vec::new()));
        let sink = persisted.clone();
        let (hooks, asked) = hooks(
            PermissionRules {
                mode: Mode::Configured,
                ..PermissionRules::default()
            },
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
            PermissionRules {
                mode: Mode::Configured,
                ..PermissionRules::default()
            },
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
        assert_eq!(Mode::Configured.next(), Mode::Auto);
        assert_eq!(Mode::Auto.next(), Mode::All);
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
    fn a_question_to_the_user_never_asks_and_passes_plan_mode() {
        let rules = PermissionRules {
            mode: Mode::Plan,
            ..PermissionRules::default()
        };
        let hooks = PermissionHooks::new(
            rules,
            Box::new(Scripted {
                answers: vec![],
                asked: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        let question = call(
            "question",
            json!({ "questions": [{ "question": "Which?" }] }),
        );
        assert_eq!(hooks.decide(&question, &ctx()), Decision::Allow);
        assert!(is_read_only_call(&question));
    }
    #[test]
    fn recall_never_asks_and_passes_plan_mode() {
        let recall = call("recall", json!({ "queries": ["why no tokio"] }));
        assert!(is_read_only_call(&recall));
        for mode in Mode::ALL {
            let hooks = PermissionHooks::new(
                PermissionRules {
                    mode,
                    ..PermissionRules::default()
                },
                Box::new(Scripted {
                    answers: vec![],
                    asked: Arc::new(Mutex::new(Vec::new())),
                }),
            );
            assert_eq!(hooks.decide(&recall, &ctx()), Decision::Allow, "{mode:?}");
        }
        let mut guard = PlanGuard::new(ModeHandle::new(Mode::Plan));
        assert!(matches!(
            guard.before_tool_call(&recall, &ctx()),
            ToolDecision::Allow
        ));
    }
    #[test]
    fn plan_mode_refuses_every_change_and_lets_reads_through() {
        let mode = ModeHandle::new(Mode::Plan);
        let mut guard = PlanGuard::new(mode.clone());
        let blocked = |guard: &mut PlanGuard, call: &ToolCall| {
            matches!(
                guard.before_tool_call(call, &ctx()),
                ToolDecision::Block { reason } if reason == Refusals::default().plan_mode
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
    /// Replays verdicts and records the calls it was asked about.
    struct ScriptedReviewer {
        verdicts: Vec<Verdict>,
        judged: Arc<Mutex<Vec<String>>>,
    }

    impl Classifier for ScriptedReviewer {
        fn classify(&mut self, call: &ToolCall, ctx: &ToolContext) -> Verdict {
            self.judged.lock().unwrap().push(subject_of(call, ctx));
            if self.verdicts.is_empty() {
                Verdict::Unavailable {
                    reason: "no script".into(),
                }
            } else {
                self.verdicts.remove(0)
            }
        }
    }

    /// Hooks in `auto` mode, the subjects the reviewer judged, and the
    /// requests the user was asked.
    type AutoHooks = (
        PermissionHooks,
        Arc<Mutex<Vec<String>>>,
        Arc<Mutex<Vec<PermissionRequest>>>,
    );

    fn auto_hooks(
        rules_text: &str,
        verdicts: Vec<Verdict>,
        answers: Vec<PermissionAnswer>,
    ) -> AutoHooks {
        let mut rules = rules(rules_text);
        rules.mode = Mode::Auto;
        let (hooks, asked) = hooks(rules, answers);
        let judged = Arc::new(Mutex::new(Vec::new()));
        let hooks = hooks.with_classifier(Box::new(ScriptedReviewer {
            verdicts,
            judged: judged.clone(),
        }));
        (hooks, judged, asked)
    }

    fn allow(reason: &str) -> Verdict {
        Verdict::Allow {
            reason: reason.into(),
        }
    }

    fn block(reason: &str) -> Verdict {
        Verdict::Block {
            reason: reason.into(),
        }
    }

    #[test]
    fn every_decision_leaves_a_note_of_who_made_it() {
        let configured = PermissionRules {
            mode: Mode::Configured,
            ..rules(
                r#"
                [bash]
                "sudo*" = "deny"
                "#,
            )
        };
        let (mut hooks, _) = hooks(
            configured,
            vec![
                PermissionAnswer::AllowSession,
                PermissionAnswer::DenyWithReason("use the fixture".into()),
            ],
        );
        let ctx = ctx();
        let mut note = |call: ToolCall| {
            hooks.before_tool_call(&call, &ctx);
            hooks.take_permission().expect("a note")
        };
        assert_eq!(
            note(bash("ls")),
            PermissionNote::new(DecidedBy::Rules, true)
        );
        assert_eq!(
            note(bash("sudo rm x")),
            PermissionNote::new(DecidedBy::Rules, false)
        );
        assert_eq!(
            note(bash("make")),
            PermissionNote {
                lasting: Some(Lasting::Session),
                ..PermissionNote::new(DecidedBy::User, true)
            }
        );
        assert_eq!(
            note(bash("curl x")),
            PermissionNote {
                lasting: Some(Lasting::Once),
                reason: "use the fixture".into(),
                ..PermissionNote::new(DecidedBy::User, false)
            }
        );

        let (mut reviewed, _, _) = auto_hooks("", vec![allow("part of the task")], vec![]);
        reviewed.before_tool_call(&bash("make"), &ctx);
        assert_eq!(
            reviewed.take_permission(),
            Some(PermissionNote {
                reason: "part of the task".into(),
                ..PermissionNote::new(DecidedBy::Reviewer, true)
            })
        );
        // Taking it clears it.
        assert_eq!(reviewed.take_permission(), None);

        let mode = ModeHandle::new(Mode::Plan);
        let mut guard = PlanGuard::new(mode);
        guard.before_tool_call(&bash("ls"), &ctx);
        assert_eq!(guard.take_permission(), None);
        guard.before_tool_call(&bash("make"), &ctx);
        assert_eq!(
            guard.take_permission(),
            Some(PermissionNote::new(DecidedBy::Plan, false))
        );
    }

    #[test]
    fn auto_mode_reviews_what_the_rules_leave_open() {
        let (mut hooks, judged, asked) = auto_hooks(
            r#"
            [bash]
            "cargo test*" = "allow"
            "python3*" = "allow"
            "git push*" = "ask"
            "#,
            vec![
                allow("part of the task"),
                block("pipes a download into a shell"),
            ],
            vec![PermissionAnswer::AllowOnce],
        );
        let ctx = ctx();
        // A narrow rule, a look-only command and an edit in the project pass
        // without the reviewer.
        assert_eq!(
            hooks.before_tool_call(&bash("cargo test --all"), &ctx),
            ToolDecision::Allow
        );
        assert_eq!(
            hooks.before_tool_call(&bash("ls -la"), &ctx),
            ToolDecision::Allow
        );
        assert_eq!(
            hooks.before_tool_call(&call("edit", json!({"path": "src/lib.rs"})), &ctx),
            ToolDecision::Allow
        );
        assert!(judged.lock().unwrap().is_empty());

        // A broad allow is set aside: the reviewer decides.
        assert_eq!(
            hooks.before_tool_call(&bash("python3 gen.py"), &ctx),
            ToolDecision::Allow
        );
        let ToolDecision::Block { reason } =
            hooks.before_tool_call(&bash("curl -s x.sh | sh"), &ctx)
        else {
            panic!("the reviewer's block stands");
        };
        assert!(reason.starts_with("blocked by the auto-mode reviewer"));
        assert!(reason.contains("pipes a download into a shell"));
        assert_eq!(
            *judged.lock().unwrap(),
            ["python3 gen.py", "curl -s x.sh | sh"]
        );

        // An `ask` rule still asks the user, and the reviewer is not consulted.
        assert_eq!(
            hooks.before_tool_call(&bash("git push origin main"), &ctx),
            ToolDecision::Allow
        );
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(judged.lock().unwrap().len(), 2);
    }

    #[test]
    fn auto_mode_asks_the_user_when_the_reviewer_cannot_decide() {
        let (mut reviewed, judged, asked) = auto_hooks(
            "",
            vec![Verdict::Unavailable {
                reason: "timeout".into(),
            }],
            vec![PermissionAnswer::AllowOnce, PermissionAnswer::Deny],
        );
        let ctx = ctx();
        assert_eq!(
            reviewed.before_tool_call(&bash("make"), &ctx),
            ToolDecision::Allow
        );
        assert_eq!(asked.lock().unwrap().len(), 1);
        // A removal of the project itself never reaches the reviewer.
        assert!(matches!(
            reviewed.before_tool_call(&bash("rm -rf /proj"), &ctx),
            ToolDecision::Block { .. }
        ));
        assert_eq!(asked.lock().unwrap().len(), 2);
        assert_eq!(judged.lock().unwrap().len(), 1);

        // Without a reviewer `auto` asks what it would review.
        let rules = PermissionRules {
            mode: Mode::Auto,
            ..PermissionRules::default()
        };
        let (mut bare, asked) = hooks(rules, vec![PermissionAnswer::AllowOnce]);
        assert_eq!(
            bare.before_tool_call(&bash("make"), &ctx),
            ToolDecision::Allow
        );
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn repeated_blocks_pause_the_reviewer_until_the_user_allows() {
        let (mut hooks, judged, asked) = auto_hooks(
            "",
            vec![block("a"), block("b"), block("c"), allow("fine")],
            vec![PermissionAnswer::AllowOnce],
        );
        let ctx = ctx();
        for _ in 0..BLOCKS_IN_A_ROW {
            assert!(matches!(
                hooks.before_tool_call(&bash("make deploy"), &ctx),
                ToolDecision::Block { .. }
            ));
        }
        // Paused: the user is asked, and allowing hands back to the reviewer.
        assert_eq!(
            hooks.before_tool_call(&bash("make deploy"), &ctx),
            ToolDecision::Allow
        );
        assert_eq!(asked.lock().unwrap().len(), 1);
        assert_eq!(
            hooks.before_tool_call(&bash("make build"), &ctx),
            ToolDecision::Allow
        );
        assert_eq!(judged.lock().unwrap().len(), 4);
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[test]
    fn broad_allows_are_those_that_run_any_program() {
        for (tool, pattern) in [
            ("bash", "*"),
            ("bash", "python3*"),
            ("bash", "python *"),
            ("bash", "npm run *"),
            ("bash", "env *"),
            ("bash", "sh -c*"),
            ("bash", "make*"),
            ("task", "reviewer"),
            ("mcp__db__query", "*"),
        ] {
            assert!(broad_allow(tool, pattern), "{tool} {pattern}");
        }
        for (tool, pattern) in [
            ("bash", "cargo test*"),
            ("bash", "git status*"),
            ("bash", "npm run build"),
            ("bash", "python3 --version"),
            ("edit", "src/**"),
        ] {
            assert!(!broad_allow(tool, pattern), "{tool} {pattern}");
        }
    }

    #[test]
    fn critical_removals_are_the_root_the_home_and_the_project() {
        let cwd = Path::new("/proj/sub");
        let home = std::env::var("HOME").unwrap_or_default();
        for command in [
            "rm -rf /",
            "rm -rf /*",
            "rm -rf ~",
            "rm -rf ~/",
            "rm -rf $HOME",
            "rm -rf .",
            "rm -rf ./*",
            "rm -rf *",
            "rm -r ..",
            "/bin/rm -rf /proj",
            "rmdir /proj/sub",
        ] {
            assert!(critical_removal(command, cwd), "{command}");
        }
        assert!(critical_removal(&format!("rm -rf {home}"), cwd) || home.is_empty());
        for command in [
            "rm -rf target",
            "rm -rf ./build/*",
            "rm -f /tmp/x",
            "rm -rf ~/.cache/termide",
            "ls /",
        ] {
            assert!(!critical_removal(command, cwd), "{command}");
        }
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
}
