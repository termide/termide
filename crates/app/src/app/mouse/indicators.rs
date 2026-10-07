//! Resource-indicator modals: CPU/RAM/Network/Disk builders and opener.
//!
//! All pure data assembly — no event handling. `handle_mouse_event` and
//! `handle_status_bar_click` in the parent module invoke these after a click
//! lands on the corresponding indicator.

use crate::app::App;
use crate::state::ActiveModal;
use termide_i18n as i18n;
use termide_modal as modal;
use termide_ui_render::{menu_bar_layout, MenuBarLayout, MenuRenderParams};

/// Pad or truncate `s` to exactly `width` display columns.
fn fit_name(s: &str, width: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    let w = s.width();
    if w <= width {
        let mut out = s.to_string();
        for _ in 0..(width - w) {
            out.push(' ');
        }
        out
    } else {
        let mut out = String::new();
        let mut cur = 0;
        for ch in s.chars() {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            if cur + cw > width - 1 {
                break;
            }
            out.push(ch);
            cur += cw;
        }
        out.push('…');
        cur += 1;
        for _ in 0..(width - cur) {
            out.push(' ');
        }
        out
    }
}

/// Map usage percentage to a styled segment via theme resource colors.
fn pct_to_style(pct: u8, theme: &termide_theme::Theme) -> termide_modal::info::SegmentStyle {
    use termide_modal::info::SegmentStyle;
    use termide_ui_render::resource_color;
    match resource_color(pct, theme) {
        c if c == theme.error => SegmentStyle::Error,
        c if c == theme.warning => SegmentStyle::Warning,
        _ => SegmentStyle::Success,
    }
}

impl App {
    /// The menu bar's layout as it is drawn now.
    pub(in crate::app) fn menu_bar(&self) -> MenuBarLayout {
        let (ram_value, ram_unit) = self.state.system_monitor.format_ram();
        let project_buttons = self.state.project_buttons();
        let params = MenuRenderParams {
            theme: self.state.theme,
            selected_menu_item: self.state.ui.selected_menu_item,
            menu_open: self.state.is_menu_open(),
            cpu_usage: self.state.system_monitor.cpu_usage(),
            ram_percent: self.state.system_monitor.ram_usage_percent(),
            ram_value,
            ram_unit,
            net_down_rate: self.state.system_monitor.net_download_rate(),
            net_up_rate: self.state.system_monitor.net_upload_rate(),
            battery: self.state.system_monitor.battery_cached(),
            projects: &project_buttons,
        };
        menu_bar_layout(self.state.terminal.width, &params)
    }

    /// Get disk space info from the active panel (if available).
    /// Uses the SystemMonitor mount cache to avoid re-reading `/proc/mounts`.
    pub(in crate::app) fn get_active_panel_disk_space(
        &self,
    ) -> Option<termide_system_monitor::DiskSpaceInfo> {
        use std::any::Any;
        let panel = self.layout_manager.active_panel()?;
        let panel_any = &**panel as &dyn Any;
        if let Some(fm) = panel_any.downcast_ref::<termide_panel_file_manager::FileManager>() {
            if fm.vfs_state().has_pending_operation() {
                return None;
            }
            return self
                .state
                .system_monitor
                .get_disk_space_info_cached(fm.current_path());
        }
        if let Some(editor) = panel_any.downcast_ref::<termide_panel_editor::Editor>() {
            if let Some(path) = editor.file_path() {
                return self.state.system_monitor.get_disk_space_info_cached(path);
            }
            return None;
        }
        // GitStatusPanel and Terminal use their own path resolution
        if let Some(git) = panel_any.downcast_ref::<termide_panel_git_status::GitStatusPanel>() {
            return git.get_disk_space_info();
        }
        if let Some(terminal) = panel_any.downcast_ref::<termide_panel_terminal::Terminal>() {
            let info = terminal.get_terminal_info();
            let cwd = std::path::Path::new(&info.cwd);
            return self.state.system_monitor.get_disk_space_info_cached(cwd);
        }
        None
    }

