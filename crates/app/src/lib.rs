//! Application orchestrator for termide.
//!
//! This crate ties together all app-* modules and provides:
//! - `App` struct - the main application
//! - `AppState` - global application state
//! - `CommandExecutor` for processing AppCommands
//! - `AppContext` trait combining all required interfaces
//! - Event loop coordination utilities
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        termide (bin)                             │
//! │  main.rs - entry point, terminal setup, App composition         │
//! └─────────────────────────────────────────────────────────────────┘
//!                                │
//!                                ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                    termide-app (this crate)                      │
//! │  App, AppState, CommandExecutor, event loop coordination        │
//! └─────────────────────────────────────────────────────────────────┘
//!            │              │              │              │
//!            ▼              ▼              ▼              ▼
//!     ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
//!     │app-event │  │app-modal │  │app-panel │  │app-watcher│
//!     └──────────┘  └──────────┘  └──────────┘  └──────────┘
//! ```

// Internal modules
pub mod app;
pub mod layout_session;
pub mod panel_ext;
pub mod state;

// Re-export main types for convenience
pub use app::App;
pub use layout_session::LayoutManagerSession;
#[allow(deprecated)]
pub use panel_ext::PanelExt;
pub use state::AppState;

// Note: anyhow::Result is available through re-exports if needed

// Re-export all app-* crates for convenience
pub use termide_app_core;
pub use termide_app_event;
pub use termide_app_modal;
pub use termide_app_panel;
pub use termide_app_session;
pub use termide_app_watcher;

// Re-export commonly used types
pub use termide_app_core::{
    AppCommand, Direction, LayoutController, Message, ModalManager, PanelProvider, PanelType,
    StateManager,
};
pub use termide_app_event::{DefaultHotkeyProcessor, HotkeyAction, HotkeyProcessor};
pub use termide_app_modal::{BatchOperationProcessor, BatchOperationState, ConflictResult};
pub use termide_app_panel::{CloseDecision, ConfirmationType, PanelFactory, PanelLifecycle};
pub use termide_app_session::{AutoSaveConfig, SessionManager, SessionState};
pub use termide_app_watcher::{MessageCollector, UpdateThrottler, WatcherRegistry};

// ============================================================================
// App Context Trait
// ============================================================================

/// Combined context trait for app operations.
///
/// This trait combines all the required interfaces for the app orchestrator.
/// Implementations provide access to state, modals, panels, and layout.
pub trait AppContext: StateManager + ModalManager + PanelProvider + LayoutController {
    /// Get terminal dimensions.
    fn terminal_size(&self) -> (u16, u16);

    /// Check if menu is open.
    fn menu_open(&self) -> bool;

    /// Check if application should quit.
    fn should_quit(&self) -> bool;

    /// Request application quit.
    fn request_quit(&mut self);
}

// ============================================================================
// Command Executor
// ============================================================================

/// Result of command execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionResult {
    /// Command executed successfully
    Success,
    /// Command requires confirmation modal
    NeedsConfirmation {
        /// Reason for confirmation
        reason: String,
    },
    /// Command was cancelled
    Cancelled,
    /// Command failed with error
    Error {
        /// Error message
        message: String,
    },
}

/// Trait for executing app commands.
///
/// Implementations process `AppCommand`s and update application state.
pub trait CommandExecutor {
    /// Execute a single command.
    fn execute(&mut self, command: AppCommand) -> ExecutionResult;

    /// Execute multiple commands in sequence.
    fn execute_all(&mut self, commands: Vec<AppCommand>) -> Vec<ExecutionResult> {
        commands.into_iter().map(|cmd| self.execute(cmd)).collect()
    }
}

/// Default command executor implementation.
///
/// Processes commands by delegating to appropriate handlers.
#[derive(Debug, Default)]
pub struct DefaultCommandExecutor {
    /// Whether to auto-save session after navigation
    pub auto_save_on_navigation: bool,
}

impl DefaultCommandExecutor {
    /// Create a new command executor.
    pub fn new() -> Self {
        Self {
            auto_save_on_navigation: true,
        }
    }

    /// Execute command on given context.
    pub fn execute_on<C: AppContext>(&self, ctx: &mut C, command: AppCommand) -> ExecutionResult {
        match command {
            AppCommand::SetStatus { message, is_error } => {
                if is_error {
                    ctx.set_error(message);
                } else {
                    ctx.set_info(message);
                }
                ExecutionResult::Success
            }

            AppCommand::ClearStatus => {
                ctx.clear_status();
                ExecutionResult::Success
            }

            AppCommand::Navigate { direction } => {
                match direction {
                    Direction::Next => ctx.next_group(),
                    Direction::Prev => ctx.prev_group(),
                    Direction::Index(idx) => ctx.set_focus(idx),
                }
                ExecutionResult::Success
            }

            AppCommand::ClosePanel => match ctx.close_active() {
                Ok(()) => ExecutionResult::Success,
                Err(e) => ExecutionResult::Error {
                    message: e.to_string(),
                },
            },

            AppCommand::Quit => {
                ctx.request_quit();
                ExecutionResult::Success
            }

            AppCommand::ForceQuit => {
                ctx.request_quit();
                ExecutionResult::Success
            }

            // Commands that need external handling
            AppCommand::OpenModal { .. }
            | AppCommand::CloseModal
            | AppCommand::CreatePanel { .. }
            | AppCommand::SaveSession
            | AppCommand::PanelEvent { .. } => {
                // These require access to components not in AppContext
                // Return success and let caller handle specially
                ExecutionResult::Success
            }
        }
    }
}

