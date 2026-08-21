//! Database viewer panel for termide.
//!
//! A read-only, git-status-shaped panel: a table selector on top and a 2D
//! pseudographic grid below. Connections come from bookmarks (the `path` field
//! holds a DB URL). Queries run on `termide-db`'s background runtime; the panel
//! polls the result receivers from `tick()`, so the UI never blocks.
//!
//! Scope of this first cut: connect → list tables → browse a table with a
//! cell cursor, sliding-window pagination, single-column sort, and copy.
//! Filtering, the row-detail modal, schema selectors and the in-app password
//! prompt are layered on next (see `ROADMAP.md.tmp`).

mod actions;
mod dropdown;
mod filter;
mod format;
mod render;

use std::any::Any;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use termide_config::Config;
use termide_core::{
    CommandResult, HotkeyTable, KeyChord, Panel, PanelCommand, PanelEvent, RenderContext,
    ScrollAxis, ScrollBars, ThemeColors, WidthPreference,
};
use termide_db::{
    ColumnInfo, Condition, DbBackend, DbConnection, DbError, Page, PageRequest, SortDir,
};
use termide_modal::{ActionButton, ActiveModal, InfoActionModal};
use termide_state::PendingAction;

use crate::dropdown::Dropdown;

/// Default sliding-window size (rows held in memory per page fetch).
/// Fetch-window size before the first render tells us the real viewport height.
const DEFAULT_PAGE_ROWS: u64 = 40;

/// Which zone has focus inside the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    DbSelector,
    TableSelector,
    Grid,
}

/// Connection lifecycle. Connecting happens on a throwaway thread (the
/// `DbConnection::connect` call blocks), so the UI stays responsive.
enum ConnState {
    Connecting(Receiver<Result<DbConnection, DbError>>),
    Connected(DbConnection),
    Failed(String),
}

/// The database viewer panel.
pub struct DbPanel {
    /// Full connection URL (may carry a password — never rendered verbatim).
    url: String,
    /// Display label (bookmark description or sanitized URL).
    label: String,
    backend: DbBackend,
    conn: ConnState,

    // --- catalog ---
    /// Whether the connection URL omitted a database (→ offer a DB selector).
    needs_db_pick: bool,
    databases: Vec<String>,
    selected_db: Option<String>,
    tables: Vec<String>,
    selected_table: Option<String>,
    columns: Vec<ColumnInfo>,

    // --- current page (sliding window) ---
    page: Page,
    total_rows: Option<i64>,
    offset: u64,
    /// Scrollbars drawn by the last render, for mouse thumb dragging.
    scrollbars: ScrollBars,

    // --- grid cursor / scroll ---
    cursor_row: usize,
    cursor_col: usize,
    row_scroll: usize,
    col_scroll: usize,
    /// After a page-up load, place the cursor on the last row of the new page.
    pending_bottom: bool,

    // --- query state ---
    filters: Vec<Condition>,
    order_by: Vec<(String, SortDir)>,
    /// Sliding-window size = the grid's visible row count (set during render),
    /// so one fetched window is exactly one screen — paging, not scrolling.
    page_rows: u64,

    // --- focus / selector ---
    section: Section,
    db_dd: Dropdown,
    table_dd: Dropdown,

    // --- async receivers (polled in tick) ---
    databases_rx: Option<Receiver<Result<Vec<String>, DbError>>>,
    tables_rx: Option<Receiver<Result<Vec<String>, DbError>>>,
    columns_rx: Option<Receiver<Result<Vec<ColumnInfo>, DbError>>>,
    count_rx: Option<Receiver<Result<i64, DbError>>>,
    page_rx: Option<Receiver<Result<Page, DbError>>>,
    loading: bool,
    /// Last query error (e.g. a bad filter). Non-fatal — shown in the status
    /// bar while the previous page stays visible.
    query_error: Option<String>,

    // --- input ---
    hotkeys: HotkeyTable,
    last_config_ptr: usize,

    // --- render cache ---
    cached_theme: ThemeColors,
    last_area: Rect,
    /// Number of data rows visible in the grid viewport (set during render).
    visible_rows: usize,
    /// Mouse hit-test geometry captured during render.
    geom: GridGeometry,

