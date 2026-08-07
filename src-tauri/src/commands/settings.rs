use crate::error::{Error, Result};
use crate::settings::{self, ClaudeSettingsImport, Environment, Settings};
use crate::state::AppState;
use tauri::State;

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    Ok(state.settings_snapshot().await)
}

/// Replace the whole settings object and persist it.
///
/// The settings form edits a local copy and saves it wholesale, which keeps
/// the command surface small and avoids partial-update races.
#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<Settings> {
    validate_settings(&settings)?;

    // Applied before the swap so a shrunk budget takes effect on the very next
    // persist rather than one write later.
    state.events.set_budget_bytes(settings.scan.cache_bytes());

    {
        let mut guard = state.settings.write().await;
        *guard = settings;
    }
    state.persist().await?;

    // Profiles or regions may have changed, so cached configs are now suspect.
    state.clients.invalidate_all().await;

    Ok(state.settings_snapshot().await)
}

/// Persist one panel group's sizes, and nothing else.
///
/// Deliberately not `save_settings`: a splitter drag is transient geometry,
/// but routing it through the full save re-validated every environment's
/// credentials, dropped every cached SDK config, and made the frontend refetch
/// the registry, profiles, logs and topology from AWS. Dragging a divider now
/// writes one map entry.
#[tauri::command]
pub async fn save_panel_sizes(
    state: State<'_, AppState>,
    id: String,
    sizes: Vec<f64>,
) -> Result<Settings> {
    {
        let mut guard = state.settings.write().await;
        guard.panel_sizes.insert(id, sizes);
    }
    state.persist().await?;
    Ok(state.settings_snapshot().await)
}

fn validate_settings(settings: &Settings) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for env in &settings.environments {
        if env.id.trim().is_empty() {
            return Err(Error::Invalid("Every environment needs an id".into()));
        }
        if !seen.insert(env.id.clone()) {
            return Err(Error::Invalid(format!(
                "Duplicate environment id '{}'",
                env.id
            )));
        }
        // Either credential source is fine, but one must be set.
        env.credential_source()?;
        if env.registry_name.trim().is_empty() {
            return Err(Error::Invalid(format!(
                "Environment '{}' has no registry name",
                env.label
            )));
        }
        if env.region.trim().is_empty() {
            return Err(Error::Invalid(format!(
                "Environment '{}' has no region",
                env.label
            )));
        }
    }

    if let Some(active) = &settings.active_environment_id {
        if !settings.environments.iter().any(|e| &e.id == active) {
            return Err(Error::Invalid(format!(
                "Active environment '{active}' is not in the environment list"
            )));
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn set_active_environment(state: State<'_, AppState>, env_id: String) -> Result<Settings> {
    {
        let mut guard = state.settings.write().await;
        guard.environment(&env_id)?;
        guard.active_environment_id = Some(env_id);
    }
    state.persist().await?;
    Ok(state.settings_snapshot().await)
}

/// Build the conventional environment for a stage, for the "add stage" button.
#[tauri::command]
pub fn default_environment_for_stage(stage: String, profile: String) -> Environment {
    Environment::for_stage(&stage, &profile)
}

#[tauri::command]
pub fn stages() -> Vec<&'static str> {
    settings::STAGES.to_vec()
}

/// Write an environment's SSO target into `~/.aws/config` as a named profile.
///
/// Purely opt-in: gebman resolves these credentials in-app and never needs a
/// profile. This exists so the same account/role is reachable from the AWS CLI
/// and other SDKs once you have set it up here.
#[tauri::command]
pub async fn export_sso_profile(
    state: State<'_, AppState>,
    env_id: String,
    profile_name: String,
) -> Result<String> {
    let env = state.resolve_environment(Some(&env_id)).await?;
    let target = env.sso.as_ref().ok_or_else(|| {
        Error::Invalid(format!(
            "Environment '{}' does not use an SSO account, so there is nothing to export",
            env.label
        ))
    })?;
    crate::aws::config_writer::export_sso_profile(&profile_name, target, &env.region)
}

/// Read the Bedrock model catalog out of a Claude Code settings file.
///
/// Returns what it found without applying it, so the UI can show the user
/// which file was used and which models were discovered before they commit.
#[tauri::command]
pub async fn import_claude_settings(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<ClaudeSettingsImport> {
    let explicit = match path {
        Some(p) if !p.trim().is_empty() => Some(p),
        _ => state.settings.read().await.llm.claude_settings_path.clone(),
    };
    settings::import_claude_settings(explicit.as_deref())
}
