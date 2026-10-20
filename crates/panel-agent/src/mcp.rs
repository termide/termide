//! The MCP servers as the panel offers them: `/mcp`, `/trust`, and each
//! server's heading in the toolset list, with its buttons and, for one that
//! waits for approval, its checkbox. Both ask the catalog; what changes
//! comes back later as late tools and is announced then.

use termide_agent_core::{McpReload, McpSignIn, McpStatus};
use termide_core::{ChecklistButton, ChecklistGroup, ChecklistSwitch};

use crate::{AgentPanel, NoticeKind};

/// The heading buttons' ids: what the button does, then the server.
const RELOAD_BUTTON: &str = "mcp-reload:";
const LOGIN_BUTTON: &str = "mcp-login:";
const LOGOUT_BUTTON: &str = "mcp-logout:";
/// The key of a held server's checkbox: ticked, the list approves it.
const TRUST_SWITCH: &str = "mcp-trust:";

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
        McpStatus::NeedsApproval => (
            t.agent_mcp_status_needs_approval().to_string(),
            NoticeKind::Warn,
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

    /// `/trust [<name>]`: what the project brings that waits for approval,
    /// with what each runs; with a name, approve that one.
    pub(crate) fn trust_command(&mut self, args: &str) {
        let t = termide_i18n::t();
        let name = args.trim();
        if !name.is_empty() {
            self.trust(name);
            return;
        }
        let servers: Vec<(String, String)> = self
            .catalog
            .mcp_status()
            .into_iter()
            .filter(|server| server.status == McpStatus::NeedsApproval)
            .map(|server| (server.name, server.launch))
            .collect();
        let hooks = self.catalog.held_hooks();
        if servers.is_empty() && hooks.is_empty() {
            return self.notice(t.agent_notice_trust_none(), NoticeKind::Info);
        }
        for (name, launch) in servers {
            self.notice(
                t.agent_notice_mcp_needs_approval_fmt(&name, &launch),
                NoticeKind::Warn,
            );
        }
        for (name, launch) in hooks {
            self.notice(
                t.agent_notice_hook_needs_approval_fmt(&name, &launch),
                NoticeKind::Warn,
            );
        }
    }

    /// Approve `name`; a server connects at once. `true` when approved.
    fn trust(&mut self, name: &str) -> bool {
        let t = termide_i18n::t();
        match self.catalog.trust(name) {
            Ok(report) => {
                self.notice(t.agent_notice_trusted_fmt(name), NoticeKind::Info);
                if report.is_some_and(|r| r.started.iter().any(|s| s == name)) {
                    self.mcp_reconnecting.insert(name.to_string());
                    let (status, kind) = status_text(&McpStatus::Connecting);
                    self.mcp_notice(name, t.agent_notice_mcp_status_fmt(name, &status), kind);
                }
                true
            }
            Err(error) => {
                self.notice(error, NoticeKind::Warn);
                false
            }
        }
    }

    /// The project's hooks held back, said once when the panel opens.
    pub(crate) fn notice_held_hooks(&mut self) {
        let t = termide_i18n::t();
        for (name, launch) in self.catalog.held_hooks() {
            self.notice(
                t.agent_notice_hook_needs_approval_fmt(&name, &launch),
                NoticeKind::Warn,
            );
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
    /// applies. A server with no tools is listed by its heading alone; one
    /// the project brings that waits for approval has a checkbox there,
    /// unticked, and nothing else: until it is approved there is nothing to
    /// reconnect or sign in to. The buttons are right-aligned,
    /// so the reload comes last: every server has it, and only some have the
    /// sign-in, so that one keeps its column.
    pub(crate) fn toolset_groups(&self) -> Vec<ChecklistGroup> {
        let t = termide_i18n::t();
        self.catalog
            .mcp_status()
            .into_iter()
            .map(|server| {
                let held = server.status == McpStatus::NeedsApproval;
                if held {
                    return ChecklistGroup {
                        name: t.agent_toolset_mcp_fmt(&server.name),
                        note: String::new(),
                        buttons: Vec::new(),
                        switch: Some(ChecklistSwitch {
                            key: format!("{TRUST_SWITCH}{}", server.name),
                            checked: false,
                        }),
                    };
                }
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
                    switch: None,
                }
            })
            .collect()
    }

    /// A heading button of the toolset list was pressed, or a held server's
    /// checkbox ticked; the list stays open. Ticking a server approves it
    /// and says "on": its tools come on when they arrive, though the
    /// session had switched them off before.
    pub(crate) fn press_toolset_button(&mut self, id: &str) {
        if let Some(server) = id.strip_prefix(TRUST_SWITCH) {
            if self.trust(server) {
                self.mcp_enable.insert(server.to_string());
            }
            return;
        }
        if let Some(server) = id.strip_prefix(RELOAD_BUTTON) {
            self.mcp_reload(Some(server));
        } else if let Some(server) = id.strip_prefix(LOGIN_BUTTON) {
            self.mcp_login(server);
        } else if let Some(server) = id.strip_prefix(LOGOUT_BUTTON) {
            self.mcp_logout(server);
        }
    }
}