    /// Pending modal request, polled by the app via `take_modal_request`.
    modal_request: Option<(PendingAction, ActiveModal)>,
}

/// Screen geometry captured each render for mouse hit-testing.
#[derive(Debug, Clone, Default)]
pub(crate) struct GridGeometry {
    /// Y of the selector row.
    selector_y: u16,
    /// X where the table-selector chip starts (db chip, if any, is left of it).
    table_selector_x: u16,
    /// Y of the column-header row (when the grid is shown).
    header_y: Option<u16>,
    /// Y of the first data row.
    data_y0: u16,
    /// Per visible column: (column index, x start, x end-exclusive).
    columns: Vec<(usize, u16, u16)>,
}

impl DbPanel {
    /// Open a panel for `url`. `label` is the bookmark description (falls back to
    /// a sanitized URL). Connection starts immediately in the background.
    pub fn new(url: impl Into<String>, label: impl Into<String>) -> Self {
        let url = url.into();
        let label_in = label.into();
        let backend = DbBackend::from_url(&url).unwrap_or(DbBackend::Sqlite);
        let label = if label_in.is_empty() {
            sanitize_url(&url)
        } else {
            label_in
        };
        let selected_db = url_database(&url);
        let needs_db_pick = backend != DbBackend::Sqlite && selected_db.is_none();
        let conn = spawn_connect(url.clone());
        Self {
            url,
            label,
            backend,
            conn: ConnState::Connecting(conn),
            needs_db_pick,
            databases: Vec::new(),
            selected_db,
            tables: Vec::new(),
            selected_table: None,
            columns: Vec::new(),
            page: Page::default(),
            total_rows: None,
            offset: 0,
            cursor_row: 0,
            cursor_col: 0,
            row_scroll: 0,
            col_scroll: 0,
            pending_bottom: false,
            scrollbars: ScrollBars::default(),
            filters: Vec::new(),
            order_by: Vec::new(),
            page_rows: DEFAULT_PAGE_ROWS,
            section: Section::TableSelector,
            db_dd: Dropdown::default(),
            table_dd: Dropdown::default(),
            databases_rx: None,
            tables_rx: None,
            columns_rx: None,
            count_rx: None,
            page_rx: None,
            loading: true,
            query_error: None,
            hotkeys: HotkeyTable::default(),
            last_config_ptr: 0,
            cached_theme: ThemeColors::default(),
            last_area: Rect::default(),
            visible_rows: 0,
            geom: GridGeometry::default(),
            modal_request: None,
        }
    }

    /// The connection URL (used for session persistence / reconnect).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Enter the failed state and offer recovery (reconnect / close). The
    /// connection never retries on its own, so the dialog is shown exactly
    /// once per failure (at the transition into `Failed`).
    fn fail(&mut self, msg: String) {
        self.conn = ConnState::Failed(msg);
        self.show_connection_error_modal();
    }

    /// Recovery dialog for a lost / failed DB connection: Reconnect or Close
    /// panel. The button id is routed back through
    /// `PendingAction::DbConnectionError`.
    fn show_connection_error_modal(&mut self) {
        let t = termide_i18n::t();
        let msg = match &self.conn {
            ConnState::Failed(e) => e.clone(),
            _ => String::new(),
        };
        let modal = InfoActionModal::new(
            t.connection_error_title(),
            vec![(String::new(), msg)],
            vec![
                ActionButton::new(t.db_reconnect(), "reconnect"),
                ActionButton::new(t.db_close_panel(), "close"),
            ],
        );
        self.modal_request = Some((
            PendingAction::DbConnectionError,
            ActiveModal::InfoAction(Box::new(modal)),
        ));
    }

    /// Re-establish the connection to the current URL with a fresh handle,
    /// dropping any stale async receivers. Driven by the recovery dialog.
    pub fn reconnect(&mut self) {
        self.conn = ConnState::Connecting(spawn_connect(self.url.clone()));
        self.query_error = None;
        self.databases_rx = None;
        self.tables_rx = None;
        self.columns_rx = None;
        self.count_rx = None;
        self.page_rx = None;
    }

