use crate::error::{Error, Result};
use serde::Serialize;
use std::collections::HashMap;

/// How a profile obtains credentials. Determines which login path applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProfileKind {
    /// Modern `sso_session = <name>` form, with the SSO details in an
    /// `[sso-session <name>]` block.
    SsoSession,
    /// Legacy form with `sso_start_url` / `sso_region` inline on the profile.
    SsoLegacy,
    /// `role_arn` + `source_profile` — refreshes via the source profile.
    AssumeRole,
    /// Static keys, credential_process, or anything else we do not manage.
    Static,
}

/// Everything the UI needs to render and log into a profile.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AwsProfile {
    pub name: String,
    pub kind: ProfileKind,
    pub region: Option<String>,
    /// Set for both SSO forms; this is what we resolve tokens against.
    pub sso_start_url: Option<String>,
    pub sso_region: Option<String>,
    pub sso_account_id: Option<String>,
    pub sso_role_name: Option<String>,
    /// Only set for `SsoSession`. Also the SSO token cache key.
    pub sso_session: Option<String>,
    /// Only set for `AssumeRole`.
    pub source_profile: Option<String>,
    /// True when the profile's credentials live in `~/.aws/credentials` rather
    /// than being derived from `~/.aws/config`.
    ///
    /// These are typically written by an external credential manager (Leapp,
    /// aws-vault, saml2aws, a CI script). pontifex cannot refresh them — the user
    /// has to renew the session in whatever tool owns it — so the UI must not
    /// offer an SSO sign-in that could never work.
    pub externally_managed: bool,
}

impl AwsProfile {
    pub fn is_sso(&self) -> bool {
        matches!(self.kind, ProfileKind::SsoSession | ProfileKind::SsoLegacy)
    }

    /// True when the profile is obviously an unfilled template rather than a
    /// real target.
    ///
    /// The AWS docs' example config is widely copy-pasted, leaving entries like
    /// `sso_start_url = https://YOUR-ORG.awsapps.com/start` and
    /// `sso_account_id = <dev-account-id>`. Picking one as a default greets the
    /// user with an unresolvable credential error and an SSO sign-in that fails
    /// with `InvalidRequestException`, so they are excluded from auto-selection.
    pub fn is_placeholder(&self) -> bool {
        let account_looks_real = self
            .sso_account_id
            .as_deref()
            .map(|id| id.len() == 12 && id.chars().all(|c| c.is_ascii_digit()))
            .unwrap_or(true); // non-SSO profiles have no account id to judge

        let url_looks_real = self
            .sso_start_url
            .as_deref()
            .map(|url| {
                let lowered = url.to_lowercase();
                !(lowered.contains("your-org")
                    || lowered.contains("your_org")
                    || lowered.contains("yourorg")
                    || lowered.contains("example")
                    || lowered.contains("my-sso")
                    || lowered.contains('<')
                    || lowered.contains('>'))
            })
            .unwrap_or(true);

        !account_looks_real || !url_looks_real
    }

    /// The key the AWS CLI uses to name this profile's token cache file.
    ///
    /// CLI v2 caches under `~/.aws/sso/cache/<sha1(key)>.json` where `key` is
    /// the *session name* for `sso_session` profiles and the *start URL* for
    /// legacy ones. Matching that exactly is what lets pontifex and the CLI share
    /// one login in both directions.
    pub fn token_cache_key(&self) -> Option<String> {
        match self.kind {
            ProfileKind::SsoSession => self.sso_session.clone(),
            ProfileKind::SsoLegacy => self.sso_start_url.clone(),
            _ => None,
        }
    }
}

