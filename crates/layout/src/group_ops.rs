//! Stacking, merging, unstacking, and cross-group panel moves for
//! [`LayoutManager`].

use anyhow::{anyhow, Result};

use crate::layout_manager::{LayoutManager, MIN_GROUP_WIDTH};
use crate::{PanelDropTarget, PanelGroup};

impl LayoutManager {
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
        let current_width = current_group.width;
        let mut panels = current_group.take_panels();
        let panel = panels.pop().ok_or_else(|| anyhow!("No panel to merge"))?;

        let left_group_idx = active_group_idx - 1;
        if let Some(left_group) = self.panel_groups.get_mut(left_group_idx) {
            left_group.add_panel(panel);
            left_group.set_expanded(left_group.len() - 1);
            // The target reclaims the merged group's column so the
            // other groups keep their widths.
            if let (Some(lw), Some(cw)) = (left_group.width, current_width) {
                left_group.width = Some(lw + cw);
            }
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
        let current_width = current_group.width;
        let mut panels = current_group.take_panels();
        let panel = panels.pop().ok_or_else(|| anyhow!("No panel to merge"))?;

        if let Some(right_group) = self.panel_groups.get_mut(active_group_idx) {
            right_group.add_panel(panel);
            right_group.set_expanded(right_group.len() - 1);
            // Reclaim the merged group's column (see `merge_into_left`).
            if let (Some(rw), Some(cw)) = (right_group.width, current_width) {
                right_group.width = Some(rw + cw);
            }
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

        // The unstacked panel moves into a new group beside the source group,
        // sharing the horizontal space the source group already occupies.
        let source_width = self.calculate_actual_widths(available_width)[active_group_idx];
        let new_group = PanelGroup::new(panel_to_extract);
        self.panel_groups.insert(active_group_idx + 1, new_group);

        let left_width = (source_width / 2).max(MIN_GROUP_WIDTH);
        let right_width = source_width.saturating_sub(left_width).max(MIN_GROUP_WIDTH);
        self.panel_groups[active_group_idx].width = Some(left_width);
        self.panel_groups[active_group_idx + 1].width = Some(right_width);

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

    /// Move an arbitrary panel from `(from_gi, from_pi)` to the given drop
    /// target. Handles source-group cleanup, target index shifting and
    /// width redistribution.
    ///
    /// Returns `(final_group_idx, final_panel_idx)` where the panel ended
    /// up, so the caller can update focus / expanded state.
    pub fn move_panel_to(
        &mut self,
        from_gi: usize,
        from_pi: usize,
        target: PanelDropTarget,
        available_width: u16,
    ) -> Result<(usize, usize)> {
        let source_group = self
            .panel_groups
            .get(from_gi)
            .ok_or_else(|| anyhow!("Invalid source group index"))?;
        if from_pi >= source_group.len() {
            return Err(anyhow!("Invalid source panel index"));
        }

        // No-op: dropping a panel exactly where it already lives.
        if let PanelDropTarget::IntoGroup {
            group_idx,
            at_position,
        } = target
        {
            if group_idx == from_gi && source_group.len() == 1 {
                return Ok((from_gi, from_pi));
            }
            if group_idx == from_gi && (at_position == from_pi || at_position == from_pi + 1) {
                return Ok((from_gi, from_pi));
            }
        }
        if let PanelDropTarget::NewGroup { insert_at } = target {
            if source_group.len() == 1 && (insert_at == from_gi || insert_at == from_gi + 1) {
                return Ok((from_gi, from_pi));
            }
        }

        // Extract the panel from the source group.
        let panel = self
            .panel_groups
            .get_mut(from_gi)
            .and_then(|g| g.remove_panel(from_pi))
            .ok_or_else(|| anyhow!("Failed to remove source panel"))?;

        // If the source group is now empty, drop it and shift downstream
        // indices so the target still points at the right slot.
        let source_was_removed = self
            .panel_groups
            .get(from_gi)
            .map(|g| g.is_empty())
            .unwrap_or(false);

        if source_was_removed {
            self.panel_groups.remove(from_gi);
        }

        // Adjust the target indices for a removed source group.
        let adjusted_target = if source_was_removed {
            match target {
                PanelDropTarget::IntoGroup {
                    group_idx,
                    at_position,
                } => {
                    let gi = if group_idx > from_gi {
                        group_idx - 1
                    } else {
                        group_idx
                    };
                    PanelDropTarget::IntoGroup {
                        group_idx: gi,
                        at_position,
                    }
                }
                PanelDropTarget::NewGroup { insert_at } => {
                    let at = if insert_at > from_gi {
                        insert_at - 1
                    } else {
                        insert_at
                    };
                    PanelDropTarget::NewGroup { insert_at: at }
                }
            }
        } else {
            target
        };

        // Perform the insertion.
        let (final_gi, final_pi) = match adjusted_target {
            PanelDropTarget::IntoGroup {
                group_idx,
                at_position,
            } => {
                let group = self
                    .panel_groups
                    .get_mut(group_idx)
                    .ok_or_else(|| anyhow!("Invalid target group index"))?;
                let pos = at_position.min(group.len());
                group.insert_panel(pos, panel);
                group.set_expanded(pos);
                if source_was_removed {
                    self.redistribute_widths_proportionally(available_width);
                }
                (group_idx, pos)
            }
            PanelDropTarget::NewGroup { insert_at } => {
                let pos = insert_at.min(self.panel_groups.len());
                self.panel_groups.insert(pos, PanelGroup::new(panel));
                self.redistribute_widths_proportionally(available_width);
                (pos, 0)
            }
        };

        self.focus = final_gi;
        Ok((final_gi, final_pi))
    }
}