    /// Take a pending modal request (polled by the app each frame).
    pub fn take_modal_request(&mut self) -> Option<(PendingAction, ActiveModal)> {
        self.modal_request.take()
    }

    /// Build the shared-status-bar summary for the current view.
    fn status_text(&self) -> String {
        let t = termide_i18n::t();
        match &self.conn {
            ConnState::Connecting(_) => t.db_status_connecting_fmt(&self.label),
            ConnState::Failed(e) => t.db_status_failed_fmt(&self.label, e),
            ConnState::Connected(_) => {
                if self.needs_db_pick && self.selected_db.is_none() {
                    return format!(
                        "{} · {} · {}",
                        self.label,
                        self.backend.label(),
                        t.db_select_database()
                    );
                }
                let Some(table) = &self.selected_table else {
                    return format!(
                        "{} · {} · {}",
                        self.label,
                        self.backend.label(),
                        t.db_select_table()
                    );
                };
                let n = self.page.rows.len() as u64;
                let range = if n == 0 {
                    t.db_rows_empty().to_string()
                } else {
                    t.db_rows_range_fmt(self.offset + 1, self.offset + n)
                };
                let total = match self.total_rows {
                    Some(tot) => t.db_total_fmt(tot),
                    None => t.db_total_unknown().to_string(),
                };
                let sort = match self.order_by.first() {
                    Some((c, d)) => {
                        let arrow = if *d == SortDir::Asc { "↑" } else { "↓" };
                        t.db_sort_fmt(c, arrow)
                    }
                    None => String::new(),
                };
                let filter = if self.filters.is_empty() {
                    String::new()
                } else {
                    t.db_filter_count_fmt(self.filters.len())
                };
                // A non-fatal query error (bad filter) is appended as a warning.
                let err = match &self.query_error {
                    Some(e) => format!(" · ⚠ {e}"),
                    None => String::new(),
                };
                format!(
                    "{} · {} · {}{}{}{}{}",
                    self.label, table, range, total, sort, filter, err
                )
            }
        }
    }

    fn status_event(&self) -> PanelEvent {
        PanelEvent::SetStatusMessage {
            message: self.status_text(),
            is_error: matches!(self.conn, ConnState::Failed(_)) || self.query_error.is_some(),
        }
    }

    /// Switch to a different database: reconnect with the chosen DB in the URL,
    /// then list its tables. No-op if it's already the selected DB.
    fn select_database(&mut self, db: String) {
        if self.selected_db.as_deref() == Some(db.as_str()) {
            self.section = Section::TableSelector;
            return;
        }
        let new_url = url_with_database(&self.url, &db);
        self.selected_db = Some(db);
        self.conn = ConnState::Connecting(spawn_connect(new_url));
        // Reset catalog/grid state for the new database.
        self.tables.clear();
        self.selected_table = None;
        self.columns.clear();
        self.page = Page::default();
        self.total_rows = None;
        self.offset = 0;
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.filters.clear();
        self.order_by.clear();
        self.section = Section::TableSelector;
        self.loading = true;
    }

    /// (Re)issue columns + count + page queries for the selected table.
    fn reload_table(&mut self) {
        self.offset = 0;
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.row_scroll = 0;
        self.col_scroll = 0;
        self.total_rows = None;
        self.filters.clear();
        self.order_by.clear();
        self.reload_all();
    }

    /// Re-issue all three queries (columns, count, page) for the current
    /// table/filter/sort/offset.
    fn reload_all(&mut self) {
        let Some(table) = self.selected_table.clone() else {
            return;
        };
        let order_by = self.order_by.clone();
        let filters = self.filters.clone();
        let offset = self.offset;
        let limit = self.page_rows;
        let rxs = if let ConnState::Connected(conn) = &self.conn {
            Some((
                conn.columns(table.clone()),
                conn.count(table.clone(), filters.clone()),
                conn.page(PageRequest {
                    table,
                    filters,
                    order_by,
                    limit,
                    offset,
                }),
            ))
        } else {
            None
        };
        if let Some((c, n, p)) = rxs {
            self.columns_rx = Some(c);
            self.count_rx = Some(n);
            self.page_rx = Some(p);
            self.loading = true;
        }
    }