/// A minimal INI parser for the AWS shared config format.
///
/// We parse the file ourselves rather than going through `aws-config` because
/// we need the *declared* SSO metadata for every profile up front (to drive the
/// login UI), and the SDK only exposes that lazily during credential
/// resolution. Sections are `[profile name]`, `[default]`, and
/// `[sso-session name]`; keys are `key = value`; `#` and `;` start comments.
fn parse_ini(content: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut sections: Vec<(String, HashMap<String, String>)> = Vec::new();
    let mut current: Option<(String, HashMap<String, String>)> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some(section) = current.take() {
                sections.push(section);
            }
            current = Some((header.trim().to_string(), HashMap::new()));
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            if let Some((_, map)) = current.as_mut() {
                // Nested sub-properties (indented under a key) are rare in
                // practice and irrelevant to us, so a flat map is enough.
                map.insert(key.trim().to_lowercase(), value.trim().to_string());
            }
        }
    }
    if let Some(section) = current {
        sections.push(section);
    }
    sections
}

/// Location of the shared config file, honouring `AWS_CONFIG_FILE`.
pub fn config_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AWS_CONFIG_FILE") {
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    dirs::home_dir().map(|h| h.join(".aws").join("config"))
}

fn credentials_path() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AWS_SHARED_CREDENTIALS_FILE") {
        if !p.is_empty() {
            return Some(std::path::PathBuf::from(p));
        }
    }
    dirs::home_dir().map(|h| h.join(".aws").join("credentials"))
}

/// Read every profile from `~/.aws/config` (plus any credentials-only profiles
/// from `~/.aws/credentials`), resolving `sso_session` references.
pub fn list_profiles() -> Result<Vec<AwsProfile>> {
    let mut profiles: Vec<AwsProfile> = Vec::new();
    let mut sso_sessions: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut raw_profiles: Vec<(String, HashMap<String, String>, bool)> = Vec::new();

    if let Some(path) = config_path() {
        if path.exists() {
            let content = std::fs::read_to_string(&path).map_err(|e| {
                Error::Internal(format!("Could not read {}: {e}", path.display()))
            })?;
            for (header, entries) in parse_ini(&content) {
                if let Some(session) = header.strip_prefix("sso-session ") {
                    sso_sessions.insert(session.trim().to_string(), entries);
                } else if let Some(name) = header.strip_prefix("profile ") {
                    raw_profiles.push((name.trim().to_string(), entries, false));
                } else if header == "default" {
                    raw_profiles.push(("default".to_string(), entries, false));
                }
            }
        }
    }

    // Profiles that only exist in ~/.aws/credentials carry static keys written
    // by an external credential manager.
    if let Some(path) = credentials_path() {
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                for (name, entries) in parse_ini(&content) {
                    if !raw_profiles.iter().any(|(n, _, _)| n == &name) {
                        raw_profiles.push((name, entries, true));
                    }
                }
            }
        }
    }

    for (name, entries, externally_managed) in raw_profiles {
        profiles.push(build_profile(
            name,
            &entries,
            &sso_sessions,
            externally_managed,
        ));
    }

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(profiles)
}

/// Resolve one parsed `[profile ...]` block into an [`AwsProfile`].
///
/// Split out so the tests can drive the real resolution rather than a
/// reimplementation of it — an inlined copy in the test module meant a bug
/// introduced here would not have failed a single test.
fn build_profile(
    name: String,
    entries: &HashMap<String, String>,
    sso_sessions: &HashMap<String, HashMap<String, String>>,
    externally_managed: bool,
) -> AwsProfile {
    let get = |k: &str| entries.get(k).cloned();

    let session_name = get("sso_session");
    let (kind, sso_start_url, sso_region) = if let Some(session) = &session_name {
        let session_cfg = sso_sessions.get(session);
        (
            ProfileKind::SsoSession,
            session_cfg.and_then(|c| c.get("sso_start_url").cloned()),
            session_cfg.and_then(|c| c.get("sso_region").cloned()),
        )
    } else if entries.contains_key("sso_start_url") {
        (
            ProfileKind::SsoLegacy,
            get("sso_start_url"),
            get("sso_region"),
        )
    } else if entries.contains_key("role_arn") {
        (ProfileKind::AssumeRole, None, None)
    } else {
        (ProfileKind::Static, None, None)
    };

    AwsProfile {
        name,
        kind,
        region: get("region"),
        sso_start_url,
        sso_region,
        sso_account_id: get("sso_account_id"),
        sso_role_name: get("sso_role_name"),
        sso_session: session_name,
        source_profile: get("source_profile"),
        externally_managed,
    }
}

