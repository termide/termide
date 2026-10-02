//! Panel group: vertically stacked panels with per-panel heights and an
//! optional "fullscreen current panel" preset.
//!
//! Every panel in a group gets at least [`MIN_PANEL_HEIGHT`] row (the
//! title bar). Free-resize state is stored in `split_heights`; entering
//! the fullscreen preset stashes the previous heights into
//! `fullscreen_cache` so a subsequent toggle restores them. The preset
//! follows focus — switching the focused panel re-applies the preset.
//!
//! A panel whose [`Panel::height_mode`] is [`HeightMode::FitContent`] is a
//! fixed slot instead: it takes the rows it asks for (capped at
//! [`FIT_MAX_SHARE`] of the column) and the free panels share the rest with
//! their cached proportions. Its own cache entry is ignored until the user
//! resizes it, which pins the panel to the cache like any other.

use termide_core::{HeightMode, Panel, PanelCommand};

/// Minimum height (rows) of a single panel — at least the header line
/// must be visible. Going below this would hide the panel entirely and
/// conflate it with removal.
pub const MIN_PANEL_HEIGHT: u16 = 1;

/// The largest part of a column a [`HeightMode::FitContent`] panel may
/// claim, as a divisor: `1 / FIT_MAX_SHARE`. Beyond that the panel scrolls
/// its content rather than squeezing the free panels.
pub const FIT_MAX_SHARE: u16 = 2;

/// Group of panels stacked vertically.
pub struct PanelGroup {
    panels: Vec<Box<dyn Panel>>,
    /// Index of the focused panel (active border + the panel boosted by
    /// the fullscreen preset, when one is active). The historical name
    /// `expanded_index` is kept to avoid a wide rename — semantically it
    /// is now just "focused".
    expanded_index: usize,
    /// Width in characters (None = auto-distribution among groups).
    pub width: Option<u16>,
    /// Cached per-panel heights. `None` means "no cache — derive equal
    /// distribution on first use"; `Some(v)` requires
    /// `v.len() == panels.len()`.
    split_heights: Option<Vec<u16>>,
    /// When `Some`, the group is in the fullscreen preset and this is
    /// the heights snapshot to restore on toggle-off. The current
    /// `split_heights` while in the preset hold `[1, …, area_height -
    /// (n - 1), …, 1]` with the maximum at `expanded_index`.
    fullscreen_cache: Option<Vec<u16>>,
    /// Per panel: the user resized it by hand, so its `HeightMode` no
    /// longer applies and the cached height does. Same length as `panels`;
    /// not persisted — a restored panel starts unpinned.
    height_pinned: Vec<bool>,
}

impl PanelGroup {
    /// Create new group with single panel.
    pub fn new(panel: Box<dyn Panel>) -> Self {
        Self {
            panels: vec![panel],
            expanded_index: 0,
            width: None,
            split_heights: None,
            fullscreen_cache: None,
            height_pinned: vec![false],
        }
    }

    /// Construct a group from already-decomposed parts. Used by the
    /// layout loader to restore heights and fullscreen-cache state.
    ///
    /// The cache is adopted only if its length matches the panel count;
    /// otherwise it is dropped (mismatched caches are stale).
    pub fn from_parts(
        panels: Vec<Box<dyn Panel>>,
        expanded_index: usize,
        width: Option<u16>,
        split_heights: Option<Vec<u16>>,
        fullscreen_cache: Option<Vec<u16>>,
    ) -> Self {
        let n = panels.len();
        let mut group = Self {
            panels,
            expanded_index,
            width,
            split_heights: None,
            fullscreen_cache: None,
            height_pinned: vec![false; n],
        };
        if n > 0 && group.expanded_index >= n {
            group.expanded_index = n - 1;
        }
        if let Some(heights) = split_heights {
            if heights.len() == n {
                group.split_heights = Some(heights);
            }
        }
        if let Some(cache) = fullscreen_cache {
            if cache.len() == n {
                group.fullscreen_cache = Some(cache);
            }
        }
        group
    }

    /// Add panel to group.
    pub fn add_panel(&mut self, panel: Box<dyn Panel>) {
        self.panels.push(panel);
        self.height_pinned.push(false);
        self.on_panels_changed_insert(self.panels.len() - 1);
    }

    /// Insert panel at specific position. Adjusts focus index and the
    /// height caches if necessary.
    pub fn insert_panel(&mut self, index: usize, panel: Box<dyn Panel>) {
        let pos = if index >= self.panels.len() {
            self.panels.push(panel);
            self.panels.len() - 1
        } else {
            self.panels.insert(index, panel);
            if index <= self.expanded_index {
                self.expanded_index += 1;
            }
            index
        };
        self.height_pinned.insert(pos, false);
        self.on_panels_changed_insert(pos);
    }

    /// Replace the panel at `index` in place, returning the old one.
    ///
    /// Panel count and layout (widths/heights, focus) are unchanged — only the
    /// occupant of the slot changes, which is what "swap this view for another"
    /// (e.g. hex ⇄ text) needs.
    pub fn replace_panel(&mut self, index: usize, panel: Box<dyn Panel>) -> Option<Box<dyn Panel>> {
        if index < self.panels.len() {
            Some(std::mem::replace(&mut self.panels[index], panel))
        } else {
            None
        }
    }

