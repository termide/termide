//! The password vault: supplying stored passwords to connections, asking
//! for missing ones, and saving the ones the user asks to keep.
//!
//! Panels report a refused login with `PanelEvent::CredentialsRequired`;
//! git network operations are recognised from git's stderr. Either way the
//! request is answered from the vault when it holds a password that has not
//! been refused yet — unlocking it with the master password first if needed
//! — and otherwise by asking the user, with a checkbox to keep the password.
//! A kept password is written only once the connection accepted it, so a
//! typo never lands in the vault. Creating the vault happens on the first
//! save.
//!
//! The prompts are ordinary input modals under `PendingAction::Vault`; what
//! a prompt is for lives in [`VaultState::prompt`], so no secret travels in
//! a pending action.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use termide_core::{CredentialAttempt, GitOperationType, PanelCommand, SecretText};
use termide_modal::{ActiveModal, ConfirmModal, InputModal};
use termide_secrets::{origin, KdfParams, SecretKind, Vault};
use termide_state::PendingAction;

use super::App;

/// Vault origin of the SSH key passphrase git network operations use. git
/// runs ssh in batch mode, which does not tell which key failed, so the
/// passphrase is kept as one entry, as the session cache always was.
pub(crate) const GIT_SSH_ORIGIN: &str = "git-ssh-key";

/// Credentials a git network operation is retried with.
#[derive(Debug, Clone)]
pub(crate) enum GitAuth {
    SshPassphrase(SecretText),
    Https { user: String, password: SecretText },
}

/// Something that needs a password.
#[derive(Debug, Clone)]
pub(crate) enum Request {
    /// A panel's refused login, answered by `PanelCommand::ProvideCredentials`.
    Panel {
        url: String,
        origin: String,
        user: Option<String>,
    },
    /// A git network operation, answered by running it again.
    Git {
        operation: GitOperationType,
        repo: PathBuf,
        target: GitTarget,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum GitTarget {
    /// A remote over HTTPS: `origin` is `https://host[:port]`.
    Https {
        origin: String,
        user: Option<String>,
    },
    /// The SSH key passphrase.
    SshKey,
}

impl Request {
    fn kind(&self) -> SecretKind {
        match self {
            Request::Panel { .. } => SecretKind::Password,
            Request::Git {
                target: GitTarget::Https { .. },
                ..
            } => SecretKind::GitHttps,
            Request::Git {
                target: GitTarget::SshKey,
                ..
            } => SecretKind::SshPassphrase,
        }
    }

    fn origin(&self) -> &str {
        match self {
            Request::Panel { origin, .. }
            | Request::Git {
                target: GitTarget::Https { origin, .. },
                ..
            } => origin,
            Request::Git {
                target: GitTarget::SshKey,
                ..
            } => GIT_SSH_ORIGIN,
        }
    }

    fn user(&self) -> Option<&str> {
        match self {
            Request::Panel { user, .. }
            | Request::Git {
                target: GitTarget::Https { user, .. },
                ..
            } => user.as_deref(),
            Request::Git {
                target: GitTarget::SshKey,
                ..
            } => None,
        }
    }

    /// What the prompts call it: `user@host` or the key passphrase.
    fn target_label(&self) -> String {
        match self {
            Request::Git {
                target: GitTarget::SshKey,
                ..
            } => termide_i18n::t().vault_target_ssh_key().to_string(),
            _ => match self.user() {
                Some(user) => format!("{user}@{}", self.origin()),
                None => self.origin().to_string(),
            },
        }
    }
}

/// A password the user asked to keep, waiting for its connection to succeed.
struct PendingSave {
    request: Request,
    secret: SecretText,
    /// Done once the secret is stored.
    then: Option<AfterSave>,
}

/// Work that must wait until a secret is safely in the vault.
enum AfterSave {
    /// Remove the password from the bookmark URL it was moved out of.
    StripBookmark { url: String, is_project: bool },
}

/// Where to go back to after a bookmark prompt.
pub(crate) struct BookmarkReturn {
    pub group: Option<String>,
    pub is_project: bool,
    pub selected: usize,
}

/// What the open `PendingAction::Vault` modal is asking for.
enum Prompt {
    /// The master password, to continue with `then`.
    Unlock { then: Request },
    /// A login password for `request`.
    Password { request: Request },
    /// The git user name for an HTTPS remote, before its password.
    GitUser { request: Request },
    /// A master password for the vault about to be created.
    NewMaster { save: PendingSave },
    /// The same master password again.
    RepeatMaster {
        save: PendingSave,
        first: SecretText,
    },
    /// Whether to move the password out of a bookmark URL.
    MoveBookmarkPassword { url: String, back: BookmarkReturn },
}

/// App-wide vault state.
pub(crate) struct VaultState {
    path: Option<PathBuf>,
    /// Loaded on first use; `None` also while no vault file exists.
    vault: Option<Vault>,
    last_used: Instant,
    prompt: Option<Prompt>,
    /// The "save in vault" checkbox of the prompt just confirmed.
    save_checked: bool,
    pending_saves: Vec<PendingSave>,
    /// Which password the git operation in flight was retried with.
    git_attempt: Option<CredentialAttempt>,
    /// The HTTPS user name it was retried with; git's refusal names none.
    git_user: Option<String>,
}

impl Default for VaultState {
    fn default() -> Self {
        Self::new(Vault::default_path())
    }
}

impl VaultState {
    pub(crate) fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            vault: None,
            last_used: Instant::now(),
            prompt: None,
            save_checked: false,
            pending_saves: Vec::new(),
            git_attempt: None,
            git_user: None,
        }
    }

