//! System-resource monitoring and modal-spinner ticks: CPU/RAM/network/disk
//! refresh on the configured interval, and animation of any open spinner or
//! resource modal.

use crate::state::ActiveModal;

use super::App;

impl App {
    /// Update system resource monitoring (CPU, RAM, network)
    /// Respects the configured update interval.
    /// Only triggers redraw if display values actually changed.
    pub(super) fn update_system_resources(&mut self) {
        let interval =
            std::time::Duration::from_millis(self.state.config.general.resource_monitor_interval);
        let elapsed = self.state.last_resource_update.elapsed();

        if elapsed >= interval {
            let old_stats = self.state.system_monitor.stats();
            let old_net_down = self.state.system_monitor.net_download_rate();
            let old_net_up = self.state.system_monitor.net_upload_rate();
            self.state.system_monitor.update();
            self.state.last_resource_update = std::time::Instant::now();
            let new_stats = self.state.system_monitor.stats();
            let new_net_down = self.state.system_monitor.net_download_rate();
            let new_net_up = self.state.system_monitor.net_upload_rate();
            // Only redraw if display values actually changed
            if old_stats.cpu_usage.round() as u8 != new_stats.cpu_usage.round() as u8
                || old_stats.memory_used / (1024 * 1024) != new_stats.memory_used / (1024 * 1024)
                || old_net_down / 1024 != new_net_down / 1024
                || old_net_up / 1024 != new_net_up / 1024
            {
                self.state.needs_redraw = true;
            }
            self.update_disk_space();
        }
    }

    /// Update cached disk space for the active panel.
    /// Called on each resource tick so status bar reads from cache instead of per-render statvfs.
    fn update_disk_space(&mut self) {
        let disk = self.get_active_panel_disk_space();
        if disk != self.state.cache.disk_space {
            self.state.cache.disk_space = disk;
            self.state.needs_redraw = true;
        }
    }

    /// Update spinner in all modals that support animation
    /// Throttled to 125ms (8 FPS) to reduce unnecessary redraws
    pub(super) fn update_modal_spinners(&mut self) {
        const SPINNER_INTERVAL: std::time::Duration = std::time::Duration::from_millis(125);

        // Throttle spinner updates for all modals
        let should_update = self
            .state
            .last_spinner_update
            .is_none_or(|t| t.elapsed() >= SPINNER_INTERVAL);

        if !should_update {
            return;
        }

        match &mut self.state.active_modal {
            Some(ActiveModal::Info(ref mut modal)) => {
                // Update spinner only if calculation is still ongoing
                if self.state.dir_size_receiver.is_some() {
                    modal.advance_spinner();
                    self.state.last_spinner_update = Some(std::time::Instant::now());
                    self.state.needs_redraw = true;
                }
            }
            Some(ActiveModal::InfoAction(ref mut modal)) => {
                // Update spinner only if operation is still ongoing
                if modal.is_operation_in_progress() {
                    modal.advance_spinner();
                    self.state.last_spinner_update = Some(std::time::Instant::now());
                    self.state.needs_redraw = true;
                }
            }
            _ => {}
        }

        // Auto-refresh resource modal per resource_monitor_interval config
        self.refresh_resource_modal();
    }

    /// Refresh resource modal content if one is open and interval has elapsed.
    fn refresh_resource_modal(&mut self) {
        let interval =
            std::time::Duration::from_millis(self.state.config.general.resource_monitor_interval);

        let Some(kind) = self.state.resource_modal_kind else {
            return;
        };

        let should_refresh = self
            .state
            .last_resource_modal_refresh
            .is_none_or(|t| t.elapsed() >= interval);

        if !should_refresh {
            return;
        }

        use crate::state::ResourceModalKind;
        let lines = match kind {
            ResourceModalKind::Cpu | ResourceModalKind::Ram => self.build_process_lines(kind),
            ResourceModalKind::Network => self.build_network_modal_lines(),
            ResourceModalKind::Disk => self.build_disk_modal_lines(),
        };

        if let Some(ActiveModal::Info(ref mut modal)) = self.state.active_modal {
            modal.set_lines(lines);
            self.state.last_resource_modal_refresh = Some(std::time::Instant::now());
            self.state.needs_redraw = true;
        }
    }
}
