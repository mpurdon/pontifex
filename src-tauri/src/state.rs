use crate::analysis_cache::AnalysisCache;
use crate::aws::clients::ClientCache;
use crate::error::{Error, Result};
use crate::events_cache::EventCache;
use crate::origin::OriginCache;
use crate::settings::{Environment, Settings};
use crate::watch::{WatchStore, Watcher};
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
    /// Watch mode's definitions, hits and cursors. See `watch`.
    pub watch: WatchStore,
    /// The pollers themselves, one per armed environment.
    pub watcher: Watcher,
    /// Answers to "where did this event type come from", a month at a time.
    pub origins: OriginCache,
    /// The last reality check per schema, a month at a time.
    pub analyses: AnalysisCache,
}

impl AppState {
    pub fn new(settings: Settings, config_dir: PathBuf, cache_dir: PathBuf) -> Self {
        let events = EventCache::load(&cache_dir);
        events.set_budget_bytes(settings.scan.cache_bytes());
        let watch = WatchStore::load(&config_dir);
        let origins = OriginCache::load(&cache_dir);
        let analyses = AnalysisCache::load(&cache_dir);
        AppState {
            settings: RwLock::new(settings),
            config_dir,
            clients: ClientCache::new(),
            events,
            watch,
            watcher: Watcher::new(),
            origins,
            analyses,
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

    /// Change one thing in the settings, persist, and hand back the result.
    ///
    /// The light path for display preferences — pane sizes, the time zone,
    /// the theme. Nothing is invalidated: no credential check, no cache drop,
    /// no refetch. The full `save_settings` command is for changes to what
    /// the app talks to.
    pub async fn update_settings(&self, apply: impl FnOnce(&mut Settings)) -> Result<Settings> {
        {
            let mut guard = self.settings.write().await;
            apply(&mut guard);
        }
        self.persist().await?;
        Ok(self.settings_snapshot().await)
    }
}
