//! The MCP servers as the panel offers them: `/mcp`, and the buttons on each
//! server's heading in the toolset list. Both ask the catalog; what changes
//! comes back later as late tools and is announced then.

use termide_agent_core::{McpReload, McpSignIn, McpStatus};
use termide_core::{ChecklistButton, ChecklistGroup};

use crate::{AgentPanel, NoticeKind};

/// The heading buttons' ids: what the button does, then the server.
const RELOAD_BUTTON: &str = "mcp-reload:";
const LOGIN_BUTTON: &str = "mcp-login:";
const LOGOUT_BUTTON: &str = "mcp-logout:";

/// A server's state in words, and how loud to say it.
fn status_text(status: &McpStatus) -> (String, NoticeKind) {
    let t = termide_i18n::t();
    match status {
        McpStatus::Connecting => (
            t.agent_mcp_status_connecting().to_string(),
            NoticeKind::Info,
        ),
        McpStatus::Ready { tools } => (t.agent_mcp_status_ready_fmt(*tools), NoticeKind::Info),
        McpStatus::Failed(error) => (t.agent_mcp_status_failed_fmt(error), NoticeKind::Warn),
        McpStatus::NeedsLogin => (
            t.agent_mcp_status_needs_login().to_string(),
            NoticeKind::Warn,
        ),
        McpStatus::SigningIn => (
            t.agent_mcp_status_signing_in().to_string(),
            NoticeKind::Info,
        ),
    }
}

impl AgentPanel {
    /// `/mcp [reload [<server>] | login <server> | logout <server>]`: with no
    /// argument, where each server stands.
    pub(crate) fn mcp_command(&mut self, args: &str) {
        let args = args.trim();
        let (verb, server) = match args.split_once(char::is_whitespace) {
            Some((verb, server)) => (verb, Some(server.trim())),
            None => (args, None),
        };
        match (verb, server) {
            ("", _) => self.mcp_list(),
            ("reload", server) => self.mcp_reload(server),
            ("login", Some(server)) => self.mcp_login(server),
            ("logout", Some(server)) => self.mcp_logout(server),
            _ => self.notice(termide_i18n::t().agent_notice_mcp_usage(), NoticeKind::Warn),
        }
    }

    fn mcp_list(&mut self) {
        let t = termide_i18n::t();
        let servers = self.catalog.mcp_status();
        if servers.is_empty() {
            self.notice(t.agent_notice_mcp_none(), NoticeKind::Info);
        }
        for server in servers {
            let (text, kind) = status_text(&server.status);
            self.notice(t.agent_notice_mcp_status_fmt(&server.name, &text), kind);
        }
    }

    /// Every server, or the one named, from the configuration as it is now.
    fn mcp_reload(&mut self, server: Option<&str>) {
        let t = termide_i18n::t();
        let report = match server {
            None => match self.catalog.mcp_reload() {
                Some(report) => {
                    self.mcp_reconnecting.extend(report.started.iter().cloned());
                    report
                }
                None => return self.notice(t.agent_notice_mcp_none(), NoticeKind::Info),
            },
            Some(server) => match self.catalog.mcp_reconnect(server) {
                Ok(report) => {
                    if report.started.iter().any(|name| name == server) {
                        self.mcp_reconnecting.insert(server.to_string());
                        let (status, kind) = status_text(&McpStatus::Connecting);
                        let line = t.agent_notice_mcp_status_fmt(server, &status);
                        self.mcp_notice(server, line, kind);
                        // Under the banner the connecting line is the answer.
                        if self.banner_shown() {
                            return;
                        }
                    }
                    report
                }
                Err(error) => {
                    return self.notice(
                        t.agent_notice_mcp_error_fmt(server, &error),
                        NoticeKind::Warn,
                    )
                }
            },
        };
        self.notice_reload(&report);
    }

    fn notice_reload(&mut self, report: &McpReload) {
        let list = |names: &[String]| {
            if names.is_empty() {
                "—".to_string()
            } else {
                names.join(", ")
            }
        };
        self.notice(
            termide_i18n::t().agent_notice_mcp_reload_fmt(
                &list(&report.started),
                &list(&report.removed),
                &list(&report.kept),
            ),
            NoticeKind::Info,
        );
    }

    fn mcp_login(&mut self, server: &str) {
        if let Err(error) = self.catalog.mcp_login(server) {
            self.notice(
                termide_i18n::t().agent_notice_mcp_error_fmt(server, &error),
                NoticeKind::Warn,
            );
        }
    }

    fn mcp_logout(&mut self, server: &str) {
        let t = termide_i18n::t();
        match self.catalog.mcp_logout(server) {
            Ok(true) => self.notice(t.agent_notice_mcp_logout_fmt(server), NoticeKind::Info),
            Ok(false) => self.notice(t.agent_notice_mcp_no_login_fmt(server), NoticeKind::Info),
            Err(error) => self.notice(
                t.agent_notice_mcp_error_fmt(server, &error),
                NoticeKind::Warn,
            ),
        }
    }

    /// A heading per configured server in the toolset list, under the name
    /// its tools are grouped by: what state it is in when not connected, a
    /// button to connect it again, and one to sign in or out where that
    /// applies. A server with no tools is listed by its heading alone. The
    /// buttons are right-aligned, so the reload comes last: every server has
    /// it, and only some have the sign-in, so that one keeps its column.
    pub(crate) fn toolset_groups(&self) -> Vec<ChecklistGroup> {
        let t = termide_i18n::t();
        if !self.toolset_lists_mcp() {
            return Vec::new();
        }
        self.catalog
            .mcp_status()
            .into_iter()
            .map(|server| {
                let note = match &server.status {
                    McpStatus::Ready { .. } => String::new(),
                    other => status_text(other).0,
                };
                let mut buttons = Vec::new();
                let sign = match server.sign_in {
                    McpSignIn::None => None,
                    McpSignIn::SignedOut => Some((LOGIN_BUTTON, "⇥")),
                    McpSignIn::SignedIn => Some((LOGOUT_BUTTON, "⇤")),
                };
                if let Some((id, icon)) = sign {
                    buttons.push(ChecklistButton {
                        id: format!("{id}{}", server.name),
                        icon: icon.into(),
                        key: 'l',
                    });
                }
                buttons.push(ChecklistButton {
                    id: format!("{RELOAD_BUTTON}{}", server.name),
                    icon: "↻".into(),
                    key: 'r',
                });
                ChecklistGroup {
                    name: t.agent_toolset_mcp_fmt(&server.name),
                    note,
                    buttons,
                }
            })
            .collect()
    }

    /// A heading button of the toolset list was pressed.
    pub(crate) fn press_toolset_button(&mut self, id: &str) {
        if let Some(server) = id.strip_prefix(RELOAD_BUTTON) {
            self.mcp_reload(Some(server));
        } else if let Some(server) = id.strip_prefix(LOGIN_BUTTON) {
            self.mcp_login(server);
        } else if let Some(server) = id.strip_prefix(LOGOUT_BUTTON) {
            self.mcp_logout(server);
        }
    }
}
