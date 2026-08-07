use crate::aws::clients::ClientCache;
use crate::events_cache::EventCache;
use crate::error::{Error, Result};
use crate::settings::{Environment, Settings};
use aws_config::SdkConfig;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared application state, managed by Tauri and injected into every command.
pub struct AppState {
    pub settings: RwLock<Settings>,
    pub config_dir: PathBuf,
    pub clients: ClientCache,
    /// Sampled events, so a schema can be re-checked without re-scanning
    /// CloudWatch. See `events_cache`.
    pub events: EventCache,
}

impl AppState {
    pub fn new(settings: Settings, config_dir: PathBuf, cache_dir: PathBuf) -> Self {
        let events = EventCache::load(&cache_dir);
        events.set_budget_bytes(settings.scan.cache_bytes());
        AppState {
            settings: RwLock::new(settings),
            config_dir,
            clients: ClientCache::new(),
            events,
        }
    }

    pub async fn settings_snapshot(&self) -> Settings {
        self.settings.read().await.clone()
    }

    /// Resolve an environment by id, falling back to the active one.
    pub async fn resolve_environment(&self, env_id: Option<&str>) -> Result<Environment> {
        let settings = self.settings.read().await;
        let env = match env_id {
            Some(id) => settings.environment(id)?,
            None => settings.active_environment()?,
        };
        Ok(env.clone())
    }

    /// An SDK config pointed at the given environment's credentials and region.
    pub async fn env_config(&self, env_id: Option<&str>) -> Result<(Environment, Arc<SdkConfig>)> {
        let env = self.resolve_environment(env_id).await?;
        let source = env.credential_source()?;
        let cfg = self.clients.config_for(&source, &env.region).await?;
        Ok((env, cfg))
    }

    /// An SDK config for Bedrock, using the separately configured LLM profile.
    ///
    /// Deliberately distinct from [`env_config`] so schema work and LLM work
    /// can authenticate as different principals.
    pub async fn llm_config(&self) -> Result<Arc<SdkConfig>> {
        let settings = self.settings.read().await;
        let profile = settings.llm.aws_profile.clone().ok_or_else(|| {
            Error::Invalid(
                "No AWS profile configured for Bedrock. Set one in Settings → AI.".into(),
            )
        })?;
        let region = settings.llm.region.clone();
        drop(settings);
        self.clients.config(&profile, &region).await
    }

    pub async fn persist(&self) -> Result<()> {
        let settings = self.settings.read().await;
        crate::settings::save(&self.config_dir, &settings)
    }
}