/// An `[sso-session <name>]` block from `~/.aws/config`.
///
/// A session is the thing you actually sign into; the accounts and roles it
/// grants are discovered from AWS, not declared locally. pontifex can therefore
/// reach any account the session covers without a profile existing for it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SsoSession {
    pub name: String,
    pub start_url: String,
    pub sso_region: String,
    /// Profiles in `~/.aws/config` that reference this session, for context.
    pub profiles: Vec<String>,
}

/// Read every SSO session declared in `~/.aws/config`.
pub fn list_sso_sessions() -> Result<Vec<SsoSession>> {
    let Some(path) = config_path() else {
        return Ok(Vec::new());
    };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| Error::Internal(format!("Could not read {}: {e}", path.display())))?;

    let mut sessions: Vec<SsoSession> = Vec::new();
    let mut references: HashMap<String, Vec<String>> = HashMap::new();

    for (header, entries) in parse_ini(&content) {
        if let Some(name) = header.strip_prefix("sso-session ") {
            // A session missing its start URL cannot be signed into, so skip it
            // rather than offering a button that fails.
            let (Some(start_url), Some(sso_region)) = (
                entries.get("sso_start_url").cloned(),
                entries.get("sso_region").cloned(),
            ) else {
                continue;
            };
            sessions.push(SsoSession {
                name: name.trim().to_string(),
                start_url,
                sso_region,
                profiles: Vec::new(),
            });
        } else if let Some(profile) = header.strip_prefix("profile ") {
            if let Some(session) = entries.get("sso_session") {
                references
                    .entry(session.clone())
                    .or_default()
                    .push(profile.trim().to_string());
            }
        }
    }

    for session in &mut sessions {
        if let Some(profiles) = references.get(&session.name) {
            session.profiles = profiles.clone();
        }
    }

    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sessions)
}

pub fn get_sso_session(name: &str) -> Result<SsoSession> {
    list_sso_sessions()?
        .into_iter()
        .find(|s| s.name == name)
        .ok_or_else(|| {
            Error::NotFound(format!(
                "No [sso-session {name}] block found in ~/.aws/config"
            ))
        })
}

