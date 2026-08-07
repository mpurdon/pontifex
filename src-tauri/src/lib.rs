// Declared first: its macros are textually scoped, so every module below can
// use `linfo!`/`lwarn!`/`ldebug!` without importing them.
#[macro_use]
pub mod logging;

pub mod aws;
mod commands;
pub mod error;
pub mod events_cache;
pub mod jira;
mod settings;
mod state;

use logging::cat;

// Public so the integration tests in `tests/` can exercise the schema
// transforms directly against the real repo fixtures.
pub mod schema;

use state::AppState;
use tauri::Manager;

/// The profile the Claude Code settings nominate for Bedrock, if it exists.
fn claude_bedrock_profile(profiles: &[aws::profiles::AwsProfile]) -> Option<String> {
    settings::import_claude_settings(None)
        .ok()?
        .aws_profile
        .filter(|p| profiles.iter().any(|candidate| &candidate.name == p))
}

/// Best guess at which profile to seed the schema environments with.
///
/// Ranked by how likely the profile is to work *right now*, because a bad guess
/// is not a neutral default — it greets a first-time user with a credential
/// error on every screen. In order:
///
/// 1. A profile whose name names this app's domain (`global-event-bus`,
///    `geb-dev`, …). Someone who set one up almost certainly meant it for this.
/// 2. An SSO profile with a live cached token — already signed in.
/// 3. A profile from `~/.aws/credentials` — an external manager (Leapp,
///    aws-vault, …) wrote credentials for it, so it is probably current.
/// 4. Any other SSO profile — signing in will fix it.
/// 5. Anything left.
///
/// Two exclusions apply throughout: unfilled template profiles (see
/// [`AwsProfile::is_placeholder`]) and the Claude Code Bedrock profile, whose
/// role is scoped to Bedrock and cannot read the schema registry. The latter
/// seeds the LLM profile slot instead.
fn guess_default_profile(profiles: &[aws::profiles::AwsProfile]) -> Option<String> {
    let bedrock = claude_bedrock_profile(profiles);

    let candidates: Vec<&aws::profiles::AwsProfile> = profiles
        .iter()
        .filter(|p| Some(&p.name) != bedrock.as_ref() && !p.is_placeholder())
        .collect();

    let signed_in = |p: &aws::profiles::AwsProfile| {
        p.is_sso()
            && matches!(aws::sso::sso_status(p), Ok(status) if status.has_token && !status.expired)
    };

    let names_this_domain = |p: &aws::profiles::AwsProfile| {
        let n = p.name.to_lowercase();
        n.contains("event-bus") || n.contains("eventbus") || n.starts_with("geb")
    };

    candidates
        .iter()
        .find(|p| names_this_domain(p))
        .or_else(|| candidates.iter().find(|p| signed_in(p)))
        .or_else(|| candidates.iter().find(|p| p.externally_managed))
        .or_else(|| candidates.iter().find(|p| p.is_sso()))
        .or_else(|| candidates.first())
        // Everything was excluded — fall back rather than leaving it unset.
        .map(|p| p.name.clone())
        .or_else(|| profiles.first().map(|p| p.name.clone()))
}