    /// Refresh the catalog and current view. While still choosing a database
    /// (URL omitted one), refresh the database list instead of listing tables
    /// from the bootstrap connection.
    fn refresh_catalog(&mut self) {
        if self.needs_db_pick && self.selected_db.is_none() {
            let rx = if let ConnState::Connected(conn) = &self.conn {
                Some(conn.list_databases())
            } else {
                None
            };
            if let Some(rx) = rx {
                self.databases_rx = Some(rx);
            }
            return;
        }
        let rx = if let ConnState::Connected(conn) = &self.conn {
            Some(conn.list_tables())
        } else {
            None
        };
        if let Some(rx) = rx {
            self.tables_rx = Some(rx);
        }
        self.reload_all();
    }

    /// Re-issue only the page query (window move / sort change), keeping the
    /// known column list and total count.
    /// Jump to an absolute scroll position reported by one of the panel's
    /// scrollbars (mouse thumb drag).
    ///
    /// The fetched window is one screen tall, so an absolute row is a
    /// page-aligned window plus a row inside it — a drag past the current
    /// window pages the query the same way `grid_down`/`grid_up` do.
    fn scroll_to(&mut self, axis: ScrollAxis, offset: usize) -> CommandResult {
        match axis {
            ScrollAxis::Horizontal => {
                self.col_scroll = offset;
                CommandResult::NeedsRedraw(true)
            }
            ScrollAxis::Vertical => {
                if self.loading_page() {
                    return CommandResult::NeedsRedraw(false);
                }
                let page = self.page_rows.max(1);
                let window = (offset as u64 / page) * page;
                let rel = offset.saturating_sub(window as usize);
                if window == self.offset {
                    self.cursor_row = rel.min(self.page.rows.len().saturating_sub(1));
                } else {
                    self.offset = window;
                    // `clamp_cursor` trims this once the window lands, the
                    // same as any other page load.
                    self.cursor_row = rel;
                    self.reload_page();
                }
                CommandResult::NeedsRedraw(true)
            }
        }
    }

    fn reload_page(&mut self) {
        let Some(table) = self.selected_table.clone() else {
            return;
        };
        let order_by = self.order_by.clone();
        let filters = self.filters.clone();
        let offset = self.offset;
        let limit = self.page_rows;
        let rx = if let ConnState::Connected(conn) = &self.conn {
            Some(conn.page(PageRequest {
                table,
                filters,
                order_by,
                limit,
                offset,
            }))
        } else {
            None
        };
        if let Some(p) = rx {
            self.page_rx = Some(p);
            self.loading = true;
        }
    }