    /// Open CPU, RAM or Network processes modal.
    pub(in crate::app) fn open_resource_modal_at(
        &mut self,
        kind: crate::state::ResourceModalKind,
        anchor: Option<(u16, u16)>,
    ) {
        use crate::state::ResourceModalKind;

        let t = i18n::t();
        let (title, lines) = match kind {
            ResourceModalKind::Cpu => {
                let title = t.resource_cpu_top_title().to_owned();
                let lines = self.build_process_lines(kind);
                (title, lines)
            }
            ResourceModalKind::Ram => {
                let title = t.resource_ram_top_title().to_owned();
                let lines = self.build_process_lines(kind);
                (title, lines)
            }
            ResourceModalKind::Network => {
                let title = t.resource_net_title().to_owned();
                let lines = self.build_network_modal_lines();
                (title, lines)
            }
            ResourceModalKind::Disk => {
                let title = t.resource_disk_title().to_owned();
                let lines = self.build_disk_modal_lines();
                (title, lines)
            }
        };
        let mut modal = modal::InfoModal::new_rich(title, lines).with_min_width(57);
        if let Some((x, y)) = anchor {
            modal = modal.with_anchor(x, y).without_button();
        }
        self.state.active_modal = Some(ActiveModal::Info(Box::new(modal)));
        self.state.resource_modal_kind = Some(kind);
        self.state.last_resource_modal_refresh = Some(std::time::Instant::now());
        self.state.needs_redraw = true;
    }

    /// Build process lines for CPU or RAM modal (header + data rows).
    ///
    /// Column order is always: Application | CPU | RAM (for both modals).
    pub(in crate::app) fn build_process_lines(
        &self,
        kind: crate::state::ResourceModalKind,
    ) -> Vec<(String, termide_modal::info::ModalValue)> {
        use crate::state::ResourceModalKind;
        use termide_modal::info::{ModalValue, SegmentStyle, StyledSegment};
        use termide_system_monitor::format_bytes;

        // Fixed name column width so CPU/RAM columns never shift
        const NAME_COL: usize = 24;

        let t = i18n::t();
        let (cpu_processes, mem_processes) = self.state.system_monitor.top_processes_cached(10);
        let processes = match kind {
            ResourceModalKind::Cpu => &cpu_processes,
            ResourceModalKind::Ram => &mem_processes,
            // Only Cpu and Ram modals have process lists;
            // Network/Disk kinds should never reach this code path.
            ResourceModalKind::Network | ResourceModalKind::Disk => {
                unreachable!("build_process_lines called with {:?} kind", kind)
            }
        };
        let total_mem = self.state.system_monitor.stats().memory_total;

        // Header row — empty key, all columns in segments
        // Columns: count(6) + CPU(7) + RAM(10) = 23 chars in segments
        let mut lines: Vec<(String, ModalValue)> = vec![(
            fit_name("", NAME_COL),
            ModalValue::Segments(vec![
                StyledSegment {
                    text: format!("{:>6}", t.resource_count()),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>7}", "CPU"),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("  {:>8}", "RAM"),
                    style: SegmentStyle::Default,
                },
            ]),
        )];

        // Data rows
        for p in processes {
            let cpu_pct = p.cpu_percent.round() as u8;
            let cpu_color = pct_to_style(cpu_pct, self.state.theme);

            let mem_pct = if total_mem > 0 {
                ((p.memory_bytes as f64 / total_mem as f64) * 100.0) as u8
            } else {
                0
            };
            let ram_color = pct_to_style(mem_pct, self.state.theme);

            let count_text = format!("{:>6}", p.count);

            let segments = vec![
                StyledSegment {
                    text: count_text,
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!(" {:>5.1}%", p.cpu_percent),
                    style: cpu_color,
                },
                StyledSegment {
                    text: format!("  {:>8}", format_bytes(p.memory_bytes)),
                    style: ram_color,
                },
            ];
            lines.push((fit_name(&p.name, NAME_COL), ModalValue::Segments(segments)));
        }

