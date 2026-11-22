//! Layout types for terminal panel arrangement.

/// Layout mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutMode {
    /// Single panel mode (width 1-80)
    Single,
    /// Multi-panel mode (width > 100)
    MultiPanel,
}

/// Layout information
#[derive(Debug, Clone)]
pub struct LayoutInfo {
    /// Layout mode
    pub mode: LayoutMode,
    /// Number of main panels
    pub main_panels_count: usize,
}

impl LayoutInfo {
    /// Calculate layout based on terminal width
    pub fn calculate(width: u16) -> Self {
        use termide_config::constants::*;

        if width <= MIN_WIDTH_MULTI_PANEL {
            // Single panel mode for narrow terminals
            Self {
                mode: LayoutMode::Single,
                main_panels_count: 1,
            }
        } else {
            // Multi-panel mode
            let main_panels_count = (width / MIN_MAIN_PANEL_WIDTH).max(1) as usize;
            Self {
                mode: LayoutMode::MultiPanel,
                main_panels_count,
            }
        }
    }
}
