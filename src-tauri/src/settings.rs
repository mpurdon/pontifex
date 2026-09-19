use crate::aws::clients::CredentialSource;
use crate::aws::sso_credentials::SsoTarget;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The default region for everything in the global-event-bus stack.
/// See `sst.config.ts` in the global-event-bus repo.
pub const DEFAULT_REGION: &str = "us-east-2";

/// Stages the SST app deploys to, in promotion order.
pub const STAGES: [&str; 4] = ["sandbox", "dev", "stg", "prd"];

/// One AWS target the app can point at: a profile, a region, the schema
/// registry in it, and the log groups that carry its events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    /// Display name, e.g. "dev".
    pub label: String,
    /// Profile name from ~/.aws/config. Empty when [`sso`] is set instead.
    ///
    /// Kept non-optional so settings files written before in-app SSO targets
    /// existed still deserialize.
    #[serde(default)]
    pub aws_profile: String,
    /// Account + role resolved in-app from a signed-in SSO session. Takes
    /// precedence over `aws_profile` when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sso: Option<SsoTarget>,
    pub region: String,
    pub registry_name: String,
    pub log_groups: Vec<String>,
    /// Extra confirmation on destructive actions. Defaults on for prd-ish names.
    #[serde(default)]
    pub protected: bool,
}

impl Environment {
    /// Build the conventional environment for a stage, matching the names the
    /// SST stack creates (`stacks/EventBus.ts`).
    pub fn for_stage(stage: &str, profile: &str) -> Self {
        Environment {
            id: stage.to_string(),
            label: stage.to_string(),
            aws_profile: profile.to_string(),
            sso: None,
            region: DEFAULT_REGION.to_string(),
            registry_name: format!("{stage}-global-registry"),
            log_groups: vec![
                format!("/aws/events/{stage}-global-events"),
                format!("/aws/events/{stage}-external-events"),
            ],
            protected: stage == "prd",
        }
    }

    /// The log group carrying the bus's own events, as opposed to the one for
    /// events arriving over the bridge — by the naming `for_stage` uses, with
    /// the first group as the fallback for environments named by hand.
    pub fn bus_log_group(&self) -> Option<&str> {
        self.log_groups
            .iter()
            .find(|g| g.contains("global-events"))
            .or(self.log_groups.first())
            .map(String::as_str)
    }

    /// True when writes to this environment should require extra confirmation.
    /// Mirrors the `CONFIRM_CLEAR_PRODUCTION` guard in the repo's clearSchemas.js.
    pub fn is_protected(&self) -> bool {
        self.protected || self.registry_name.contains("prd") || self.label.contains("prd")
    }

    /// Where this environment's credentials come from. An in-app SSO target
    /// wins over a profile name, so switching an environment to SSO does not
    /// require clearing the old profile field.
    pub fn credential_source(&self) -> Result<CredentialSource> {
        match &self.sso {
            // A target needs both halves: `sso:GetRoleCredentials` is called
            // with the account and the role, so a half-filled one cannot
            // resolve. Caught here because the failure otherwise surfaces from
            // deep inside the credential provider as "the session has expired",
            // which sends you to sign in again and again to no effect.
            Some(target)
                if target.account_id.trim().is_empty() || target.role_name.trim().is_empty() =>
            {
                Err(Error::Invalid(format!(
                    "Environment '{}' has an incomplete SSO selection — pick an account and \
                     a role in Settings → Event bus environments.",
                    self.label
                )))
            }
            Some(target) => Ok(CredentialSource::Sso(target.clone())),
            None if !self.aws_profile.trim().is_empty() => {
                Ok(CredentialSource::Profile(self.aws_profile.clone()))
            }
            None => Err(Error::Invalid(format!(
                "Environment '{}' has no AWS profile or SSO account selected",
                self.label
            ))),
        }
    }
}