/// Look for a global-event-bus checkout in the conventional place, so the
/// topology view and import/export have a sensible default.
fn guess_repo_path() -> Option<std::path::PathBuf> {
    let home = dirs::home_dir()?;
    let candidate = home.join("Projects").join("trajector").join("global-event-bus");
    candidate
        .join("stacks")
        .join("busConfiguration.ts")
        .exists()
        .then_some(candidate)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;

            // Settings are read before the logger is installed, because they
            // carry the level. Anything worth saying about that read is held
            // and logged once the logger exists.
            let mut deferred: Vec<String> = Vec::new();
            let loaded = match settings::load(&config_dir) {
                Ok(s) => s,
                Err(e) => {
                    // A corrupt settings file should not stop the app from
                    // starting — fall back to defaults and let the user fix it.
                    deferred.push(format!(
                        "could not load settings, starting from defaults: {e}"
                    ));
                    None
                }
            };

            let level = logging::level_from_str(
                loaded.as_ref().map(|s| s.log_level.as_str()).unwrap_or("info"),
            );

            // Always log to a file, not only in debug: the Developer tab
            // exists to answer "what happened", which needs a record that
            // outlives the terminal the app was started from.
            //
            // The format puts the category before the message so the Developer
            // tab can parse and filter on it: `[time][LEVEL][category] text`.
            app.handle().plugin(
                tauri_plugin_log::Builder::default()
                    .level(level)
                    // The AWS SDK and hyper are extremely chatty at debug and
                    // would bury our own lines at the very levels you turn on
                    // to see them.
                    .level_for("aws_smithy_runtime", log::LevelFilter::Warn)
                    .level_for("aws_smithy_runtime_api", log::LevelFilter::Warn)
                    .level_for("aws_config", log::LevelFilter::Warn)
                    .level_for("aws_sdk_sso", log::LevelFilter::Warn)
                    .level_for("aws_sdk_ssooidc", log::LevelFilter::Warn)
                    .level_for("aws_sdk_sts", log::LevelFilter::Warn)
                    .level_for("hyper", log::LevelFilter::Warn)
                    .level_for("hyper_util", log::LevelFilter::Warn)
                    .level_for("rustls", log::LevelFilter::Warn)
                    .level_for("h2", log::LevelFilter::Warn)
                    .level_for("tracing", log::LevelFilter::Warn)
                    .format(|out, message, record| {
                        out.finish(format_args!(
                            "[{}][{}][{}] {}",
                            time::OffsetDateTime::now_utc()
                                .format(&time::format_description::well_known::Rfc3339)
                                .unwrap_or_default(),
                            record.level(),
                            record.target(),
                            message
                        ))
                    })
                    .targets([
                        tauri_plugin_log::Target::new(
                            tauri_plugin_log::TargetKind::LogDir {
                                file_name: Some("gebman".into()),
                            },
                        ),
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    ])
                    .max_file_size(5_000_000)
                    .build(),
            )?;

            for message in deferred {
                lerror!(cat::APP, "{message}");
            }

            linfo!(
                cat::APP,
                "gebman {} starting — {}/{}, log level {level}",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH
            );

            let settings = match loaded {
                Some(s) => s,
                None => {
                    let profiles = aws::profiles::list_profiles().unwrap_or_default();
                    let mut bootstrapped = settings::Settings::bootstrap(
                        guess_default_profile(&profiles).as_deref(),
                        guess_repo_path(),
                    );

                    // Seed the AI side from the Claude Code settings, which is
                    // where the Bedrock profile and inference profile ARNs live.
                    if let Ok(import) = settings::import_claude_settings(None) {
                        bootstrapped.llm.models = import.models;
                        bootstrapped.llm.selected_model_id =
                            bootstrapped.llm.models.first().map(|m| m.id.clone());
                        bootstrapped.llm.aws_profile =
                            claude_bedrock_profile(&profiles).or(import.aws_profile);
                        if let Some(region) = import.region {
                            bootstrapped.llm.region = region;
                        }
                        bootstrapped.llm.claude_settings_path = Some(import.source_path);
                    }

                    if let Err(e) = settings::save(&config_dir, &bootstrapped) {
                        lwarn!(cat::APP, "could not write initial settings: {e}");
                    }
                    bootstrapped
                }
            };

            let cache_dir = app.path().app_cache_dir()?;
            app.manage(AppState::new(settings, config_dir, cache_dir));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // settings
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::save_panel_sizes,
            commands::settings::set_active_environment,
            commands::settings::default_environment_for_stage,
            commands::settings::stages,
            commands::settings::import_claude_settings,
            // auth
            commands::auth::list_profiles,
            commands::auth::check_profile,
            commands::auth::check_environment,
            commands::auth::sso_login,
            commands::auth::refresh_credentials,
            commands::auth::list_sso_sessions,
            commands::auth::sso_session_login,
            commands::auth::list_sso_accounts,
            commands::auth::list_sso_account_roles,
            commands::settings::export_sso_profile,
            // developer tools
            commands::devtools::dev_info,
            commands::devtools::read_app_logs,
            commands::devtools::open_app_path,
            commands::devtools::clear_app_logs,
            commands::devtools::ui_log,
            commands::devtools::log_categories,
            // schemas
            commands::schemas::list_registries,
            commands::schemas::list_schemas,
            commands::schemas::describe_schema,
            commands::schemas::list_schema_versions,
            commands::schemas::schema_history,
            commands::schemas::search_schemas,
            commands::schemas::validate_schema,
            commands::schemas::put_schema,
            commands::schemas::delete_schema,
            commands::schemas::new_schema_draft,
            commands::schemas::simplify_schema,
            commands::schemas::widen_nullable_schema,
            commands::schemas::plan_import,
            commands::schemas::apply_import,
            commands::schemas::export_schemas,
            // bedrock
            commands::bedrock::ai_generate,
            commands::bedrock::list_bedrock_models,
            // reality check
            commands::reality::check_against_events,
            commands::reality::apply_field_suggestions,
            commands::reality::add_observed_field,
            commands::reality::registry_report,
            commands::reality::event_cache_stats,
            commands::reality::clear_event_cache,
            commands::reality::draft_from_events,
            commands::reality::issues_for_schemas,
            commands::reality::event_sources,
            // jira
            commands::jira::jira_status,
            commands::jira::set_jira_app,
            commands::jira::parse_jira_authorize_url,
            commands::jira::jira_connect,
            commands::jira::jira_sites,
            commands::jira::select_jira_site,
            commands::jira::jira_disconnect,
            commands::jira::jira_projects,
            commands::jira::jira_issue_types,
            commands::jira::jira_required_fields,
            commands::jira::set_jira_field_defaults,
            commands::jira::preview_jira_ticket,
            commands::jira::file_jira_ticket,
            commands::jira::file_jira_tickets,
            // logs
            commands::logs::list_log_groups,
            commands::logs::query_logs,
            // topology
            commands::topology::get_topology,
            commands::topology::preview_bus_configuration,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