pub fn get_profile(name: &str) -> Result<AwsProfile> {
    list_profiles()?
        .into_iter()
        .find(|p| p.name == name)
        // Classified as Auth, not NotFound: the user's problem is that they
        // cannot authenticate, and the message names the usual cause — a
        // credential manager that deletes its profile when the session ends.
        .ok_or_else(|| {
            Error::Auth(format!(
                "Profile '{name}' is not in ~/.aws/config. Tools that write temporary \
                 credentials (Leapp, aws-vault) remove the profile when the session \
                 ends — re-activate it there, or point this environment at an SSO \
                 account instead."
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the real shape of the user's ~/.aws/config: one sso-session
    // profile, one legacy inline-SSO profile, and one source_profile chain.
    const SAMPLE: &str = r#"
[default]
region=us-east-2

[profile SAFE_PPE]
source_profile=nieto
region=us-east-1

[profile other-org-admin]
sso_start_url = https://other-org.awsapps.com/start
sso_region = us-east-1
sso_account_id = 999999999999
sso_role_name = OrganizationAdmin
region = us-east-1

[sso-session trajector]
sso_start_url = https://d-9067f2a2b3.awsapps.com/start
sso_region = us-east-2
sso_registration_scopes = sso:account:access

[profile claude-code-bedrock]
sso_session = trajector
sso_account_id = 111111111111
sso_role_name = ClaudeCodeBedrockAccess
region = us-east-2
"#;

    /// Resolve one profile out of `SAMPLE` through the real `build_profile`,
    /// so these tests fail when that resolution breaks.
    fn parse(name: &str) -> AwsProfile {
        let mut sessions: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut raw: Vec<(String, HashMap<String, String>)> = Vec::new();
        for (header, entries) in parse_ini(SAMPLE) {
            if let Some(s) = header.strip_prefix("sso-session ") {
                sessions.insert(s.trim().to_string(), entries);
            } else if let Some(n) = header.strip_prefix("profile ") {
                raw.push((n.trim().to_string(), entries));
            } else if header == "default" {
                raw.push(("default".to_string(), entries));
            }
        }
        let (n, entries) = raw.into_iter().find(|(n, _)| n == name).unwrap();
        build_profile(n, &entries, &sessions, false)
    }

    #[test]
    fn resolves_sso_session_indirection() {
        let p = parse("claude-code-bedrock");
        assert_eq!(p.kind, ProfileKind::SsoSession);
        assert_eq!(
            p.sso_start_url.as_deref(),
            Some("https://d-9067f2a2b3.awsapps.com/start")
        );
        assert_eq!(p.sso_region.as_deref(), Some("us-east-2"));
        // Cache key is the *session name*, not the URL.
        assert_eq!(p.token_cache_key().as_deref(), Some("trajector"));
    }

    #[test]
    fn handles_legacy_inline_sso() {
        let p = parse("other-org-admin");
        assert_eq!(p.kind, ProfileKind::SsoLegacy);
        // Legacy profiles key their cache on the start URL.
        assert_eq!(
            p.token_cache_key().as_deref(),
            Some("https://other-org.awsapps.com/start")
        );
    }

    #[test]
    fn classifies_non_sso_profiles() {
        assert_eq!(parse("SAFE_PPE").kind, ProfileKind::Static);
        assert_eq!(parse("SAFE_PPE").source_profile.as_deref(), Some("nieto"));
        assert_eq!(parse("default").kind, ProfileKind::Static);
        assert!(parse("default").token_cache_key().is_none());
    }

    #[test]
    fn detects_unfilled_template_profiles() {
        // The AWS docs' example, copy-pasted and never filled in. Selecting it
        // produces an unresolvable credential error and an SSO sign-in that
        // fails with InvalidRequestException.
        let template = AwsProfile {
            name: "other-org-dev".into(),
            kind: ProfileKind::SsoLegacy,
            region: Some("us-east-1".into()),
            sso_start_url: Some("https://YOUR-ORG.awsapps.com/start".into()),
            sso_region: Some("us-east-1".into()),
            sso_account_id: Some("<dev-account-id>".into()),
            sso_role_name: Some("Developer".into()),
            sso_session: None,
            source_profile: None,
            externally_managed: false,
        };
        assert!(template.is_placeholder());

        // Either half alone is enough to disqualify it.
        let bad_account = AwsProfile {
            sso_start_url: Some("https://real-org.awsapps.com/start".into()),
            ..template.clone()
        };
        assert!(bad_account.is_placeholder());

        let bad_url = AwsProfile {
            sso_account_id: Some("123456789012".into()),
            ..template.clone()
        };
        assert!(bad_url.is_placeholder());
    }

    #[test]
    fn accepts_real_profiles() {
        assert!(!parse("claude-code-bedrock").is_placeholder());
        assert!(!parse("other-org-admin").is_placeholder());
        // Non-SSO profiles have no account id or URL to judge.
        assert!(!parse("SAFE_PPE").is_placeholder());
        assert!(!parse("default").is_placeholder());
    }

    #[test]
    fn ignores_comments_and_blank_lines() {
        let sections = parse_ini("# comment\n\n[profile a]\n; another\nregion = us-east-1\n");
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].1.get("region").unwrap(), "us-east-1");
    }
}
