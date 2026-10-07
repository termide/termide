//! Credential exchange between panels and the app.
//!
//! A panel whose connection was refused emits
//! [`PanelEvent::CredentialsRequired`](crate::PanelEvent::CredentialsRequired).
//! The app finds the password — in the password vault or by asking the user —
//! and broadcasts [`PanelCommand::ProvideCredentials`](crate::PanelCommand::ProvideCredentials)
//! (or `CancelCredentials`); the panel waiting for that URL takes it. When the
//! connection then succeeds the panel emits `CredentialsAccepted`, which is
//! when the app saves a password the user asked to keep. The URL is the
//! key throughout and never contains a password.

use std::fmt;

/// Which password the refused connection used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialAttempt {
    /// None given: the panel's default authentication (keys, agent,
    /// anonymous, a password-less database login).
    Initial,
    /// The one stored in the password vault.
    Stored,
    /// One the user typed.
    Typed,
}

/// A password in transit; never printed by `Debug` and wiped on drop.
#[derive(Clone)]
pub struct SecretText(zeroize::Zeroizing<String>);

impl SecretText {
    pub fn new(value: impl Into<String>) -> Self {
        Self(zeroize::Zeroizing::new(value.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText(***)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_secret() {
        let s = SecretText::new("hunter2");
        assert_eq!(format!("{s:?}"), "SecretText(***)");
        assert_eq!(s.expose(), "hunter2");
    }
}
