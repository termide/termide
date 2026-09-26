//! Where the agent's own files live.
//!
//! Three roots, highest priority first: `.termide/ai/` in the directory the
//! panel works in, the same in the directory termide was opened in (when that
//! is another directory), and `ai/` in the user's configuration directory.
//! A file is taken from the first root that has it; a directory of named
//! entries (agents, skills, prompts) is the union of all roots, a name from a
//! higher root hiding the same name below.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::acp::AcpConfig;
use crate::commands::{CommandScript, COMMANDS_DIR};
use crate::compaction::{CompactionPrompts, SEED_COMPACT, SEED_COMPACTED};
use crate::context::SEED_TEMPLATE;
use crate::goal::{GoalPrompt, SEED_GOAL};
use crate::handoff::{HandoffPrompt, SEED_HANDOFF};
use crate::hooks::{HookConfig, HOOKS_FILE};
use crate::mcp::{McpServerConfig, MCP_FILE};
use crate::permissions::Mode;
use crate::plan::{PlanPrompt, SEED_PLAN};

/// The `ai` directory inside the configuration directory.
pub const GLOBAL_AGENT_DIR: &str = "ai";
/// The `ai` directory inside a project or working directory.
pub const PROJECT_AGENT_DIR: &str = ".termide/ai";
/// The system prompt template of the default agent, at the root of an `ai`
/// directory; also what a custom agent without a `SOUL.md` uses.
pub const ROOT_SOUL_FILE: &str = "AGENTS.md";
/// The system prompt template of a custom agent: `agents/<name>/SOUL.md`.
pub const SOUL_FILE: &str = "SOUL.md";
/// The settings of an agent: `agents/<name>/agent.toml`.
pub const SPEC_FILE: &str = "agent.toml";
/// The agent used when none is chosen.
pub const DEFAULT_AGENT: &str = "default";
/// Session logs under the `ai` directory: `sessions/<working directory>/`.
pub const SESSIONS_DIR: &str = "sessions";
/// Skills under an `ai` directory: `skills/<name>/SKILL.md`.
pub const SKILLS_DIR: &str = "skills";
/// The cross-agent skills directory of a project (agentskills.io), read
/// beside termide's own so skills written for other agents work unchanged.
pub const SHARED_SKILLS_DIR: &str = ".agents/skills";
/// The file that makes a directory a skill.
pub const SKILL_FILE: &str = "SKILL.md";
/// Prompt templates under an `ai` directory: `prompts/<name>.md`, typed as
/// `/<name>` in the panel.
pub const PROMPTS_DIR: &str = "prompts";
/// termide's own prompts under an `ai` directory: `system/compact.md` and
/// `system/compacted.md` so far.
pub const SYSTEM_DIR: &str = "system";
/// Command shims: executables named after a command that shadow it on the
/// built-in agent's `PATH`, so a shell command runs a token-saving wrapper.
/// Only the configuration level is honoured — a shim runs silently on every
/// matching command, so a project must not be able to plant one.
pub const SHIMS_DIR: &str = "shims";
/// Search engines for `web_search`, one `<name>.toml` each.
pub const WEB_ENGINES_DIR: &str = "web/engines";
/// The web tools' browser profile, at the configuration level only: the
/// agent's own cookies, never the user's browser profile.
pub const BROWSER_PROFILE_DIR: &str = "web/browser";

/// The shipped search engines, by name.
pub const SEED_ENGINES: [(&str, &str); 4] = [
    (
        "duckduckgo",
        include_str!("../assets/web/engines/duckduckgo.toml"),
    ),
    ("bing", include_str!("../assets/web/engines/bing.toml")),
    ("google", include_str!("../assets/web/engines/google.toml")),
    ("yandex", include_str!("../assets/web/engines/yandex.toml")),
];

/// One prompt template: `/<name> args` in the input expands to `body` with
/// `$ARGUMENTS` and `$1`…`$9` filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptTemplate {
    pub name: String,
    pub description: String,
    /// What to type after the name, for the picker (`argument-hint`).
    pub argument_hint: String,
    pub body: String,
}

impl PromptTemplate {
    /// The body with `args` substituted: `$ARGUMENTS` takes the whole
    /// string, `$1`…`$9` its whitespace-separated words. A body without any
    /// placeholder gets non-empty `args` appended on a line of their own, so
    /// `/review src/x.rs` works with a template that never mentions
    /// arguments (Claude Code's and Codex's behaviour).
    #[must_use]
    pub fn expand(&self, args: &str) -> String {
        let args = args.trim();
        let words: Vec<&str> = args.split_whitespace().collect();
        let mut out = String::with_capacity(self.body.len() + args.len());
        let mut used = false;
        let mut rest = self.body.as_str();
        while let Some(i) = rest.find('$') {
            out.push_str(&rest[..i]);
            let tail = &rest[i + 1..];
            if let Some(after) = tail.strip_prefix("ARGUMENTS") {
                out.push_str(args);
                used = true;
                rest = after;
            } else if let Some(digit) = tail.chars().next().filter(char::is_ascii_digit) {
                let index = digit.to_digit(10).unwrap_or(0) as usize;
                if index >= 1 {
                    out.push_str(words.get(index - 1).copied().unwrap_or(""));
                    used = true;
                } else {
                    out.push_str("$0");
                }
                rest = &tail[1..];
            } else {
                out.push('$');
                rest = tail;
            }
        }
        out.push_str(rest);
        let mut out = out.trim_end().to_string();
        if !used && !args.is_empty() {
            out.push_str("\n\n");
            out.push_str(args);
        }
        out
    }
}