    /// Remove panel from group by index.
    pub fn remove_panel(&mut self, index: usize) -> Option<Box<dyn Panel>> {
        if index >= self.panels.len() {
            return None;
        }
        let panel = self.panels.remove(index);
        self.height_pinned.remove(index);

        if self.panels.is_empty() {
            self.expanded_index = 0;
        } else if self.expanded_index == index {
            self.expanded_index = index.saturating_sub(1);
        } else if self.expanded_index > index {
            self.expanded_index -= 1;
        } else if self.expanded_index >= self.panels.len() {
            self.expanded_index = self.panels.len() - 1;
        }

        self.on_panels_changed_remove(index);
        Some(panel)
    }

    /// Set focused panel by index. If the fullscreen preset is active,
    /// the preset is re-applied for the new focus so the visible panel
    /// follows the focus marker (matches the legacy accordion UX).
    pub fn set_expanded(&mut self, index: usize) {
        if index < self.panels.len() {
            self.expanded_index = index;
            self.refresh_fullscreen_if_active();
            self.panels[index].handle_command(PanelCommand::RefreshIfStale);
        }
    }

    /// Get focused panel index.
    pub fn expanded_index(&self) -> usize {
        self.expanded_index
    }

    /// Switch focus to next panel in group; re-applies fullscreen preset
    /// when active.
    pub fn next_panel(&mut self) {
        if !self.panels.is_empty() {
            self.expanded_index = (self.expanded_index + 1) % self.panels.len();
            self.refresh_fullscreen_if_active();
            self.panels[self.expanded_index].handle_command(PanelCommand::RefreshIfStale);
        }
    }

    /// Switch focus to previous panel in group; re-applies fullscreen
    /// preset when active.
    pub fn prev_panel(&mut self) {
        if !self.panels.is_empty() {
            self.expanded_index = if self.expanded_index == 0 {
                self.panels.len() - 1
            } else {
                self.expanded_index - 1
            };
            self.refresh_fullscreen_if_active();
            self.panels[self.expanded_index].handle_command(PanelCommand::RefreshIfStale);
        }
    }

    /// Number of panels in group.
    pub fn len(&self) -> usize {
        self.panels.len()
    }

    /// Whether the group is empty.
    pub fn is_empty(&self) -> bool {
        self.panels.is_empty()
    }

    /// Mutable reference to panels.
    pub fn panels_mut(&mut self) -> &mut [Box<dyn Panel>] {
        &mut self.panels
    }

    /// Reference to panels.
    pub fn panels(&self) -> &[Box<dyn Panel>] {
        &self.panels
    }

    /// Mutable reference to focused panel.
    pub fn expanded_panel_mut(&mut self) -> Option<&mut Box<dyn Panel>> {
        self.panels.get_mut(self.expanded_index)
    }

    /// Reference to focused panel.
    #[allow(clippy::borrowed_box)]
    pub fn expanded_panel(&self) -> Option<&Box<dyn Panel>> {
        self.panels.get(self.expanded_index)
    }

    /// Move panel up (swap with previous). Keeps focus and the height
    /// caches consistent with the new ordering.
    pub fn move_panel_up(&mut self, index: usize) -> anyhow::Result<()> {
        if index == 0 || self.panels.is_empty() {
            return Ok(());
        }
        if index >= self.panels.len() {
            return Err(anyhow::anyhow!("Panel index out of bounds"));
        }
        self.panels.swap(index - 1, index);
        self.height_pinned.swap(index - 1, index);
        if self.expanded_index == index {
            self.expanded_index = index - 1;
        } else if self.expanded_index == index - 1 {
            self.expanded_index = index;
        }
        if let Some(heights) = self.split_heights.as_mut() {
            heights.swap(index - 1, index);
        }
        if let Some(cache) = self.fullscreen_cache.as_mut() {
            cache.swap(index - 1, index);
        }
        Ok(())
    }

    /// Move panel down (swap with next). Keeps focus and the height
    /// caches consistent with the new ordering.
    pub fn move_panel_down(&mut self, index: usize) -> anyhow::Result<()> {
        if self.panels.is_empty() {
            return Ok(());
        }
        if index >= self.panels.len() - 1 {
            return Ok(());
        }
        self.panels.swap(index, index + 1);
        self.height_pinned.swap(index, index + 1);
        if self.expanded_index == index {
            self.expanded_index = index + 1;
        } else if self.expanded_index == index + 1 {
            self.expanded_index = index;
        }
        if let Some(heights) = self.split_heights.as_mut() {
            heights.swap(index, index + 1);
        }
        if let Some(cache) = self.fullscreen_cache.as_mut() {
            cache.swap(index, index + 1);
        }
        Ok(())
    }

    /// Take all panels from the group (empties it).
    pub fn take_panels(self) -> Vec<Box<dyn Panel>> {
        self.panels
    }

    // ----- Heights API -----

    /// Cached split heights, if any. Callers needing usable heights for
    /// a given area should prefer [`Self::effective_split_heights`].
    pub fn split_heights(&self) -> Option<&[u16]> {
        self.split_heights.as_deref()
    }

    /// Snapshot of the heights kept aside while the fullscreen preset is
    /// active (`None` when not in fullscreen). Used by layout save.
    pub fn fullscreen_cache(&self) -> Option<&[u16]> {
        self.fullscreen_cache.as_deref()
    }

