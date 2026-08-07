use crate::aws::profiles;
use crate::aws::sso;
use aws_config::BehaviorVersion;
use aws_credential_types::provider::{self, error::CredentialsError, ProvideCredentials};
use aws_credential_types::Credentials;
use serde::{Deserialize, Serialize};

/// An account + role reachable through an SSO session.
///
/// This is what an environment stores instead of a profile name when its
/// credentials are resolved in-app: the session is what you sign into, and the
/// account/role pair is what that sign-in is redeemed for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SsoTarget {
    /// Name of an `[sso-session]` block in `~/.aws/config`.
    pub session: String,
    pub account_id: String,
    pub role_name: String,
    /// Human-readable account name, cached for display so the settings screen
    /// can label the target without a network call.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
}

impl SsoTarget {
    pub fn label(&self) -> String {
        match &self.account_name {
            Some(name) => format!("{name} · {}", self.role_name),
            None => format!("{} · {}", self.account_id, self.role_name),
        }
    }
}

/// Resolves credentials by redeeming an SSO access token for a role's
/// temporary credentials.
///
/// This is exactly what `aws sso login` + a profile would do, minus the
/// profile: it means gebman can reach any account the session grants without
/// anything being written to `~/.aws/config`.
///
/// Returned credentials are short-lived (typically one hour). The SDK's
/// credentials cache calls back here when they near expiry, so the provider
/// stays stateless and always reads the current token from disk — which also
/// means a fresh sign-in takes effect without rebuilding anything.
#[derive(Debug)]
pub struct SsoRoleProvider {
    target: SsoTarget,
}

impl SsoRoleProvider {
    pub fn new(target: SsoTarget) -> Self {
        Self { target }
    }

    async fn load(&self) -> Result<Credentials, CredentialsError> {
        let session = profiles::get_sso_session(&self.target.session)
            .map_err(|e| CredentialsError::invalid_configuration(e.to_string()))?;

        let access_token = sso::require_token(&self.target.session)
            .map_err(|e| CredentialsError::invalid_configuration(e.to_string()))?;

        let cfg = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(session.sso_region.clone()))
            .no_credentials()
            .load()
            .await;
        let client = aws_sdk_sso::Client::new(&cfg);

        let out = client
            .get_role_credentials()
            .access_token(&access_token)
            .account_id(&self.target.account_id)
            .role_name(&self.target.role_name)
            .send()
            .await
            .map_err(|e| {
                let message = crate::aws::clients::describe_sdk_error(&e);
                // A rejected token means the session lapsed between our expiry
                // check and this call; surface it as a config error so the UI
                // routes it to a sign-in rather than a retry.
                CredentialsError::invalid_configuration(format!(
                    "Could not get credentials for {} in {}: {message}",
                    self.target.role_name, self.target.account_id
                ))
            })?;

        let role = out
            .role_credentials()
            .ok_or_else(|| CredentialsError::invalid_configuration("SSO returned no credentials"))?;

        let (Some(key), Some(secret)) = (role.access_key_id(), role.secret_access_key()) else {
            return Err(CredentialsError::invalid_configuration(
                "SSO returned incomplete credentials",
            ));
        };

        // `expiration` is epoch milliseconds.
        let expires_at = if role.expiration() > 0 {
            Some(
                std::time::UNIX_EPOCH
                    + std::time::Duration::from_millis(role.expiration() as u64),
            )
        } else {
            None
        };

        Ok(Credentials::new(
            key,
            secret,
            role.session_token().map(str::to_string),
            expires_at,
            "GebmanSso",
        ))
    }
}

impl ProvideCredentials for SsoRoleProvider {
    fn provide_credentials<'a>(&'a self) -> provider::future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        provider::future::ProvideCredentials::new(self.load())
    }
}

/// An account the signed-in session grants access to.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SsoAccount {
    pub account_id: String,
    pub account_name: Option<String>,
    pub email: Option<String>,
    /// True when the account looks like one of the Global Event Bus accounts,
    /// so the picker can default to showing just those.
    pub matches_event_bus: bool,
}

