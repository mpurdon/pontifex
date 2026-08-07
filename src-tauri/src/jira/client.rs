//! Talking to Jira Cloud once there is a grant.
//!
//! Requests go to `api.atlassian.com/ex/jira/{cloudid}`, not to the site's own
//! hostname — that is how OAuth 2.0 (3LO) works, and hitting
//! `your-domain.atlassian.net` with a bearer token fails in a way that looks
//! like a credentials problem but is not.

use crate::error::{Error, Result};
use crate::jira::tokens::{self, StoredTokens};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const API_BASE: &str = "https://api.atlassian.com";

/// One HTTP client for the process.
///
/// `reqwest::Client` owns the connection pool, so building one per command
/// meant a fresh TCP and TLS handshake for every status check, project list
/// and field lookup — and the settings screen issues one per mapped project.
pub(crate) fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new)
}

/// One Atlassian site the current grant can reach.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Site {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

/// An authenticated Jira client for one site.
///
/// Holds the tokens it was built with and writes back any refresh, so a
/// rotated refresh token is never lost — losing one means signing in again.
pub struct JiraClient {
    tokens: StoredTokens,
}

impl JiraClient {
    /// Build a client from stored tokens, refreshing first if they are stale.
    pub async fn connect(client_id: &str, client_secret: &str) -> Result<Self> {
        let stored = tokens::load_tokens()?.ok_or_else(|| {
            Error::Auth("Not connected to Jira. Connect in Settings → Jira.".into())
        })?;

        let tokens = if stored.is_stale() {
            let refreshed = super::oauth::refresh(client_id, client_secret, &stored).await?;
            tokens::save_tokens(&refreshed)?;
            refreshed
        } else {
            stored
        };

        Ok(JiraClient { tokens })
    }

    pub fn site_url(&self) -> Option<&str> {
        self.tokens.site_url.as_deref()
    }

    fn cloud_id(&self) -> Result<&str> {
        self.tokens.cloud_id.as_deref().ok_or_else(|| {
            Error::Invalid("No Jira site chosen yet. Pick one in Settings → Jira.".into())
        })
    }

    /// Sites this grant can reach.
    ///
    /// Worth re-reading rather than caching: a site-restricted grant only ever
    /// covers what the user picked on the consent screen, and an account-level
    /// one can gain sites later.
    pub async fn sites(&self) -> Result<Vec<Site>> {
        let response = http()
            .get(format!("{API_BASE}/oauth/token/accessible-resources"))
            .bearer_auth(&self.tokens.access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Cannot reach Atlassian: {e}")))?;

        let value = read_json(response, "listing your Jira sites").await?;
        serde_json::from_value(value)
            .map_err(|e| Error::Internal(format!("Atlassian returned sites we cannot read: {e}")))
    }

    fn url(&self, path: &str) -> Result<String> {
        Ok(format!(
            "{API_BASE}/ex/jira/{}/rest/api/2{path}",
            self.cloud_id()?
        ))
    }

    pub async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let response = http()
            .get(self.url(path)?)
            .query(query)
            .bearer_auth(&self.tokens.access_token)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Cannot reach Jira: {e}")))?;
        read_json(response, path).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let response = http()
            .post(self.url(path)?)
            .bearer_auth(&self.tokens.access_token)
            .header("Accept", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Internal(format!("Cannot reach Jira: {e}")))?;
        read_json(response, path).await
    }
}

async fn read_json(response: reqwest::Response, context: &str) -> Result<Value> {
    let status = response.status();
    let text = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(classify(status, &text, context));
    }
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text)
        .map_err(|e| Error::Internal(format!("Jira returned a response we cannot read: {e}")))
}

/// Jira's own words about a failure, mapped onto our error kinds.
///
/// Jira buries the useful sentence in `errorMessages` or `errors`; a bare
/// "403 Forbidden" sends people to check their credentials when the real
/// answer is that they cannot create issues in that project.
pub fn classify(status: reqwest::StatusCode, body: &str, context: &str) -> Error {
    let detail = jira_message(body).unwrap_or_else(|| body.trim().chars().take(300).collect());
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };

    match status.as_u16() {
        401 => Error::Auth(format!(
            "Jira rejected the session while {context}{detail}. Reconnect in Settings → Jira."
        )),
        403 => Error::Forbidden(format!("Jira refused while {context}{detail}")),
        404 => Error::NotFound(format!("Jira found nothing while {context}{detail}")),
        400 | 422 => Error::Invalid(format!("Jira rejected the request while {context}{detail}")),
        _ => Error::Internal(format!("Jira failed while {context} ({status}){detail}")),
    }
}

/// Pull the human sentence out of a Jira error body.
fn jira_message(body: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(body).ok()?;

    let mut parts: Vec<String> = parsed
        .get("errorMessages")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    // Field-level errors are keyed by field name — "project: is required" is
    // far more useful than the generic message that accompanies it.
    if let Some(fields) = parsed.get("errors").and_then(Value::as_object) {
        for (field, message) in fields {
            if let Some(text) = message.as_str() {
                parts.push(format!("{field}: {text}"));
            }
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn surfaces_the_field_error_jira_actually_returned() {
        let body = r#"{"errorMessages":[],"errors":{"project":"project is required"}}"#;
        let error = classify(StatusCode::BAD_REQUEST, body, "creating the ticket");
        assert!(matches!(error, Error::Invalid(_)));
        assert!(error.to_string().contains("project is required"), "{error}");
    }

    #[test]
    fn a_permission_failure_says_what_was_refused_not_just_403() {
        let body = r#"{"errorMessages":["You do not have permission to create issues in this project."]}"#;
        let error = classify(StatusCode::FORBIDDEN, body, "creating the ticket in IPP");
        assert!(matches!(error, Error::Forbidden(_)));
        assert!(error.to_string().contains("permission to create issues"));
        assert!(error.to_string().contains("IPP"));
    }

    #[test]
    fn an_expired_session_points_at_reconnecting() {
        let error = classify(StatusCode::UNAUTHORIZED, "", "listing projects");
        assert!(matches!(error, Error::Auth(_)));
        assert!(error.to_string().contains("Settings → Jira"));
    }

    #[test]
    fn a_body_that_is_not_json_still_reaches_the_user() {
        let error = classify(StatusCode::INTERNAL_SERVER_ERROR, "gateway exploded", "posting");
        assert!(error.to_string().contains("gateway exploded"));
    }
}
