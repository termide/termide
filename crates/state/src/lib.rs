//! State types and data structures for termide.
//!
//! This crate contains pure data types used throughout the application,
//! without dependencies on specific implementations.

mod ai_section;
mod batch;
mod layout;
mod operations;
mod pending_action;
mod ui;

// Re-export all public types for backward compatibility.
pub use ai_section::AiSection;
pub use batch::{
    BatchOperation, BatchOperationType, ConflictMode, DirSizeResult, PauseState, RenamePattern,
    SourceLocation,
};
pub use layout::{LayoutInfo, LayoutMode};
pub use operations::{ActiveOperation, OperationProgress, OperationType, SpeedTracker};
pub use pending_action::{PendingAction, ProjectsOrigin};
pub use ui::{
    DragState, PanelActionMenuState, PanelDragSource, PanelDragState, SubmenuState, TerminalState,
    UiState,
};