/// Heuristic for "this account belongs to the thing gebman manages".
///
/// Matches on the account's display name rather than a hardcoded id list, so
/// new stages or renamed accounts do not silently drop out of the filter.
fn looks_like_event_bus_account(name: Option<&str>) -> bool {
    let Some(name) = name else { return false };
    let lowered = name.to_lowercase();
    lowered.contains("event bus") || lowered.contains("event-bus") || lowered.contains("eventbus")
}

async fn sso_client(session_name: &str) -> crate::error::Result<aws_sdk_sso::Client> {
    let session = profiles::get_sso_session(session_name)?;
    let cfg = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_config::Region::new(session.sso_region))
        .no_credentials()
        .load()
        .await;
    Ok(aws_sdk_sso::Client::new(&cfg))
}

/// Every account the session's current token grants access to.
pub async fn list_accounts(session_name: &str) -> crate::error::Result<Vec<SsoAccount>> {
    let access_token = sso::require_token(session_name)?;
    let client = sso_client(session_name).await?;

    let mut out = Vec::new();
    let mut pages = client
        .list_accounts()
        .access_token(&access_token)
        .into_paginator()
        .send();

    while let Some(page) = pages.next().await {
        let page = page.map_err(crate::aws::clients::map_sdk_error)?;
        for account in page.account_list() {
            let Some(account_id) = account.account_id() else {
                continue;
            };
            let name = account.account_name().map(str::to_string);
            out.push(SsoAccount {
                account_id: account_id.to_string(),
                matches_event_bus: looks_like_event_bus_account(name.as_deref()),
                account_name: name,
                email: account.email_address().map(str::to_string),
            });
        }
    }

    out.sort_by(|a, b| {
        a.account_name
            .as_deref()
            .unwrap_or(&a.account_id)
            .cmp(b.account_name.as_deref().unwrap_or(&b.account_id))
    });
    Ok(out)
}

/// The roles the session grants in a given account.
pub async fn list_account_roles(
    session_name: &str,
    account_id: &str,
) -> crate::error::Result<Vec<String>> {
    let access_token = sso::require_token(session_name)?;
    let client = sso_client(session_name).await?;

    let mut out = Vec::new();
    let mut pages = client
        .list_account_roles()
        .access_token(&access_token)
        .account_id(account_id)
        .into_paginator()
        .send();

    while let Some(page) = pages.next().await {
        let page = page.map_err(crate::aws::clients::map_sdk_error)?;
        for role in page.role_list() {
            if let Some(name) = role.role_name() {
                out.push(name.to_string());
            }
        }
    }

    out.sort();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_event_bus_accounts_by_name() {
        // The real account names from the trajector SSO directory.
        for name in [
            "Global Event Bus Production",
            "Global Event Bus Development",
            "Global Event Bus Staging",
            "Global Event Bus Sandbox",
        ] {
            assert!(looks_like_event_bus_account(Some(name)), "{name}");
        }
    }

    #[test]
    fn ignores_unrelated_accounts() {
        for name in ["Employee Portal Production", "AIDC Testing", "appeng-dev"] {
            assert!(!looks_like_event_bus_account(Some(name)), "{name}");
        }
        assert!(!looks_like_event_bus_account(None));
    }

    #[test]
    fn labels_a_target_for_display() {
        let with_name = SsoTarget {
            session: "trajector".into(),
            account_id: "211125309232".into(),
            role_name: "AdministratorAccess".into(),
            account_name: Some("Global Event Bus Development".into()),
        };
        assert_eq!(
            with_name.label(),
            "Global Event Bus Development · AdministratorAccess"
        );

        let without_name = SsoTarget {
            account_name: None,
            ..with_name
        };
        assert_eq!(without_name.label(), "211125309232 · AdministratorAccess");
    }
}