    /// The vault, re-read from disk so changes by other termide processes
    /// show; `None` when there is none.
    fn current(&mut self) -> Result<Option<&mut Vault>, termide_secrets::Error> {
        let Some(path) = self.path.clone() else {
            return Ok(None);
        };
        match &mut self.vault {
            Some(v) => v.reload()?,
            None => self.vault = Vault::open(&path)?,
        }
        Ok(self.vault.as_mut())
    }

    fn has_entry(&mut self, request: &Request) -> bool {
        match self.current() {
            Ok(Some(v)) => v
                .find(request.kind(), request.origin(), request.user())
                .is_some(),
            Ok(None) => false,
            Err(e) => {
                log::warn!("password vault unreadable: {e}");
                false
            }
        }
    }

    /// Whether the vault is unlocked.
    pub(crate) fn is_unlocked(&self) -> bool {
        self.vault.as_ref().is_some_and(Vault::is_unlocked)
    }

    /// Lock the vault; returns whether it was unlocked.
    pub(crate) fn lock(&mut self) -> bool {
        let was = self.is_unlocked();
        if let Some(v) = &mut self.vault {
            v.lock();
        }
        was
    }

    /// Lock after `idle` without use. Cheap enough for every loop pass.
    pub(crate) fn lock_if_idle(&mut self, idle: Option<Duration>) -> bool {
        match idle {
            Some(limit) if self.is_unlocked() && self.last_used.elapsed() >= limit => self.lock(),
            _ => false,
        }
    }

    /// Record the checkbox of a vault prompt about to close.
    pub(crate) fn note_checkbox(&mut self, checked: bool) {
        self.save_checked = checked;
    }
}