    /// Poll all in-flight receivers; returns true if anything changed.
    fn poll_async(&mut self) -> bool {
        let mut changed = false;

        // Connection establishment.
        if let ConnState::Connecting(rx) = &self.conn {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(conn) => {
                        self.conn = ConnState::Connected(conn);
                        if let ConnState::Connected(c) = &self.conn {
                            if self.needs_db_pick && self.selected_db.is_none() {
                                // No database in the URL: enumerate and let the
                                // user pick before listing tables.
                                self.databases_rx = Some(c.list_databases());
                                self.section = Section::DbSelector;
                            } else {
                                self.tables_rx = Some(c.list_tables());
                            }
                        }
                    }
                    Err(e) => {
                        let msg = if e.is_auth() {
                            termide_i18n::t().db_auth_failed_fmt(&e.to_string())
                        } else {
                            e.to_string()
                        };
                        self.fail(msg);
                        self.loading = false;
                    }
                }
                changed = true;
            }
        }

        if let Some(rx) = &self.databases_rx {
            if let Ok(result) = rx.try_recv() {
                self.databases_rx = None;
                match result {
                    Ok(dbs) => {
                        self.databases = dbs;
                        // Auto-select the first database so data shows right
                        // away; the user can switch via the selector chip.
                        if self.selected_db.is_none() {
                            if let Some(first) = self.databases.first().cloned() {
                                self.select_database(first);
                            }
                        }
                    }
                    Err(e) => self.fail(e.to_string()),
                }
                changed = true;
            }
        }

        if let Some(rx) = &self.tables_rx {
            if let Ok(result) = rx.try_recv() {
                self.tables_rx = None;
                match result {
                    Ok(tables) => {
                        self.tables = tables;
                        let still_present = self
                            .selected_table
                            .as_ref()
                            .is_some_and(|t| self.tables.iter().any(|n| n == t));
                        if !still_present {
                            // Initial load, or the selected table vanished:
                            // (re-)select the first table and load it.
                            self.selected_table = self.tables.first().cloned();
                            if self.selected_table.is_some() {
                                self.section = Section::Grid;
                                self.reload_table();
                            } else {
                                self.loading = false;
                            }
                        }
                    }
                    Err(e) => self.fail(e.to_string()),
                }
                changed = true;
            }
        }

        if let Some(rx) = &self.columns_rx {
            if let Ok(result) = rx.try_recv() {
                self.columns_rx = None;
                if let Ok(cols) = result {
                    self.columns = cols;
                }
                changed = true;
            }
        }

        if let Some(rx) = &self.count_rx {
            if let Ok(result) = rx.try_recv() {
                self.count_rx = None;
                if let Ok(n) = result {
                    self.total_rows = Some(n);
                }
                changed = true;
            }
        }

        if let Some(rx) = &self.page_rx {
            if let Ok(result) = rx.try_recv() {
                self.page_rx = None;
                self.loading = false;
                match result {
                    Ok(page) => {
                        self.page = page;
                        self.query_error = None;
                        if self.pending_bottom {
                            self.cursor_row = self.page.rows.len().saturating_sub(1);
                            self.pending_bottom = false;
                        }
                        self.clamp_cursor();
                    }
                    // A failed query (e.g. a bad filter) is non-fatal: keep the
                    // connection and the previous page, surface the error.
                    Err(e) => self.query_error = Some(e.to_string()),
                }
                changed = true;
            }
        }

        changed
    }

    fn clamp_cursor(&mut self) {
        let rows = self.page.rows.len();
        if rows == 0 {
            self.cursor_row = 0;
        } else if self.cursor_row >= rows {
            self.cursor_row = rows - 1;
        }
        let cols = self.col_count();
        if cols == 0 {
            self.cursor_col = 0;
        } else if self.cursor_col >= cols {
            self.cursor_col = cols - 1;
        }
    }

    /// Number of columns to render (from catalog, falling back to page columns).
    fn col_count(&self) -> usize {
        if !self.columns.is_empty() {
            self.columns.len()
        } else {
            self.page.columns.len()
        }
    }

    fn column_names(&self) -> Vec<String> {
        if !self.columns.is_empty() {
            self.columns.iter().map(|c| c.name.clone()).collect()
        } else {
            self.page.columns.clone()
        }
    }

    fn is_connected(&self) -> bool {
        matches!(self.conn, ConnState::Connected(_))
    }
}

/// Build the configurable hotkey table for the DB panel.
fn build_db_hotkey_table(config: &Config) -> HotkeyTable {
    let mut t = HotkeyTable::new();
    let kb = &config.database.keybindings;
    t.insert("sort", &kb.sort);
    t.insert("filter", &kb.filter);
    t.insert("clear_filter", &kb.clear_filter);
    t.insert("detail", &kb.detail);
    t.insert("copy_row", &kb.copy_row);
    t.insert("refresh", &kb.refresh);
    t
}

/// Spawn a thread that connects and ships the handle (or error) back.
fn spawn_connect(url: String) -> Receiver<Result<DbConnection, DbError>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("termide-db-connect".into())
        .spawn(move || {
            let _ = tx.send(DbConnection::connect(&url));
        })
        .ok();
    rx
}

