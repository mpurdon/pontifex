//! The origin story of an event type, and the GitHub access behind it.

use crate::error::{Error, Result};
use crate::origin::{github, wiring, EventOrigin, GithubOutcome};
use crate::schema::model::EventIdentity;
use crate::state::AppState;
use serde::Serialize;
use std::path::PathBuf;
use tauri::State;

/// The bus checkout's `owner/repo`, when a checkout is configured.
async fn bus_slug(settings: &crate::settings::Settings) -> Option<String> {
    match settings.event_bus_repo_path.as_deref() {
        Some(path) => wiring::github_slug(std::path::Path::new(path)).await,
        None => None,
    }
}

fn org_of(slug: &str) -> Option<String> {
    slug.split('/').next().map(str::to_string)
}

/// The organisation: configured, or read off the bus checkout's remote —
/// which runs git, so only when it has to.
pub async fn resolve_org(settings: &crate::settings::Settings) -> Option<String> {
    if let Some(org) = settings.github.org.clone().filter(|o| !o.trim().is_empty()) {
        return Some(org);
    }
    bus_slug(settings).await.as_deref().and_then(org_of)
}

/// The cached origin of an event type, if a lookup has run for it. A map
/// read, nothing more: callers that only want to enrich something else must
/// not pay for a search.
pub async fn cached_origin(
    state: &AppState,
    org: Option<&str>,
    source: &str,
    detail_type: &str,
) -> Option<EventOrigin> {
    state.origins.get(org, source, detail_type).await
}

/// Where an event type came from. Cached for a week unless `refresh`.
#[tauri::command]
pub async fn event_origin(
    state: State<'_, AppState>,
    name: String,
    refresh: Option<bool>,
    // How many matching files to examine; more than the default means a
    // fresh lookup, since the cached one stopped short.
    limit: Option<usize>,
) -> Result<EventOrigin> {
    let identity = EventIdentity::from_schema_name(&name)?;
    let settings = state.settings_snapshot().await;
    let org = resolve_org(&settings).await;

    let max_files = limit.unwrap_or(github::MAX_FILES);
    if refresh != Some(true) && limit.is_none() {
        if let Some(cached) = state
            .origins
            .get(org.as_deref(), &identity.source, &identity.detail_type)
            .await
        {
            return Ok(cached);
        }
    }

    let repo = settings.event_bus_repo_path.clone().map(PathBuf::from);
    let slug = bus_slug(&settings).await;
    let wiring = match &repo {
        Some(path) => wiring::first_mention(path, &identity.detail_type).await?,
        None => None,
    };
    let wiring_repo_url = slug.as_ref().map(|s| format!("https://github.com/{s}"));

    let (producers, github) = match (&org, github::token().await?) {
        (Some(org), (Some(token), _)) => {
            github::lookup(
                &github::Client::new(token),
                org,
                &identity.source,
                &identity.detail_type,
                slug.as_deref(),
                &settings.github.ignore,
                max_files,
            )
            .await
        }
        (None, _) => (
            Vec::new(),
            GithubOutcome::failed("unconfigured", None, "No GitHub organisation is set and none could be read from the bus checkout's remote. Set one in Settings → Repo.".into()),
        ),
        (Some(org), (None, _)) => (
            Vec::new(),
            GithubOutcome::failed(
                "unconfigured",
                Some(org.clone()),
                "No GitHub token: store one in Settings → Repo, or sign in with the GitHub CLI (`gh auth login`).".into(),
            ),
        ),
    };

    let result = EventOrigin {
        source: identity.source.clone(),
        detail_type: identity.detail_type.clone(),
        wiring,
        wiring_repo_url,
        producers,
        github,
        cached_at: crate::events_cache::now_ms(),
    };
    state.origins.put(org.as_deref(), result.clone()).await;
    Ok(result)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubStatus {
    pub org: Option<String>,
    /// Where the org came from when not configured: the bus repo's remote.
    pub org_from_remote: Option<String>,
    pub token_source: github::TokenSource,
    /// What the ignore list is when left alone, for the reset button and the
    /// placeholder — so the frontend never carries its own copy.
    pub default_ignore: Vec<String>,
}

#[tauri::command]
pub async fn github_status(state: State<'_, AppState>) -> Result<GithubStatus> {
    let settings = state.settings_snapshot().await;
    let org_from_remote = bus_slug(&settings).await.as_deref().and_then(org_of);
    let (_, token_source) = github::token().await?;
    Ok(GithubStatus {
        org: settings.github.org.clone(),
        org_from_remote,
        token_source,
        default_ignore: crate::settings::default_origin_ignore(),
    })
}

/// Store a personal access token in the keychain, or clear it when empty.
#[tauri::command]
pub async fn set_github_token(token: String) -> Result<()> {
    if token.trim().is_empty() {
        github::clear_token()
    } else {
        github::store_token(&token)
    }
}

/// Who the current token authenticates as, to confirm it works.
#[tauri::command]
pub async fn github_check() -> Result<String> {
    let (token, source) = github::token().await?;
    let token = token.ok_or_else(|| {
        Error::Auth("No GitHub token: store one here or run `gh auth login`.".into())
    })?;
    let login = github::Client::new(token).viewer().await?;
    Ok(format!(
        "{login} via {}",
        match source {
            github::TokenSource::Keychain => "the stored token",
            github::TokenSource::GhCli => "the GitHub CLI",
            github::TokenSource::None => "nothing",
        }
    ))
}