// ============================================================================
// Event Loop Helpers
// ============================================================================

/// Event types for the main loop.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Keyboard input
    Key(crossterm::event::KeyEvent),
    /// Mouse input
    Mouse(crossterm::event::MouseEvent),
    /// Terminal resize
    Resize { width: u16, height: u16 },
    /// Focus gained
    FocusGained,
    /// Focus lost
    FocusLost,
    /// Tick (for background processing)
    Tick,
}

/// State of the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopState {
    /// Continue processing events
    Continue,
    /// Redraw is needed
    NeedsRedraw,
    /// Exit the loop
    Exit,
}

impl LoopState {
    /// Check if redraw is needed.
    pub fn needs_redraw(&self) -> bool {
        matches!(self, Self::NeedsRedraw)
    }

    /// Check if loop should exit.
    pub fn should_exit(&self) -> bool {
        matches!(self, Self::Exit)
    }

    /// Combine with another state (more urgent state wins).
    pub fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Exit, _) | (_, Self::Exit) => Self::Exit,
            (Self::NeedsRedraw, _) | (_, Self::NeedsRedraw) => Self::NeedsRedraw,
            _ => Self::Continue,
        }
    }
}

// ============================================================================
// Hotkey Integration
// ============================================================================

/// Process hotkey and convert to command.
pub fn process_hotkey(
    processor: &impl HotkeyProcessor,
    key: &crossterm::event::KeyEvent,
) -> Option<AppCommand> {
    processor
        .process_hotkey(key)
        .and_then(|action| action.to_command())
}

/// Check if key event is a global hotkey.
pub fn is_global_hotkey(
    processor: &impl HotkeyProcessor,
    key: &crossterm::event::KeyEvent,
) -> bool {
    processor.process_hotkey(key).is_some()
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_execution_result_variants() {
        let success = ExecutionResult::Success;
        assert_eq!(success, ExecutionResult::Success);

        let error = ExecutionResult::Error {
            message: "test".to_string(),
        };
        assert!(matches!(error, ExecutionResult::Error { .. }));
    }

    #[test]
    fn test_loop_state_needs_redraw() {
        assert!(!LoopState::Continue.needs_redraw());
        assert!(LoopState::NeedsRedraw.needs_redraw());
        assert!(!LoopState::Exit.needs_redraw());
    }

    #[test]
    fn test_loop_state_should_exit() {
        assert!(!LoopState::Continue.should_exit());
        assert!(!LoopState::NeedsRedraw.should_exit());
        assert!(LoopState::Exit.should_exit());
    }

    #[test]
    fn test_loop_state_combine() {
        // Exit wins over everything
        assert_eq!(
            LoopState::Continue.combine(LoopState::Exit),
            LoopState::Exit
        );
        assert_eq!(
            LoopState::NeedsRedraw.combine(LoopState::Exit),
            LoopState::Exit
        );
        assert_eq!(
            LoopState::Exit.combine(LoopState::Continue),
            LoopState::Exit
        );

        // NeedsRedraw wins over Continue
        assert_eq!(
            LoopState::Continue.combine(LoopState::NeedsRedraw),
            LoopState::NeedsRedraw
        );
        assert_eq!(
            LoopState::NeedsRedraw.combine(LoopState::Continue),
            LoopState::NeedsRedraw
        );

        // Continue + Continue = Continue
        assert_eq!(
            LoopState::Continue.combine(LoopState::Continue),
            LoopState::Continue
        );
    }

    #[test]
    fn test_default_command_executor_new() {
        let executor = DefaultCommandExecutor::new();
        assert!(executor.auto_save_on_navigation);
    }

    #[test]
    fn test_app_event_variants() {
        let key_event = AppEvent::Key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        ));
        assert!(matches!(key_event, AppEvent::Key(_)));

        let resize = AppEvent::Resize {
            width: 100,
            height: 50,
        };
        if let AppEvent::Resize { width, height } = resize {
            assert_eq!(width, 100);
            assert_eq!(height, 50);
        }

        let tick = AppEvent::Tick;
        assert!(matches!(tick, AppEvent::Tick));
    }

    #[test]
    fn test_process_hotkey() {
        let processor = DefaultHotkeyProcessor::new();

        // Alt+Q should produce Quit command
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('q'),
            crossterm::event::KeyModifiers::ALT,
        );
        let cmd = process_hotkey(&processor, &key);
        assert!(matches!(cmd, Some(AppCommand::Quit)));

        // Regular key should not produce command
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('a'),
            crossterm::event::KeyModifiers::NONE,
        );
        let cmd = process_hotkey(&processor, &key);
        assert!(cmd.is_none());
    }

    #[test]
    fn test_is_global_hotkey() {
        let processor = DefaultHotkeyProcessor::new();

        // Alt+M is a hotkey
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('m'),
            crossterm::event::KeyModifiers::ALT,
        );
        assert!(is_global_hotkey(&processor, &key));

        // Regular 'm' is not
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('m'),
            crossterm::event::KeyModifiers::NONE,
        );
        assert!(!is_global_hotkey(&processor, &key));
    }
}
