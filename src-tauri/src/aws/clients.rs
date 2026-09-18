use crate::error::{classify, Error, Result};
use crate::logging::cat;
use aws_config::{BehaviorVersion, SdkConfig};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::aws::sso_credentials::{SsoRoleProvider, SsoTarget};

/// Where an environment's credentials come from.
///
/// A named profile delegates to the standard AWS credential chain; an SSO
/// target is redeemed in-app from a signed-in session, so it works without any
/// profile existing for that account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CredentialSource {
    Profile(String),
    Sso(SsoTarget),
}

impl CredentialSource {
    /// The SSO session this source depends on, if any. Used to decide which
    /// sign-in to offer when credentials fail.
    pub fn sso_session(&self) -> Option<&str> {
        match self {
            CredentialSource::Sso(target) => Some(&target.session),
            CredentialSource::Profile(_) => None,
        }
    }
}

/// Cache key for a resolved SDK config.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConfigKey {
    pub source: CredentialSource,
    pub region: String,
}

/// Caches one `SdkConfig` per (profile, region).
///
/// Building an `SdkConfig` re-reads the config files and re-resolves the SSO
/// token, so doing it per request would hammer the disk and, worse, re-run the
/// credential chain on every keystroke in the schema browser. The underlying
/// credentials provider caches and refreshes on its own, so a long-lived
/// `SdkConfig` still picks up a fresh token after a login — except that a login
/// writes a *new* cache file, so `invalidate` drops the entry to force a
/// clean re-resolve.
#[derive(Default)]
pub struct ClientCache {
    configs: RwLock<HashMap<ConfigKey, Arc<SdkConfig>>>,
    /// Long-running work that should retry the moment credentials change —
    /// the watch pollers, parked on an expired token — subscribes here.
    /// Each subscriber has its own `Notify`, so a change that lands while a
    /// subscriber is busy is kept as a permit rather than lost.
    subscribers: std::sync::Mutex<Vec<std::sync::Weak<tokio::sync::Notify>>>,
}

impl ClientCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Config for a named profile.
    pub async fn config(&self, profile: &str, region: &str) -> Result<Arc<SdkConfig>> {
        self.config_for(&CredentialSource::Profile(profile.to_string()), region)
            .await
    }

    pub async fn config_for(
        &self,
        source: &CredentialSource,
        region: &str,
    ) -> Result<Arc<SdkConfig>> {
        let key = ConfigKey {
            source: source.clone(),
            region: region.to_string(),
        };

        if let Some(cfg) = self.configs.read().await.get(&key) {
            ldebug!(
                cat::AWS,
                "sdk config cache hit for {} in {region}",
                describe_source(source)
            );
            return Ok(cfg.clone());
        }

        let timer = crate::logging::Timed::start(
            cat::AWS,
            format!(
                "resolving credentials for {} in {region}",
                describe_source(source)
            ),
        );
        let cfg = Arc::new(build_config(source, region).await);
        timer.done("config built");

        let mut guard = self.configs.write().await;
        Ok(guard.entry(key).or_insert(cfg).clone())
    }

    /// Drop cached configs for a profile. Call after a successful SSO login so
    /// the next request re-reads the freshly written token cache.
    pub async fn invalidate(&self, profile: &str) {
        linfo!(
            cat::AWS,
            "invalidating cached configs for profile {profile}"
        );
        self.configs
            .write()
            .await
            .retain(|k, _| k.source != CredentialSource::Profile(profile.to_string()));
        self.announce_change();
    }

    /// Drop cached configs that depend on an SSO session, after signing into it.
    pub async fn invalidate_session(&self, session: &str) {
        linfo!(
            cat::AWS,
            "invalidating cached configs for SSO session {session}"
        );
        self.configs
            .write()
            .await
            .retain(|k, _| k.source.sso_session() != Some(session));
        self.announce_change();
    }

    pub async fn invalidate_all(&self) {
        linfo!(cat::AWS, "invalidating every cached SDK config");
        self.configs.write().await.clear();
        self.announce_change();
    }

    /// A handle that is notified whenever credentials are invalidated. Hold
    /// it for the life of the task; `notified().await` completes on the next
    /// change, or at once if one arrived since the last wait.
    pub fn subscribe(&self) -> Arc<tokio::sync::Notify> {
        let notify = Arc::new(tokio::sync::Notify::new());
        self.subscribers
            .lock()
            .expect("client cache subscribers poisoned")
            .push(Arc::downgrade(&notify));
        notify
    }

    /// Wake every live subscriber, forgetting the ones whose task is gone.
    fn announce_change(&self) {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("client cache subscribers poisoned");
        subscribers.retain(|weak| match weak.upgrade() {
            Some(notify) => {
                notify.notify_one();
                true
            }
            None => false,
        });
    }

    /// How many SDK configs are cached, for the diagnostics screen.
    pub async fn count(&self) -> usize {
        self.configs.read().await.len()
    }
}