/// One skill as the prompt lists it: name, one-line description and where
/// its `SKILL.md` is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

/// Split the YAML front matter (`---` fenced `key: value` lines) off a
/// Markdown file. Returns the fields and the body; a file without front
/// matter is all body.
#[must_use]
pub fn split_front_matter(text: &str) -> (BTreeMap<String, String>, &str) {
    let mut fields = BTreeMap::new();
    let Some(rest) = text.strip_prefix("---") else {
        return (fields, text);
    };
    let Some(rest) = rest
        .strip_prefix('\n')
        .or_else(|| rest.strip_prefix("\r\n"))
    else {
        return (fields, text);
    };
    let Some(end) = rest.find("\n---") else {
        return (fields, text);
    };
    for line in rest[..end].lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'');
        fields.insert(key.trim().to_string(), value.to_string());
    }
    let body = &rest[end + 4..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    (fields, body)
}

/// The shipped assets written into the configuration's `ai` directory, as
/// `(relative path, contents)`. The single source of truth for what
/// [`ensure_global_layout`] seeds and keeps up to date.
fn shipped_assets() -> Vec<(String, &'static str)> {
    let mut assets = vec![
        (ROOT_SOUL_FILE.to_string(), SEED_TEMPLATE),
        (format!("{SYSTEM_DIR}/compact.md"), SEED_COMPACT),
        (format!("{SYSTEM_DIR}/compacted.md"), SEED_COMPACTED),
        (format!("{SYSTEM_DIR}/plan.md"), SEED_PLAN),
        (format!("{SYSTEM_DIR}/goal.md"), SEED_GOAL),
        (format!("{SYSTEM_DIR}/handoff.md"), SEED_HANDOFF),
    ];
    assets.extend(
        SEED_ENGINES
            .iter()
            .map(|(name, seed)| (format!("{WEB_ENGINES_DIR}/{name}.toml"), *seed)),
    );
    assets
}

/// Records the hash of each shipped asset as termide last wrote it, so an
/// update can tell a file the user never touched (safe to refresh in place)
/// from one they edited (left alone, the new default offered as `<file>.new`).
const SEEDS_MANIFEST: &str = ".seeds.toml";

