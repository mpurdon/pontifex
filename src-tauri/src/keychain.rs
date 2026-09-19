//! The OS keychain, in terms of what to do when it refuses.
//!
//! Every secret Pontifex keeps — Jira's client secret and tokens, a GitHub
//! token — goes through here, so the one refusal worth explaining is
//! explained once.

use crate::error::{Error, Result};

/// One keychain service, holding named entries.
pub struct Keychain {
    service: &'static str,
}

impl Keychain {
    pub const fn new(service: &'static str) -> Self {
        Keychain { service }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry> {
        keyring::Entry::new(self.service, name)
            .map_err(|e| Error::Internal(format!("Cannot reach the OS keychain: {e}")))
    }

    /// A keychain refusal, said in terms of what to do about it.
    ///
    /// The platform message for a denied or mis-answered access dialog is
    /// "the user name or passphrase you entered is not correct", which sounds
    /// like the remote service's credentials are wrong when nothing about it
    /// is involved. Every build is a new binary to the keychain, so the dialog
    /// reappears after a rebuild and this is the failure you get for
    /// dismissing it.
    fn refused(&self, name: &str, action: &str, e: keyring::Error) -> Error {
        let advice = match &e {
            keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => format!(
                " — macOS asks permission the first time a new build touches this item, and a \
                 dismissed or denied dialog is remembered as a refusal. Open Keychain Access, find \
                 the “{service}” item “{name}”, and under Access Control allow Pontifex — or clear it \
                 with `security delete-generic-password -s {service} -a {name}` and enter it again.",
                service = self.service
            ),
            _ => String::new(),
        };
        Error::Internal(format!("Cannot {action} the OS keychain: {e}{advice}"))
    }

    /// Read an entry, treating "not there" as `None` rather than an error.
    pub fn read(&self, name: &str) -> Result<Option<String>> {
        match self.entry(name)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(self.refused(name, "read", e)),
        }
    }

    pub fn write(&self, name: &str, value: &str) -> Result<()> {
        self.entry(name)?
            .set_password(value)
            .map_err(|e| self.refused(name, "write to", e))
    }

    pub fn clear(&self, name: &str) -> Result<()> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(self.refused(name, "clear", e)),
        }
    }
}
