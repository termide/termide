//! The password vault: supplying stored passwords to connections, asking
//! for missing ones, and saving the ones the user asks to keep; and the API
//! keys of agent connections, which the settings store and the model calls
//! ask for (see `api_keys`).
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
//! a pending action. A prompt asked while the settings are open puts them
//! aside and brings them back once the vault's business is done.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use termide_core::{CredentialAttempt, GitOperationType, PanelCommand, SecretText};
use termide_modal::{ActiveModal, ApiKeyChange, ConfirmModal, InputModal, SettingsModal};
use termide_secrets::{origin, KdfParams, SecretKind, Vault};
use termide_state::PendingAction;

use super::api_keys::{self, Broker};
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
    /// The API key of agent connection `connection`, answered to the model
    /// calls waiting for it. Never typed in a prompt: without the vault's
    /// key the calls fail.
    ApiKey { connection: String },
    /// The key of a connection renamed from `from` to `to`, moved to the
    /// new name.
    ApiKeyMove { from: String, to: String },
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
            Request::ApiKey { .. } | Request::ApiKeyMove { .. } => SecretKind::ApiKey,
        }
    }

    fn is_api_key(&self) -> bool {
        self.kind() == SecretKind::ApiKey
    }

    fn origin(&self) -> &str {
        match self {
            Request::ApiKey { connection } => connection,
            Request::ApiKeyMove { from, .. } => from,
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
            }
            | Request::ApiKey { .. }
            | Request::ApiKeyMove { .. } => None,
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
    /// Clear the key pasted into the connection's `api_key_env`.
    ClearApiKeyEnv { connection: String },
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
    /// Model calls waiting for an API key from the vault.
    key_waiters: Vec<api_keys::Ask>,
    /// Changes to the API keys the settings asked for, made in order; one
    /// that needs a prompt holds the rest back.
    key_changes: VecDeque<ApiKeyChange>,
    /// The settings, put aside while a vault prompt shows.
    parked_settings: Option<Box<SettingsModal>>,
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
            key_waiters: Vec::new(),
            key_changes: VecDeque::new(),
            parked_settings: None,
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

    /// Queue the API key changes the settings asked for.
    pub(crate) fn queue_key_changes(&mut self, changes: Vec<ApiKeyChange>) {
        self.key_changes.extend(changes);
    }

    /// The connections the vault keeps an API key for.
    pub(crate) fn stored_api_keys(&mut self) -> BTreeSet<String> {
        match self.current() {
            Ok(Some(v)) => v
                .entries()
                .into_iter()
                .filter(|e| e.secret_kind() == Some(SecretKind::ApiKey))
                .map(|e| e.origin)
                .collect(),
            Ok(None) => BTreeSet::new(),
            Err(e) => {
                log::warn!("password vault unreadable: {e}");
                BTreeSet::new()
            }
        }
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

    /// Lock the vault after the configured idle time, answer the model
    /// calls waiting for an API key, and go on with the settings' key
    /// changes. Called every loop pass.
    pub(super) fn vault_tick(&mut self) {
        let idle = self.vault_idle_limit();
        if self.vault.lock_if_idle(idle) {
            log::info!("password vault locked after idle time");
        }
        self.serve_api_keys();
        self.continue_key_changes();
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
        self.park_settings();
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
        if request.is_api_key() {
            return self.api_key_unavailable(request);
        }
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
            Request::ApiKey { connection } => self.hand_api_key(connection, Ok(Some(secret))),
            Request::ApiKeyMove { from, to } => self.move_api_key(from, to, &secret),
        }
    }

    fn cancel_request(&mut self, request: &Request) {
        if request.is_api_key() {
            return self.api_key_unavailable(request.clone());
        }
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
                let t = termide_i18n::t();
                let saved = match r {
                    Request::ApiKey { connection } => {
                        Broker::global().remember(connection, save.secret.clone());
                        t.vault_api_key_saved()
                    }
                    _ => t.vault_saved(),
                };
                self.state.set_info(saved.to_string());
                match &save.then {
                    Some(AfterSave::StripBookmark { url, is_project }) => {
                        self.strip_bookmark_password(url, *is_project)
                    }
                    Some(AfterSave::ClearApiKeyEnv { connection }) => {
                        if let Some(settings) = self.settings_modal_mut() {
                            settings.forget_api_key_env(connection);
                        }
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
        self.answer_vault_prompt(value);
        self.continue_key_changes();
    }

    fn answer_vault_prompt(&mut self, value: &dyn std::any::Any) {
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
        self.dismiss_vault_prompt();
        self.continue_key_changes();
    }

    fn dismiss_vault_prompt(&mut self) {
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

    // === API keys of agent connections ===

    /// Answer the model calls waiting for a connection's key: from the keys
    /// handed out before, else from the vault, unlocking it first if need
    /// be. A prompt or another modal open holds the answer back.
    fn serve_api_keys(&mut self) {
        let broker = Broker::global();
        self.vault.key_waiters.extend(broker.take_asks());
        self.vault
            .key_waiters
            .retain(|ask| !ask.cancel.is_cancelled());
        let Some(connection) = self.vault.key_waiters.first().map(|a| a.connection.clone()) else {
            return;
        };
        let busy = self.state.active_modal.is_some()
            || self.vault.prompt.is_some()
            || self.vault.parked_settings.is_some();
        if busy {
            return;
        }
        match broker.remembered(&connection) {
            Some(key) => self.hand_api_key(&connection, Ok(Some(key))),
            None => self.answer(Request::ApiKey { connection }, CredentialAttempt::Initial),
        }
    }

    /// Answer every call waiting for the key of `connection`; a key is
    /// kept for the calls to come.
    fn hand_api_key(&mut self, connection: &str, answer: api_keys::Answer) {
        if let Ok(Some(key)) = &answer {
            Broker::global().remember(connection, key.clone());
        }
        let (now, later) = std::mem::take(&mut self.vault.key_waiters)
            .into_iter()
            .partition::<Vec<_>, _>(|ask| ask.connection == connection);
        self.vault.key_waiters = later;
        for ask in now {
            // A call stopped meanwhile no longer listens.
            let _ = ask.reply.send(answer.clone());
        }
    }

    /// The vault did not give the key `request` is for: the master password
    /// prompt was cancelled, or the entry is gone.
    fn api_key_unavailable(&mut self, request: Request) {
        let stored = self.vault.has_entry(&request);
        match request {
            Request::ApiKey { connection } => {
                let answer = if stored { Err(()) } else { Ok(None) };
                self.hand_api_key(&connection, answer);
            }
            Request::ApiKeyMove { from, to } if stored => self
                .state
                .set_error(termide_i18n::t().vault_api_key_not_moved(&from, &to)),
            _ => {}
        }
    }

    /// Keep the key of connection `from` under `to`.
    fn move_api_key(&mut self, from: &str, to: &str, key: &SecretText) {
        let Some(v) = self.vault.vault.as_mut() else {
            return;
        };
        let moved = v
            .put(SecretKind::ApiKey, to, None, key.expose(), None)
            .and_then(|()| v.remove(SecretKind::ApiKey, from, None));
        match moved {
            Ok(_) => self.vault.last_used = Instant::now(),
            Err(e) => self.vault_error(&e),
        }
    }

    /// The settings modal, open or put aside.
    fn settings_modal_mut(&mut self) -> Option<&mut SettingsModal> {
        if let Some(ActiveModal::Settings(modal)) = self.state.active_modal.as_mut() {
            return Some(modal);
        }
        self.vault.parked_settings.as_deref_mut()
    }

    /// Put the open settings aside for a vault prompt, keeping a change it
    /// has not handed over yet.
    fn park_settings(&mut self) {
        if !matches!(self.state.active_modal, Some(ActiveModal::Settings(_))) {
            return;
        }
        if let Some(ActiveModal::Settings(mut modal)) = self.state.active_modal.take() {
            if let Some(save) = modal.take_save_request() {
                self.settings_save = Some(save);
            }
            let changes = modal.take_api_key_changes();
            self.vault.queue_key_changes(changes);
            self.vault.parked_settings = Some(modal);
        }
    }

    /// Take the API key changes the open settings asked for and make them.
    pub(super) fn take_settings_key_changes(&mut self) {
        if let Some(ActiveModal::Settings(modal)) = self.state.active_modal.as_mut() {
            let changes = modal.take_api_key_changes();
            self.vault.queue_key_changes(changes);
        }
        self.continue_key_changes();
    }

    /// Make the queued key changes until one needs a prompt. Once none is
    /// left and no prompt shows, the settings put aside come back, told
    /// which keys the vault keeps now. Cheap with nothing to do.
    pub(super) fn continue_key_changes(&mut self) {
        if self.vault.key_changes.is_empty() && self.vault.parked_settings.is_none() {
            return;
        }
        let mut changed = false;
        while self.vault.prompt.is_none() {
            let Some(change) = self.vault.key_changes.pop_front() else {
                break;
            };
            self.make_key_change(change);
            changed = true;
        }
        let settled = self.vault.prompt.is_none() && self.vault.key_changes.is_empty();
        if settled && self.state.active_modal.is_none() {
            if let Some(modal) = self.vault.parked_settings.take() {
                self.state
                    .set_pending_action(PendingAction::Settings, ActiveModal::Settings(modal));
                self.state.needs_redraw = true;
                changed = true;
            }
        }
        if changed {
            let names = self.vault.stored_api_keys();
            if let Some(settings) = self.settings_modal_mut() {
                settings.set_stored_api_keys(names);
            }
        }
    }

    fn make_key_change(&mut self, change: ApiKeyChange) {
        match change {
            ApiKeyChange::Store {
                connection,
                key,
                from_env,
            } => {
                let then = from_env.then(|| AfterSave::ClearApiKeyEnv {
                    connection: connection.clone(),
                });
                self.save_password(PendingSave {
                    request: Request::ApiKey { connection },
                    secret: key,
                    then,
                });
            }
            ApiKeyChange::Remove { connection } => {
                Broker::global().forget(&connection);
                let removed = match self.vault.current() {
                    Ok(Some(v)) => v.remove(SecretKind::ApiKey, &connection, None),
                    Ok(None) => Ok(false),
                    Err(e) => Err(e),
                };
                match removed {
                    Ok(true) => self
                        .state
                        .set_info(termide_i18n::t().vault_api_key_removed().to_string()),
                    Ok(false) => {}
                    Err(e) => self.vault_error(&e),
                }
            }
            ApiKeyChange::Rename { from, to } => {
                Broker::global().rename(&from, &to);
                let request = Request::ApiKeyMove { from, to };
                if self.vault.has_entry(&request) {
                    self.fulfil_from_vault(request);
                }
            }
        }
    }

    /// The key of connection `name` for listing its models in the settings:
    /// one known already or in the unlocked vault. Never asks.
    pub(super) fn settings_api_key(
        &mut self,
        name: &str,
        connection: &termide_config::Connection,
    ) -> Option<String> {
        let broker = Broker::global();
        if let Some(key) = broker.known(name, connection) {
            return Some(key.expose().to_string());
        }
        if !self.vault.is_unlocked() {
            return None;
        }
        let found = match self.vault.current() {
            Ok(Some(v)) => v.get(SecretKind::ApiKey, name, None).ok().flatten(),
            _ => None,
        };
        let (_, key) = found?;
        self.vault.last_used = Instant::now();
        broker.remember(name, SecretText::new(key.as_str()));
        Some(key.to_string())
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
    fn api_key_requests_address_the_connection_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.toml");
        let mut state = VaultState::new(Some(path.clone()));
        assert!(state.stored_api_keys().is_empty(), "no vault yet");
        let fast = KdfParams {
            m_cost: 64,
            t_cost: 1,
            p_cost: 1,
        };
        let mut vault = Vault::create(&path, "m", fast).unwrap();
        vault
            .put(SecretKind::ApiKey, "openrouter", None, "k", None)
            .unwrap();
        vault
            .put(SecretKind::Password, "sftp://h", Some("u"), "p", None)
            .unwrap();
        assert_eq!(
            state.stored_api_keys(),
            BTreeSet::from(["openrouter".to_string()])
        );
        let use_key = Request::ApiKey {
            connection: "openrouter".into(),
        };
        let moved = Request::ApiKeyMove {
            from: "openrouter".into(),
            to: "or".into(),
        };
        assert!(state.has_entry(&use_key) && state.has_entry(&moved));
        assert!(!state.has_entry(&Request::ApiKey {
            connection: "or".into()
        }));
        assert_eq!(use_key.target_label(), "openrouter");
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
