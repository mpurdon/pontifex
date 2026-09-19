use crate::aws::clients::map_sdk_error;
use crate::aws::profiles::{AwsProfile, ProfileKind};
use crate::error::{Error, Result};
use aws_config::BehaviorVersion;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::path::PathBuf;
use time::OffsetDateTime;

/// The client name we register with AWS SSO OIDC. Shows up in the SSO portal's
/// device-authorization screen, so make it recognisable.
const CLIENT_NAME: &str = "pontifex";
const CLIENT_TYPE: &str = "public";
const DEFAULT_SCOPE: &str = "sso:account:access";

/// Shape of the AWS CLI v2 SSO token cache file.
///
/// We read *and* write this format so a login in either pontifex or the CLI is
/// visible to the other. Field names are the CLI's, which is why they are
/// camelCase and why `startUrl`/`region` are included even though we do not
/// strictly need them — the CLI validates them on read.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedToken {
    pub start_url: String,
    pub region: String,
    pub access_token: String,
    /// RFC 3339, always UTC with a `Z` suffix.
    pub expires_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

fn sso_cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| Error::Internal("No home directory".into()))?;
    Ok(home.join(".aws").join("sso").join("cache"))
}

/// The CLI names each cache file `sha1(key).json`, lowercase hex.
fn cache_file_for(key: &str) -> Result<PathBuf> {
    let digest = Sha1::digest(key.as_bytes());
    Ok(sso_cache_dir()?.join(format!("{}.json", hex::encode(digest))))
}

fn parse_expiry(s: &str) -> Option<OffsetDateTime> {
    // The CLI writes RFC 3339, but older versions wrote `%Y-%m-%dT%H:%M:%SUTC`.
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .ok()
        .or_else(|| {
            let normalized = s.trim_end_matches("UTC");
            let fmt =
                time::macros::format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]");
            time::PrimitiveDateTime::parse(normalized, fmt)
                .ok()
                .map(|dt| dt.assume_utc())
        })
}

/// Current SSO session state for a profile, as shown in the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SsoStatus {
    /// False for non-SSO profiles — nothing to log into.
    pub applicable: bool,
    pub has_token: bool,
    /// True only when the token is past use *and* cannot be renewed
    /// silently; a lapsed access token with a live refresh token is not
    /// expired from the user's point of view.
    pub expired: bool,
    /// RFC 3339 expiry of the cached access token, when there is one.
    pub expires_at: Option<String>,
    /// The cache holds a refresh token and a live client registration, so
    /// the access token renews on its own until the SSO session itself ends.
    #[serde(default)]
    pub refreshable: bool,
}

/// Read the cached token for a cache key (an SSO session name, or a legacy
/// profile's start URL).
pub fn read_token_by_key(key: &str) -> Result<Option<CachedToken>> {
    let path = cache_file_for(key)?;
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    match serde_json::from_str::<CachedToken>(&raw) {
        Ok(token) => Ok(Some(token)),
        // A malformed cache file is not fatal — treat it as "not logged in"
        // so the user can just log in again rather than hitting a hard error.
        Err(_) => Ok(None),
    }
}

/// An SSO OIDC client for a region. The endpoints are unauthenticated, so no
/// credentials are needed.
async fn oidc_client(region: &str) -> aws_sdk_ssooidc::Client {
    let cfg = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()))
        .no_credentials()
        .load()
        .await;
    aws_sdk_ssooidc::Client::new(&cfg)
}

/// `expires_in` seconds from now, as the RFC 3339 stamp the CLI cache holds.
fn expiry_from_now(expires_in: i32) -> Result<String> {
    (OffsetDateTime::now_utc() + time::Duration::seconds(expires_in as i64))
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(Error::internal)
}

/// True when the access token ends within `margin` of now, or is unparseable.
fn expires_within(token: &CachedToken, margin: time::Duration) -> bool {
    parse_expiry(&token.expires_at)
        .map(|exp| exp <= OffsetDateTime::now_utc() + margin)
        .unwrap_or(true)
}

/// True when a token is absent, unparseable, or about to expire.
///
/// A token expiring within a minute counts as expired so we do not start a long
/// operation on credentials that die mid-flight.
fn is_expired(token: &CachedToken) -> bool {
    expires_within(token, time::Duration::minutes(1))
}

/// Renew this far ahead of expiry, so a long operation started now does not
/// run into the boundary, and so the poller never sees a lapse at all.
const REFRESH_MARGIN: time::Duration = time::Duration::minutes(10);

