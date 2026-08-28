//! Atlassian OAuth 2.0 (3LO), the desktop way.
//!
//! Atlassian supports only the authorization code grant — no implicit flow and
//! no public-client PKCE — so the exchange needs the client secret. That is
//! why the secret is a configured value in the keychain rather than something
//! compiled in: a binary cannot keep it.
//!
//! The redirect therefore has to land somewhere this process can hear it. A
//! loopback listener on a fixed port is the standard answer for a desktop app
//! (RFC 8252); the port is fixed rather than ephemeral because it has to match
//! the callback URL registered in the developer console character for
//! character.

use crate::error::{Error, Result};
use crate::jira::tokens::StoredTokens;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const AUTHORIZE_URL: &str = "https://auth.atlassian.com/authorize";
const TOKEN_URL: &str = "https://auth.atlassian.com/oauth/token";

/// What the app asks for.
///
/// `offline_access` is the one that is easy to miss and impossible to add
/// later without a fresh consent: without it there is no refresh token, and
/// the integration stops working an hour after every sign-in.
pub const SCOPES: &str = "read:jira-work write:jira-work read:jira-user offline_access";

/// How long a user gets to finish signing in before the listener gives up.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(300);

/// A callback URL this process can actually receive on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackTarget {
    /// Exactly as registered — sent verbatim as `redirect_uri`.
    pub url: String,
    pub host: String,
    pub port: u16,
}

/// Check a callback URL is one we can listen on, and pull out where to listen.
///
/// Loopback only: the redirect has to arrive at this process, and a URL
/// pointing anywhere else would consent successfully and then hand the code to
/// a machine that is not this one.
pub fn parse_callback(url: &str) -> Result<CallbackTarget> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|e| Error::Invalid(format!("'{url}' is not a URL: {e}")))?;

    if parsed.scheme() != "http" {
        return Err(Error::Invalid(format!(
            "The callback URL has to be http on a loopback address — pontifex cannot terminate \
             TLS for '{url}'."
        )));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| Error::Invalid(format!("'{url}' has no host")))?
        .to_string();

    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1" | "[::1]") {
        return Err(Error::Invalid(format!(
            "The callback URL must point at this machine — '{host}' is not a loopback address. \
             Use http://127.0.0.1:53682/callback."
        )));
    }

    let port = parsed.port().ok_or_else(|| {
        Error::Invalid(format!(
            "The callback URL needs an explicit port, e.g. http://{host}:53682/callback."
        ))
    })?;

    Ok(CallbackTarget {
        url: url.trim().to_string(),
        host,
        port,
    })
}

/// What the console's authorization URL generator tells us.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizeUrlParts {
    pub client_id: String,
    pub callback_url: String,
    /// Scopes the app is configured with, in the order the console listed them.
    pub scopes: Vec<String>,
    /// Scopes pontifex needs that the app does not have.
    ///
    /// Worth catching here: a missing scope is otherwise a consent-screen
    /// error much later, phrased in Atlassian's terms rather than in terms of
    /// the checkbox that was not ticked.
    pub missing_scopes: Vec<String>,
}

/// Read an authorization URL copied from the developer console.
///
/// The console generates the whole URL once an app is configured, which means
/// it already contains the client id, the registered callback and the granted
/// scopes — every value that otherwise gets transcribed by hand, and has to
/// match byte for byte.
pub fn parse_authorize_url(url: &str) -> Result<AuthorizeUrlParts> {
    let parsed = reqwest::Url::parse(url.trim()).map_err(|e| {
        Error::Invalid(format!(
            "That does not look like a URL ({e}). Copy the whole authorization URL from \
             Authorization → OAuth 2.0 (3LO) in the developer console."
        ))
    })?;

    let params: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();

    let client_id = params.get("client_id").cloned().unwrap_or_default();
    if client_id.trim().is_empty() {
        return Err(Error::Invalid(
            "That URL has no client_id. Use the authorization URL the console generates, not \
             the documentation example."
                .into(),
        ));
    }

    let callback_url = params.get("redirect_uri").cloned().unwrap_or_default();
    // Fail here rather than at sign-in: a redirect we cannot listen on is the
    // difference between "this URL is wrong" and "the browser hung".
    let callback = parse_callback(&callback_url)?;

    let scopes: Vec<String> = params
        .get("scope")
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default();

    let missing_scopes = SCOPES
        .split_whitespace()
        // `offline_access` is requested in the URL rather than granted in the
        // console, so the generator omits it and its absence means nothing.
        .filter(|needed| *needed != "offline_access")
        .filter(|needed| !scopes.iter().any(|have| have == needed))
        .map(str::to_string)
        .collect();

    Ok(AuthorizeUrlParts {
        client_id,
        callback_url: callback.url,
        scopes,
        missing_scopes,
    })
}

