//! Layout manager for panel arrangement.

use anyhow::{anyhow, Result};

use termide_config::Config;
use termide_core::Panel;

use crate::PanelGroup;

/// Panel layout manager with accordion support.
pub struct LayoutManager {
    /// Panel groups (horizontal columns with vertical accordion inside).
    pub panel_groups: Vec<PanelGroup>,
    /// Current focus (active group index).
    pub focus: usize,
}

impl LayoutManager {
    /// Create new empty manager.
    pub fn new() -> Self {
        Self {
            panel_groups: Vec::new(),
            focus: 0,
        }
    }

    /// Add panel with automatic stacking based on available width.
    pub fn add_panel(&mut self, panel: Box<dyn Panel>, config: &Config, terminal_width: u16) {
        let available_width = terminal_width;

        if self.panel_groups.is_empty() {
            let group = PanelGroup::new(panel);
            self.panel_groups.push(group);
            self.focus = 0;
            return;
        }

        let active_group_idx = self.focus;
        let num_groups_after_split = self.panel_groups.len() + 1;
        let new_width_if_split = available_width / num_groups_after_split as u16;

        if new_width_if_split < config.general.min_panel_width {
            // Auto-stacking: add to current group vertically
            if let Some(active_group) = self.panel_groups.get_mut(active_group_idx) {
                active_group.add_panel(panel);
                active_group.set_expanded(active_group.len() - 1);
                self.focus = active_group_idx;
            }
        } else {
            // Create new group horizontally
            let new_group = PanelGroup::new(panel);
            self.panel_groups.push(new_group);
            self.focus = self.panel_groups.len() - 1;
            self.redistribute_widths_proportionally(available_width);
        }
    }

    /// Toggle panel stacking/unstacking with smart direction choice.
    pub fn toggle_panel_stacking(&mut self, available_width: u16) -> Result<()> {
        let active_group_idx = self.focus;

        let group = self
            .panel_groups
            .get(active_group_idx)
            .ok_or_else(|| anyhow!("No active group"))?;

        let group_len = group.len();

        if group_len == 1 {
            if self.panel_groups.len() == 1 {
                return Err(anyhow!("Only one group exists, nothing to merge with"));
            }

            // Priority: left
            if active_group_idx > 0 {
                self.merge_into_left(active_group_idx, available_width)
            } else if active_group_idx + 1 < self.panel_groups.len() {
                self.merge_into_right(active_group_idx, available_width)
            } else {
                Err(anyhow!("No adjacent group found"))
            }
        } else {
            self.unstack_current_panel(active_group_idx, available_width)
        }
    }

    fn merge_into_left(&mut self, active_group_idx: usize, available_width: u16) -> Result<()> {
        if active_group_idx == 0 {
            return Err(anyhow!("No left group to merge into"));
        }

        let current_group = self.panel_groups.remove(active_group_idx);
        let mut panels = current_group.take_panels();
        let panel = panels.pop().ok_or_else(|| anyhow!("No panel to merge"))?;

        let left_group_idx = active_group_idx - 1;
        if let Some(left_group) = self.panel_groups.get_mut(left_group_idx) {
            left_group.add_panel(panel);
            left_group.set_expanded(left_group.len() - 1);
        }

        self.focus = left_group_idx;
        self.redistribute_widths_proportionally(available_width);
        Ok(())
    }

    fn merge_into_right(&mut self, active_group_idx: usize, available_width: u16) -> Result<()> {
        if active_group_idx >= self.panel_groups.len().saturating_sub(1) {
            return Err(anyhow!("No right group to merge into"));
        }

        let current_group = self.panel_groups.remove(active_group_idx);
        let mut panels = current_group.take_panels();
        let panel = panels.pop().ok_or_else(|| anyhow!("No panel to merge"))?;

        if let Some(right_group) = self.panel_groups.get_mut(active_group_idx) {
            right_group.add_panel(panel);
            right_group.set_expanded(right_group.len() - 1);
        }

        self.focus = active_group_idx;
        self.redistribute_widths_proportionally(available_width);
        Ok(())
    }