/// A Bedrock model the AI panel can target. `model_id` is whatever Bedrock
/// accepts: a foundation model id, an inference profile id, or an
/// application-inference-profile ARN.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BedrockModel {
    pub id: String,
    pub label: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmSettings {
    /// Profile used *only* for Bedrock calls. May differ from the schema profile.
    pub aws_profile: Option<String>,
    pub region: String,
    pub models: Vec<BedrockModel>,
    pub selected_model_id: Option<String>,
    /// Where to import the model catalog from. Defaults to the Claude Code
    /// settings file; see `import_claude_settings`.
    pub claude_settings_path: Option<String>,
}

impl Default for LlmSettings {
    fn default() -> Self {
        LlmSettings {
            aws_profile: None,
            region: DEFAULT_REGION.to_string(),
            models: Vec::new(),
            selected_model_id: None,
            claude_settings_path: None,
        }
    }
}

/// Limits on how much work a CloudWatch scan may do, and how much of its
/// result is kept.
///
/// These were hardcoded and scattered: the time budget lived on the report
/// toolbar, the event cap was a literal in the frontend, and the cache size
/// was a Rust constant. They are one subject — how much sampling costs — so
/// they are configured in one place.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSettings {
    /// Wall-clock ceiling on a scan, in seconds.
    #[serde(default = "default_scan_seconds")]
    pub max_seconds: u64,
    /// Events collected per scan, divided across the time slices.
    #[serde(default = "default_scan_events")]
    pub max_events: usize,
    /// Disk budget for the sampled-event cache, in megabytes.
    #[serde(default = "default_cache_mb")]
    pub cache_mb: usize,
}

fn default_scan_seconds() -> u64 {
    30
}

fn default_scan_events() -> usize {
    5_000
}

fn default_cache_mb() -> usize {
    32
}

impl Default for ScanSettings {
    fn default() -> Self {
        ScanSettings {
            max_seconds: default_scan_seconds(),
            max_events: default_scan_events(),
            cache_mb: default_cache_mb(),
        }
    }
}

impl ScanSettings {
    /// The bounds the backend honours, wherever a limit comes from.
    ///
    /// Held here rather than at each call site: commands that accept a
    /// per-request override used to re-type these literals, so raising a
    /// ceiling here left those paths silently capped at the old value.
    pub const MIN_SECONDS: u64 = 5;
    pub const MAX_SECONDS: u64 = 600;
    pub const MIN_EVENTS: usize = 100;
    pub const MAX_EVENTS: usize = 50_000;

    /// A caller's budget if given, else the configured one — same bounds either way.
    pub fn seconds_or(&self, requested: Option<u64>) -> u64 {
        requested
            .unwrap_or(self.max_seconds)
            .clamp(Self::MIN_SECONDS, Self::MAX_SECONDS)
    }

    pub fn events_or(&self, requested: Option<usize>) -> usize {
        requested
            .unwrap_or(self.max_events)
            .clamp(Self::MIN_EVENTS, Self::MAX_EVENTS)
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache_mb.clamp(4, 1024) * 1024 * 1024
    }
}

/// One routing rule: which Jira project owns an event source.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRoute {
    pub id: String,
    /// Glob over the event source, `*` being the only metacharacter.
    pub pattern: String,
    pub project_key: String,
    /// Overrides [`JiraSettings::issue_type`] for this team, since not every
    /// project calls a bug a Bug.
    #[serde(default)]
    pub issue_type: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee_account_id: Option<String>,
}

/// A routing rule matched against who owns the producer rather than what
/// it is called: the CODEOWNERS team (`@org/team`) or repository
/// (`org/repo`) the origin lookup found. Same shape as a source rule; only
/// what the pattern is held against differs.
pub type OwnerRoute = SourceRoute;