/// Whether the cache holds what `CreateToken` needs for a refresh grant: the
/// refresh token, and the client registration it was issued to, unexpired.
/// The CLI writes all of these for an `sso-session` with registration
/// scopes, and so does our own sign-in.
fn can_refresh(token: &CachedToken) -> bool {
    let registered = token.client_id.is_some() && token.client_secret.is_some();
    let registration_live = token
        .registration_expires_at
        .as_deref()
        .and_then(parse_expiry)
        .map(|exp| exp > OffsetDateTime::now_utc())
        .unwrap_or(true);
    token.refresh_token.is_some() && registered && registration_live
}

/// Refreshes take turns: two environments on one session renewing at once
/// would each spend the same refresh token, and the second would fail.
static REFRESH_TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A usable access token for a session, renewed silently when it is near
/// its end and the cache allows. The browser is only needed once the SSO
/// session itself — the refresh token — has run out.
pub async fn fresh_token(session: &str) -> Result<String> {
    let token = read_token_by_key(session)?
        .ok_or_else(|| Error::Auth(format!("Not signed in to the '{session}' SSO session")))?;
    if !expires_within(&token, REFRESH_MARGIN) {
        return Ok(token.access_token);
    }
    if can_refresh(&token) {
        let _turn = REFRESH_TURN.lock().await;
        // Someone else may have renewed while we waited for the turn.
        let token = read_token_by_key(session)?.unwrap_or_else(|| token.clone());
        if !expires_within(&token, REFRESH_MARGIN) {
            return Ok(token.access_token);
        }
        match refresh(session, &token).await {
            Ok(renewed) => return Ok(renewed.access_token),
            Err(e) => lwarn!(
                crate::logging::cat::SSO,
                "could not refresh the '{session}' SSO token: {e}"
            ),
        }
    }
    if is_expired(&token) {
        return Err(Error::Auth(format!(
            "The '{session}' SSO session has expired — sign in again"
        )));
    }
    Ok(token.access_token)
}

/// Renew now regardless of how much life the token has left, for the
/// explicit "refresh" action. Returns the new expiry.
pub async fn refresh_now(session: &str) -> Result<String> {
    let token = read_token_by_key(session)?
        .ok_or_else(|| Error::Auth(format!("Not signed in to the '{session}' SSO session")))?;
    if !can_refresh(&token) {
        return Err(Error::Auth(format!(
            "The '{session}' SSO session cannot be renewed silently — sign in again"
        )));
    }
    let _turn = REFRESH_TURN.lock().await;
    Ok(refresh(session, &token).await?.expires_at)
}

/// The refresh grant, and the cache file rewritten the way the CLI would.
async fn refresh(session: &str, token: &CachedToken) -> Result<CachedToken> {
    let (Some(client_id), Some(client_secret), Some(refresh_token)) = (
        token.client_id.as_deref(),
        token.client_secret.as_deref(),
        token.refresh_token.as_deref(),
    ) else {
        return Err(Error::Auth("no refresh token in the SSO cache".into()));
    };
    let out = oidc_client(&token.region)
        .await
        .create_token()
        .client_id(client_id)
        .client_secret(client_secret)
        .grant_type("refresh_token")
        .refresh_token(refresh_token)
        .send()
        .await
        .map_err(map_sdk_error)?;
    let access_token = out
        .access_token()
        .ok_or_else(|| Error::Aws("SSO returned no access token on refresh".into()))?
        .to_string();
    let expires_at = expiry_from_now(out.expires_in())?;
    let renewed = CachedToken {
        access_token,
        expires_at,
        // AWS may rotate the refresh token; keep the old one if it did not.
        refresh_token: out
            .refresh_token()
            .map(str::to_string)
            .or_else(|| token.refresh_token.clone()),
        ..token.clone()
    };
    write_cached_token(session, &renewed)?;
    linfo!(
        crate::logging::cat::SSO,
        "renewed the '{session}' SSO token silently; valid until {}",
        renewed.expires_at
    );
    Ok(renewed)
}

/// Sign-in state for a cache key, regardless of whether a profile uses it.
pub fn status_for_key(key: &str) -> Result<SsoStatus> {
    let Some(token) = read_token_by_key(key)? else {
        return Ok(SsoStatus {
            applicable: true,
            has_token: false,
            expired: true,
            expires_at: None,
            refreshable: false,
        });
    };
    let refreshable = can_refresh(&token);
    Ok(SsoStatus {
        applicable: true,
        has_token: true,
        expired: is_expired(&token) && !refreshable,
        expires_at: Some(token.expires_at.clone()),
        refreshable,
    })
}

