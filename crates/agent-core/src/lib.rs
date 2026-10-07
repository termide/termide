//! Provider-agnostic core of the termide coding agent.
//!
//! The crate owns the pieces that do not depend on a model vendor or on the
//! UI: the transcript types ([`Message`]), the [`Provider`] and [`Tool`]
//! contracts, the agent loop ([`Agent`]) with its steering and follow-up
//! queues, and a thread-backed [`AgentRuntime`] that panels poll from their
//! `tick()` like every other background pipeline in termide.
//!
//! Design rules:
//!
//! - A provider stream never fails. Errors and aborts are encoded in the
//!   final [`AssistantMessage`] through [`StopReason`], so the loop has one
//!   code path for "the model answered".
//! - One turn is one assistant message plus the tool calls it requested.
//!   Steering messages are delivered after the tool batch of the finished
//!   turn and before the next model call; follow-up messages only when the
//!   agent would otherwise stop.
//! - Tool calls can be vetoed by [`Hooks::before_tool_call`]; that is where a
//!   permission prompt plugs in.

pub mod acp;
pub mod agent;
pub mod ask;
pub mod cancel;
pub mod checkpoints;
pub mod classifier;
pub mod commands;
pub mod compaction;
pub mod context;
pub mod goal;
pub mod handoff;
pub mod hooks;
pub mod layers;
pub mod mcp;
pub mod message;
pub mod permissions;
pub mod plan;
pub mod provider;
pub mod prune;
pub mod recall;
pub mod recap;
pub mod refusals;
pub mod runtime;
pub mod session;
pub mod shell;
pub mod suggest;
pub mod tool;
pub mod tool_text;

pub use acp::{companion_tools, AcpConfig, AcpFlavor, ACP_PROVIDER, COMPANION_TOOLS, MODEL_OPTION};
pub use agent::{
    execute_tool, judge_tool_call, run_judged_call, Agent, AgentConfig, AgentEvent, ChainedHooks,
    Hooks, JudgedCall, Judgment, NoHooks, QueueHandle, QueueMode, ToolDecision,
};
pub use ask::{
    question_channel, Question, QuestionAnswer, QuestionEnvelope, QuestionOption, QuestionReply,
    UserAsker,
};
pub use cancel::CancelToken;
pub use checkpoints::{Checkpoint, CheckpointHooks, CheckpointStore, SavedFile, Undone};
pub use classifier::{
    parse_classification, Classifier, ClassifyPrompt, IntentEntry, IntentLog, ModelClassifier,
    ReviewerSetup, SessionView, Verdict, SEED_CLASSIFY,
};
pub use commands::{CommandScript, COMMANDS_DIR};
pub use compaction::{CompactionPolicy, CompactionPrompts, CompactionReason};
pub use context::{
    build_system_prompt, civil_date, discover_context_files, ContextFile, PromptOptions,
    SEED_TEMPLATE,
};
pub use goal::{parse_verdict, GoalPrompt, GoalVerdict, SEED_GOAL};
pub use handoff::{HandoffPrompt, SEED_HANDOFF};
pub use hooks::{HookConfig, HookEvent, HOOKS_FILE};
pub use layers::{
    ensure_global_layout, expand_arguments, split_front_matter, AgentDefinition, AgentDirs,
    AgentSpec, DefinitionProblem, LoadedSkill, PromptTemplate, SkillInfo, AGENT_FILE,
    BROWSER_PROFILE_DIR, DEFAULT_AGENT, DEFAULT_AGENT_FILE, GLOBAL_AGENT_DIR, PROJECT_AGENT_DIR,
    PROMPTS_DIR, SEED_ENGINES, SESSIONS_DIR, SHARED_SKILLS_DIR, SHIMS_DIR, SKILLS_DIR, SKILL_FILE,
    SYSTEM_DIR, WEB_ENGINES_DIR,
};
pub use mcp::{
    expand_env, mcp_servers_from_json, McpOAuth, McpReload, McpServerConfig, McpServerState,
    McpSignIn, McpStatus, McpTarget, MCP_FILE, MCP_JSON_FILE,
};
pub use message::{
    now_millis, AssistantContent, AssistantMessage, Message, StopReason, ToolCall,
    ToolResultContent, ToolResultMessage, Usage, UserContent, UserMessage,
};
pub use permissions::{
    is_read_only_call, permission_channel, shell_parts, subject_of, AskedPart, AutoDenyPrompter,
    ChannelPrompter, DecidedBy, Decision, Lasting, Mode, ModeHandle, PermissionAnswer,
    PermissionEnvelope, PermissionHooks, PermissionNote, PermissionPrompter, PermissionRequest,
    PermissionRules, PersistRule, PersistScope, PlanGuard, RuleTables, ShellPart,
};
pub use plan::{PlanPrompt, SEED_PLAN};
pub use provider::{
    one_shot, ModelInfo, ModelSpec, Provider, Request, StreamEvent, ThinkingLevel, ToolSpec,
};
pub use prune::{prune_by, prune_to_decisions};
pub use recall::{RecallPrompt, SEED_RECALL};
pub use recap::{recap, RECAP_LIMIT};
pub use refusals::{Refusals, SEED_PERMISSIONS};
pub use runtime::{
    AgentCommand, AgentRuntime, Backend, BackendModel, BackendOption, BackendSetup, HostTools,
    PromptError,
};
pub use session::{
    Entry, EntryKind, ExternalSessionRef, LoggedMessage, Session, SessionHeader, SessionModel,
    SessionSummary, Timing,
};
pub use shell::{ShellOutput, ShellRunner};
pub use suggest::{
    suggestion_channel, CommandSuggester, Suggestion, SuggestionEnvelope, SuggestionReply,
};
pub use tool::{LateTools, Tool, ToolContext, ToolRegistry, ToolUpdate};
pub use tool_text::{apply_tool_texts, ToolText, SEED_TOOLS, TOOLS_DIR};