/// The URL to open in the user's browser.
///
/// `prompt=consent` is required by Atlassian, and `state` is the CSRF guard —
/// the callback is an unauthenticated local HTTP endpoint, so a mismatched
/// state is the only thing standing between it and a code planted by any page
/// the user happens to have open.
pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str) -> Result<String> {
    let url = reqwest::Url::parse_with_params(
        AUTHORIZE_URL,
        &[
            ("audience", "api.atlassian.com"),
            ("client_id", client_id),
            ("scope", SCOPES),
            ("redirect_uri", redirect_uri),
            ("state", state),
            ("response_type", "code"),
            ("prompt", "consent"),
        ],
    )
    .map_err(|e| Error::Internal(format!("Cannot build the Jira authorize URL: {e}")))?;
    Ok(url.to_string())
}

/// Parse the query string off a raw `GET /callback?... HTTP/1.1` request line.
///
/// Rebased onto a dummy origin so the same URL parser that reads the console's
/// authorization URL reads the callback too — percent-escapes, `+`, multi-byte
/// sequences and all. Hand-decoding this was a second decoder in one file, and
/// the hand-written one was the one guarding the CSRF `state` compare.
fn params_from_request_line(line: &str) -> HashMap<String, String> {
    let Some(target) = line.split_whitespace().nth(1) else {
        return HashMap::new();
    };
    reqwest::Url::parse("http://callback.invalid")
        .and_then(|base| base.join(target))
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

fn closing_page(message: &str) -> String {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>pontifex</title>\
         <body style=\"font:14px -apple-system,system-ui,sans-serif;padding:3rem;color:#ddd;background:#1a1a1a\">\
         <p>{message}</p><p style=\"color:#888\">You can close this tab and go back to pontifex.</p>"
    );
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

/// Wait for Atlassian to redirect the browser back here, and return the code.
///
/// Binds before the browser is opened by the caller, so a consent that
/// completes instantly cannot arrive before anything is listening.
pub async fn listen_for_code(
    listeners: Vec<TcpListener>,
    expected_state: &str,
) -> Result<String> {
    let accept = async {
        loop {
            // Whichever address the browser chose. `accept` is cancel-safe, so
            // rebuilding the futures each round loses nothing.
            let (mut socket, _) = {
                let pending: Vec<_> = listeners
                    .iter()
                    .map(|l| Box::pin(l.accept()) as std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<(tokio::net::TcpStream, std::net::SocketAddr)>> + Send>>)
                    .collect();
                futures::future::select_all(pending).await.0?
            };

            let mut buffer = [0u8; 4096];
            let read = socket.read(&mut buffer).await?;
            let request = String::from_utf8_lossy(&buffer[..read]);
            let line = request.lines().next().unwrap_or_default();

            // Browsers ask for /favicon.ico on the way past; answering the
            // wrong request as if it were the callback would abandon the flow.
            if !line.contains("/callback") {
                let _ = socket.write_all(closing_page("Waiting for Jira…").as_bytes()).await;
                continue;
            }

            let params = params_from_request_line(line);

            if let Some(error) = params.get("error") {
                let described = params
                    .get("error_description")
                    .cloned()
                    .unwrap_or_else(|| error.clone());
                let _ = socket
                    .write_all(closing_page("Jira refused the request.").as_bytes())
                    .await;
                return Ok(Err(Error::Auth(format!("Jira declined the sign-in: {described}"))));
            }

            if params.get("state").map(String::as_str) != Some(expected_state) {
                let _ = socket
                    .write_all(closing_page("That sign-in did not match.").as_bytes())
                    .await;
                return Ok(Err(Error::Auth(
                    "The Jira sign-in did not match the request that started it. Try again.".into(),
                )));
            }

            let Some(code) = params.get("code").cloned() else {
                let _ = socket
                    .write_all(closing_page("No authorization code came back.").as_bytes())
                    .await;
                return Ok(Err(Error::Auth(
                    "Jira redirected back without an authorization code.".into(),
                )));
            };

            let _ = socket
                .write_all(closing_page("Connected to Jira.").as_bytes())
                .await;
            let _ = socket.flush().await;
            return Ok::<_, std::io::Error>(Ok(code));
        }
    };

    match tokio::time::timeout(CONSENT_TIMEOUT, accept).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(Error::Internal(format!("Waiting for the Jira redirect failed: {e}"))),
        Err(_) => Err(Error::Auth(
            "Timed out waiting for the Jira sign-in to finish.".into(),
        )),
    }
}

