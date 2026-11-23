//! Built-in tools of the termide coding agent.
//!
//! Four tools cover what a coding agent needs on a local checkout: `read`,
//! `edit`, `write` and `shell`; `skill` joins them when the project or the
//! user defines skills, `question` when someone watches the run to answer.
//! `checklist` keeps the steps of the work at hand.
//! The tools that reach the rest of termide (`terminals`, `debug`,
//! `start_session`, `jobs`, `sessions`, `send_message`, `suggest_*`) live here too, but
//! the app registers them, since it owns what they reach.
//! Search is left to the shell (`rg`, `find`), which every model already
//! knows. Contracts follow the cross-agent
//! comparison in `doc/en/agent-design.md`: numbered lines on read,
//! search/replace with a unique anchor and tolerant whitespace matching on
//! edit, head-and-tail truncation of shell output with the full log saved to
//! a file.

mod args;
mod autorun;
mod capture;
mod checklist;
mod clean;
mod debug;
mod edit;
mod jobs;
mod lsp;
mod peers;
mod question;
mod read;
mod shell;
mod skill;
mod start_session;
mod suggest;
mod terminals;
mod truncate;
mod write;

use std::path::PathBuf;
use std::sync::Arc;

use termide_agent_core::ToolRegistry;

pub use autorun::{SuggestGoalTool, SuggestLoopTool};
pub use checklist::ChecklistTool;
pub use clean::{clean_output, Cleaned};
pub use debug::{parse_action as parse_debug_action, DebugAction, DebugCheck, DebugRun, DebugTool};
pub use edit::EditTool;
pub use jobs::{JobsTool, OwnTasks};
pub use lsp::{set_lsp_check, FileError, LspCheck, LspReport};
pub use peers::{PeerInfo, PeerList, PeerSend, SendMessageTool, SessionsTool};
pub use question::QuestionTool;
pub use read::ReadTool;
pub use shell::{ShellProgram, ShellTool};
pub use skill::SkillTool;
pub use suggest::SuggestCommandTool;
/// How `jobs` words a line, for the sources that fill it.
pub mod jobs_text {
    pub use crate::jobs::{duration, first_line};
}
pub use start_session::{
    ForkedTask, SessionContext, SessionReply, SessionRequest, SessionShow, StartSession,
    StartSessionTool, SubagentOutcome, SubagentTask, START_SESSION,
};
pub use terminals::TerminalsTool;
pub use write::WriteTool;

/// The default registry: `read`, `edit`, `write`, `shell`, `checklist`, in
/// prompt order.
/// `shim_path` is the command-shim directory prepended to `shell`'s `PATH`
/// (the configuration-level `shims/`); `None` leaves `PATH` untouched.
#[must_use]
pub fn builtin_tools(shim_path: Option<PathBuf>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.insert(Arc::new(ReadTool));
    registry.insert(Arc::new(EditTool));
    registry.insert(Arc::new(WriteTool));
    registry.insert(Arc::new(ShellTool {
        shim_path,
        ..ShellTool::default()
    }));
    registry.insert(Arc::new(ChecklistTool));
    registry
}
