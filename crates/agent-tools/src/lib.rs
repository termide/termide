//! Built-in tools of the termide coding agent.
//!
//! Four tools cover what a coding agent needs on a local checkout: `read`,
//! `edit`, `write` and `bash`; `skill` joins them when the project or the
//! user defines skills, `question` when someone watches the run to answer.
//! Search is left to the shell (`rg`, `find`), which every model already
//! knows. Contracts follow the cross-agent
//! comparison in `doc/en/agent-design.md`: numbered lines on read,
//! search/replace with a unique anchor and tolerant whitespace matching on
//! edit, head-and-tail truncation of shell output with the full log saved to
//! a file.

mod args;
mod bash;
mod clean;
mod edit;
mod question;
mod read;
mod skill;
mod suggest;
mod task;
mod truncate;
mod write;

use std::path::PathBuf;
use std::sync::Arc;

use termide_agent_core::ToolRegistry;

pub use bash::BashTool;
pub use clean::{clean_output, Cleaned};
pub use edit::EditTool;
pub use question::QuestionTool;
pub use read::ReadTool;
pub use skill::SkillTool;
pub use suggest::SuggestCommandTool;
pub use task::{SubagentRun, TaskTool};
pub use write::WriteTool;

/// The default registry: `read`, `edit`, `write`, `bash`, in prompt order.
/// `shim_path` is the command-shim directory prepended to `bash`'s `PATH`
/// (the configuration-level `shims/`); `None` leaves `PATH` untouched.
#[must_use]
pub fn builtin_tools(shim_path: Option<PathBuf>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.insert(Arc::new(ReadTool));
    registry.insert(Arc::new(EditTool));
    registry.insert(Arc::new(WriteTool));
    registry.insert(Arc::new(BashTool {
        shim_path,
        ..BashTool::default()
    }));
    registry
}