pub fn sso_status(profile: &AwsProfile) -> Result<SsoStatus> {
    if !profile.is_sso() {
        return Ok(SsoStatus {
            applicable: false,
            has_token: false,
            expired: false,
            expires_at: None,
            refreshable: false,
        });
    }
    match profile.token_cache_key() {
        Some(key) => status_for_key(&key),
        None => Ok(SsoStatus {
            applicable: true,
            has_token: false,
            expired: true,
            expires_at: None,
            refreshable: false,
        }),
    }
}

fn write_cached_token(key: &str, token: &CachedToken) -> Result<()> {
    let dir = sso_cache_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = cache_file_for(key)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string(token)?)?;
    std::fs::rename(&tmp, &path)?;

    // The token is a bearer credential; keep it owner-only like the CLI does.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Emitted to the frontend when the device flow needs the user to visit a URL.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorization {
    pub verification_uri: String,
    /// Pre-fills the code; this is the URL we actually open.
    pub verification_uri_complete: Option<String>,
    pub user_code: String,
    pub expires_in: i32,
    pub interval: i32,
}

/// Internal handle carrying everything needed to finish a device-code login.
pub struct PendingLogin {
    pub authorization: DeviceAuthorization,
    client_id: String,
    client_secret: String,
    device_code: String,
    registration_expires_at: Option<String>,
    start_url: String,
    sso_region: String,
    cache_key: String,
}

/// Start the OIDC device authorization flow for a profile.
pub async fn begin_login(profile: &AwsProfile) -> Result<PendingLogin> {
    if !profile.is_sso() {
        return Err(Error::Invalid(format!(
            "Profile '{}' is not an SSO profile, so there is nothing to sign into",
            profile.name
        )));
    }

    let start_url = profile.sso_start_url.clone().ok_or_else(|| {
        Error::Invalid(format!(
            "Profile '{}' has no sso_start_url{}",
            profile.name,
            if profile.kind == ProfileKind::SsoSession {
                format!(
                    " — is the [sso-session {}] block present in ~/.aws/config?",
                    profile.sso_session.as_deref().unwrap_or("?")
                )
            } else {
                String::new()
            }
        ))
    })?;

    let sso_region = profile
        .sso_region
        .clone()
        .or_else(|| profile.region.clone())
        .ok_or_else(|| Error::Invalid(format!("Profile '{}' has no sso_region", profile.name)))?;

    let cache_key = profile
        .token_cache_key()
        .ok_or_else(|| Error::Invalid("Cannot determine SSO token cache key".into()))?;

    begin_login_with(&start_url, &sso_region, &cache_key).await
}

/// Start the OIDC device authorization flow.
///
/// Returns the URL/code to show the user; call [`poll_for_token`] to finish.
/// Split in two so the UI can render the code and open the browser while the
/// poll runs in the background.
///
/// Keyed on the token cache entry rather than a profile, so a bare
/// `[sso-session]` can be signed into without any profile referencing it.
pub async fn begin_login_with(
    start_url: &str,
    sso_region: &str,
    cache_key: &str,
) -> Result<PendingLogin> {
    let start_url = start_url.to_string();
    let sso_region = sso_region.to_string();
    let cache_key = cache_key.to_string();

    let client = oidc_client(&sso_region).await;

    let registration = client
        .register_client()
        .client_name(CLIENT_NAME)
        .client_type(CLIENT_TYPE)
        .scopes(DEFAULT_SCOPE)
        .send()
        .await
        .map_err(map_sdk_error)?;

    let client_id = registration
        .client_id()
        .ok_or_else(|| Error::Aws("SSO did not return a client id".into()))?
        .to_string();
    let client_secret = registration
        .client_secret()
        .ok_or_else(|| Error::Aws("SSO did not return a client secret".into()))?
        .to_string();

    let device = client
        .start_device_authorization()
        .client_id(&client_id)
        .client_secret(&client_secret)
        .start_url(&start_url)
        .send()
        .await
        .map_err(map_sdk_error)?;

    let user_code = device
        .user_code()
        .ok_or_else(|| Error::Aws("SSO did not return a user code".into()))?
        .to_string();
    let device_code = device
        .device_code()
        .ok_or_else(|| Error::Aws("SSO did not return a device code".into()))?
        .to_string();
    let verification_uri = device
        .verification_uri()
        .ok_or_else(|| Error::Aws("SSO did not return a verification URI".into()))?
        .to_string();

    Ok(PendingLogin {
        authorization: DeviceAuthorization {
            verification_uri,
            verification_uri_complete: device.verification_uri_complete().map(str::to_string),
            user_code,
            expires_in: device.expires_in(),
            // AWS returns 0 when it has no preference; 5s is the CLI's default.
            interval: if device.interval() > 0 {
                device.interval()
            } else {
                5
            },
        },
        client_id,
        client_secret,
        device_code,
        registration_expires_at: OffsetDateTime::from_unix_timestamp(
            registration.client_secret_expires_at(),
        )
        .ok()
        .and_then(|dt| {
            dt.format(&time::format_description::well_known::Rfc3339)
                .ok()
        }),
        start_url,
        sso_region,
        cache_key,
    })
}