/// A stable content hash (FNV-1a, 64-bit) for change detection across runs and
/// versions. Not cryptographic; collisions are irrelevant here.
fn seed_hash(content: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in content.as_bytes() {
        h ^= u64::from(*byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The `<path>.new` sidecar where an updated default is left when the user has
/// edited the original.
fn dot_new(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".new");
    PathBuf::from(name)
}

/// Lay out the `ai` directory of the configuration and keep its shipped assets
/// current. Empty `agents/`, `skills/`, `prompts/`, `commands/`, `system/` and
/// `shims/` are created. Each shipped file (`AGENTS.md`, the `system/` prompts)
/// is then reconciled against the version it was last written from, recorded in
/// `.seeds.toml`:
///
/// - missing → written;
/// - unchanged since termide last wrote it, and the shipped version changed →
///   refreshed in place (the user is on defaults, so keep them current);
/// - edited by the user, and the shipped version changed → left alone, the new
///   default written beside it as `<file>.new` for the user to merge;
/// - otherwise untouched.
///
/// Safe to call on every start.
pub fn ensure_global_layout(global: &Path) -> std::io::Result<()> {
    for dir in [
        "agents",
        "skills",
        "prompts",
        COMMANDS_DIR,
        SYSTEM_DIR,
        SHIMS_DIR,
        WEB_ENGINES_DIR,
    ] {
        std::fs::create_dir_all(global.join(dir))?;
    }

    let manifest_path = global.join(SEEDS_MANIFEST);
    let mut manifest: BTreeMap<String, String> = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default();
    let mut manifest_changed = false;

    for (relative, seed) in shipped_assets() {
        let path = global.join(&relative);
        let shipped = seed_hash(seed);
        let last = manifest.get(&relative).cloned();
        match std::fs::read_to_string(&path) {
            // Missing: write it.
            Err(_) => std::fs::write(&path, seed)?,
            // Already on the current shipped version: nothing to do.
            Ok(current) if seed_hash(&current) == shipped => {}
            // Untouched since we last wrote it, but the shipped version moved:
            // refresh in place.
            Ok(current) if last.as_deref() == Some(seed_hash(&current).as_str()) => {
                std::fs::write(&path, seed)?;
            }
            // User-edited and the shipped version differs from the one we last
            // offered: leave theirs, drop the new default beside it once.
            Ok(_) if last.as_deref() != Some(shipped.as_str()) => {
                std::fs::write(dot_new(&path), seed)?;
            }
            Ok(_) => {}
        }
        if last.as_deref() != Some(shipped.as_str()) {
            manifest.insert(relative, shipped);
            manifest_changed = true;
        }
    }

    if manifest_changed {
        if let Ok(text) = toml::to_string(&manifest) {
            let _ = std::fs::write(&manifest_path, text);
        }
    }
    Ok(())
}

/// `agents/<name>/agent.toml`: what sets an agent apart from the configured
/// defaults. Every field is optional; an absent one keeps the default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// One line for the agent picker.
    #[serde(default)]
    pub description: String,
    /// Model id at the configured endpoint.
    #[serde(default)]
    pub model: Option<String>,
    /// Permission mode the agent starts in.
    #[serde(default)]
    pub mode: Option<Mode>,
    /// Tools the agent may use, by name; all built-in tools when absent.
    #[serde(default)]
    pub tools: Option<Vec<String>>,
    /// An external agent spoken to over ACP instead of the built-in loop;
    /// `model`, `mode` and `tools` then do not apply.
    #[serde(default)]
    pub acp: Option<AcpConfig>,
}

/// An agent as the roots define it: its prompt template and its settings,
/// each from the highest root that has the file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentDefinition {
    pub name: String,
    pub soul: Option<String>,
    pub spec: AgentSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDirs {
    roots: Vec<PathBuf>,
    /// Skill directories in priority order: each level's `ai/skills`
    /// followed by its `.agents/skills`.
    skill_roots: Vec<PathBuf>,
    /// The configuration level, the user's own files, when there is one.
    global: Option<PathBuf>,
}

impl AgentDirs {
    /// `cwd` is where the panel works, `project_root` where termide was
    /// opened, `global` the agent directory under the configuration
    /// directory. Roots that do not exist yet are kept: they say where a
    /// file would be looked for.
    #[must_use]
    pub fn new(cwd: &Path, project_root: Option<&Path>, global: Option<&Path>) -> Self {
        let mut roots = vec![cwd.join(PROJECT_AGENT_DIR)];
        let mut skill_roots = vec![
            cwd.join(PROJECT_AGENT_DIR).join(SKILLS_DIR),
            cwd.join(SHARED_SKILLS_DIR),
        ];
        if let Some(root) = project_root {
            let dir = root.join(PROJECT_AGENT_DIR);
            if !roots.contains(&dir) {
                roots.push(dir.clone());
                skill_roots.push(dir.join(SKILLS_DIR));
                skill_roots.push(root.join(SHARED_SKILLS_DIR));
            }
        }
        if let Some(global) = global {
            roots.push(global.to_path_buf());
            skill_roots.push(global.join(SKILLS_DIR));
        }
        Self {
            roots,
            skill_roots,
            global: global.map(Path::to_path_buf),
        }
    }

    /// Command scripts (`commands/<name>`, executables) across the roots by
    /// name, sorted; the ones from the configuration level are trusted.
    #[must_use]
    pub fn commands(&self) -> Vec<CommandScript> {
        self.merged_entries(COMMANDS_DIR)
            .into_values()
            .filter_map(|path| {
                let trusted = self
                    .global
                    .as_ref()
                    .is_some_and(|global| path.starts_with(global));
                CommandScript::from_file(&path, trusted)
            })
            .collect()
    }

    /// The compaction prompts: `system/compact.md` and `system/compacted.md`
    /// from the highest root that has each, the shipped seeds otherwise.
    #[must_use]
    pub fn compaction_prompts(&self) -> CompactionPrompts {
        CompactionPrompts::from_files(
            &self.system_file("compact.md", SEED_COMPACT),
            &self.system_file("compacted.md", SEED_COMPACTED),
        )
    }

    /// The plan-mode texts: `system/plan.md` from the first level that has
    /// it, the seed otherwise.
    #[must_use]
    pub fn plan_prompt(&self) -> PlanPrompt {
        PlanPrompt::from_file(&self.system_file("plan.md", SEED_PLAN))
    }

    /// The goal-judge texts: `system/goal.md` from the first level that has
    /// it, the seed otherwise.
    #[must_use]
    pub fn goal_prompt(&self) -> GoalPrompt {
        GoalPrompt::from_file(&self.system_file("goal.md", SEED_GOAL))
    }

    /// The handoff-brief texts: `system/handoff.md` from the first level that
    /// has it, the seed otherwise.
    #[must_use]
    pub fn handoff_prompt(&self) -> HandoffPrompt {
        HandoffPrompt::from_file(&self.system_file("handoff.md", SEED_HANDOFF))
    }

    /// The command-shim directory (`shims/`), from the configuration level
    /// only — a shim runs silently, so a project's is never trusted. `None`
    /// when there is no configuration level or it does not exist yet.
    #[must_use]
    pub fn shims_dir(&self) -> Option<PathBuf> {
        let dir = self.global.as_ref()?.join(SHIMS_DIR);
        dir.is_dir().then_some(dir)
    }

    /// Where the web tools' browser keeps its profile; `None` without a
    /// configuration level. Not created here: the browser does that on first
    /// use.
    #[must_use]
    pub fn browser_profile(&self) -> Option<PathBuf> {
        Some(self.global.as_ref()?.join(BROWSER_PROFILE_DIR))
    }

    /// The text of search engine `name`: `web/engines/<name>.toml` from the
    /// highest level that has it, else the shipped seed of that name.
    #[must_use]
    pub fn web_engine(&self, name: &str) -> Option<String> {
        let file = self
            .find_file(Path::new(WEB_ENGINES_DIR).join(format!("{name}.toml")))
            .and_then(|path| std::fs::read_to_string(&path).ok());
        file.or_else(|| {
            SEED_ENGINES
                .iter()
                .find(|(seed, _)| *seed == name)
                .map(|(_, text)| (*text).to_string())
        })
    }

    /// `system/<name>` from the first level that has a non-empty one, else
    /// `seed`.
    fn system_file(&self, name: &str, seed: &str) -> String {
        self.find_file(Path::new(SYSTEM_DIR).join(name))
            .and_then(|path| match std::fs::read_to_string(&path) {
                Ok(text) if !text.trim().is_empty() => Some(text),
                Ok(_) => None,
                Err(error) => {
                    log::warn!("cannot read {}: {error}", path.display());
                    None
                }
            })
            .unwrap_or_else(|| seed.to_string())
    }

    /// Command hooks from every root's `hooks.toml`, by name; a higher root's
    /// table for a name wins, and `enabled = false` there drops the hook.
    /// They run in name order.
    #[must_use]
    pub fn hooks(&self) -> BTreeMap<String, HookConfig> {
        let mut hooks: BTreeMap<String, HookConfig> = BTreeMap::new();
        for root in &self.roots {
            let path = root.join(HOOKS_FILE);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let parsed: BTreeMap<String, HookConfig> = match toml::from_str(&text) {
                Ok(parsed) => parsed,
                Err(error) => {
                    log::warn!("ignoring {}: {error}", path.display());
                    continue;
                }
            };
            for (name, config) in parsed {
                hooks.entry(name).or_insert(config);
            }
        }
        hooks.retain(|_, config| config.enabled);
        hooks
    }

    /// MCP servers from every root's `mcp.toml`, by name; a higher root's
    /// table for a name wins, and `enabled = false` there drops the server.
    /// A file that does not parse is reported and skipped.
    #[must_use]
    pub fn mcp_servers(&self) -> BTreeMap<String, McpServerConfig> {
        let mut servers: BTreeMap<String, McpServerConfig> = BTreeMap::new();
        for root in &self.roots {
            let path = root.join(MCP_FILE);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let parsed: BTreeMap<String, McpServerConfig> = match toml::from_str(&text) {
                Ok(parsed) => parsed,
                Err(error) => {
                    log::warn!("ignoring {}: {error}", path.display());
                    continue;
                }
            };
            for (name, config) in parsed {
                servers.entry(name).or_insert(config);
            }
        }
        servers.retain(|_, config| config.enabled);
        servers
    }

    /// Every prompt template the roots define (`prompts/<name>.md`), by
    /// name, sorted; a higher root hides a lower one. The name is the file
    /// name, description and argument hint come from the front matter.
    #[must_use]
    pub fn prompts(&self) -> Vec<PromptTemplate> {
        self.merged_entries(PROMPTS_DIR)
            .into_iter()
            .filter_map(|(file, path)| {
                let name = file.strip_suffix(".md")?.to_string();
                if name.is_empty() || !path.is_file() {
                    return None;
                }
                let text = std::fs::read_to_string(&path).ok()?;
                let (fields, body) = split_front_matter(&text);
                Some(PromptTemplate {
                    name,
                    description: fields.get("description").cloned().unwrap_or_default(),
                    argument_hint: fields.get("argument-hint").cloned().unwrap_or_default(),
                    body: body.trim().to_string(),
                })
            })
            .collect()
    }

    /// Every skill the roots define, by name, sorted; a name in a higher
    /// root hides the same name below. A skill is a directory holding a
    /// `SKILL.md`; the name and description come from its front matter, the
    /// directory name standing in for a missing name.
    #[must_use]
    pub fn skills(&self) -> Vec<SkillInfo> {
        let mut skills: BTreeMap<String, SkillInfo> = BTreeMap::new();
        for root in &self.skill_roots {
            let Ok(read_dir) = std::fs::read_dir(root) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let path = entry.path().join(SKILL_FILE);
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let (fields, _) = split_front_matter(&text);
                let dir_name = entry.file_name().to_string_lossy().into_owned();
                let name = fields
                    .get("name")
                    .filter(|n| !n.is_empty())
                    .cloned()
                    .unwrap_or(dir_name);
                skills.entry(name.clone()).or_insert(SkillInfo {
                    name,
                    description: fields.get("description").cloned().unwrap_or_default(),
                    path,
                });
            }
        }
        skills.into_values().collect()
    }

    /// Roots in priority order.
    #[must_use]
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The first root that has a file at `relative`.
    #[must_use]
    pub fn find_file(&self, relative: impl AsRef<Path>) -> Option<PathBuf> {
        self.roots
            .iter()
            .map(|root| root.join(relative.as_ref()))
            .find(|path| path.is_file())
    }

    /// Entries of the directory `relative` across all roots, by name. A name
    /// present in several roots resolves to the highest one.
    #[must_use]
    pub fn merged_entries(&self, relative: impl AsRef<Path>) -> BTreeMap<String, PathBuf> {
        let mut entries = BTreeMap::new();
        for root in &self.roots {
            let Ok(read_dir) = std::fs::read_dir(root.join(relative.as_ref())) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                entries.entry(name).or_insert_with(|| entry.path());
            }
        }
        entries
    }

    /// The settings of `agent`; defaults when no root has an `agent.toml`
    /// or the file does not parse (which is logged).
    #[must_use]
    pub fn spec(&self, agent: &str) -> AgentSpec {
        let Some(path) = self.find_file(Path::new("agents").join(agent).join(SPEC_FILE)) else {
            return AgentSpec::default();
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                log::warn!("cannot read {}: {error}", path.display());
                return AgentSpec::default();
            }
        };
        match toml::from_str(&text) {
            Ok(spec) => spec,
            Err(error) => {
                log::warn!("ignoring {}: {error}", path.display());
                AgentSpec::default()
            }
        }
    }

    /// Prompt template and settings of `agent` together.
    #[must_use]
    pub fn agent(&self, agent: &str) -> AgentDefinition {
        AgentDefinition {
            name: agent.to_string(),
            soul: self.soul(agent),
            spec: self.spec(agent),
        }
    }

    /// Names of the agents any root defines, plus the default one, which
    /// exists even without files.
    #[must_use]
    pub fn agents(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .merged_entries("agents")
            .into_iter()
            .filter(|(_, path)| path.is_dir())
            .map(|(name, _)| name)
            .collect();
        if !names.iter().any(|n| n == DEFAULT_AGENT) {
            names.insert(0, DEFAULT_AGENT.to_string());
        }
        names
    }

    /// The directory of `agent` in the highest root that defines it, for
    /// editing or removing it; `None` for the default agent (it has no
    /// directory of its own) or a name no root defines.
    #[must_use]
    pub fn agent_dir(&self, agent: &str) -> Option<PathBuf> {
        if agent == DEFAULT_AGENT {
            return None;
        }
        self.roots
            .iter()
            .map(|root| root.join("agents").join(agent))
            .find(|path| path.is_dir())
    }

    /// The file of prompt `name` (`prompts/<name>.md`) in the highest root that
    /// defines it, for editing or removing it.
    #[must_use]
    pub fn prompt_path(&self, name: &str) -> Option<PathBuf> {
        self.find_file(Path::new(PROMPTS_DIR).join(format!("{name}.md")))
    }

    /// The system prompt template of `agent`: its own `agents/<name>/SOUL.md`
    /// when a root has one, else the root `AGENTS.md` of the highest root
    /// that has it (which is all the default agent has).
    #[must_use]
    pub fn soul(&self, agent: &str) -> Option<String> {
        let own = (agent != DEFAULT_AGENT)
            .then(|| self.find_file(Path::new("agents").join(agent).join(SOUL_FILE)))
            .flatten();
        let path = own.or_else(|| self.find_file(ROOT_SOUL_FILE))?;
        match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => Some(text),
            Ok(_) => None,
            Err(error) => {
                log::warn!("cannot read {}: {error}", path.display());
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_come_from_the_highest_root_and_directories_merge() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("proj/sub");
        let project = tmp.path().join("proj");
        let global = tmp.path().join("config/agent");
        for (root, name, body) in [
            (&cwd, "AGENTS.md", "sub soul"),
            (&global, "AGENTS.md", "global soul"),
            (&project, "agents/review/SOUL.md", "review"),
            (
                &project,
                "agents/bare/agent.toml",
                "description = \"no soul\"",
            ),
            (&cwd, "skills/a/SKILL.md", "a from sub"),
            (&project, "skills/a/SKILL.md", "a from project"),
            (&project, "skills/b/SKILL.md", "b"),
            (&global, "skills/c/SKILL.md", "c"),
        ] {
            let base = if root == &global {
                root.clone()
            } else {
                root.join(PROJECT_AGENT_DIR)
            };
            let path = base.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        let dirs = AgentDirs::new(&cwd, Some(&project), Some(&global));
        assert_eq!(dirs.roots().len(), 3);
        assert_eq!(dirs.soul(DEFAULT_AGENT).as_deref(), Some("sub soul"));
        assert_eq!(dirs.soul("review").as_deref(), Some("review"));
        // An agent without a SOUL.md speaks with the root template.
        assert_eq!(dirs.soul("bare").as_deref(), Some("sub soul"));
        assert_eq!(
            AgentDirs::new(&project, None, None).soul(DEFAULT_AGENT),
            None
        );

        let skills = dirs.merged_entries("skills");
        let names: Vec<&String> = skills.keys().collect();
        assert_eq!(names, ["a", "b", "c"]);
        assert!(skills["a"].starts_with(cwd.join(PROJECT_AGENT_DIR)));
        assert!(skills["b"].starts_with(project.join(PROJECT_AGENT_DIR)));

        // The same directory twice is one root; without a global there are two
        // roots at most.
        assert_eq!(
            AgentDirs::new(&project, Some(&project), None).roots().len(),
            1
        );
        let agents = dirs.merged_entries("agents");
        assert_eq!(agents.keys().collect::<Vec<_>>(), ["bare", "review"]);

        // Path resolvers point at the highest root that defines the item.
        assert_eq!(
            dirs.agent_dir("review"),
            Some(project.join(PROJECT_AGENT_DIR).join("agents/review"))
        );
        assert_eq!(dirs.agent_dir(DEFAULT_AGENT), None);
        assert_eq!(dirs.agent_dir("missing"), None);
    }

    #[test]
    fn prompt_path_resolves_the_highest_root() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let global = tmp.path().join("config/agent");
        for (base, name) in [
            (project.join(PROJECT_AGENT_DIR), "prompts/review.md"),
            (global.clone(), "prompts/review.md"),
            (global.clone(), "prompts/global-only.md"),
        ] {
            let path = base.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "---\n---\nbody").unwrap();
        }
        let dirs = AgentDirs::new(&project, Some(&project), Some(&global));
        assert_eq!(
            dirs.prompt_path("review"),
            Some(project.join(PROJECT_AGENT_DIR).join("prompts/review.md"))
        );
        assert_eq!(
            dirs.prompt_path("global-only"),
            Some(global.join("prompts/global-only.md"))
        );
        assert_eq!(dirs.prompt_path("missing"), None);
    }
    #[test]
    fn agent_settings_come_from_agent_toml_and_default_otherwise() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("ai");
        let review = global.join("agents/review");
        std::fs::create_dir_all(&review).unwrap();
        std::fs::write(
            review.join(SPEC_FILE),
            "description = \"Reviews diffs\"\nmodel = \"big\"\nmode = \"accept-edits\"\ntools = [\"read\", \"bash\"]\n",
        )
        .unwrap();
        let broken = global.join("agents/broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join(SPEC_FILE), "mode = 42\n").unwrap();

        let dirs = AgentDirs::new(tmp.path(), None, Some(&global));
        let spec = dirs.spec("review");
        assert_eq!(spec.description, "Reviews diffs");
        assert_eq!(spec.model.as_deref(), Some("big"));
        assert_eq!(spec.mode, Some(Mode::Edit));
        assert_eq!(
            spec.tools,
            Some(vec!["read".to_string(), "bash".to_string()])
        );
        assert_eq!(dirs.spec("broken"), AgentSpec::default());
        assert_eq!(dirs.spec(DEFAULT_AGENT), AgentSpec::default());
        assert_eq!(dirs.agents(), ["default", "broken", "review"]);
        let definition = dirs.agent("review");
        assert_eq!(definition.name, "review");
        assert!(definition.soul.is_none());
    }
    #[test]
    fn the_global_layout_is_created_once_and_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("ai");
        ensure_global_layout(&global).unwrap();
        for dir in ["agents", "skills", "prompts", "commands", "system", "shims"] {
            assert!(global.join(dir).is_dir(), "{dir}");
        }
        // The shim directory is the configuration level's, and never a
        // project's — a shim runs silently, so it must be trusted.
        let dirs_shim = AgentDirs::new(tmp.path(), Some(tmp.path()), Some(&global));
        assert_eq!(dirs_shim.shims_dir(), Some(global.join("shims")));
        assert_eq!(
            AgentDirs::new(tmp.path(), Some(tmp.path()), None).shims_dir(),
            None
        );
        let soul = global.join(ROOT_SOUL_FILE);
        assert_eq!(std::fs::read_to_string(&soul).unwrap(), SEED_TEMPLATE);
        assert_eq!(
            std::fs::read_to_string(global.join("system/compact.md")).unwrap(),
            SEED_COMPACT
        );
        // The compaction prompts follow the files, a project level first.
        let dirs = AgentDirs::new(tmp.path(), None, Some(&global));
        assert_eq!(dirs.compaction_prompts(), CompactionPrompts::default());
        let project = tmp.path().join(".termide/ai/system");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("compacted.md"), "Recap: {{summary}}").unwrap();
        assert_eq!(dirs.compaction_prompts().wrapper, "Recap: {{summary}}");
        assert_eq!(
            dirs.compaction_prompts().request,
            CompactionPrompts::default().request
        );
        // Plan mode's text the same way.
        assert_eq!(
            std::fs::read_to_string(global.join("system/plan.md")).unwrap(),
            SEED_PLAN
        );
        assert_eq!(dirs.plan_prompt(), PlanPrompt::default());
        std::fs::write(
            project.join("plan.md"),
            "---\nrequest: Go.\n---\nPlan first.",
        )
        .unwrap();
        assert_eq!(dirs.plan_prompt().request, "Go.");
        assert_eq!(dirs.plan_prompt().instructions, "Plan first.");

        std::fs::write(&soul, "mine").unwrap();
        ensure_global_layout(&global).unwrap();
        assert_eq!(std::fs::read_to_string(&soul).unwrap(), "mine");
        let dirs = AgentDirs::new(tmp.path(), None, Some(&global));
        assert_eq!(dirs.soul(DEFAULT_AGENT).as_deref(), Some("mine"));
    }

    #[test]
    fn shipped_assets_refresh_when_untouched_and_offer_new_when_edited() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("ai");
        ensure_global_layout(&global).unwrap();
        let compact = global.join("system/compact.md");
        let manifest = global.join(SEEDS_MANIFEST);
        assert_eq!(std::fs::read_to_string(&compact).unwrap(), SEED_COMPACT);

        // A file the user never touched, recorded from an older shipment, is
        // refreshed in place when the shipped version moves on.
        std::fs::write(&compact, "OLD SHIPPED").unwrap();
        let old = seed_hash("OLD SHIPPED");
        std::fs::write(&manifest, format!("\"system/compact.md\" = \"{old}\"\n")).unwrap();
        ensure_global_layout(&global).unwrap();
        assert_eq!(std::fs::read_to_string(&compact).unwrap(), SEED_COMPACT);
        assert!(!dot_new(&compact).exists());

        // An edited file is kept; the new default lands beside it as `.new`.
        std::fs::write(&compact, "MY EDITS").unwrap();
        std::fs::write(&manifest, format!("\"system/compact.md\" = \"{old}\"\n")).unwrap();
        ensure_global_layout(&global).unwrap();
        assert_eq!(std::fs::read_to_string(&compact).unwrap(), "MY EDITS");
        assert_eq!(
            std::fs::read_to_string(dot_new(&compact)).unwrap(),
            SEED_COMPACT
        );

        // The shipment is now recorded, so a rerun does not re-drop `.new`.
        std::fs::remove_file(dot_new(&compact)).unwrap();
        ensure_global_layout(&global).unwrap();
        assert!(!dot_new(&compact).exists());
        assert_eq!(std::fs::read_to_string(&compact).unwrap(), "MY EDITS");
    }

    #[test]
    fn skills_merge_termide_and_shared_directories_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().join("proj/sub");
        let project = tmp.path().join("proj");
        let global = tmp.path().join("ai");
        let write = |dir: &Path, body: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(SKILL_FILE), body).unwrap();
        };
        write(
            &cwd.join(".termide/ai/skills/deploy"),
            "---\nname: deploy\ndescription: \"Ship it\"\n---\nSteps here.\n",
        );
        write(
            &project.join(".agents/skills/deploy"),
            "---\nname: deploy\ndescription: hidden\n---\n",
        );
        write(
            &project.join(".agents/skills/review"),
            "---\ndescription: Review a diff\nallowed-tools: read\n---\nHow to review.\n",
        );
        write(&global.join("skills/notes"), "No front matter at all.\n");
        std::fs::create_dir_all(global.join("skills/not-a-skill")).unwrap();

        let dirs = AgentDirs::new(&cwd, Some(&project), Some(&global));
        let skills = dirs.skills();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["deploy", "notes", "review"]);
        assert_eq!(skills[0].description, "Ship it");
        assert!(skills[0].path.starts_with(cwd.join(".termide/ai/skills")));
        assert_eq!(skills[1].description, "");
        assert_eq!(skills[2].description, "Review a diff");

        let (fields, body) = split_front_matter("---\nname: x\n---\nbody\n");
        assert_eq!(fields["name"], "x");
        assert_eq!(body, "body\n");
        assert_eq!(split_front_matter("plain").1, "plain");
    }
    #[test]
    fn prompts_come_from_markdown_files_and_expand_their_arguments() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("ai");
        let project = tmp.path().join("proj");
        let write = |dir: &Path, file: &str, body: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join(file), body).unwrap();
        };
        write(
            &global.join("prompts"),
            "review.md",
            "---\ndescription: Review a file\nargument-hint: <path>\n---\nReview $1 for bugs. Notes: $ARGUMENTS\n",
        );
        write(&global.join("prompts"), "notes.txt", "not a prompt");
        write(
            &project.join(".termide/ai/prompts"),
            "review.md",
            "Project review of $1.",
        );
        write(
            &project.join(".termide/ai/prompts"),
            "tests.md",
            "Write tests.",
        );

        let dirs = AgentDirs::new(&project, None, Some(&global));
        let prompts = dirs.prompts();
        let names: Vec<&str> = prompts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["review", "tests"]);
        assert_eq!(prompts[0].body, "Project review of $1.");
        assert_eq!(
            prompts[0].expand("src/x.rs extra"),
            "Project review of src/x.rs."
        );
        // No placeholder: the arguments follow the body.
        assert_eq!(
            prompts[1].expand("for parser"),
            "Write tests.\n\nfor parser"
        );
        assert_eq!(prompts[1].expand(""), "Write tests.");

        let global_only = AgentDirs::new(tmp.path(), None, Some(&global)).prompts();
        assert_eq!(global_only[0].description, "Review a file");
        assert_eq!(global_only[0].argument_hint, "<path>");
        assert_eq!(
            global_only[0].expand("a.rs b.rs"),
            "Review a.rs for bugs. Notes: a.rs b.rs"
        );
        // Unknown dollar words and $0 pass through untouched.
        let odd = PromptTemplate {
            name: "odd".into(),
            description: String::new(),
            argument_hint: String::new(),
            body: "Cost $5, $HOME, $0, $2 end".into(),
        };
        assert_eq!(odd.expand("one"), "Cost , $HOME, $0,  end");
    }
    #[test]
    fn mcp_servers_merge_by_name_and_can_be_switched_off_above() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let global = tmp.path().join("ai");
        std::fs::create_dir_all(project.join(".termide/ai")).unwrap();
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join(MCP_FILE),
            "[github]\ncommand = \"npx\"\n\n[fs]\ncommand = \"fs-server\"\ntools = [\"read_file\"]\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".termide/ai").join(MCP_FILE),
            "[github]\ncommand = \"npx\"\nenabled = false\n\n[db]\ncommand = \"db-server\"\n",
        )
        .unwrap();
        let servers = AgentDirs::new(&project, None, Some(&global)).mcp_servers();
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["db", "fs"]);
        assert_eq!(
            servers["fs"].tools.as_deref(),
            Some(&["read_file".to_string()][..])
        );
        std::fs::write(global.join(MCP_FILE), "not = toml = at all").unwrap();
        assert_eq!(
            AgentDirs::new(&project, None, Some(&global))
                .mcp_servers()
                .keys()
                .collect::<Vec<_>>(),
            ["db"]
        );
    }
    #[test]
    fn hooks_merge_by_name_like_servers() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let global = tmp.path().join("ai");
        std::fs::create_dir_all(project.join(".termide/ai")).unwrap();
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join(HOOKS_FILE),
            "[audit]\nevent = \"after_tool_call\"\ncommand = \"audit.sh\"\n\n[guard]\nevent = \"before_tool_call\"\ncommand = \"guard.sh\"\n",
        )
        .unwrap();
        std::fs::write(
            project.join(".termide/ai").join(HOOKS_FILE),
            "[audit]\nevent = \"after_tool_call\"\ncommand = \"x\"\nenabled = false\n",
        )
        .unwrap();
        let hooks = AgentDirs::new(&project, None, Some(&global)).hooks();
        assert_eq!(hooks.keys().collect::<Vec<_>>(), ["guard"]);
    }
    #[cfg(unix)]
    #[test]
    fn command_scripts_merge_by_name_and_only_the_configuration_level_is_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("proj");
        let global = tmp.path().join("ai");
        let write = |dir: &Path, name: &str, body: &str| {
            std::fs::create_dir_all(dir).unwrap();
            let path = dir.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write(
            &global.join("commands"),
            "review",
            "#!/bin/sh\n# description: global review\necho g\n",
        );
        write(
            &project.join(".termide/ai/commands"),
            "review",
            "#!/bin/sh\n# description: project review\necho p\n",
        );
        write(
            &project.join(".termide/ai/commands"),
            "issue",
            "#!/bin/sh\necho i\n",
        );
        let commands = AgentDirs::new(&project, None, Some(&global)).commands();
        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["issue", "review"]);
        assert!(!commands[0].trusted);
        assert_eq!(commands[1].description, "project review");
        assert!(
            !commands[1].trusted,
            "the project level hides the trusted global one"
        );
        let global_only = AgentDirs::new(tmp.path(), None, Some(&global)).commands();
        assert!(global_only[0].trusted);
    }
}