/// Everything about Jira that is not a secret.
///
/// The client secret and the OAuth tokens are deliberately absent: they live
/// in the OS keychain, because this file is plain JSON in the app's config
/// directory and gets copied around.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraSettings {
    /// From the Atlassian developer console. Empty until someone registers an app.
    #[serde(default)]
    pub client_id: String,
    /// The callback URL registered for the app, used verbatim.
    ///
    /// Stored whole rather than as a port, because it has to match what the
    /// developer console has character for character — and `localhost` and
    /// `127.0.0.1` are the same machine but not the same string.
    #[serde(default = "default_callback_url")]
    pub callback_url: String,
    /// Where a source with no matching rule goes. Without it, an unrouted
    /// source cannot be filed at all — which is better than filing it into the
    /// wrong team's backlog.
    #[serde(default)]
    pub default_project: Option<String>,
    #[serde(default = "default_issue_type")]
    pub issue_type: String,
    /// Applied to every ticket, on top of the per-route ones.
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub routes: Vec<SourceRoute>,
    /// Consulted when no source rule matches: routes by who the origin
    /// lookup says publishes the event.
    #[serde(default)]
    pub owner_routes: Vec<OwnerRoute>,
    /// Values for fields a project makes mandatory, keyed by project then field id.
    ///
    /// Projects can demand anything — a "Discovery Environment", a team, a
    /// component — and the demand belongs to the project rather than to any
    /// one routing rule, so several rules pointing at one project share these.
    /// Stored as the JSON Jira wants (`{"id": "10500"}`, a bare string, an
    /// array), because that is what varies per field type.
    #[serde(default)]
    pub field_defaults:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, serde_json::Value>>,
}

fn default_callback_url() -> String {
    "http://127.0.0.1:53682/callback".to_string()
}

fn default_issue_type() -> String {
    "Bug".to_string()
}

impl Default for JiraSettings {
    fn default() -> Self {
        JiraSettings {
            client_id: String::new(),
            callback_url: default_callback_url(),
            default_project: None,
            issue_type: default_issue_type(),
            labels: Vec::new(),
            routes: Vec::new(),
            owner_routes: Vec::new(),
            field_defaults: Default::default(),
        }
    }
}

impl JiraSettings {
    /// Whether an app has been registered. Distinct from being signed in.
    pub fn is_configured(&self) -> bool {
        !self.client_id.trim().is_empty()
    }
}

/// Where the organisation's code lives, for the origin lookup. The token is
/// in the keychain (or borrowed from the GitHub CLI), never here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubSettings {
    /// Organisation to search. Unset: read from the bus checkout's remote.
    #[serde(default)]
    pub org: Option<String>,
    /// Path globs the origin lookup never examines: tests, specs, fixtures
    /// and the like, which name an event type without producing or
    /// consuming it. Same pattern rules as CODEOWNERS.
    #[serde(default = "default_origin_ignore")]
    pub ignore: Vec<String>,
}

impl Default for GithubSettings {
    fn default() -> Self {
        GithubSettings {
            org: None,
            ignore: default_origin_ignore(),
        }
    }
}

