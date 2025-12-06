use std::any::Any;

use super::{
    debug::Debug, editor::Editor, file_manager::FileManager, terminal_pty::Terminal, Panel,
};

/// Extension trait for convenient downcasting of Panel trait objects
pub trait PanelExt {
    /// Downcast to Editor (immutable)
    #[allow(dead_code)] // May be used in future
    fn as_editor(&self) -> Option<&Editor>;

    /// Downcast to Editor (mutable)
    fn as_editor_mut(&mut self) -> Option<&mut Editor>;

    /// Downcast to FileManager (immutable)
    fn as_file_manager(&self) -> Option<&FileManager>;

    /// Downcast to FileManager (mutable)
    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager>;

    /// Downcast to Terminal (immutable)
    #[allow(dead_code)] // May be used in future
    fn as_terminal(&self) -> Option<&Terminal>;

    /// Downcast to Terminal (mutable)
    #[allow(dead_code)] // May be used in future
    fn as_terminal_mut(&mut self) -> Option<&mut Terminal>;

    /// Downcast to Debug (immutable)
    #[allow(dead_code)] // May be used in future
    fn as_debug(&self) -> Option<&Debug>;

    /// Check if panel is a Debug panel
    fn is_debug(&self) -> bool;
}

impl PanelExt for dyn Panel {
    fn as_editor(&self) -> Option<&Editor> {
        (self as &dyn Any).downcast_ref::<Editor>()
    }

    fn as_editor_mut(&mut self) -> Option<&mut Editor> {
        (self as &mut dyn Any).downcast_mut::<Editor>()
    }

    fn as_file_manager(&self) -> Option<&FileManager> {
        (self as &dyn Any).downcast_ref::<FileManager>()
    }

    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager> {
        (self as &mut dyn Any).downcast_mut::<FileManager>()
    }

    fn as_terminal(&self) -> Option<&Terminal> {
        (self as &dyn Any).downcast_ref::<Terminal>()
    }

    fn as_terminal_mut(&mut self) -> Option<&mut Terminal> {
        (self as &mut dyn Any).downcast_mut::<Terminal>()
    }

    fn as_debug(&self) -> Option<&Debug> {
        (self as &dyn Any).downcast_ref::<Debug>()
    }

    fn is_debug(&self) -> bool {
        (self as &dyn Any).is::<Debug>()
    }
}

impl PanelExt for Box<dyn Panel> {
    fn as_editor(&self) -> Option<&Editor> {
        (&**self as &dyn Any).downcast_ref::<Editor>()
    }

    fn as_editor_mut(&mut self) -> Option<&mut Editor> {
        (&mut **self as &mut dyn Any).downcast_mut::<Editor>()
    }

    fn as_file_manager(&self) -> Option<&FileManager> {
        (&**self as &dyn Any).downcast_ref::<FileManager>()
    }

    fn as_file_manager_mut(&mut self) -> Option<&mut FileManager> {
        (&mut **self as &mut dyn Any).downcast_mut::<FileManager>()
    }

    fn as_terminal(&self) -> Option<&Terminal> {
        (&**self as &dyn Any).downcast_ref::<Terminal>()
    }

    fn as_terminal_mut(&mut self) -> Option<&mut Terminal> {
        (&mut **self as &mut dyn Any).downcast_mut::<Terminal>()
    }

    fn as_debug(&self) -> Option<&Debug> {
        (&**self as &dyn Any).downcast_ref::<Debug>()
    }

    fn is_debug(&self) -> bool {
        (&**self as &dyn Any).is::<Debug>()
    }
}
