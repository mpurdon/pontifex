//! Where the Jira credentials live: the OS keychain, not settings.json.
//!
//! The AWS side of this app deliberately writes its tokens to
//! `~/.aws/sso/cache`, because the AWS CLI owns that format and sharing it is
//! the point. Nothing owns ours, so it goes where the platform keeps secrets.
//!
//! Two entries rather than one: the client secret is configuration that
//! survives a sign-out, and the tokens are a session that does not.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

const SERVICE: &str = "gebman.jira";
const TOKENS_ENTRY: &str = "oauth-tokens";
const SECRET_ENTRY: &str = "client-secret";

/// What the keychain last told us, so the OS is asked once rather than once
/// per command.
///
/// macOS ties an item's ACL to the exact binary that created it, so every
/// rebuild is a new application as far as the keychain is concerned and the
/// permission dialog returns. Every dialog is a chance to dismiss it, and a
/// dismissed dialog is an error — which is how one refused prompt turned into
/// "not connected" across the whole screen. Reading once per launch turns a
/// prompt per status poll into a prompt per run.
///
/// `None` means "not asked yet"; `Some(None)` means "asked, and there is
/// nothing there". Failures are deliberately not cached, so choosing "Always
/// Allow" on a retry takes effect immediately.
#[derive(Default)]
struct Remembered {
    secret: Option<Option<String>>,
    tokens: Option<Option<StoredTokens>>,
}

fn remembered() -> &'static std::sync::RwLock<Remembered> {
    static CACHE: std::sync::OnceLock<std::sync::RwLock<Remembered>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Read of the process-wide cache, tolerating a poisoned lock.
///
/// A panic while holding it would otherwise make the app permanently unable to
/// reach Jira; the worst case here is one extra keychain read.
fn with_cache<T>(read: impl FnOnce(&Remembered) -> T) -> Option<T> {
    remembered().read().ok().map(|cache| read(&cache))
}

fn update_cache(write: impl FnOnce(&mut Remembered)) {
    if let Ok(mut cache) = remembered().write() {
        write(&mut cache);
    }
}

/// A live grant: what to send, when it dies, and which site it is for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredTokens {
    pub access_token: String,
    /// Rotates on every refresh, so the new one must replace this.
    pub refresh_token: Option<String>,
    /// Unix seconds. Refreshed early rather than on expiry, see [`is_stale`].
    pub expires_at: i64,
    pub scope: String,
    /// The site this grant is being used against.
    pub cloud_id: Option<String>,
    pub site_url: Option<String>,
    pub site_name: Option<String>,
}

impl StoredTokens {
    /// True when the access token should be refreshed before the next call.
    ///
    /// A minute of slack, because a token that expires mid-flight fails the
    /// request rather than the check, and that failure reads as "Jira rejected
    /// you" rather than "your hour was up".
    pub fn is_stale(&self) -> bool {
        OffsetDateTime::now_utc().unix_timestamp() >= self.expires_at - 60
    }
}

fn entry(name: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(SERVICE, name)
        .map_err(|e| Error::Internal(format!("Cannot reach the OS keychain: {e}")))
}

/// A keychain refusal, said in terms of what to do about it.
///
/// The platform message for a denied or mis-answered access dialog is "the
/// user name or passphrase you entered is not correct", which sounds like the
/// Jira credentials are wrong when nothing about Jira is involved. Every build
/// is a new binary to the keychain, so the dialog reappears after a rebuild
/// and this is the failure you get for dismissing it.
fn keychain_error(action: &str, e: keyring::Error) -> Error {
    let advice = match &e {
        keyring::Error::PlatformFailure(_) | keyring::Error::NoStorageAccess(_) => {
            " — macOS asks permission the first time a new build touches this item. \
             Try again and choose “Always Allow”. If it keeps refusing, clear the item with \
             `security delete-generic-password -s gebman.jira` and enter the secret again."
        }
        _ => "",
    };
    Error::Internal(format!("Cannot {action} the OS keychain: {e}{advice}"))
}

/// Read a keychain entry, treating "not there" as `None` rather than an error.
fn read(name: &str) -> Result<Option<String>> {
    match entry(name)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(keychain_error("read", e)),
    }
}

fn write(name: &str, value: &str) -> Result<()> {
    entry(name)?
        .set_password(value)
        .map_err(|e| keychain_error("write to", e))
}

fn clear(name: &str) -> Result<()> {
    match entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(keychain_error("clear", e)),
    }
}

pub fn load_tokens() -> Result<Option<StoredTokens>> {
    if let Some(Some(hit)) = with_cache(|cache| cache.tokens.clone()) {
        return Ok(hit);
    }

    // A keychain entry written by an older build is not worth failing over —
    // it means "sign in again", which is what `None` already says.
    let stored = read(TOKENS_ENTRY)?.and_then(|raw| serde_json::from_str(&raw).ok());
    update_cache(|cache| cache.tokens = Some(stored.clone()));
    Ok(stored)
}

pub fn save_tokens(tokens: &StoredTokens) -> Result<()> {
    write(TOKENS_ENTRY, &serde_json::to_string(tokens)?)?;
    update_cache(|cache| cache.tokens = Some(Some(tokens.clone())));
    Ok(())
}

pub fn clear_tokens() -> Result<()> {
    clear(TOKENS_ENTRY)?;
    update_cache(|cache| cache.tokens = Some(None));
    Ok(())
}

pub fn load_client_secret() -> Result<Option<String>> {
    if let Some(Some(hit)) = with_cache(|cache| cache.secret.clone()) {
        return Ok(hit);
    }

    let secret = read(SECRET_ENTRY)?;
    update_cache(|cache| cache.secret = Some(secret.clone()));
    Ok(secret)
}

pub fn save_client_secret(secret: &str) -> Result<()> {
    write(SECRET_ENTRY, secret)?;
    update_cache(|cache| cache.secret = Some(Some(secret.to_string())));
    Ok(())
}

pub fn clear_client_secret() -> Result<()> {
    clear(SECRET_ENTRY)?;
    update_cache(|cache| cache.secret = Some(None));
    Ok(())
}