/// Bind the loopback listeners named by the callback URL.
///
/// Plural because `localhost` is two addresses: browsers commonly resolve it
/// to `::1` while a naive bind takes `127.0.0.1`, and a redirect that lands on
/// the address nobody is listening on looks exactly like the sign-in hanging.
pub async fn bind(target: &CallbackTarget) -> Result<Vec<TcpListener>> {
    let addresses: Vec<std::net::IpAddr> = match target.host.as_str() {
        "localhost" => vec![
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        ],
        "::1" | "[::1]" => vec![std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)],
        _ => vec![std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)],
    };

    let mut listeners = Vec::new();
    let mut last_error = None;
    for address in addresses {
        match TcpListener::bind((address, target.port)).await {
            Ok(listener) => listeners.push(listener),
            // One of a pair failing is normal on a host without IPv6; only
            // failing them all is a problem.
            Err(e) => last_error = Some(e),
        }
    }

    if listeners.is_empty() {
        let detail = last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no address to bind".into());
        return Err(Error::Internal(format!(
            "Cannot listen on {} for the Jira sign-in ({detail}). Something else is using that \
             port — close it, or register a different callback URL in the Atlassian developer \
             console and paste it into Settings → Jira.",
            target.url,
        )));
    }

    Ok(listeners)
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    scope: String,
}

async fn post_token(body: serde_json::Value) -> Result<TokenResponse> {
    let response = crate::jira::client::http()
        .post(TOKEN_URL)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Internal(format!("Cannot reach Atlassian: {e}")))?;

    let status = response.status();
    let text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        // Atlassian's token errors are the ones that actually happen — a stale
        // redirect URI, a rotated secret — so the body is worth surfacing
        // rather than replacing with "authentication failed".
        return Err(Error::Auth(format!(
            "Atlassian rejected the token request ({status}): {}",
            text.trim()
        )));
    }

    serde_json::from_str(&text)
        .map_err(|e| Error::Internal(format!("Atlassian returned a token we cannot read: {e}")))
}

fn to_stored(response: TokenResponse, previous: Option<&StoredTokens>) -> StoredTokens {
    StoredTokens {
        access_token: response.access_token,
        // A refresh that returns no new refresh token leaves the old one
        // valid; dropping it here would sign the user out an hour later.
        refresh_token: response
            .refresh_token
            .or_else(|| previous.and_then(|p| p.refresh_token.clone())),
        expires_at: OffsetDateTime::now_utc().unix_timestamp() + response.expires_in,
        scope: response.scope,
        cloud_id: previous.and_then(|p| p.cloud_id.clone()),
        site_url: previous.and_then(|p| p.site_url.clone()),
        site_name: previous.and_then(|p| p.site_name.clone()),
    }
}

pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<StoredTokens> {
    let response = post_token(serde_json::json!({
        "grant_type": "authorization_code",
        "client_id": client_id,
        "client_secret": client_secret,
        "code": code,
        // Atlassian checks this against the authorize call, so it has to be
        // the same string both times — not merely the same location.
        "redirect_uri": redirect_uri,
    }))
    .await?;
    Ok(to_stored(response, None))
}

pub async fn refresh(
    client_id: &str,
    client_secret: &str,
    current: &StoredTokens,
) -> Result<StoredTokens> {
    let refresh_token = current.refresh_token.as_deref().ok_or_else(|| {
        Error::Auth(
            "This Jira session has no refresh token — connect again in Settings → Jira.".into(),
        )
    })?;

    let response = post_token(serde_json::json!({
        "grant_type": "refresh_token",
        "client_id": client_id,
        "client_secret": client_secret,
        "refresh_token": refresh_token,
    }))
    .await?;
    Ok(to_stored(response, Some(current)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_code_off_a_callback_request() {
        let params = params_from_request_line("GET /callback?code=abc123&state=xyz HTTP/1.1");
        assert_eq!(params.get("code").map(String::as_str), Some("abc123"));
        assert_eq!(params.get("state").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn decodes_percent_escapes_in_the_callback() {
        let params = params_from_request_line(
            "GET /callback?error=access_denied&error_description=User%20said%20no HTTP/1.1",
        );
        assert_eq!(
            params.get("error_description").map(String::as_str),
            Some("User said no"),
        );
    }

    #[test]
    fn survives_a_request_with_no_query_at_all() {
        assert!(params_from_request_line("GET /favicon.ico HTTP/1.1").is_empty());
        assert!(params_from_request_line("").is_empty());
    }

    /// Shaped like what the developer console's generator produces.
    fn generated_url(redirect: &str, scopes: &str) -> String {
        format!(
            "https://auth.atlassian.com/authorize?audience=api.atlassian.com\
             &client_id=aBcD1234efGH&scope={}&redirect_uri={}\
             &state=%24%7BYOUR_USER_BOUND_VALUE%7D&response_type=code&prompt=consent",
            scopes.replace(':', "%3A").replace(' ', "%20"),
            redirect
                .replace(':', "%3A")
                .replace('/', "%2F"),
        )
    }

    #[test]
    fn reads_the_console_generated_authorization_url() {
        let parts = parse_authorize_url(&generated_url(
            "http://127.0.0.1:53682/callback",
            "read:jira-work write:jira-work read:jira-user",
        ))
        .unwrap();

        assert_eq!(parts.client_id, "aBcD1234efGH");
        assert_eq!(parts.callback_url, "http://127.0.0.1:53682/callback");
        assert_eq!(parts.scopes.len(), 3);
        assert!(parts.missing_scopes.is_empty(), "{:?}", parts.missing_scopes);
    }

    #[test]
    fn reads_a_url_copied_verbatim_from_the_console() {
        // Byte-for-byte what the generator produces, placeholder and all: the
        // `state` it hands you is the literal `${YOUR_USER_BOUND_VALUE}`, with
        // unencoded braces that a stricter parser would refuse. pontifex
        // substitutes its own state anyway.
        let parts = parse_authorize_url(
            "https://auth.atlassian.com/authorize?audience=api.atlassian.com\
             &client_id=aBcD1234efGH&scope=read%3Ajira-work%20write%3Ajira-work%20read%3Ajira-user\
             &redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback\
             &state=${YOUR_USER_BOUND_VALUE}&response_type=code&prompt=consent",
        )
        .unwrap();

        assert_eq!(parts.client_id, "aBcD1234efGH");
        assert_eq!(parts.callback_url, "http://127.0.0.1:53682/callback");
        assert_eq!(
            parts.scopes,
            vec!["read:jira-work", "write:jira-work", "read:jira-user"],
        );
        assert!(parts.missing_scopes.is_empty());
    }

    #[test]
    fn names_the_scope_that_was_not_ticked_in_the_console() {
        let parts = parse_authorize_url(&generated_url(
            "http://127.0.0.1:53682/callback",
            "read:jira-work",
        ))
        .unwrap();

        assert_eq!(
            parts.missing_scopes,
            vec!["write:jira-work", "read:jira-user"],
        );
    }

    #[test]
    fn does_not_demand_offline_access_of_the_console() {
        // It is requested in the URL, never granted in the console, so the
        // generator omits it and its absence means nothing.
        let parts = parse_authorize_url(&generated_url(
            "http://127.0.0.1:53682/callback",
            "read:jira-work write:jira-work read:jira-user",
        ))
        .unwrap();
        assert!(!parts.missing_scopes.iter().any(|s| s == "offline_access"));
    }

    #[test]
    fn refuses_a_url_with_nothing_in_it_worth_reading() {
        assert!(parse_authorize_url("not a url").is_err());
        assert!(parse_authorize_url("https://auth.atlassian.com/authorize").is_err());
    }

    #[test]
    fn accepts_the_loopback_spellings_and_rejects_the_rest() {
        assert_eq!(
            parse_callback("http://localhost:9000/cb").unwrap().port,
            9000,
        );
        assert_eq!(
            parse_callback("http://127.0.0.1:53682/callback").unwrap().host,
            "127.0.0.1",
        );
        // Verbatim, including a trailing path we would not have chosen.
        assert_eq!(
            parse_callback("http://127.0.0.1:53682/oauth/atlassian")
                .unwrap()
                .url,
            "http://127.0.0.1:53682/oauth/atlassian",
        );

        // A redirect we cannot receive: consent would succeed and the code
        // would go to a machine that is not this one.
        assert!(parse_callback("https://example.com/callback").is_err());
        assert!(parse_callback("http://example.com:53682/callback").is_err());
        // No port means no listener.
        assert!(parse_callback("http://127.0.0.1/callback").is_err());
    }

    #[test]
    fn asks_for_offline_access_so_the_grant_outlives_the_hour() {
        let url =
            authorize_url("client-1", "http://127.0.0.1:53682/callback", "state-1").unwrap();
        assert!(url.contains("offline_access"), "{url}");
        assert!(url.contains("audience=api.atlassian.com"), "{url}");
        assert!(url.contains("prompt=consent"), "{url}");
        assert!(
            url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A53682%2Fcallback"),
            "{url}",
        );
    }

    #[test]
    fn a_refresh_without_a_new_refresh_token_keeps_the_old_one() {
        let previous = StoredTokens {
            access_token: "old".into(),
            refresh_token: Some("keep-me".into()),
            expires_at: 0,
            scope: String::new(),
            cloud_id: Some("cloud-1".into()),
            site_url: None,
            site_name: None,
        };
        let response = TokenResponse {
            access_token: "new".into(),
            refresh_token: None,
            expires_in: 3600,
            scope: String::new(),
        };
        let next = to_stored(response, Some(&previous));
        assert_eq!(next.refresh_token.as_deref(), Some("keep-me"));
        // The chosen site survives a refresh too, or every refresh would send
        // the user back to the site picker.
        assert_eq!(next.cloud_id.as_deref(), Some("cloud-1"));
    }

    #[test]
    fn treats_a_token_as_stale_a_minute_before_it_expires() {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let almost = StoredTokens {
            access_token: String::new(),
            refresh_token: None,
            expires_at: now + 30,
            scope: String::new(),
            cloud_id: None,
            site_url: None,
            site_name: None,
        };
        assert!(almost.is_stale());

        let fresh = StoredTokens {
            expires_at: now + 600,
            ..almost
        };
        assert!(!fresh.is_stale());
    }
}
