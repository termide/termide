//! Layout manager for panel arrangement.

use anyhow::{anyhow, Result};

use termide_config::Config;
use termide_core::{Panel, WidthPreference};

use crate::PanelGroup;

/// Minimum width, in columns, that a panel group may be shrunk to.
pub const MIN_GROUP_WIDTH: u16 = 20;

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

        let num_groups_after_split = self.panel_groups.len() + 1;
        let new_width_if_split = available_width / num_groups_after_split as u16;

        if new_width_if_split < config.general.auto_stack_threshold {
            // Auto-stacking: pick group by width preference
            let target_group_idx = self.find_preferred_group(&*panel);
            let group = &mut self.panel_groups[target_group_idx];
            let insert_pos = group.expanded_index() + 1;
            group.insert_panel(insert_pos, panel);
            group.set_expanded(insert_pos);
            self.focus = target_group_idx;
        } else {
            // Create new group horizontally
            let new_group = PanelGroup::new(panel);
            self.panel_groups.push(new_group);
            self.focus = self.panel_groups.len() - 1;
            self.redistribute_widths_proportionally(available_width);
        }
    }

    /// Add panel without changing focus.
    /// Used for preview panels where focus should stay on the source panel.
    pub fn add_panel_without_focus(
        &mut self,
        panel: Box<dyn Panel>,
        config: &Config,
        terminal_width: u16,
    ) {
        let saved_focus = self.focus;
        self.add_panel(panel, config, terminal_width);
        self.focus = saved_focus;
    }

    /// Replace the active (focused, expanded) panel in place, returning the old
    /// one. Layout and focus are preserved — used to swap one view of a file
    /// for another (e.g. hex ⇄ text).
    pub fn replace_active_panel(&mut self, panel: Box<dyn Panel>) -> Option<Box<dyn Panel>> {
        let group = self.panel_groups.get_mut(self.focus)?;
        let idx = group.expanded_index();
        group.replace_panel(idx, panel)
    }

    /// Find panel by name, expand it in its group, return mutable reference.
    /// Does NOT change focus. Used for reusing existing panels.
    pub fn find_and_expand_panel_by_name(&mut self, name: &str) -> Option<&mut Box<dyn Panel>> {
        // First pass: find the group and panel index
        let mut found: Option<(usize, usize)> = None;
        for (group_idx, group) in self.panel_groups.iter().enumerate() {
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if panel.name() == name {
                    found = Some((group_idx, panel_idx));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        // Second pass: expand and return mutable reference
        if let Some((group_idx, panel_idx)) = found {
            let group = &mut self.panel_groups[group_idx];
            group.set_expanded(panel_idx);
            return group.panels_mut().get_mut(panel_idx);
        }
        None
    }

    /// Like [`Self::find_and_expand_panel_by_name`], but also moves keyboard
    /// focus to the group containing the panel. Used when reusing an existing
    /// viewer should focus it (the plain variant leaves focus untouched, e.g.
    /// for background image previews).
    pub fn focus_and_expand_panel_by_name(&mut self, name: &str) -> Option<&mut Box<dyn Panel>> {
        let mut found: Option<(usize, usize)> = None;
        for (group_idx, group) in self.panel_groups.iter().enumerate() {
            for (panel_idx, panel) in group.panels().iter().enumerate() {
                if panel.name() == name {
                    found = Some((group_idx, panel_idx));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }

        if let Some((group_idx, panel_idx)) = found {
            self.focus = group_idx;
            let group = &mut self.panel_groups[group_idx];
            group.set_expanded(panel_idx);
            return group.panels_mut().get_mut(panel_idx);
        }
        None
    }

    /// Find the best group for a panel based on its width preference.
    fn find_preferred_group(&self, panel: &dyn Panel) -> usize {
        match panel.width_preference() {
            WidthPreference::NoPreference => self.focus,
            WidthPreference::PreferNarrow => self
                .panel_groups
                .iter()
                .enumerate()
                .min_by_key(|(_, g)| g.width.unwrap_or(u16::MAX))
                .map(|(idx, _)| idx)
                .unwrap_or(self.focus),
            WidthPreference::PreferWide => self
                .panel_groups
                .iter()
                .enumerate()
                .max_by_key(|(_, g)| g.width.unwrap_or(0))
                .map(|(idx, _)| idx)
                .unwrap_or(self.focus),
        }
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

    /// Iterator over all panels with their expanded state (mutable).
    /// Returns `(panel, is_expanded)` for each panel.
    pub fn iter_all_panels_with_expanded_state_mut(
        &mut self,
    ) -> impl Iterator<Item = (&mut Box<dyn Panel>, bool)> {
        self.panel_groups.iter_mut().flat_map(|g| {
            let expanded = g.expanded_index();
            g.panels_mut()
                .iter_mut()
                .enumerate()
                .map(move |(idx, panel)| (panel, idx == expanded))
        })
    }

    /// Iterator over only expanded (visible) panels (mutable).
    pub fn iter_expanded_panels_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Panel>> {
        self.panel_groups
            .iter_mut()
            .filter_map(|g| g.expanded_panel_mut())
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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{buffer::Buffer, layout::Rect};
    use std::any::Any;
    use termide_core::{PanelEvent, RenderContext, WidthPreference};

    /// Minimal mock panel for layout tests.
    struct MockPanel {
        name: &'static str,
        width_pref: WidthPreference,
    }

    impl MockPanel {
        fn new(name: &'static str) -> Self {
            Self {
                name,
                width_pref: WidthPreference::NoPreference,
            }
        }
    }

    impl Panel for MockPanel {
        fn name(&self) -> &'static str {
            self.name
        }
        fn title(&self) -> String {
            self.name.to_string()
        }
        fn render(&mut self, _area: Rect, _buf: &mut Buffer, _ctx: &RenderContext) {}
        fn handle_key(&mut self, _chord: termide_core::KeyChord) -> Vec<PanelEvent> {
            vec![]
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
        fn width_preference(&self) -> WidthPreference {
            self.width_pref
        }
    }

    fn make_config(threshold: u16) -> Config {
        let mut config = Config::default();
        config.general.auto_stack_threshold = threshold;
        config
    }

    fn panel(name: &'static str) -> Box<dyn Panel> {
        Box::new(MockPanel::new(name))
    }

    // =========================================================================
    // Panel stacking / unstacking
    // =========================================================================

    #[test]
    fn test_add_panel_to_empty_layout() {
        let mut lm = LayoutManager::new();
        let config = make_config(80);
        lm.add_panel(panel("a"), &config, 200);
        assert_eq!(lm.group_count(), 1);
        assert_eq!(lm.panel_count(), 1);
        assert_eq!(lm.focus, 0);
    }

    #[test]
    fn test_add_panel_creates_new_group_when_wide() {
        let mut lm = LayoutManager::new();
        let config = make_config(40); // threshold 40
        lm.add_panel(panel("a"), &config, 200);
        lm.add_panel(panel("b"), &config, 200);
        // 200 / 2 = 100 >= 40, so new group
        assert_eq!(lm.group_count(), 2);
        assert_eq!(lm.panel_count(), 2);
        assert_eq!(lm.focus, 1); // focus moves to new panel
    }

    #[test]
    fn test_add_panel_stacks_when_narrow() {
        let mut lm = LayoutManager::new();
        let config = make_config(80); // threshold 80
        lm.add_panel(panel("a"), &config, 100);
        lm.add_panel(panel("b"), &config, 100);
        // 100 / 2 = 50 < 80, so auto-stack
        assert_eq!(lm.group_count(), 1);
        assert_eq!(lm.panel_count(), 2);
    }

    #[test]
    fn test_unstack_panel_from_group() {
        let mut lm = LayoutManager::new();
        let config = make_config(80);
        // Create a single group with 2 panels (force stack)
        lm.add_panel(panel("a"), &config, 100);
        lm.add_panel(panel("b"), &config, 100);
        assert_eq!(lm.group_count(), 1);
        assert_eq!(lm.panel_count(), 2);

        // Unstack should create a new group
        lm.toggle_panel_stacking(200).unwrap();
        assert_eq!(lm.group_count(), 2);
        assert_eq!(lm.panel_count(), 2);
    }

    #[test]
    fn test_repeated_toggle_keeps_even_split() {
        let mut lm = LayoutManager::new();
        let config = make_config(80);
        // Two panels stacked in a single group.
        lm.add_panel(panel("a"), &config, 100);
        lm.add_panel(panel("b"), &config, 100);
        assert_eq!(lm.group_count(), 1);

        // Repeatedly toggle between stacked and side-by-side. Every time the
        // panels land side-by-side they should keep an even split, not collapse
        // the group to the minimum width.
        for cycle in 0..5 {
            lm.toggle_panel_stacking(200).unwrap();
            assert_eq!(lm.group_count(), 2, "cycle {cycle}: should be side-by-side");
            let widths = lm.calculate_actual_widths(200);
            assert_eq!(
                widths,
                vec![100, 100],
                "cycle {cycle}: unstack should be even"
            );

            lm.toggle_panel_stacking(200).unwrap();
            assert_eq!(
                lm.group_count(),
                1,
                "cycle {cycle}: should be stacked again"
            );
        }
    }

    #[test]
    fn test_unstack_preserves_other_group_widths() {
        let mut lm = LayoutManager::new();
        let config = make_config(20);
        // Three side-by-side groups at 100 each (300 wide), then stack a second
        // panel into the middle group.
        lm.add_panel(panel("a"), &config, 300);
        lm.add_panel(panel("b"), &config, 300);
        lm.add_panel(panel("c"), &config, 300);
        assert_eq!(lm.group_count(), 3);
        // Normalise to an even 100/100/100 baseline.
        lm.panel_groups[0].width = Some(100);
        lm.panel_groups[1].width = Some(100);
        lm.panel_groups[2].width = Some(100);
        lm.focus = 1;
        lm.panel_groups[1].add_panel(panel("b2"));
        assert_eq!(lm.panel_groups[1].len(), 2);

        // Unstacking the middle group splits *its* column in two and leaves the
        // outer groups untouched.
        lm.toggle_panel_stacking(300).unwrap();
        assert_eq!(lm.group_count(), 4);
        let widths = lm.calculate_actual_widths(300);
        assert_eq!(widths, vec![100, 50, 50, 100]);
    }

    #[test]
    fn test_three_panel_toggle_is_stable() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        // Two panels side by side, normalised to an even baseline.
        lm.add_panel(panel("a"), &config, 200);
        lm.add_panel(panel("b"), &config, 200);
        assert_eq!(lm.group_count(), 2);
        lm.panel_groups[0].width = Some(100);
        lm.panel_groups[1].width = Some(100);
        // Stack a third panel under the right group.
        lm.focus = 1;
        lm.panel_groups[1].add_panel(panel("c"));
        assert_eq!(lm.panel_groups[1].len(), 2);

        // Toggling back and forth must be a round-trip: the right stack
        // splits to [100, 50, 50] when unstacked and recombines to [100, 100]
        // when re-stacked.
        for cycle in 0..5 {
            lm.toggle_panel_stacking(200).unwrap();
            assert_eq!(lm.group_count(), 3, "cycle {cycle}: should be unstacked");
            assert_eq!(
                lm.calculate_actual_widths(200),
                vec![100, 50, 50],
                "cycle {cycle}: unstacked widths"
            );

            lm.toggle_panel_stacking(200).unwrap();
            assert_eq!(lm.group_count(), 2, "cycle {cycle}: should be re-stacked");
            assert_eq!(
                lm.calculate_actual_widths(200),
                vec![100, 100],
                "cycle {cycle}: re-stacked widths"
            );
        }
    }

    #[test]
    fn test_stack_panel_merges_into_left() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 200);
        lm.add_panel(panel("b"), &config, 200);
        assert_eq!(lm.group_count(), 2);

        // Focus on group 1 (single panel), stacking merges into left
        lm.focus = 1;
        lm.toggle_panel_stacking(200).unwrap();
        assert_eq!(lm.group_count(), 1);
        assert_eq!(lm.panel_count(), 2);
        assert_eq!(lm.focus, 0);
    }

    // =========================================================================
    // Focus tracking after layout changes
    // =========================================================================

    #[test]
    fn test_focus_updates_on_add_panel() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 400);
        assert_eq!(lm.focus, 0);
        lm.add_panel(panel("b"), &config, 400);
        assert_eq!(lm.focus, 1);
        lm.add_panel(panel("c"), &config, 400);
        assert_eq!(lm.focus, 2);
    }

    #[test]
    fn test_add_panel_without_focus_preserves_focus() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 400);
        lm.add_panel_without_focus(panel("b"), &config, 400);
        assert_eq!(lm.focus, 0); // focus stays on first panel
        assert_eq!(lm.group_count(), 2);
    }

    #[test]
    fn test_focus_after_close_last_group() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 400);
        lm.add_panel(panel("b"), &config, 400);
        lm.focus = 1;
        lm.close_active_panel(400).unwrap();
        assert_eq!(lm.group_count(), 1);
        assert_eq!(lm.focus, 0);
    }

    // =========================================================================
    // Width redistribution
    // =========================================================================

    #[test]
    fn test_single_group_gets_full_width() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 200);
        lm.redistribute_widths_proportionally(200);
        assert_eq!(lm.panel_groups[0].width, Some(200));
    }

    #[test]
    fn test_widths_assigned_after_multiple_groups() {
        let mut lm = LayoutManager::new();
        let config = make_config(20);
        lm.add_panel(panel("a"), &config, 200);
        lm.add_panel(panel("b"), &config, 200);
        lm.add_panel(panel("c"), &config, 200);

        let widths = lm.calculate_actual_widths(200);
        let total: u16 = widths.iter().sum();
        // Total widths should equal available width
        assert_eq!(total, 200);
    }

    #[test]
    fn test_redistribute_widths_empty() {
        let mut lm = LayoutManager::new();
        // Should not panic
        lm.redistribute_widths_proportionally(200);
        assert!(lm.calculate_actual_widths(200).is_empty());
    }

    // =========================================================================
    // Panel navigation
    // =========================================================================

    #[test]
    fn test_next_prev_group_wrapping() {
        let mut lm = LayoutManager::new();
        let config = make_config(20);
        lm.add_panel(panel("a"), &config, 400);
        lm.add_panel(panel("b"), &config, 400);
        lm.add_panel(panel("c"), &config, 400);

        lm.focus = 0;
        lm.next_group();
        assert_eq!(lm.focus, 1);
        lm.next_group();
        assert_eq!(lm.focus, 2);
        // Wrap around
        lm.next_group();
        assert_eq!(lm.focus, 0);

        // prev wraps back
        lm.prev_group();
        assert_eq!(lm.focus, 2);
    }

    #[test]
    fn test_next_prev_panel_in_group() {
        let mut lm = LayoutManager::new();
        let config = make_config(80);
        // Force stacking
        lm.add_panel(panel("a"), &config, 100);
        lm.add_panel(panel("b"), &config, 100);
        lm.add_panel(panel("c"), &config, 100);
        assert_eq!(lm.group_count(), 1);

        let group = &lm.panel_groups[0];
        let initial_expanded = group.expanded_index();

        lm.next_panel_in_group();
        let after = lm.panel_groups[0].expanded_index();
        assert_eq!(after, (initial_expanded + 1) % 3);
    }

    // =========================================================================
    // Panel move operations
    // =========================================================================

    #[test]
    fn test_move_panel_to_next_group_swaps_single() {
        let mut lm = LayoutManager::new();
        let config = make_config(20);
        lm.add_panel(panel("a"), &config, 400);
        lm.add_panel(panel("b"), &config, 400);
        assert_eq!(lm.group_count(), 2);

        // Move group 0 (single panel) to next — this swaps the groups
        lm.focus = 0;
        lm.move_panel_to_next_group(400).unwrap();
        assert_eq!(lm.group_count(), 2); // swap, not merge
        assert_eq!(lm.focus, 1);
    }

    #[test]
    fn test_move_panel_to_prev_group_swaps_single() {
        let mut lm = LayoutManager::new();
        let config = make_config(20);
        lm.add_panel(panel("a"), &config, 400);
        lm.add_panel(panel("b"), &config, 400);
        assert_eq!(lm.group_count(), 2);

        lm.focus = 1;
        lm.move_panel_to_prev_group(400).unwrap();
        assert_eq!(lm.group_count(), 2); // swap, not merge
        assert_eq!(lm.focus, 0);
    }

    #[test]
    fn test_move_panel_from_stacked_group_merges() {
        let mut lm = LayoutManager::new();
        let config = make_config(80);
        // Create group with 2 stacked panels
        lm.add_panel(panel("a"), &config, 100);
        lm.add_panel(panel("b"), &config, 100);
        assert_eq!(lm.group_count(), 1);
        assert_eq!(lm.panel_count(), 2);

        // Now add a separate group (wide enough)
        let config2 = make_config(20);
        lm.add_panel(panel("c"), &config2, 400);
        assert_eq!(lm.group_count(), 2);

        // Focus on first group (has 2 panels), move expanded panel to next group
        lm.focus = 0;
        lm.panel_groups[0].set_expanded(1); // expand "b"
        lm.move_panel_to_next_group(400).unwrap();
        // "b" moved to group 1, group 0 still has "a"
        assert_eq!(lm.group_count(), 2);
        assert_eq!(lm.panel_groups[1].len(), 2); // c + b
    }

    #[test]
    fn test_move_panel_up_down_in_group() {
        let mut lm = LayoutManager::new();
        let config = make_config(80);
        lm.add_panel(panel("a"), &config, 100);
        lm.add_panel(panel("b"), &config, 100);
        lm.add_panel(panel("c"), &config, 100);
        assert_eq!(lm.group_count(), 1);

        // expanded is the last added panel ("c" at index 2)
        let group = &lm.panel_groups[0];
        assert_eq!(group.expanded_index(), 2);

        // Move down should be no-op (already at bottom)
        lm.move_panel_down_in_group().unwrap();
        assert_eq!(lm.panel_groups[0].expanded_index(), 2);

        // Move up should swap with index 1
        lm.move_panel_up_in_group().unwrap();
        assert_eq!(lm.panel_groups[0].expanded_index(), 1);
    }

    // =========================================================================
    // Edge cases
    // =========================================================================

    #[test]
    fn test_close_last_panel_removes_group() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 200);
        lm.add_panel(panel("b"), &config, 200);
        assert_eq!(lm.group_count(), 2);

        // Close panel in second group
        lm.focus = 1;
        lm.close_active_panel(200).unwrap();
        assert_eq!(lm.group_count(), 1);
    }

    #[test]
    fn test_close_all_panels() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 200);
        lm.close_active_panel(200).unwrap();
        assert_eq!(lm.group_count(), 0);
        assert!(!lm.has_panels());
        assert_eq!(lm.panel_count(), 0);
    }

    #[test]
    fn test_active_panel_with_no_panels() {
        let lm = LayoutManager::new();
        assert!(lm.active_panel().is_none());
    }

    #[test]
    fn test_next_group_with_no_panels() {
        let mut lm = LayoutManager::new();
        // Should not panic
        lm.next_group();
        lm.prev_group();
        assert_eq!(lm.focus, 0);
    }

    #[test]
    fn test_set_focus_out_of_bounds() {
        let mut lm = LayoutManager::new();
        let config = make_config(40);
        lm.add_panel(panel("a"), &config, 200);
        lm.set_focus(100); // out of bounds
        assert_eq!(lm.focus, 0); // unchanged
    }
}