    /// Whether the group is currently in the fullscreen preset.
    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen_cache.is_some()
    }

    /// Replace the split-heights cache directly. The caller is
    /// responsible for length and sum invariants — for normalisation,
    /// chase with [`Self::redistribute_heights_proportionally`].
    ///
    /// Implicitly exits the fullscreen preset (clears
    /// `fullscreen_cache`): an explicit heights override is a free-
    /// resize action, and leaving the preset cache in place would let
    /// a later toggle restore stale heights.
    pub fn set_split_heights(&mut self, heights: Vec<u16>) {
        if heights.len() == self.panels.len() {
            self.fullscreen_cache = None;
            self.split_heights = Some(heights);
        }
    }

    /// Exit the fullscreen preset if it is active, restoring the
    /// cached pre-fullscreen heights as the new free-resize state
    /// (rescaled to `area_height`). No-op when not in fullscreen.
    ///
    /// Used before any operation that takes manual control of heights
    /// (mouse divider drag, header-drag resize, header-drag drop into
    /// the group, `Ctrl+Alt+=/-`) so the preset's `[1, …, big, …, 1]`
    /// shape doesn't interfere.
    pub fn exit_fullscreen_preset(&mut self, area_height: u16) {
        if let Some(cached) = self.fullscreen_cache.take() {
            if cached.len() == self.panels.len() {
                let mut restored = cached;
                redistribute_proportionally(&mut restored, area_height, MIN_PANEL_HEIGHT);
                self.split_heights = Some(restored);
            }
        }
    }

    /// Compute heights to use for rendering given the available area.
    /// Uses the cache when its length matches the panel count, otherwise
    /// falls back to equal distribution. Always rescales to fit
    /// `area_height` so geometry is stable across terminal resizes.
    /// Does not mutate state.
    pub fn effective_split_heights(&self, area_height: u16) -> Vec<u16> {
        let n = self.panels.len();
        if n == 0 {
            return Vec::new();
        }
        let mut heights = match self.split_heights.as_ref() {
            Some(cached) if cached.len() == n => cached.clone(),
            _ => equal_heights(n, area_height),
        };

        let fits = self.fit_heights(area_height);
        let free: Vec<usize> = (0..n).filter(|&i| fits[i].is_none()).collect();
        if free.len() == n {
            redistribute_proportionally(&mut heights, area_height, MIN_PANEL_HEIGHT);
            return heights;
        }

        // Fit panels are fixed slots; the free ones share what is left with
        // their cached proportions. `fit_heights` already left MIN for each
        // free panel, so the remainder is never short.
        let fit_total: u16 = fits.iter().flatten().sum();
        let mut free_heights: Vec<u16> = free.iter().map(|&i| heights[i]).collect();
        redistribute_proportionally(
            &mut free_heights,
            area_height.saturating_sub(fit_total),
            MIN_PANEL_HEIGHT,
        );
        let mut free_heights = free_heights.into_iter();
        for (i, slot) in heights.iter_mut().enumerate() {
            *slot = match fits[i] {
                Some(fit) => fit,
                None => free_heights.next().unwrap_or(MIN_PANEL_HEIGHT),
            };
        }
        heights
    }

    /// The fixed height of every [`HeightMode::FitContent`] panel that is
    /// not pinned, `None` for the free ones. A lone panel is always free —
    /// there is nobody to leave the rest of the column to. Each fit panel
    /// is capped at `1 / FIT_MAX_SHARE` of the column, and all of them
    /// together at what leaves the free panels [`MIN_PANEL_HEIGHT`] each.
    fn fit_heights(&self, area_height: u16) -> Vec<Option<u16>> {
        let n = self.panels.len();
        if n < 2 {
            return vec![None; n];
        }
        let wanted: Vec<Option<u16>> = self
            .panels
            .iter()
            .zip(&self.height_pinned)
            .map(|(panel, &pinned)| match panel.height_mode() {
                HeightMode::FitContent(rows) if !pinned => Some(rows),
                _ => None,
            })
            .collect();
        let free_count = wanted.iter().filter(|w| w.is_none()).count() as u16;
        if free_count == 0 {
            return vec![None; n];
        }
        let cap = (area_height / FIT_MAX_SHARE).max(MIN_PANEL_HEIGHT);
        let mut budget = area_height.saturating_sub(free_count * MIN_PANEL_HEIGHT);
        wanted
            .into_iter()
            .map(|rows| {
                let rows = rows?;
                let fit = rows
                    .clamp(MIN_PANEL_HEIGHT, cap)
                    .min(budget)
                    .max(MIN_PANEL_HEIGHT);
                budget = budget.saturating_sub(fit);
                Some(fit)
            })
            .collect()
    }

    /// Per panel: whether it shows content in a column `area_height` rows
    /// tall, i.e. has at least one row below its title bar. A panel at
    /// [`MIN_PANEL_HEIGHT`] — collapsed by the fullscreen preset or shrunk by
    /// hand — is a bare header and can skip work until it grows again.
    pub fn content_visibility(&self, area_height: u16) -> Vec<bool> {
        self.effective_split_heights(area_height)
            .into_iter()
            .map(|h| h >= 2)
            .collect()
    }

    /// Whether the panel at `index` follows its `HeightMode` (false) or a
    /// height the user set by hand (true).
    pub fn is_height_pinned(&self, index: usize) -> bool {
        self.height_pinned.get(index).copied().unwrap_or(false)
    }

    /// Bring the cache in line with what is on screen and return it. Manual
    /// height operations start from here so a fit panel's on-screen height,
    /// not its stale cache entry, is what the user's adjustment applies to.
    fn materialize_heights(&mut self, area_height: u16) -> Vec<u16> {
        let heights = self.effective_split_heights(area_height);
        self.split_heights = Some(heights.clone());
        heights
    }

    /// Pin every panel whose height a manual operation changed, so the
    /// change sticks instead of being overridden by `HeightMode` on the
    /// next frame.
    fn pin_changed(&mut self, before: &[u16], after: &[u16]) {
        for (i, (b, a)) in before.iter().zip(after).enumerate() {
            if b != a {
                if let Some(pinned) = self.height_pinned.get_mut(i) {
                    *pinned = true;
                }
            }
        }
    }

    /// Toggle the "fullscreen current panel" preset.
    ///
    /// Off → On: stash the current heights into the cache and apply
    /// `[1, …, area_height − (n − 1), …, 1]` with the maximum at
    /// `expanded_index`. Off-state heights are restored on toggle-off.
    ///
    /// On → Off: restore the cached heights (rescaled to current
    /// `area_height`).
    ///
    /// No-op for groups with fewer than two panels.
    pub fn toggle_fullscreen(&mut self, area_height: u16) {
        if self.panels.len() < 2 {
            return;
        }
        if self.fullscreen_cache.is_some() {
            // Off
            if let Some(cache) = self.fullscreen_cache.take() {
                let mut restored = cache;
                redistribute_proportionally(&mut restored, area_height, MIN_PANEL_HEIGHT);
                self.split_heights = Some(restored);
            }
        } else {
            // On
            let stash = self.effective_split_heights(area_height);
            self.fullscreen_cache = Some(stash);
            self.apply_fullscreen_preset(area_height);
        }
    }

    /// Redistribute split heights proportionally to a new `area_height`.
    /// Runs whenever the group resizes so heights track the available
    /// area without losing user proportions.
    pub fn redistribute_heights_proportionally(&mut self, area_height: u16) {
        let n = self.panels.len();
        if n == 0 {
            return;
        }
        if self.is_fullscreen() {
            // The preset is derived from area_height itself, so just
            // re-apply it instead of rescaling proportionally.
            self.apply_fullscreen_preset(area_height);
        } else {
            let mut heights = self
                .split_heights
                .clone()
                .unwrap_or_else(|| equal_heights(n, area_height));
            redistribute_proportionally(&mut heights, area_height, MIN_PANEL_HEIGHT);
            self.split_heights = Some(heights);
        }
    }

    /// Grow the focused panel by `lines`, taking space from neighbours.
    /// Cascades downward first (next, next+1, …); falls back upward
    /// when every panel below is at [`MIN_PANEL_HEIGHT`].
    ///
    /// Manual heights operation — exits the fullscreen preset when
    /// active so the user's adjustment lands on the cached free-resize
    /// state instead of the preset's `[1, …, big, …, 1]` shape.
    pub fn grow_focused(&mut self, lines: u16, area_height: u16) {
        if self.panels.len() < 2 || lines == 0 {
            return;
        }
        self.exit_fullscreen_preset(area_height);
        let n = self.panels.len();
        let before = self.materialize_heights(area_height);
        let mut heights = before.clone();
        let focused = self.expanded_index.min(n - 1);
        let mut to_add = lines;
        let mut i = focused + 1;
        while to_add > 0 && i < n {
            let take = heights[i].saturating_sub(MIN_PANEL_HEIGHT).min(to_add);
            heights[i] -= take;
            heights[focused] += take;
            to_add -= take;
            i += 1;
        }
        let mut j = focused;
        while to_add > 0 && j > 0 {
            j -= 1;
            let take = heights[j].saturating_sub(MIN_PANEL_HEIGHT).min(to_add);
            heights[j] -= take;
            heights[focused] += take;
            to_add -= take;
        }
        self.pin_changed(&before, &heights);
        self.split_heights = Some(heights);
    }

    /// Shrink the focused panel by `lines`, giving the freed space to
    /// the next neighbour below (or above when focused is the last
    /// panel).
    ///
    /// Manual heights operation — exits the fullscreen preset when
    /// active.
    pub fn shrink_focused(&mut self, lines: u16, area_height: u16) {
        if self.panels.len() < 2 || lines == 0 {
            return;
        }
        self.exit_fullscreen_preset(area_height);
        let n = self.panels.len();
        let before = self.materialize_heights(area_height);
        let mut heights = before.clone();
        let focused = self.expanded_index.min(n - 1);
        let available = heights[focused].saturating_sub(MIN_PANEL_HEIGHT);
        let actual = lines.min(available);
        if actual == 0 {
            return;
        }
        let recipient = if focused + 1 < n {
            focused + 1
        } else if focused > 0 {
            focused - 1
        } else {
            return;
        };
        heights[focused] -= actual;
        heights[recipient] += actual;
        self.pin_changed(&before, &heights);
        self.split_heights = Some(heights);
    }

    /// Apply a drag delta to the divider between panels `upper_idx` and
    /// `upper_idx + 1`. Positive `delta_lines` grows the upper panel,
    /// negative grows the lower. Heights clamp to [`MIN_PANEL_HEIGHT`].
    ///
    /// Manual heights operation — exits the fullscreen preset when
    /// active so the resize lands on the user's free-resize layout.
    pub fn resize_panel_divider(&mut self, upper_idx: usize, delta_lines: i32, area_height: u16) {
        let n = self.panels.len();
        if upper_idx + 1 >= n {
            return;
        }
        self.exit_fullscreen_preset(area_height);
        let before = self.materialize_heights(area_height);
        let mut heights = before.clone();
        let upper = heights[upper_idx] as i32;
        let lower = heights[upper_idx + 1] as i32;
        let min = MIN_PANEL_HEIGHT as i32;
        let new_upper = (upper + delta_lines).clamp(min, upper + lower - min);
        let new_lower = upper + lower - new_upper;
        heights[upper_idx] = new_upper as u16;
        heights[upper_idx + 1] = new_lower as u16;
        self.pin_changed(&before, &heights);
        self.split_heights = Some(heights);
    }

    // ----- Internal helpers -----

    fn refresh_fullscreen_if_active(&mut self) {
        if self.fullscreen_cache.is_some() {
            // Recompute the preset heights for the new focus. The cache
            // (pre-fullscreen heights) is intentionally left intact so
            // toggling off restores the original layout.
            let area_height = self
                .split_heights
                .as_ref()
                .map(|h| h.iter().sum())
                .unwrap_or(0);
            self.apply_fullscreen_preset(area_height);
        }
    }

    fn apply_fullscreen_preset(&mut self, area_height: u16) {
        let n = self.panels.len();
        if n == 0 {
            return;
        }
        let boosted = self.fullscreen_boost_index();
        let collapsed = MIN_PANEL_HEIGHT;
        let collapsed_total = collapsed as u32 * (n as u32 - 1);
        let boosted_height = (area_height as u32)
            .saturating_sub(collapsed_total)
            .max(collapsed as u32) as u16;
        let mut heights = vec![collapsed; n];
        heights[boosted] = boosted_height;
        self.split_heights = Some(heights);
    }

    /// The panel the fullscreen preset enlarges: the focused one, unless it
    /// is a fit panel. A fit panel keeps its own rows under the preset, so
    /// boosting it would collapse nothing and leave the free panels to share
    /// the column evenly; the free panel the preset enlarged before keeps
    /// the column instead.
    fn fullscreen_boost_index(&self) -> usize {
        let n = self.panels.len();
        let focused = self.expanded_index.min(n - 1);
        let is_fit = |i: usize| {
            matches!(self.panels[i].height_mode(), HeightMode::FitContent(_))
                && !self.is_height_pinned(i)
        };
        if n < 2 || !is_fit(focused) {
            return focused;
        }
        let current = self.split_heights.as_deref().filter(|h| h.len() == n);
        (0..n)
            .filter(|&i| !is_fit(i))
            .max_by_key(|&i| (current.map_or(0, |h| h[i]), std::cmp::Reverse(i)))
            .unwrap_or(focused)
    }

    fn on_panels_changed_insert(&mut self, inserted_at: usize) {
        let n = self.panels.len();
        Self::insert_into_cache(&mut self.split_heights, inserted_at, n);
        Self::insert_into_cache(&mut self.fullscreen_cache, inserted_at, n);
        if self.is_fullscreen() {
            // Re-derive the preset against the new panel count.
            let area_height = self
                .split_heights
                .as_ref()
                .map(|h| h.iter().sum())
                .unwrap_or(0);
            self.apply_fullscreen_preset(area_height);
        }
    }

    fn on_panels_changed_remove(&mut self, removed_at: usize) {
        Self::remove_from_cache(&mut self.split_heights, removed_at);
        Self::remove_from_cache(&mut self.fullscreen_cache, removed_at);
        if self.is_fullscreen() {
            let area_height = self
                .split_heights
                .as_ref()
                .map(|h| h.iter().sum())
                .unwrap_or(0);
            self.apply_fullscreen_preset(area_height);
        }
    }

    fn insert_into_cache(slot: &mut Option<Vec<u16>>, inserted_at: usize, new_len: usize) {
        if let Some(heights) = slot.as_mut() {
            let old_total: u32 = heights.iter().map(|&v| v as u32).sum();
            let new_share = if new_len > 0 {
                (old_total / new_len as u32) as u16
            } else {
                MIN_PANEL_HEIGHT
            };
            heights.insert(inserted_at, new_share.max(MIN_PANEL_HEIGHT));
            redistribute_proportionally(heights, old_total as u16, MIN_PANEL_HEIGHT);
        }
    }

    fn remove_from_cache(slot: &mut Option<Vec<u16>>, removed_at: usize) {
        if let Some(heights) = slot.as_mut() {
            if removed_at < heights.len() {
                let target_sum: u32 = heights.iter().map(|&v| v as u32).sum();
                heights.remove(removed_at);
                if heights.is_empty() {
                    *slot = None;
                } else {
                    redistribute_proportionally(heights, target_sum as u16, MIN_PANEL_HEIGHT);
                }
            }
        }
    }
}

