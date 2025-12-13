//! Panel group implementation for accordion-style layout.

use termide_core::Panel;

/// Group of panels in accordion (vertical stack).
pub struct PanelGroup {
    panels: Vec<Box<dyn Panel>>,
    expanded_index: usize,
    /// Width in characters (None = auto-distribution).
    pub width: Option<u16>,
}

impl PanelGroup {
    /// Create new group with single panel.
    pub fn new(panel: Box<dyn Panel>) -> Self {
        Self {
            panels: vec![panel],
            expanded_index: 0,
            width: None,
        }
    }

    /// Add panel to group.
    pub fn add_panel(&mut self, panel: Box<dyn Panel>) {
        self.panels.push(panel);
    }

    /// Remove panel from group by index.
    pub fn remove_panel(&mut self, index: usize) -> Option<Box<dyn Panel>> {
        if index >= self.panels.len() {
            return None;
        }

        let panel = self.panels.remove(index);

        // Adjust expanded_index
        if self.panels.is_empty() {
            self.expanded_index = 0;
        } else if self.expanded_index >= self.panels.len() {
            self.expanded_index = self.panels.len() - 1;
        }

        Some(panel)
    }

    /// Set expanded panel by index.
    pub fn set_expanded(&mut self, index: usize) {
        if index < self.panels.len() {
            self.expanded_index = index;
        }
    }

    /// Get expanded panel index.
    pub fn expanded_index(&self) -> usize {
        self.expanded_index
    }

    /// Switch to next panel in group.
    pub fn next_panel(&mut self) {
        if !self.panels.is_empty() {
            self.expanded_index = (self.expanded_index + 1) % self.panels.len();
        }
    }

    /// Switch to previous panel in group.
    pub fn prev_panel(&mut self) {
        if !self.panels.is_empty() {
            self.expanded_index = if self.expanded_index == 0 {
                self.panels.len() - 1
            } else {
                self.expanded_index - 1
            };
        }
    }

    /// Get number of panels in group.
    pub fn len(&self) -> usize {
        self.panels.len()
    }

    /// Check if group is empty.
    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    /// Get mutable reference to panels.
    pub fn panels_mut(&mut self) -> &mut Vec<Box<dyn Panel>> {
        &mut self.panels
    }

    /// Get reference to panels.
    pub fn panels(&self) -> &Vec<Box<dyn Panel>> {
        &self.panels
    }

    /// Get mutable reference to expanded panel.
    pub fn expanded_panel_mut(&mut self) -> Option<&mut Box<dyn Panel>> {
        self.panels.get_mut(self.expanded_index)
    }

    /// Get reference to expanded panel.
    #[allow(clippy::borrowed_box)]
    pub fn expanded_panel(&self) -> Option<&Box<dyn Panel>> {
        self.panels.get(self.expanded_index)
    }

    /// Move panel up (swap with previous).
    pub fn move_panel_up(&mut self, index: usize) -> anyhow::Result<()> {
        if index == 0 || self.panels.is_empty() {
            return Ok(());
        }
        if index >= self.panels.len() {
            return Err(anyhow::anyhow!("Panel index out of bounds"));
        }

        self.panels.swap(index - 1, index);

        if self.expanded_index == index {
            self.expanded_index = index - 1;
        } else if self.expanded_index == index - 1 {
            self.expanded_index = index;
        }

        Ok(())
    }

    /// Move panel down (swap with next).
    pub fn move_panel_down(&mut self, index: usize) -> anyhow::Result<()> {
        if self.panels.is_empty() {
            return Ok(());
        }
        if index >= self.panels.len() - 1 {
            return Ok(());
        }

        self.panels.swap(index, index + 1);

        if self.expanded_index == index {
            self.expanded_index = index + 1;
        } else if self.expanded_index == index + 1 {
            self.expanded_index = index;
        }

        Ok(())
    }

    /// Take all panels from group (empties the group).
    pub fn take_panels(self) -> Vec<Box<dyn Panel>> {
        self.panels
    }
}