        lines
    }

    /// Build disk space modal lines (header + data rows).
    ///
    /// Columns: type | free | used | total. One row per space pool: APFS
    /// volumes of one container share their free space and are listed once.
    pub(in crate::app) fn build_disk_modal_lines(
        &self,
    ) -> Vec<(String, termide_modal::info::ModalValue)> {
        use termide_modal::info::{ModalValue, SegmentStyle, StyledSegment};
        use termide_system_monitor::format_bytes;

        use unicode_width::UnicodeWidthStr;

        let t = i18n::t();
        let disks = self.state.system_monitor.get_all_disk_space_info_cached();

        // Filesystem type column: as wide as the localized header or the
        // longest label ("MS-DOS"), so the columns never collide.
        let type_col = disks
            .iter()
            .map(|d| d.fs_type_label().map(|s| s.width()).unwrap_or(0))
            .max()
            .unwrap_or(0)
            .max(t.resource_disk_type().width());

        // Header row: type | free | used | total
        let mut lines: Vec<(String, ModalValue)> = vec![(
            String::new(),
            ModalValue::Segments(vec![
                StyledSegment {
                    text: format!("{:<w$} ", t.resource_disk_type(), w = type_col),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>13}", t.resource_disk_free()),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>13}", t.resource_disk_used()),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>10}", t.resource_disk_total()),
                    style: SegmentStyle::Default,
                },
            ]),
        )];

        // Data rows
        for d in &disks {
            let name = d.device_name().unwrap_or_else(|| "???".to_string());
            let usage = d.usage_percent();
            let avail_pct = 100_u8.saturating_sub(usage);
            let used_color = pct_to_style(usage, self.state.theme);
            let segments = vec![
                StyledSegment {
                    text: format!(
                        "{:<w$} ",
                        d.fs_type_label().unwrap_or_default(),
                        w = type_col
                    ),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>3}% {:>8}", avail_pct, format_bytes(d.available)),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>3}% {:>8}", usage, format_bytes(d.used())),
                    style: used_color,
                },
                StyledSegment {
                    text: format!("  {:>8}", format_bytes(d.total)),
                    style: SegmentStyle::Default,
                },
            ];
            lines.push((name, ModalValue::Segments(segments)));
        }

        lines
    }

    /// Build network activity modal lines (header + data rows + speed footer).
    ///
    /// Columns: Application | Ports | Conn
    pub(in crate::app) fn build_network_modal_lines(
        &self,
    ) -> Vec<(String, termide_modal::info::ModalValue)> {
        use termide_modal::info::{ModalValue, SegmentStyle, StyledSegment};

        use unicode_width::UnicodeWidthStr;

        const NAME_COL: usize = 20;
        const PORTS_COL: usize = 14;

        fn fit_ports(ports: &[u16], width: usize) -> String {
            if ports.is_empty() {
                let mut s = "—".to_string();
                s.push_str(&" ".repeat(width - 1));
                return s;
            }
            let full: String = ports
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            if full.width() <= width {
                let pad = width - full.width();
                format!("{}{}", full, " ".repeat(pad))
            } else {
                // Truncate with …
                let mut out = String::new();
                let mut cur = 0;
                for ch in full.chars() {
                    let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                    if cur + cw > width - 1 {
                        break;
                    }
                    out.push(ch);
                    cur += cw;
                }
                out.push('…');
                for _ in 0..(width - cur - 1) {
                    out.push(' ');
                }
                out
            }
        }

        let processes = self.state.system_monitor.top_network_processes_cached(10);

        // Header row
        let mut lines: Vec<(String, ModalValue)> = vec![(
            fit_name("", NAME_COL),
            ModalValue::Segments(vec![
                StyledSegment {
                    text: format!("{:<width$}", "Ports", width = PORTS_COL),
                    style: SegmentStyle::Default,
                },
                StyledSegment {
                    text: format!("{:>5}", "Conn"),
                    style: SegmentStyle::Default,
                },
            ]),
        )];

        // Data rows
        for p in &processes {
            let ports_text = fit_ports(&p.listening_ports, PORTS_COL);
            let ports_style = if p.listening_ports.is_empty() {
                SegmentStyle::Disabled
            } else {
                SegmentStyle::Success
            };
            let conn_text = format!("{:>5}", p.connections);
            let conn_style = if p.connections == 0 {
                SegmentStyle::Disabled
            } else {
                SegmentStyle::Default
            };
            let segments = vec![
                StyledSegment {
                    text: ports_text,
                    style: ports_style,
                },
                StyledSegment {
                    text: conn_text,
                    style: conn_style,
                },
            ];
            lines.push((fit_name(&p.name, NAME_COL), ModalValue::Segments(segments)));
        }

        lines
    }
}