/// What a fresh install skips. Editable in Settings → Repo.
pub fn default_origin_ignore() -> Vec<String> {
    [
        "*.test.*",
        "*.spec.*",
        "*.snap",
        "*.md",
        "openapi-spec.*",
        "**/__tests__/**",
        "**/tests/**",
        "**/test/**",
        "**/__mocks__/**",
        "**/mocks/**",
        "**/fixtures/**",
        "**/docs/**",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub environments: Vec<Environment>,
    pub active_environment_id: Option<String>,
    #[serde(default)]
    pub llm: LlmSettings,
    /// Sampling and cache limits.
    #[serde(default)]
    pub scan: ScanSettings,
    /// Where producer bugs get filed. See `docs/jira-integration.md`.
    #[serde(default)]
    pub jira: JiraSettings,
    /// Where producer code lives, for "who publishes this".
    #[serde(default)]
    pub github: GithubSettings,
    /// Path to a local checkout of global-event-bus. Used by the topology view
    /// to read `stacks/busConfiguration.ts`, and as the default import/export root.
    pub event_bus_repo_path: Option<String>,
    /// Event sources pinned to the top of the schema list, so the ones you are
    /// actively working on stay reachable in a list of hundreds.
    #[serde(default)]
    pub pinned_sources: Vec<String>,
    /// Persisted panel sizes, keyed by layout id.
    #[serde(default)]
    pub panel_sizes: std::collections::BTreeMap<String, Vec<f64>>,
    /// Reveals the Developer tab: logs, storage usage and cache state.
    #[serde(default)]
    pub developer_mode: bool,
    /// How much to log: `off`, `error`, `warn`, `info`, `debug`, `trace`.
    ///
    /// `debug` records every command's arguments and every AWS call; `trace`
    /// adds per-item detail. Both are noisy by design — the Developer tab
    /// filters by category, which is what makes that volume usable.
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".to_string()
}

impl Settings {
    pub fn active_environment(&self) -> Result<&Environment> {
        let id = self
            .active_environment_id
            .as_deref()
            .ok_or_else(|| Error::Invalid("No environment selected".into()))?;
        self.environments
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| Error::Invalid(format!("Unknown environment '{id}'")))
    }

    pub fn environment(&self, id: &str) -> Result<&Environment> {
        self.environments
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| Error::Invalid(format!("Unknown environment '{id}'")))
    }

    /// First-run defaults: one environment per stage, all on whichever profile
    /// looks most plausible, plus the repo path if it is where we expect.
    pub fn bootstrap(default_profile: Option<&str>, repo_path: Option<PathBuf>) -> Self {
        let profile = default_profile.unwrap_or("default");
        let environments: Vec<Environment> = STAGES
            .iter()
            .map(|stage| Environment::for_stage(stage, profile))
            .collect();
        Settings {
            active_environment_id: environments.first().map(|e| e.id.clone()),
            environments,
            llm: LlmSettings::default(),
            scan: ScanSettings::default(),
            jira: JiraSettings::default(),
            github: GithubSettings::default(),
            event_bus_repo_path: repo_path.map(|p| p.to_string_lossy().into_owned()),
            pinned_sources: Vec::new(),
            panel_sizes: Default::default(),
            developer_mode: false,
            log_level: default_log_level(),
        }
    }
}

/// Where settings live on disk. Kept next to the other Tauri app data so it
/// follows the platform convention on macOS, Windows and Linux.
pub fn settings_path(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join("settings.json")
}

pub fn load(app_config_dir: &Path) -> Result<Option<Settings>> {
    let path = settings_path(app_config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)?;
    let settings = serde_json::from_str(&raw).map_err(|e| {
        Error::Internal(format!(
            "settings.json is corrupt ({e}); delete {} to reset",
            path.display()
        ))
    })?;
    Ok(Some(settings))
}

pub fn save(app_config_dir: &Path, settings: &Settings) -> Result<()> {
    write_json_atomic(&settings_path(app_config_dir), settings)
}

/// Write a JSON document so a crash mid-write cannot leave a truncated file:
/// to a sibling temp file first, then renamed into place. Creates the parent
/// directory. The one way every on-disk record in the app is written.
pub fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<()> {
    write_bytes_atomic(path, serde_json::to_string_pretty(value)?.as_bytes())
}

/// Write a file whole or not at all: a temp file beside it, then a rename.
pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A temp name unique to this write: two writers of the same file at once
    // (two pollers persisting the watch store) must not share one.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{}.{seq}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// A Bedrock model catalog discovered in a Claude Code settings file.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeSettingsImport {
    /// The file we actually read.
    pub source_path: String,
    pub aws_profile: Option<String>,
    pub region: Option<String>,
    pub models: Vec<BedrockModel>,
}

/// Candidate paths for the Claude Code settings that carry the Bedrock config.
///
/// The primary file has been renamed to `.superseded` on at least one machine,
/// so we accept either and report which one we used.
fn claude_settings_candidates(explicit: Option<&str>) -> Vec<PathBuf> {
    if let Some(p) = explicit {
        return vec![PathBuf::from(shellexpand_home(p))];
    }
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let claude = home.join(".claude");
    vec![
        claude.join("trajector-settings.json"),
        claude.join("trajector-settings.json.superseded"),
        claude.join("settings.json"),
    ]
}