/// Distribute `total` across `n` slots as evenly as possible
/// (largest-remainder).
fn equal_heights(n: usize, total: u16) -> Vec<u16> {
    if n == 0 {
        return Vec::new();
    }
    let per = total / n as u16;
    let rem = total % n as u16;
    (0..n as u16)
        .map(|i| if i < rem { per + 1 } else { per }.max(MIN_PANEL_HEIGHT))
        .collect()
}

/// Rescale `values` so their sum equals `target_sum`, preserving
/// relative proportions and ensuring every value is at least `min`.
/// Uses largest-remainder distribution for sub-pixel leftovers. Mirrors
/// `LayoutManager::redistribute_widths_proportionally` for the vertical
/// axis.
fn redistribute_proportionally(values: &mut [u16], target_sum: u16, min: u16) {
    let n = values.len();
    if n == 0 {
        return;
    }
    let min_total = (min as u32) * (n as u32);

    if min_total >= target_sum as u32 {
        for v in values.iter_mut() {
            *v = min;
        }
        let allocated = (min as u32) * (n as u32 - 1);
        if let Some(last) = values.last_mut() {
            *last = (target_sum as u32)
                .saturating_sub(allocated)
                .max(min as u32) as u16;
        }
        return;
    }

    let current_sum: u32 = values.iter().map(|&v| v as u32).sum();
    if current_sum == 0 {
        let equal = equal_heights(n, target_sum);
        values.copy_from_slice(&equal);
        return;
    }

    let mut new_values: Vec<u16> = Vec::with_capacity(n);
    let mut remainders: Vec<(usize, f64)> = Vec::with_capacity(n);
    let mut allocated: u32 = 0;

    for (idx, &v) in values.iter().enumerate() {
        let proportion = v as f64 / current_sum as f64;
        let exact = target_sum as f64 * proportion;
        let floored = (exact.floor() as u16).max(min);
        new_values.push(floored);
        let remainder = if floored > min {
            exact - floored as f64
        } else {
            -1.0
        };
        remainders.push((idx, remainder));
        allocated += floored as u32;
    }

    let mut leftover = (target_sum as u32).saturating_sub(allocated) as u16;
    if leftover > 0 {
        remainders.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        for &(idx, _) in &remainders {
            if leftover == 0 {
                break;
            }
            new_values[idx] += 1;
            leftover -= 1;
        }
    }

    let mut total: u32 = new_values.iter().map(|&v| v as u32).sum();
    while total > target_sum as u32 {
        if let Some(idx) = new_values
            .iter()
            .enumerate()
            .filter(|(_, &v)| v > min)
            .max_by_key(|(_, &v)| v)
            .map(|(i, _)| i)
        {
            new_values[idx] -= 1;
            total -= 1;
        } else {
            break;
        }
    }

    for (i, &v) in new_values.iter().enumerate() {
        values[i] = v;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use std::any::Any;
    use termide_core::{HeightMode, Panel, PanelEvent, RenderContext};

    struct DummyPanel(&'static str);

    impl Panel for DummyPanel {
        fn name(&self) -> &'static str {
            self.0
        }
        fn title(&self) -> String {
            self.0.to_string()
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
    }

    fn make_panel(name: &'static str) -> Box<dyn Panel> {
        Box::new(DummyPanel(name))
    }

    /// A panel that asks for a fixed number of rows, like the operations
    /// panel does for its cards. The cell lets a test change the request
    /// after the panel is in a group.
    struct FitPanel(std::cell::Cell<u16>);

    impl Panel for FitPanel {
        fn name(&self) -> &'static str {
            "fit"
        }
        fn title(&self) -> String {
            "fit".to_string()
        }
        fn render(&mut self, _area: Rect, _buf: &mut Buffer, _ctx: &RenderContext) {}
        fn handle_key(&mut self, _chord: termide_core::KeyChord) -> Vec<PanelEvent> {
            vec![]
        }
        fn height_mode(&self) -> HeightMode {
            HeightMode::FitContent(self.0.get())
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    fn make_fit_panel(rows: u16) -> Box<dyn Panel> {
        Box::new(FitPanel(std::cell::Cell::new(rows)))
    }

    /// `[a, fit(rows), c]` with `a` focused.
    fn group_with_fit(rows: u16) -> PanelGroup {
        let mut g = PanelGroup::new(make_panel("a"));
        g.add_panel(make_fit_panel(rows));
        g.add_panel(make_panel("c"));
        g
    }

    fn set_fit_rows(g: &PanelGroup, index: usize, rows: u16) {
        g.panels()[index]
            .as_any()
            .downcast_ref::<FitPanel>()
            .unwrap()
            .0
            .set(rows);
    }

    #[test]
    fn fit_panel_takes_its_rows_and_free_panels_share_the_rest() {
        let mut g = group_with_fit(8);
        g.set_split_heights(vec![20, 10, 10]);
        let h = g.effective_split_heights(40);
        assert_eq!(h[1], 8);
        assert_eq!(h.iter().sum::<u16>(), 40);
        // The free pair keeps its 2:1 proportion over the remaining 32.
        assert_eq!(h[0], 21);
        assert_eq!(h[2], 11);
    }

    #[test]
    fn fit_panel_follows_its_content_between_frames() {
        let g = group_with_fit(5);
        assert_eq!(g.effective_split_heights(40)[1], 5);
        set_fit_rows(&g, 1, 11);
        assert_eq!(g.effective_split_heights(40)[1], 11);
        assert_eq!(g.effective_split_heights(40).iter().sum::<u16>(), 40);
    }

    #[test]
    fn fit_panel_is_capped_to_its_share_of_the_column() {
        let g = group_with_fit(60);
        let h = g.effective_split_heights(40);
        assert_eq!(h[1], 40 / FIT_MAX_SHARE);
        assert_eq!(h.iter().sum::<u16>(), 40);
        // A tiny column still leaves every free panel its minimum row.
        let h = g.effective_split_heights(4);
        assert!(h.iter().all(|&x| x >= MIN_PANEL_HEIGHT), "{h:?}");
        assert_eq!(h.iter().sum::<u16>(), 4);
    }

    #[test]
    fn a_lone_fit_panel_fills_its_column() {
        let g = PanelGroup::new(make_fit_panel(5));
        assert_eq!(g.effective_split_heights(30), vec![30]);
    }

    #[test]
    fn resizing_a_fit_panel_by_hand_pins_it() {
        let mut g = group_with_fit(8);
        assert!(!g.is_height_pinned(1));
        // Drag the divider under the fit panel three rows down.
        g.resize_panel_divider(1, 3, 40);
        assert!(g.is_height_pinned(1));
        assert!(
            g.is_height_pinned(2),
            "the neighbour that gave the rows is pinned too"
        );
        assert!(!g.is_height_pinned(0));
        let h = g.effective_split_heights(40);
        assert_eq!(h[1], 11, "{h:?}");
        // The content request no longer matters for a pinned panel.
        set_fit_rows(&g, 1, 20);
        assert_eq!(g.effective_split_heights(40)[1], 11);
    }

    #[test]
    fn growing_a_free_panel_starts_from_what_is_on_screen() {
        let mut g = group_with_fit(8);
        g.set_split_heights(vec![16, 16, 8]);
        // On screen: fit is 8, the free pair shares 32 as 2:1 → [21, 8, 11].
        g.grow_focused(2, 40);
        let h = g.effective_split_heights(40);
        assert_eq!(h, vec![23, 6, 11], "{h:?}");
        assert!(
            g.is_height_pinned(1),
            "the fit panel lost rows to the grow, so it is pinned"
        );
    }

    #[test]
    fn fullscreen_preset_keeps_the_fit_panel_and_boosts_the_focused_one() {
        let mut g = group_with_fit(8);
        g.toggle_fullscreen(40);
        let h = g.effective_split_heights(40);
        assert_eq!(h, vec![31, 8, 1], "{h:?}");
        // Focus on the fit panel: it stays at its rows and the free panel
        // the preset enlarged keeps the column.
        g.set_expanded(1);
        let h = g.effective_split_heights(40);
        assert_eq!(h, vec![31, 8, 1], "{h:?}");
        // Focus moving on to a free panel boosts that one as before.
        g.set_expanded(2);
        let h = g.effective_split_heights(40);
        assert_eq!(h, vec![1, 8, 31], "{h:?}");
    }

    #[test]
    fn opening_a_fit_panel_under_fullscreen_keeps_the_column_collapsed() {
        // The operations panel opens right after the focused panel of a
        // fullscreen column and takes the column's focus.
        let mut g = PanelGroup::new(make_panel("a"));
        g.add_panel(make_panel("b"));
        g.toggle_fullscreen(40);
        g.insert_panel(1, make_fit_panel(5));
        g.set_expanded(1);
        let h = g.effective_split_heights(40);
        assert_eq!(h, vec![34, 5, 1], "{h:?}");
        // Closing it hands the preset back to the panel it opened after.
        g.remove_panel(1);
        let h = g.effective_split_heights(40);
        assert_eq!(h, vec![39, 1], "{h:?}");
    }

    #[test]
    fn pinned_flags_follow_panels_through_insert_remove_and_swap() {
        let mut g = group_with_fit(8);
        g.resize_panel_divider(1, 1, 40);
        assert!(g.is_height_pinned(1) && g.is_height_pinned(2));
        g.insert_panel(0, make_panel("z"));
        assert!(!g.is_height_pinned(0) && g.is_height_pinned(2) && g.is_height_pinned(3));
        g.move_panel_up(2).unwrap();
        assert!(g.is_height_pinned(1) && !g.is_height_pinned(2));
        g.remove_panel(1);
        assert!(!g.is_height_pinned(1) && g.is_height_pinned(2));
        assert_eq!(g.height_pinned.len(), g.len());
    }

    fn three_panel_group() -> PanelGroup {
        let mut g = PanelGroup::new(make_panel("a"));
        g.add_panel(make_panel("b"));
        g.add_panel(make_panel("c"));
        g
    }

    #[test]
    fn redistribute_preserves_sum() {
        let mut v = vec![10u16, 20, 30];
        redistribute_proportionally(&mut v, 60, 1);
        assert_eq!(v.iter().sum::<u16>(), 60);

        let mut v = vec![10u16, 20, 30];
        redistribute_proportionally(&mut v, 100, 1);
        assert_eq!(v.iter().sum::<u16>(), 100);
    }

    #[test]
    fn redistribute_respects_min() {
        let mut v = vec![1u16, 1, 100];
        redistribute_proportionally(&mut v, 12, 3);
        assert!(v.iter().all(|&x| x >= 3));
        assert_eq!(v.iter().sum::<u16>(), 12);
    }

    #[test]
    fn equal_heights_balanced_with_remainder() {
        let h = equal_heights(3, 10);
        assert_eq!(h.iter().sum::<u16>(), 10);
        assert!(h[0] >= h[2]);
    }

    #[test]
    fn toggle_fullscreen_round_trips() {
        let mut g = three_panel_group();
        // Pre-set distinctive heights.
        g.set_split_heights(vec![7, 13, 10]);
        assert!(!g.is_fullscreen());

        g.toggle_fullscreen(30);
        assert!(g.is_fullscreen());
        let h = g.split_heights().unwrap();
        // [1, max, 1] with focus 0 => [28, 1, 1]
        assert_eq!(h.iter().sum::<u16>(), 30);
        assert_eq!(h[0], 28);
        assert_eq!(h[1], 1);
        assert_eq!(h[2], 1);
        // Cache holds pre-toggle layout (rescaled to 30).
        assert_eq!(g.fullscreen_cache().unwrap().iter().sum::<u16>(), 30);

        g.toggle_fullscreen(30);
        assert!(!g.is_fullscreen());
        let restored = g.split_heights().unwrap();
        assert_eq!(restored.iter().sum::<u16>(), 30);
    }

    #[test]
    fn next_panel_in_fullscreen_moves_preset() {
        let mut g = three_panel_group();
        g.toggle_fullscreen(30);
        let h = g.split_heights().unwrap();
        assert_eq!(h, &[28, 1, 1]);

        g.next_panel(); // focus -> 1
        let h = g.split_heights().unwrap();
        assert_eq!(h, &[1, 28, 1]);

        g.next_panel(); // focus -> 2
        let h = g.split_heights().unwrap();
        assert_eq!(h, &[1, 1, 28]);

        g.prev_panel(); // back to 1
        let h = g.split_heights().unwrap();
        assert_eq!(h, &[1, 28, 1]);
    }

    #[test]
    fn toggle_noop_for_single_panel() {
        let mut g = PanelGroup::new(make_panel("solo"));
        g.toggle_fullscreen(30);
        assert!(!g.is_fullscreen());
    }

    #[test]
    fn grow_focused_takes_from_neighbour_below() {
        let mut g = three_panel_group();
        g.set_split_heights(vec![10, 10, 10]);
        g.set_expanded(0);
        g.grow_focused(3, 30);
        let h = g.split_heights().unwrap();
        assert_eq!(h.iter().sum::<u16>(), 30);
        assert_eq!(h[0], 13);
        assert_eq!(h[1], 7);
        assert_eq!(h[2], 10);
    }

    #[test]
    fn grow_focused_cascades_when_neighbour_at_min() {
        let mut g = three_panel_group();
        g.set_split_heights(vec![10, MIN_PANEL_HEIGHT, 19]);
        g.set_expanded(0);
        g.grow_focused(5, 30);
        let h = g.split_heights().unwrap();
        assert_eq!(h.iter().sum::<u16>(), 30);
        assert_eq!(h[0], 15);
        assert_eq!(h[1], MIN_PANEL_HEIGHT);
        assert_eq!(h[2], 14);
    }

    #[test]
    fn shrink_focused_to_min_clamps() {
        let mut g = three_panel_group();
        g.set_split_heights(vec![10, 10, 10]);
        g.set_expanded(0);
        g.shrink_focused(20, 30);
        let h = g.split_heights().unwrap();
        assert_eq!(h.iter().sum::<u16>(), 30);
        assert_eq!(h[0], MIN_PANEL_HEIGHT);
    }

    #[test]
    fn resize_panel_divider_redistributes_pair() {
        let mut g = three_panel_group();
        g.set_split_heights(vec![10, 10, 10]);
        g.resize_panel_divider(0, 4, 30);
        let h = g.split_heights().unwrap();
        assert_eq!(h.iter().sum::<u16>(), 30);
        assert_eq!(h[0], 14);
        assert_eq!(h[1], 6);
        assert_eq!(h[2], 10);
    }

    #[test]
    fn insert_panel_rebalances() {
        let mut g = three_panel_group();
        g.set_split_heights(vec![10, 10, 10]);
        g.insert_panel(1, make_panel("x"));
        assert_eq!(g.len(), 4);
        let h = g.split_heights().unwrap();
        assert_eq!(h.len(), 4);
        assert_eq!(h.iter().sum::<u16>(), 30);
    }

    #[test]
    fn remove_panel_redistributes() {
        let mut g = three_panel_group();
        g.set_split_heights(vec![10, 10, 10]);
        g.remove_panel(1);
        assert_eq!(g.len(), 2);
        let h = g.split_heights().unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h.iter().sum::<u16>(), 30);
    }

    #[test]
    fn move_panel_swaps_heights_and_cache() {
        let mut g = three_panel_group();
        g.set_split_heights(vec![5, 15, 10]);
        g.toggle_fullscreen(30);
        // Cache has pre-toggle [5, 15, 10] (rescaled to 30 → still [5, 15, 10]).
        g.move_panel_up(2).unwrap();
        // Heights and cache both swapped (idx 1 ↔ 2).
        let cache = g.fullscreen_cache().unwrap();
        assert_eq!(cache, &[5, 10, 15]);
    }

    #[test]
    fn effective_split_heights_does_not_mutate() {
        let g = three_panel_group();
        let h = g.effective_split_heights(30);
        assert_eq!(h.iter().sum::<u16>(), 30);
        assert!(g.split_heights().is_none());
    }
}
