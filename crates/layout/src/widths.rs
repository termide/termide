//! Group width distribution and divider geometry for [`LayoutManager`].

use crate::layout_manager::{LayoutManager, MIN_GROUP_WIDTH};

impl LayoutManager {
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
            self.panel_groups[0].width = Some(available_width.max(MIN_GROUP_WIDTH));
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
                let fixed_total: u16 = fixed_groups.iter().sum();
                let remaining = available_width.saturating_sub(fixed_total);
                let per_auto = (remaining / auto_count as u16).max(MIN_GROUP_WIDTH);
                for group in self.panel_groups.iter_mut() {
                    if group.width.is_none() {
                        group.width = Some(per_auto);
                    }
                }
            } else {
                let actual_widths_before_freeze = self.calculate_actual_widths(available_width);
                for (idx, &width) in actual_widths_before_freeze.iter().enumerate() {
                    if self.panel_groups[idx].width.is_none() {
                        self.panel_groups[idx].width = Some(width.max(MIN_GROUP_WIDTH));
                    }
                }
            }
        }

        let actual_widths = self.calculate_actual_widths(available_width);
        let total_actual: u16 = actual_widths.iter().sum();

        if total_actual == 0 {
            return;
        }

        let min_width = MIN_GROUP_WIDTH;
        let n = actual_widths.len();
        let min_total = min_width * n as u16;

        // If all groups at minimum already exceed budget, just assign minimums.
        if min_total >= available_width {
            let mut new_widths = vec![min_width; n];
            // Give any leftover to the last group (may be 0)
            let last = n - 1;
            new_widths[last] = available_width
                .saturating_sub(min_width * (n - 1) as u16)
                .max(min_width);
            for (idx, &width) in new_widths.iter().enumerate() {
                self.panel_groups[idx].width = Some(width);
            }
            return;
        }

        // Compute proportional widths using floor, enforcing minimum.
        // Track fractional remainders for largest-remainder distribution.
        let mut new_widths = Vec::with_capacity(n);
        let mut remainders = Vec::with_capacity(n);
        let mut allocated_width: u16 = 0;

        for (idx, &actual_width) in actual_widths.iter().enumerate() {
            let proportion = actual_width as f64 / total_actual as f64;
            let exact = available_width as f64 * proportion;
            let floored = (exact.floor() as u16).max(min_width);
            new_widths.push(floored);
            // Only groups above minimum can receive remainder pixels
            let remainder = if floored > min_width {
                exact - floored as f64
            } else {
                // Was clamped to min_width, fractional part is not meaningful
                -1.0
            };
            remainders.push((idx, remainder));
            allocated_width += floored;
        }

        // Distribute leftover pixels to groups with the largest fractional remainders
        let mut leftover = available_width.saturating_sub(allocated_width);
        if leftover > 0 {
            remainders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for &(idx, _) in &remainders {
                if leftover == 0 {
                    break;
                }
                new_widths[idx] += 1;
                leftover -= 1;
            }
        }

        // If over-allocated (due to min_width clamps pushing total up), trim largest groups
        let mut total: u16 = new_widths.iter().sum();
        while total > available_width {
            // Find largest group above minimum
            if let Some(idx) = new_widths
                .iter()
                .enumerate()
                .filter(|(_, &w)| w > min_width)
                .max_by_key(|(_, &w)| w)
                .map(|(i, _)| i)
            {
                new_widths[idx] -= 1;
                total -= 1;
            } else {
                break; // all at minimum, can't reduce further
            }
        }

        for (idx, &width) in new_widths.iter().enumerate() {
            self.panel_groups[idx].width = Some(width);
        }
    }

    /// Find divider at given position (for drag resize).
    ///
    /// Returns divider index if position is within grab zone (±1 from divider).
    /// Divider N is between groups N and N+1.
    pub fn find_divider_at_position(&self, x: u16, y: u16, terminal_height: u16) -> Option<usize> {
        // Skip menu row (y == 0) and status bar (y == terminal_height - 1)
        if y == 0 || y >= terminal_height.saturating_sub(1) {
            return None;
        }

        // Need at least 2 groups for a divider
        if self.panel_groups.len() < 2 {
            return None;
        }

        let mut current_x: u16 = 0;
        for (idx, group) in self.panel_groups.iter().enumerate() {
            current_x += group.width.unwrap_or(0);

            // Check if this is not the last group (divider exists after it)
            if idx < self.panel_groups.len() - 1 {
                // Grab zone: [current_x - 1, current_x]
                if x >= current_x.saturating_sub(1) && x <= current_x {
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Get X positions of all dividers.
    ///
    /// Returns Vec of (divider_index, x_position).
    pub fn get_divider_positions(&self) -> Vec<(usize, u16)> {
        let mut positions = Vec::new();
        let mut current_x: u16 = 0;

        for (idx, group) in self.panel_groups.iter().enumerate() {
            current_x += group.width.unwrap_or(0);

            // Divider exists after each group except the last
            if idx < self.panel_groups.len() - 1 {
                positions.push((idx, current_x));
            }
        }
        positions
    }

    /// Resize two adjacent groups.
    ///
    /// `left_idx` is the index of the left group (divider is between left_idx and left_idx+1).
    pub fn resize_groups(&mut self, left_idx: usize, new_left_width: u16, new_right_width: u16) {
        if left_idx + 1 >= self.panel_groups.len() {
            return;
        }

        self.panel_groups[left_idx].width = Some(new_left_width);
        self.panel_groups[left_idx + 1].width = Some(new_right_width);
    }
}