/// Expand a leading `~` so users can type `~/.claude/...` in the settings UI.
pub fn shellexpand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    p.to_string()
}

/// Parse the Bedrock model catalog out of a Claude Code settings file.
///
/// Reads `env.ANTHROPIC_DEFAULT_{OPUS,SONNET,HAIKU}_MODEL` (application
/// inference profile ARNs), plus `env.AWS_PROFILE` / `env.AWS_REGION` as
/// sensible defaults for the LLM profile.
pub fn import_claude_settings(explicit: Option<&str>) -> Result<ClaudeSettingsImport> {
    let candidates = claude_settings_candidates(explicit);
    let path = candidates
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| {
            Error::NotFound(
                "No Claude Code settings file found. Expected ~/.claude/trajector-settings.json (or .superseded)".into(),
            )
        })?;

    let raw = std::fs::read_to_string(&path)?;
    let json: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| Error::Invalid(format!("{} is not valid JSON: {e}", path.display())))?;

    let env = json.get("env").and_then(|v| v.as_object());
    let get = |key: &str| -> Option<String> {
        env.and_then(|e| e.get(key))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };

    let mut models = Vec::new();
    for (key, label) in [
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", "Opus"),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", "Sonnet"),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "Haiku"),
    ] {
        if let Some(model_id) = get(key) {
            models.push(BedrockModel {
                id: label.to_lowercase(),
                label: label.to_string(),
                model_id,
            });
        }
    }

    if models.is_empty() {
        return Err(Error::NotFound(format!(
            "{} has no ANTHROPIC_DEFAULT_*_MODEL entries under `env`",
            path.display()
        )));
    }

    Ok(ClaudeSettingsImport {
        source_path: path.to_string_lossy().into_owned(),
        aws_profile: get("AWS_PROFILE"),
        region: get("AWS_REGION").or_else(|| get("AWS_DEFAULT_REGION")),
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(sso: Option<SsoTarget>, profile: &str) -> Environment {
        Environment {
            id: "prd".into(),
            label: "prd".into(),
            aws_profile: profile.into(),
            sso,
            region: DEFAULT_REGION.into(),
            registry_name: "prd-global-registry".into(),
            log_groups: vec!["/aws/events/prd-global-events".into()],
            protected: true,
        }
    }

    fn target(account: &str, role: &str) -> SsoTarget {
        SsoTarget {
            session: "trajector".into(),
            account_id: account.into(),
            role_name: role.into(),
            account_name: None,
        }
    }

    #[test]
    fn a_settings_file_written_before_scan_limits_still_loads() {
        // Every existing install has a settings.json with no `scan` key. A
        // missing-field error here would fail the load and reset the user's
        // environments to bootstrapped defaults on launch.
        let older = r#"{
            "environments": [],
            "activeEnvironmentId": null,
            "eventBusRepoPath": null
        }"#;
        let loaded: Settings = serde_json::from_str(older).expect("older settings must load");
        assert_eq!(loaded.scan.max_seconds, 30);
        assert_eq!(loaded.scan.max_events, 5_000);
        assert_eq!(loaded.scan.cache_mb, 32);
    }

    #[test]
    fn scan_limits_round_trip_through_json() {
        let mut settings = Settings::bootstrap(Some("p"), None);
        settings.scan.max_seconds = 120;
        settings.scan.max_events = 25_000;
        settings.scan.cache_mb = 128;

        let text = serde_json::to_string(&settings).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.scan.max_seconds, 120);
        assert_eq!(back.scan.max_events, 25_000);
        assert_eq!(back.scan.cache_mb, 128);
        // camelCase on the wire, matching the TypeScript interface.
        assert!(text.contains("maxSeconds"), "{text}");
        assert!(text.contains("cacheMb"), "{text}");
    }

    #[test]
    fn scan_limits_are_clamped_to_what_the_backend_honours() {
        // The UI offers only sane values, but a hand-edited settings.json must
        // not be able to request a 10-hour scan or a 4-byte cache.
        let wild = ScanSettings {
            max_seconds: 100_000,
            max_events: 10_000_000,
            cache_mb: 0,
        };
        assert_eq!(wild.seconds_or(None), 600);
        assert_eq!(wild.events_or(None), 50_000);
        assert_eq!(wild.cache_bytes(), 4 * 1024 * 1024);

        let tiny = ScanSettings {
            max_seconds: 0,
            max_events: 1,
            cache_mb: 99_999,
        };
        assert_eq!(tiny.seconds_or(None), 5);
        assert_eq!(tiny.events_or(None), 100);
        assert_eq!(tiny.cache_bytes(), 1024 * 1024 * 1024);
    }

    #[test]
    fn a_caller_supplied_limit_is_held_to_the_same_bounds() {
        // The whole point of consolidating these: a per-request override used
        // to be clamped by literals at the call site, so raising a ceiling here
        // left those paths silently capped at the old value.
        let settings = ScanSettings::default();
        assert_eq!(
            settings.seconds_or(Some(100_000)),
            ScanSettings::MAX_SECONDS
        );
        assert_eq!(settings.seconds_or(Some(0)), ScanSettings::MIN_SECONDS);
        assert_eq!(
            settings.events_or(Some(10_000_000)),
            ScanSettings::MAX_EVENTS
        );
        assert_eq!(settings.events_or(Some(1)), ScanSettings::MIN_EVENTS);
        // A sane override passes through untouched.
        assert_eq!(settings.seconds_or(Some(45)), 45);
        assert_eq!(settings.events_or(Some(2_000)), 2_000);
    }

    #[test]
    fn an_incomplete_sso_selection_is_rejected_by_name() {
        // A blank account or role reaches GetRoleCredentials and comes back as
        // "the session has expired", which sends the user to sign in forever.
        for (account, role) in [("", "ReadOnly"), ("1234", ""), ("", ""), ("  ", " ")] {
            let env = env_with(Some(target(account, role)), "");
            let err = env.credential_source().unwrap_err();
            let message = err.to_string();
            assert!(
                message.contains("incomplete SSO selection"),
                "expected an actionable message for ({account:?}, {role:?}), got: {message}"
            );
        }
    }

    #[test]
    fn a_complete_sso_selection_resolves() {
        let env = env_with(Some(target("111111111111", "ReadOnlyAccess")), "");
        assert!(matches!(
            env.credential_source().unwrap(),
            CredentialSource::Sso(_)
        ));
    }

    #[test]
    fn a_profile_is_used_when_no_sso_target_is_set() {
        let env = env_with(None, "global-event-bus");
        assert!(matches!(
            env.credential_source().unwrap(),
            CredentialSource::Profile(p) if p == "global-event-bus"
        ));
    }

    #[test]
    fn an_environment_with_neither_says_so() {
        let env = env_with(None, "   ");
        let message = env_with(None, "")
            .credential_source()
            .unwrap_err()
            .to_string();
        assert!(message.contains("no AWS profile or SSO account"));
        assert!(env.credential_source().is_err());
    }

    #[test]
    fn stage_environment_matches_sst_naming() {
        let env = Environment::for_stage("dev", "some-profile");
        assert_eq!(env.registry_name, "dev-global-registry");
        assert_eq!(env.region, "us-east-2");
        assert!(env
            .log_groups
            .contains(&"/aws/events/dev-global-events".to_string()));
        assert!(env
            .log_groups
            .contains(&"/aws/events/dev-external-events".to_string()));
        assert!(!env.is_protected());
    }

    #[test]
    fn prd_is_protected_even_if_flag_unset() {
        let mut env = Environment::for_stage("prd", "p");
        env.protected = false;
        assert!(env.is_protected());
    }
}