    fn unstack_current_panel(
        &mut self,
        active_group_idx: usize,
        available_width: u16,
    ) -> Result<()> {
        let group = self
            .panel_groups
            .get_mut(active_group_idx)
            .ok_or_else(|| anyhow!("No active group"))?;

        if group.len() <= 1 {
            return Err(anyhow!("Panel is already alone in group"));
        }

        let expanded_idx = group.expanded_index();
        let panel_to_extract = group
            .remove_panel(expanded_idx)
            .ok_or_else(|| anyhow!("No panel to unstack"))?;

        let new_group = PanelGroup::new(panel_to_extract);
        self.panel_groups.insert(active_group_idx + 1, new_group);
        self.focus = active_group_idx + 1;
        self.redistribute_widths_proportionally(available_width);
        Ok(())
    }

    /// Move panel to previous group.
    pub fn move_panel_to_prev_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = self.focus;

        if group_idx == 0 {
            return Ok(());
        }

        if self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1) {
            self.panel_groups.swap(group_idx - 1, group_idx);
            self.focus = group_idx - 1;
        } else {
            let group = self
                .panel_groups
                .get_mut(group_idx)
                .expect("group_idx validated at function start");
            let expanded_idx = group.expanded_index();
            let panel = group
                .remove_panel(expanded_idx)
                .expect("expanded panel must exist in non-empty group");

            let prev_group = self
                .panel_groups
                .get_mut(group_idx - 1)
                .expect("prev group exists since group_idx > 0");
            prev_group.add_panel(panel);
            prev_group.set_expanded(prev_group.len() - 1);
            self.focus = group_idx - 1;

            if self
                .panel_groups
                .get(group_idx)
                .map(|g| g.is_empty())
                .unwrap_or(false)
            {
                self.panel_groups.remove(group_idx);
                self.redistribute_widths_proportionally(available_width);
            }
        }
        Ok(())
    }

    /// Move panel to next group.
    pub fn move_panel_to_next_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = self.focus;

        if group_idx >= self.panel_groups.len().saturating_sub(1) {
            return Ok(());
        }

        if self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1) {
            self.panel_groups.swap(group_idx, group_idx + 1);
            self.focus = group_idx + 1;
        } else {
            let group = self
                .panel_groups
                .get_mut(group_idx)
                .expect("group_idx validated at function start");
            let expanded_idx = group.expanded_index();
            let panel = group
                .remove_panel(expanded_idx)
                .expect("expanded panel must exist in non-empty group");

            let next_group = self
                .panel_groups
                .get_mut(group_idx + 1)
                .expect("next group exists since group_idx < len-1");
            next_group.add_panel(panel);
            next_group.set_expanded(next_group.len() - 1);
            self.focus = group_idx + 1;

            if self
                .panel_groups
                .get(group_idx)
                .map(|g| g.is_empty())
                .unwrap_or(false)
            {
                self.panel_groups.remove(group_idx);
                self.focus = group_idx;
                self.redistribute_widths_proportionally(available_width);
            }
        }
        Ok(())
    }

    /// Move panel to first group.
    pub fn move_panel_to_first_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = self.focus;

        if group_idx == 0 {
            return Ok(());
        }

        let is_alone = self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1);
        let group = self
            .panel_groups
            .get_mut(group_idx)
            .expect("group_idx validated at function start");
        let expanded_idx = group.expanded_index();
        let panel = group
            .remove_panel(expanded_idx)
            .expect("expanded panel must exist in non-empty group");

        let first_group = self
            .panel_groups
            .get_mut(0)
            .expect("at least one group must exist");
        first_group.add_panel(panel);
        let target_len = first_group.len();
        first_group.set_expanded(target_len - 1);
        self.focus = 0;

        if is_alone {
            self.panel_groups.remove(group_idx);
            self.redistribute_widths_proportionally(available_width);
        }
        Ok(())
    }

    /// Move panel to last group.
    pub fn move_panel_to_last_group(&mut self, available_width: u16) -> Result<()> {
        let group_idx = self.focus;
        let last_idx = self.panel_groups.len().saturating_sub(1);

        if group_idx == last_idx {
            return Ok(());
        }

        let is_alone = self.panel_groups.get(group_idx).map(|g| g.len()) == Some(1);
        let group = self
            .panel_groups
            .get_mut(group_idx)
            .expect("group_idx validated at function start");
        let expanded_idx = group.expanded_index();
        let panel = group
            .remove_panel(expanded_idx)
            .expect("expanded panel must exist in non-empty group");

        let last_group = self
            .panel_groups
            .get_mut(last_idx)
            .expect("last_idx is valid since group_idx != last_idx");
        last_group.add_panel(panel);
        let target_len = last_group.len();
        last_group.set_expanded(target_len - 1);

        if is_alone {
            self.panel_groups.remove(group_idx);
            self.redistribute_widths_proportionally(available_width);
        }

        self.focus = self.panel_groups.len().saturating_sub(1);
        Ok(())
    }

    /// Switch to next group (horizontal).
    pub fn next_group(&mut self) {
        if !self.panel_groups.is_empty() {
            self.focus = (self.focus + 1) % self.panel_groups.len();
        }
    }

    /// Switch to previous group (horizontal).
    pub fn prev_group(&mut self) {
        if !self.panel_groups.is_empty() {
            self.focus = if self.focus == 0 {
                self.panel_groups.len() - 1
            } else {
                self.focus - 1
            };
        }
    }

    /// Switch to next panel in current group (vertical).
    pub fn next_panel_in_group(&mut self) {
        if let Some(group) = self.panel_groups.get_mut(self.focus) {
            group.next_panel();
        }
    }

    /// Switch to previous panel in current group (vertical).
    pub fn prev_panel_in_group(&mut self) {
        if let Some(group) = self.panel_groups.get_mut(self.focus) {
            group.prev_panel();
        }
    }

    /// Move active panel up in current group.
    pub fn move_panel_up_in_group(&mut self) -> Result<()> {
        let group = self
            .panel_groups
            .get_mut(self.focus)
            .ok_or_else(|| anyhow!("No active group"))?;
        let expanded_idx = group.expanded_index();
        group.move_panel_up(expanded_idx)
    }

    /// Move active panel down in current group.
    pub fn move_panel_down_in_group(&mut self) -> Result<()> {
        let group = self
            .panel_groups
            .get_mut(self.focus)
            .ok_or_else(|| anyhow!("No active group"))?;
        let expanded_idx = group.expanded_index();
        group.move_panel_down(expanded_idx)
    }

    /// Get mutable reference to active panel.
    pub fn active_panel_mut(&mut self) -> Option<&mut Box<dyn Panel>> {
        self.panel_groups
            .get_mut(self.focus)
            .and_then(|group| group.expanded_panel_mut())
    }

    /// Get reference to active panel.
    #[allow(clippy::borrowed_box)]
    pub fn active_panel(&self) -> Option<&Box<dyn Panel>> {
        self.panel_groups
            .get(self.focus)
            .and_then(|group| group.expanded_panel())
    }

    /// Get active group index.
    pub fn active_group_index(&self) -> Option<usize> {
        Some(self.focus)
    }

    /// Iterator over all panels (mutable).
    pub fn iter_all_panels_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Panel>> {
        self.panel_groups
            .iter_mut()
            .flat_map(|g| g.panels_mut().iter_mut())
    }

    /// Close active panel.
    pub fn close_active_panel(&mut self, available_width: u16) -> Result<()> {
        let active_group_idx = self.focus;

        let group = self
            .panel_groups
            .get_mut(active_group_idx)
            .ok_or_else(|| anyhow!("No active group"))?;

        if group.len() <= 1 {
            self.panel_groups.remove(active_group_idx);

            if !self.panel_groups.is_empty() {
                self.focus = active_group_idx.min(self.panel_groups.len() - 1);
            } else {
                self.focus = 0;
            }
            self.redistribute_widths_proportionally(available_width);
        } else {
            let expanded_idx = group.expanded_index();
            group.remove_panel(expanded_idx);
        }
        Ok(())
    }

    /// Check if active panel can be closed.
    pub fn can_close_active(&self) -> bool {
        !self.panel_groups.is_empty()
    }

    /// Check if there are any panels.
    pub fn has_panels(&self) -> bool {
        !self.panel_groups.is_empty()
    }

    /// Get total panel count.
    pub fn panel_count(&self) -> usize {
        self.panel_groups.iter().map(|g| g.len()).sum()
    }

    /// Calculate actual widths of all groups.
    pub fn calculate_actual_widths(&self, available_width: u16) -> Vec<u16> {
        if self.panel_groups.is_empty() {
            return Vec::new();
        }

        let total_fixed_width: u16 = self.panel_groups.iter().filter_map(|g| g.width).sum();
        let auto_count = self
            .panel_groups
            .iter()
            .filter(|g| g.width.is_none())
            .count();
        let remaining_width = available_width.saturating_sub(total_fixed_width);
        let auto_width = if auto_count > 0 {
            remaining_width / auto_count as u16
        } else {
            0
        };

        self.panel_groups
            .iter()
            .map(|g| g.width.unwrap_or(auto_width))
            .collect()
    }

    /// Proportionally redistribute group widths.
    pub fn redistribute_widths_proportionally(&mut self, available_width: u16) {
        if self.panel_groups.is_empty() {
            return;
        }

        if self.panel_groups.len() == 1 {
            self.panel_groups[0].width = Some(available_width.max(20));
            return;
        }

        // Freeze auto-width groups
        let has_auto_groups = self.panel_groups.iter().any(|g| g.width.is_none());
        if has_auto_groups {
            let auto_count = self
                .panel_groups
                .iter()
                .filter(|g| g.width.is_none())
                .count();
            let fixed_groups: Vec<u16> = self.panel_groups.iter().filter_map(|g| g.width).collect();

            if !fixed_groups.is_empty() && auto_count > 0 {
                let avg_fixed_width: u16 =
                    fixed_groups.iter().sum::<u16>() / fixed_groups.len() as u16;
                for group in self.panel_groups.iter_mut() {
                    if group.width.is_none() {
                        group.width = Some(avg_fixed_width.max(20));
                    }
                }
            } else {
                let actual_widths_before_freeze = self.calculate_actual_widths(available_width);
                for (idx, &width) in actual_widths_before_freeze.iter().enumerate() {
                    if self.panel_groups[idx].width.is_none() {
                        self.panel_groups[idx].width = Some(width.max(20));
                    }
                }
            }
        }

        let actual_widths = self.calculate_actual_widths(available_width);
        let total_actual: u16 = actual_widths.iter().sum();

        if total_actual == 0 {
            return;
        }

        let mut new_widths = Vec::with_capacity(actual_widths.len());
        let mut allocated_width: u16 = 0;

        for (idx, &actual_width) in actual_widths.iter().enumerate() {
            let is_last = idx == actual_widths.len() - 1;
            let width = if is_last {
                available_width.saturating_sub(allocated_width).max(20)
            } else {
                let proportion = actual_width as f64 / total_actual as f64;
                let w = (available_width as f64 * proportion).round() as u16;
                let w = w.max(20);
                allocated_width = allocated_width.saturating_add(w);
                w
            };
            new_widths.push(width);
        }

        for (idx, &width) in new_widths.iter().enumerate() {
            self.panel_groups[idx].width = Some(width);
        }
    }

    /// Set focus to specific group index.
    pub fn set_focus(&mut self, index: usize) {
        if index < self.panel_groups.len() {
            self.focus = index;
        }
    }

    /// Get mutable reference to group by index.
    pub fn get_group_mut(&mut self, index: usize) -> Option<&mut PanelGroup> {
        self.panel_groups.get_mut(index)
    }

    /// Get reference to group by index.
    pub fn get_group(&self, index: usize) -> Option<&PanelGroup> {
        self.panel_groups.get(index)
    }

    /// Get number of groups.
    pub fn group_count(&self) -> usize {
        self.panel_groups.len()
    }
}

impl Default for LayoutManager {
    fn default() -> Self {
        Self::new()
    }
}