fn user_name(value: &dyn std::any::Any) -> Option<String> {
    value
        .downcast_ref::<String>()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn secret(value: &dyn std::any::Any) -> Option<SecretText> {
    value
        .downcast_ref::<String>()
        .filter(|s| !s.is_empty())
        .map(|s| SecretText::new(s.as_str()))
}

impl App {
    fn vault_idle_limit(&self) -> Option<Duration> {
        match self.state.config.vault.lock_after_mins {
            0 => None,
            mins => Some(Duration::from_secs(mins * 60)),
        }
    }

    /// Lock the vault after the configured idle time. Called every loop pass.
    pub(super) fn vault_tick(&mut self) {
        let idle = self.vault_idle_limit();
        if self.vault.lock_if_idle(idle) {
            log::info!("password vault locked after idle time");
        }
    }

    /// Lock the vault now (menu action).
    pub(super) fn vault_lock_now(&mut self) {
        self.vault.lock();
        self.state
            .set_info(termide_i18n::t().vault_locked().to_string());
    }

    fn vault_error(&mut self, e: &termide_secrets::Error) {
        log::warn!("password vault: {e}");
        self.state
            .set_error(termide_i18n::t().vault_error(&e.to_string()));
    }

    fn open_vault_prompt(&mut self, prompt: Prompt, modal: InputModal) {
        self.vault.prompt = Some(prompt);
        self.vault.save_checked = false;
        self.state
            .set_pending_action(PendingAction::Vault, ActiveModal::Input(Box::new(modal)));
        self.state.needs_redraw = true;
    }

    // === Entry points ===

    /// A panel's login to `url` was refused.
    pub(super) fn event_credentials_required(&mut self, url: String, attempt: CredentialAttempt) {
        let Some(parts) = origin::parse_url(&url) else {
            log::warn!("credentials requested for an unparsable URL");
            return;
        };
        let request = Request::Panel {
            url,
            origin: parts.origin,
            user: parts.user,
        };
        self.answer(request, attempt);
    }

    /// A panel's login succeeded with the password last provided.
    pub(super) fn event_credentials_accepted(&mut self, url: &str) {
        let at = self
            .vault
            .pending_saves
            .iter()
            .position(|s| matches!(&s.request, Request::Panel { url: u, .. } if u == url));
        if let Some(at) = at {
            let save = self.vault.pending_saves.remove(at);
            self.save_password(save);
        }
    }

    /// A git network operation finished (or failed for another reason than
    /// credentials). Saves a kept password on success.
    pub(super) fn vault_git_finished(&mut self, success: bool) {
        self.vault.git_attempt = None;
        self.vault.git_user = None;
        let at = self
            .vault
            .pending_saves
            .iter()
            .position(|s| matches!(s.request, Request::Git { .. }));
        if let Some(at) = at {
            let save = self.vault.pending_saves.remove(at);
            if success {
                self.save_password(save);
            }
        }
    }

    /// A git network operation failed with `stderr`. Returns whether it was
    /// an authentication failure this handled (by retrying or asking).
    pub(super) fn vault_git_auth_failed(
        &mut self,
        operation: &str,
        repo: PathBuf,
        stderr: &str,
    ) -> bool {
        let operation = match operation {
            "push" => GitOperationType::Push,
            "pull" => GitOperationType::Pull,
            _ => GitOperationType::Fetch,
        };
        let retried_user = self.vault.git_user.take();
        let target = match https_auth_url(stderr).and_then(|url| origin::parse_url(&url)) {
            Some(parts) => GitTarget::Https {
                origin: parts.origin,
                user: parts.user.or(retried_user),
            },
            None if is_ssh_auth_failure(stderr) => GitTarget::SshKey,
            None => {
                // Not about credentials: forget the retry state.
                self.vault_git_finished(false);
                return false;
            }
        };
        let attempt = self
            .vault
            .git_attempt
            .take()
            .unwrap_or(CredentialAttempt::Initial);
        // A failed retry drops the password it wanted to keep.
        self.vault
            .pending_saves
            .retain(|s| !matches!(s.request, Request::Git { .. }));
        if matches!(target, GitTarget::SshKey) {
            // Never retry the session's cached passphrase twice.
            self.state.git_ssh_passphrase = None;
        }
        self.answer(
            Request::Git {
                operation,
                repo,
                target,
            },
            attempt,
        );
        true
    }

    // === The flow ===

    /// Answer `request`, whose last login used `attempt`.
    fn answer(&mut self, request: Request, attempt: CredentialAttempt) {
        if attempt == CredentialAttempt::Initial && self.vault.has_entry(&request) {
            self.fulfil_from_vault(request);
        } else {
            self.ask_password(request, attempt);
        }
    }

    fn fulfil_from_vault(&mut self, request: Request) {
        let found = match self.vault.current() {
            Ok(Some(v)) => v.get(request.kind(), request.origin(), request.user()),
            Ok(None) => Ok(None),
            Err(e) => Err(e),
        };
        match found {
            Ok(Some((info, secret))) => {
                self.vault.last_used = Instant::now();
                let request = match request {
                    // The entry names the user a user-less HTTPS URL lacks.
                    Request::Git {
                        operation,
                        repo,
                        target: GitTarget::Https { origin, user: None },
                    } => Request::Git {
                        operation,
                        repo,
                        target: GitTarget::Https {
                            origin,
                            user: info.user,
                        },
                    },
                    other => other,
                };
                self.deliver(
                    &request,
                    SecretText::new(secret.as_str()),
                    CredentialAttempt::Stored,
                );
            }
            Ok(None) => self.ask_password(request, CredentialAttempt::Initial),
            Err(termide_secrets::Error::Locked) => self.ask_unlock(request, false),
            Err(e) => {
                self.vault_error(&e);
                self.ask_password(request, CredentialAttempt::Initial);
            }
        }
    }

    fn ask_unlock(&mut self, then: Request, wrong: bool) {
        let t = termide_i18n::t();
        let prompt = if wrong {
            t.vault_unlock_wrong()
        } else {
            t.vault_unlock_prompt()
        };
        let modal = InputModal::new(t.vault_unlock_title(), prompt).password();
        self.open_vault_prompt(Prompt::Unlock { then }, modal);
    }

    fn ask_password(&mut self, request: Request, attempt: CredentialAttempt) {
        if let Request::Git {
            target: GitTarget::Https { user: None, .. },
            ..
        } = &request
        {
            return self.ask_git_user(request);
        }
        let t = termide_i18n::t();
        let target = request.target_label();
        let prompt = match attempt {
            CredentialAttempt::Initial => t.vault_password_prompt(&target),
            CredentialAttempt::Stored => t.vault_stored_refused(&target),
            CredentialAttempt::Typed => t.vault_password_refused(&target),
        };
        let modal = InputModal::new(t.vault_password_title(), prompt)
            .password()
            .with_checkbox(t.vault_save_checkbox().to_string())
            // Replacing a refused stored password is the likely wish.
            .checkbox_checked(attempt == CredentialAttempt::Stored);
        self.open_vault_prompt(Prompt::Password { request }, modal);
    }

    fn ask_git_user(&mut self, request: Request) {
        let t = termide_i18n::t();
        let modal = InputModal::new(
            t.vault_password_title(),
            t.git_username_prompt(request.origin()),
        );
        self.open_vault_prompt(Prompt::GitUser { request }, modal);
    }

    fn ask_new_master(&mut self, save: PendingSave, mismatch: bool) {
        let t = termide_i18n::t();
        let prompt = if mismatch {
            t.vault_create_mismatch()
        } else {
            t.vault_create_prompt()
        };
        let modal = InputModal::new(t.vault_create_title(), prompt).password();
        self.open_vault_prompt(Prompt::NewMaster { save }, modal);
    }

    /// Hand `secret` to whoever asked.
    fn deliver(&mut self, request: &Request, secret: SecretText, source: CredentialAttempt) {
        match request {
            Request::Panel { url, .. } => {
                let mut taken = false;
                for panel in self.layout_manager.iter_all_panels_mut() {
                    let result = panel.handle_command(PanelCommand::ProvideCredentials {
                        url,
                        password: &secret,
                        source,
                    });
                    if matches!(result, termide_core::CommandResult::Handled(true)) {
                        taken = true;
                        break;
                    }
                }
                if !taken {
                    log::info!("no panel waits for the provided credentials any more");
                }
                self.state.needs_redraw = true;
            }
            Request::Git {
                operation,
                repo,
                target,
            } => {
                let auth = match target {
                    GitTarget::SshKey => {
                        self.state.git_ssh_passphrase = Some(secret.expose().to_string());
                        GitAuth::SshPassphrase(secret)
                    }
                    GitTarget::Https { user, .. } => {
                        self.vault.git_user = user.clone();
                        GitAuth::Https {
                            user: user.clone().unwrap_or_default(),
                            password: secret,
                        }
                    }
                };
                self.vault.git_attempt = Some(source);
                if let Err(e) = self.event_git_operation(*operation, repo.clone(), Some(auth)) {
                    log::error!("git retry failed to start: {e}");
                }
            }
        }
    }

    fn cancel_request(&mut self, request: &Request) {
        if let Request::Panel { url, .. } = request {
            for panel in self.layout_manager.iter_all_panels_mut() {
                let result = panel.handle_command(PanelCommand::CancelCredentials { url });
                if matches!(result, termide_core::CommandResult::Handled(true)) {
                    break;
                }
            }
        }
    }

    /// Keep `save.secret` in the vault, creating the vault first if needed.
    fn save_password(&mut self, save: PendingSave) {
        let exists = match self.vault.current() {
            Ok(v) => v.is_some(),
            Err(e) => {
                self.vault_error(&e);
                return;
            }
        };
        if !exists {
            return self.ask_new_master(save, false);
        }
        self.store(&save);
    }

    fn store(&mut self, save: &PendingSave) {
        let r = &save.request;
        let result = match self.vault.vault.as_mut() {
            Some(v) => v.put(r.kind(), r.origin(), r.user(), save.secret.expose(), None),
            None => return,
        };
        match result {
            Ok(()) => {
                self.state
                    .set_info(termide_i18n::t().vault_saved().to_string());
                match &save.then {
                    Some(AfterSave::StripBookmark { url, is_project }) => {
                        self.strip_bookmark_password(url, *is_project)
                    }
                    None => {}
                }
            }
            Err(e) => self.vault_error(&e),
        }
    }

    /// After a bookmark with `url` was saved: if the URL carries a password,
    /// offer to move it into the vault. Returns whether the offer is shown
    /// (the caller then leaves returning to the menu to it).
    pub(super) fn offer_bookmark_password_move(&mut self, url: &str, back: BookmarkReturn) -> bool {
        let has_password = origin::parse_url(url).is_some_and(|p| p.password.is_some());
        if !has_password {
            return false;
        }
        let t = termide_i18n::t();
        let modal = ConfirmModal::new(t.vault_password_title(), t.vault_move_bookmark_password());
        self.vault.prompt = Some(Prompt::MoveBookmarkPassword {
            url: url.to_string(),
            back,
        });
        self.state
            .set_pending_action(PendingAction::Vault, ActiveModal::Confirm(Box::new(modal)));
        self.state.needs_redraw = true;
        true
    }

    fn move_bookmark_password(&mut self, url: String, is_project: bool) {
        let (Some(parts), Some(stripped)) = (origin::parse_url(&url), origin::strip_password(&url))
        else {
            return;
        };
        let Some(password) = parts.password else {
            return;
        };
        self.save_password(PendingSave {
            request: Request::Panel {
                url: stripped,
                origin: parts.origin,
                user: parts.user,
            },
            secret: SecretText::new(password.as_str()),
            then: Some(AfterSave::StripBookmark { url, is_project }),
        });
    }

    /// Rewrite every bookmark with `url` to the password-free URL.
    fn strip_bookmark_password(&mut self, url: &str, is_project: bool) {
        let Some(stripped) = origin::strip_password(url) else {
            return;
        };
        let result = if is_project {
            let dir = self.state.project_root.join(".termide");
            match self.state.project_bookmarks.as_mut() {
                Some(proj) => {
                    for b in proj.bookmarks.iter_mut().filter(|b| b.path == url) {
                        b.path = stripped.clone();
                    }
                    proj.save_to_dir(&dir)
                }
                None => Ok(()),
            }
        } else {
            for b in self
                .state
                .bookmarks
                .bookmarks
                .iter_mut()
                .filter(|b| b.path == url)
            {
                b.path = stripped.clone();
            }
            self.state.bookmarks.save()
        };
        if let Err(e) = result {
            self.state
                .set_error(format!("Failed to save bookmarks: {e}"));
        }
    }

    /// A vault prompt was confirmed with `value`.
    pub(super) fn vault_prompt_confirmed(&mut self, value: &dyn std::any::Any) {
        let Some(prompt) = self.vault.prompt.take() else {
            return;
        };
        let checked = std::mem::take(&mut self.vault.save_checked);
        match prompt {
            Prompt::Unlock { then } => {
                let Some(master) = secret(value) else {
                    return self.ask_unlock(then, true);
                };
                let result = match self.vault.vault.as_mut() {
                    Some(v) => v.unlock(master.expose()),
                    None => Err(termide_secrets::Error::Locked),
                };
                match result {
                    Ok(()) => {
                        self.vault.last_used = Instant::now();
                        self.fulfil_from_vault(then);
                    }
                    Err(termide_secrets::Error::WrongPassword) => self.ask_unlock(then, true),
                    Err(e) => {
                        self.vault_error(&e);
                        self.ask_password(then, CredentialAttempt::Initial);
                    }
                }
            }
            Prompt::Password { request } => {
                let Some(password) = secret(value) else {
                    return self.cancel_request(&request);
                };
                if checked {
                    self.vault.pending_saves.push(PendingSave {
                        request: request.clone(),
                        secret: password.clone(),
                        then: None,
                    });
                }
                self.deliver(&request, password, CredentialAttempt::Typed);
            }
            Prompt::GitUser { request } => {
                let Some(name) = user_name(value) else {
                    return;
                };
                let request = match request {
                    Request::Git {
                        operation,
                        repo,
                        target: GitTarget::Https { origin, .. },
                    } => Request::Git {
                        operation,
                        repo,
                        target: GitTarget::Https {
                            origin,
                            user: Some(name),
                        },
                    },
                    other => other,
                };
                // A stored password for this user is tried first.
                self.answer(request, CredentialAttempt::Initial);
            }
            Prompt::NewMaster { save } => match secret(value) {
                Some(first) => {
                    let t = termide_i18n::t();
                    let modal =
                        InputModal::new(t.vault_create_title(), t.vault_create_repeat()).password();
                    self.open_vault_prompt(Prompt::RepeatMaster { save, first }, modal);
                }
                None => self.ask_new_master(save, false),
            },
            Prompt::MoveBookmarkPassword { url, back } => {
                let yes = value.downcast_ref::<bool>().copied().unwrap_or(false);
                if yes {
                    self.move_bookmark_password(url, back.is_project);
                }
                if self.state.active_modal.is_none() {
                    self.reopen_bookmarks_menu(back.group, back.is_project, back.selected);
                }
            }
            Prompt::RepeatMaster { save, first } => {
                let again = secret(value);
                if again.as_ref().map(SecretText::expose) != Some(first.expose()) {
                    return self.ask_new_master(save, true);
                }
                let Some(path) = self.vault.path.clone() else {
                    return;
                };
                match Vault::create(&path, first.expose(), KdfParams::default()) {
                    Ok(v) => {
                        self.vault.vault = Some(v);
                        self.vault.last_used = Instant::now();
                        self.store(&save);
                    }
                    Err(e) => self.vault_error(&e),
                }
            }
        }
    }

    /// A vault prompt was cancelled.
    pub(super) fn vault_prompt_cancelled(&mut self) {
        let Some(prompt) = self.vault.prompt.take() else {
            return;
        };
        match prompt {
            // Without the master password the user may still know the
            // login password itself.
            Prompt::Unlock { then } => self.ask_password(then, CredentialAttempt::Initial),
            Prompt::Password { request } | Prompt::GitUser { request } => {
                self.cancel_request(&request)
            }
            Prompt::NewMaster { .. } | Prompt::RepeatMaster { .. } => {}
            Prompt::MoveBookmarkPassword { back, .. } => {
                self.reopen_bookmarks_menu(back.group, back.is_project, back.selected)
            }
        }
    }

    /// Before a vault prompt closes, keep its checkbox.
    pub(super) fn capture_vault_checkbox(&mut self) {
        if matches!(self.state.pending_action, Some(PendingAction::Vault)) {
            if let Some(ActiveModal::Input(modal)) = &self.state.active_modal {
                let checked = modal.is_checkbox_checked();
                self.vault.note_checkbox(checked);
            }
        }
    }
}

/// The URL git could not authenticate to over HTTP(S), from its stderr:
/// `could not read Username for 'https://host': terminal prompts disabled`
/// or `Authentication failed for 'https://host/repo.git/'`.
fn https_auth_url(stderr: &str) -> Option<String> {
    const MARKERS: [&str; 3] = [
        "could not read Username for '",
        "could not read Password for '",
        "Authentication failed for '",
    ];
    MARKERS.iter().find_map(|marker| {
        let rest = &stderr[stderr.find(marker)? + marker.len()..];
        let url = &rest[..rest.find('\'')?];
        (url.starts_with("https://") || url.starts_with("http://")).then(|| url.to_string())
    })
}

fn is_ssh_auth_failure(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    (s.contains("permission denied") && s.contains("publickey")) || s.contains("passphrase")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_failure_urls_are_found() {
        assert_eq!(
            https_auth_url(
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
            )
            .as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            https_auth_url(
                "remote: Invalid\nfatal: Authentication failed for 'https://git.example/x.git/'"
            )
            .as_deref(),
            Some("https://git.example/x.git/")
        );
        assert!(https_auth_url("git@github.com: Permission denied (publickey).").is_none());
    }

    #[test]
    fn ssh_failures_are_recognised() {
        assert!(is_ssh_auth_failure(
            "git@github.com: Permission denied (publickey)."
        ));
        assert!(!is_ssh_auth_failure("fatal: repository not found"));
    }

    #[test]
    fn idle_lock_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.toml");
        let mut state = VaultState::new(Some(path.clone()));
        let fast = KdfParams {
            m_cost: 64,
            t_cost: 1,
            p_cost: 1,
        };
        state.vault = Some(Vault::create(&path, "m", fast).unwrap());
        assert!(!state.lock_if_idle(None));
        assert!(!state.lock_if_idle(Some(Duration::from_secs(3600))));
        assert!(state.is_unlocked());
        assert!(state.lock_if_idle(Some(Duration::ZERO)));
        assert!(!state.is_unlocked());
    }
}