/// How a credential source reads in a log line.
fn describe_source(source: &CredentialSource) -> String {
    match source {
        CredentialSource::Profile(p) => format!("profile '{p}'"),
        CredentialSource::Sso(t) => {
            format!(
                "SSO {}/{} via session '{}'",
                t.account_id, t.role_name, t.session
            )
        }
    }
}

async fn build_config(source: &CredentialSource, region: &str) -> SdkConfig {
    let builder = aws_config::defaults(BehaviorVersion::latest())
        .region(aws_config::Region::new(region.to_string()));

    match source {
        CredentialSource::Profile(profile) => builder.profile_name(profile).load().await,
        // The provider re-reads the token cache on every refresh, so a fresh
        // sign-in takes effect without rebuilding this config.
        CredentialSource::Sso(target) => {
            builder
                .credentials_provider(SsoRoleProvider::new(target.clone()))
                .load()
                .await
        }
    }
}

/// Who the given profile currently authenticates as.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CallerIdentity {
    pub account: Option<String>,
    pub arn: Option<String>,
    pub user_id: Option<String>,
}

/// Resolve credentials and confirm they work by calling `sts:GetCallerIdentity`.
///
/// This is the app's liveness check for a profile: it proves both that the
/// credential chain resolves and that the token has not expired, which reading
/// the config file alone cannot tell us.
pub async fn caller_identity(cfg: &SdkConfig) -> Result<CallerIdentity> {
    let timer = crate::logging::Timed::start(cat::AWS, "sts:GetCallerIdentity");
    let client = aws_sdk_sts::Client::new(cfg);
    let out = client.get_caller_identity().send().await.map_err(|e| {
        let code = e
            .as_service_error()
            .and_then(|se| se.meta().code())
            .map(str::to_string);
        let error = classify(code.as_deref(), describe_sdk_error(&e));
        error
    });
    let out = match out {
        Ok(out) => {
            timer.done(format!(
                "authenticated as {}",
                out.arn().unwrap_or("(no arn)")
            ));
            out
        }
        Err(e) => {
            timer.failed(&e);
            return Err(e);
        }
    };
    Ok(CallerIdentity {
        account: out.account().map(str::to_string),
        arn: out.arn().map(str::to_string),
        user_id: out.user_id().map(str::to_string),
    })
}

/// Flatten an SDK error into the most specific message available.
///
/// `SdkError`'s Display is famously unhelpful ("service error"), with the real
/// cause only reachable through the source chain — which is exactly where
/// "token expired" and "no credentials" live. Walking the chain is what makes
/// [`classify`] able to tell an auth problem from a genuine API failure.
pub fn describe_sdk_error<E, R>(
    err: &aws_smithy_runtime_api::client::result::SdkError<E, R>,
) -> String
where
    E: std::error::Error + 'static,
    R: std::fmt::Debug + 'static,
{
    use std::error::Error as _;

    let mut parts: Vec<String> = Vec::new();
    let top = err.to_string();
    if !top.is_empty() {
        parts.push(top);
    }

    let mut source: Option<&(dyn std::error::Error + 'static)> = err.source();
    while let Some(e) = source {
        let msg = e.to_string();
        if !msg.is_empty() && !parts.iter().any(|p| p == &msg) {
            parts.push(msg);
        }
        source = e.source();
    }

    if parts.is_empty() {
        "Unknown AWS error".to_string()
    } else {
        parts.join(": ")
    }
}

/// Extract a service error code from any SDK error, if one is present.
pub fn error_code<E, R>(
    err: &aws_smithy_runtime_api::client::result::SdkError<E, R>,
) -> Option<String>
where
    E: std::error::Error + aws_smithy_types::error::metadata::ProvideErrorMetadata + 'static,
    R: std::fmt::Debug + 'static,
{
    err.as_service_error()
        .and_then(|se| se.code())
        .map(str::to_string)
}

/// Convert an SDK error into our error type, classifying it along the way.
pub fn map_sdk_error<E, R>(err: aws_smithy_runtime_api::client::result::SdkError<E, R>) -> Error
where
    E: std::error::Error + aws_smithy_types::error::metadata::ProvideErrorMetadata + 'static,
    R: std::fmt::Debug + 'static,
{
    let code = error_code(&err);
    let message = describe_sdk_error(&err);
    classify(code.as_deref(), message)
}