/// Strip a password from a URL for display (`scheme://user:***@host/…`).
fn sanitize_url(url: &str) -> String {
    // Find "://", then the authority up to the next '/'.
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after = scheme_end + 3;
    let rest = &url[after..];
    let authority_end = rest.find('/').unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if let Some(at) = authority.rfind('@') {
        let userinfo = &authority[..at];
        if let Some(colon) = userinfo.find(':') {
            let user = &userinfo[..colon];
            return format!(
                "{}://{}:***@{}{}",
                &url[..scheme_end],
                user,
                &authority[at + 1..],
                &rest[authority_end..]
            );
        }
    }
    url.to_string()
}

/// Extract the database name from a connection URL's path, if any.
fn url_database(url: &str) -> Option<String> {
    let after = url.split("://").nth(1)?;
    let slash = after.find('/')?;
    let path = &after[slash + 1..];
    let db = path.split(['?', '#']).next().unwrap_or("");
    if db.is_empty() {
        None
    } else {
        Some(db.to_string())
    }
}

/// Replace (or insert) the database in a connection URL, preserving the query.
fn url_with_database(url: &str, db: &str) -> String {
    let Some(sep) = url.find("://") else {
        return url.to_string();
    };
    let scheme = &url[..sep];
    let after = &url[sep + 3..];
    let auth_end = after.find(['/', '?']).unwrap_or(after.len());
    let authority = &after[..auth_end];
    let rest = &after[auth_end..];
    let query = match rest.find('?') {
        Some(q) => &rest[q..],
        None => "",
    };
    format!("{scheme}://{authority}/{db}{query}")
}

impl Panel for DbPanel {
    fn name(&self) -> &'static str {
        "db"
    }

    fn title(&self) -> String {
        match &self.selected_table {
            Some(t) => format!("DB: {} · {}", self.label, t),
            None => format!("DB: {}", self.label),
        }
    }

    fn prepare_render(&mut self, theme: &termide_theme::Theme, config: &Arc<Config>) {
        self.cached_theme = ThemeColors::from(theme);
        let config_ptr = Arc::as_ptr(config) as usize;
        if self.last_config_ptr != config_ptr {
            self.last_config_ptr = config_ptr;
            self.hotkeys = build_db_hotkey_table(config);
        }
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, ctx: &RenderContext) {
        self.last_area = area;
        self.render_content(
            area,
            buf,
            ctx.is_focused,
            ctx.border_right_x,
            ctx.border_bottom_y,
        );
    }

    fn handle_key(&mut self, chord: KeyChord) -> Vec<PanelEvent> {
        self.handle_key_impl(chord)
    }

    fn handle_mouse(
        &mut self,
        event: crossterm::event::MouseEvent,
        _panel_area: Rect,
    ) -> Vec<PanelEvent> {
        self.handle_mouse_impl(event)
    }

    fn tick(&mut self) -> Vec<PanelEvent> {
        if self.poll_async() {
            vec![PanelEvent::NeedsRedraw, self.status_event()]
        } else {
            vec![]
        }
    }

    fn handle_command(&mut self, cmd: PanelCommand<'_>) -> CommandResult {
        match cmd {
            // Global clipboard: copy the current cell value (previously the
            // per-panel `copy_cell` keybinding). The whole-row copy stays a
            // panel keybinding (`copy_row`).
            PanelCommand::Copy => {
                if let Some(text) = self.copy_text(false) {
                    let _ = termide_clipboard::copy(&text);
                }
                CommandResult::Handled(true)
            }
            PanelCommand::GetScrollBars => CommandResult::ScrollBars(self.scrollbars),
            PanelCommand::SetScrollOffset { axis, offset } => self.scroll_to(axis, offset),
            // Read-only table view: nothing to cut or paste into.
            PanelCommand::Cut => CommandResult::Handled(false),
            PanelCommand::Paste => CommandResult::Handled(false),
            _ => CommandResult::None,
        }
    }

    fn captures_escape(&self) -> bool {
        self.table_dd.open || self.db_dd.open
    }

    fn width_preference(&self) -> WidthPreference {
        WidthPreference::PreferWide
    }

    fn to_session(&self, _session_dir: &std::path::Path) -> Option<termide_core::SessionPanel> {
        Some(termide_core::SessionPanel::Database {
            url: self.url.clone(),
            label: self.label.clone(),
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