/// Poll `CreateToken` until the user approves in the browser, then persist the
/// token in the AWS CLI's cache format.
pub async fn poll_for_token(pending: PendingLogin) -> Result<CachedToken> {
    let client = oidc_client(&pending.sso_region).await;

    let mut interval = std::time::Duration::from_secs(pending.authorization.interval as u64);
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(pending.authorization.expires_in.max(60) as u64);

    loop {
        if std::time::Instant::now() >= deadline {
            return Err(Error::Auth(
                "Sign-in timed out waiting for browser approval".into(),
            ));
        }

        tokio::time::sleep(interval).await;

        let result = client
            .create_token()
            .client_id(&pending.client_id)
            .client_secret(&pending.client_secret)
            .grant_type("urn:ietf:params:oauth:grant-type:device_code")
            .device_code(&pending.device_code)
            .send()
            .await;

        match result {
            Ok(out) => {
                let access_token = out
                    .access_token()
                    .ok_or_else(|| Error::Aws("SSO returned no access token".into()))?
                    .to_string();

                let expires_at = expiry_from_now(out.expires_in())?;

                let token = CachedToken {
                    start_url: pending.start_url.clone(),
                    region: pending.sso_region.clone(),
                    access_token,
                    expires_at,
                    client_id: Some(pending.client_id.clone()),
                    client_secret: Some(pending.client_secret.clone()),
                    registration_expires_at: pending.registration_expires_at.clone(),
                    refresh_token: out.refresh_token().map(str::to_string),
                };

                write_cached_token(&pending.cache_key, &token)?;
                return Ok(token);
            }
            Err(err) => {
                use aws_sdk_ssooidc::operation::create_token::CreateTokenError as E;
                match err.as_service_error() {
                    // Expected while the user is still in the browser.
                    Some(E::AuthorizationPendingException(_)) => continue,
                    // AWS is asking us to back off.
                    Some(E::SlowDownException(_)) => {
                        interval += std::time::Duration::from_secs(5);
                        continue;
                    }
                    Some(E::ExpiredTokenException(_)) => {
                        return Err(Error::Auth(
                            "The sign-in request expired before it was approved".into(),
                        ))
                    }
                    _ => return Err(map_sdk_error(err)),
                }
            }
        }
    }
}

/// Fall back to the AWS CLI when the native flow cannot run.
///
/// Used when the profile shape is one we do not model (e.g. a
/// `credential_process` wrapper) or when `begin_login` errors out. The CLI
/// writes to the same token cache, so the result is indistinguishable to the
/// rest of the app.
pub async fn login_via_cli(profile_name: &str) -> Result<String> {
    let output = tokio::process::Command::new("aws")
        .arg("sso")
        .arg("login")
        .arg("--profile")
        .arg(profile_name)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::Internal(
                    "AWS CLI not found on PATH. Install AWS CLI v2 or use the built-in sign-in."
                        .into(),
                )
            } else {
                Error::Internal(format!("Could not run `aws sso login`: {e}"))
            }
        })?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if output.status.success() {
        Ok(format!("{stdout}{stderr}").trim().to_string())
    } else {
        Err(Error::Auth(format!(
            "`aws sso login --profile {profile_name}` failed: {}",
            if stderr.trim().is_empty() {
                stdout.trim()
            } else {
                stderr.trim()
            }
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_filename_matches_aws_cli_scheme() {
        // The AWS CLI hashes the session name (or start URL) with SHA-1 and
        // hex-encodes it. Verified against a known SHA-1 digest.
        let path = cache_file_for("trajector").unwrap();
        let name = path.file_name().unwrap().to_string_lossy();
        let expected = hex::encode(Sha1::digest(b"trajector"));
        assert_eq!(name, format!("{expected}.json"));
        assert_eq!(name.len(), 40 + ".json".len());
    }

    #[test]
    fn parses_rfc3339_expiry() {
        let dt = parse_expiry("2026-07-30T18:00:00Z").expect("rfc3339 should parse");
        assert_eq!(dt.year(), 2026);
    }

    #[test]
    fn parses_legacy_utc_suffix_expiry() {
        let dt = parse_expiry("2026-07-30T18:00:00UTC").expect("legacy format should parse");
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.offset(), time::UtcOffset::UTC);
    }

    #[test]
    fn unparseable_expiry_is_treated_as_expired() {
        assert!(parse_expiry("not a date").is_none());
    }
}
