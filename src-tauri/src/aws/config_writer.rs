use crate::aws::sso_credentials::SsoTarget;
use crate::error::{Error, Result};
use std::path::PathBuf;

/// Render an `[sso-session]`-backed profile block.
///
/// Deliberately the modern `sso_session` form rather than inline SSO keys, so
/// the exported profile reuses the session the user already signs into and
/// stays in step if its start URL ever changes.
///
/// A provenance comment is written above the block. A `gm-` name prefix helps
/// you spot pontifex's profiles in a picker, but names get edited and prefixes
/// collide — the comment is what makes "who wrote this, and is it safe to
/// delete" answerable even after a rename.
fn render_profile(name: &str, target: &SsoTarget, region: &str) -> String {
    let account = target
        .account_name
        .as_deref()
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();

    format!(
        "\n# written by pontifex — {account_id}{account} as {role}\n\
         [profile {name}]\n\
         sso_session = {session}\n\
         sso_account_id = {account_id}\n\
         sso_role_name = {role}\n\
         region = {region}\n",
        session = target.session,
        account_id = target.account_id,
        role = target.role_name,
    )
}

fn config_path() -> Result<PathBuf> {
    crate::aws::profiles::config_path()
        .ok_or_else(|| Error::Internal("No home directory".into()))
}

/// True when `~/.aws/config` already declares this profile.
fn profile_exists(content: &str, name: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed == format!("[profile {name}]") || (name == "default" && trimmed == "[default]")
    })
}

/// Append a profile for an SSO target to `~/.aws/config`.
///
/// Only ever called from an explicit "Export as AWS profile" action — pontifex
/// resolves credentials in-app and never needs this itself. Appends rather than
/// rewrites, so a config managed by another tool is left intact, and refuses to
/// clobber an existing profile of the same name.
pub fn export_sso_profile(name: &str, target: &SsoTarget, region: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::Invalid("Profile name cannot be empty".into()));
    }
    // Section headers are whitespace-delimited; a name with spaces or brackets
    // would produce a block no AWS tool can parse.
    if name.contains(|c: char| c.is_whitespace() || c == '[' || c == ']') {
        return Err(Error::Invalid(
            "Profile name cannot contain spaces or brackets".into(),
        ));
    }

    let path = config_path()?;
    let existing = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };

    if profile_exists(&existing, name) {
        return Err(Error::Invalid(format!(
            "~/.aws/config already has a profile named '{name}' — pick another name"
        )));
    }

    // The session must exist locally, or the exported profile is unusable.
    crate::aws::profiles::get_sso_session(&target.session)?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(&render_profile(name, target, region));

    // Write-then-rename: a crash mid-write must not truncate a file other
    // tools depend on.
    let tmp = path.with_extension("pontifex-tmp");
    std::fs::write(&tmp, &updated)?;
    std::fs::rename(&tmp, &path)?;

    Ok(path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> SsoTarget {
        SsoTarget {
            session: "trajector".into(),
            account_id: "111111111111".into(),
            role_name: "AdministratorAccess".into(),
            account_name: Some("Global Event Bus Development".into()),
        }
    }

    #[test]
    fn renders_a_session_backed_profile() {
        let block = render_profile("gm-dev", &target(), "us-east-2");
        assert!(block.contains("[profile gm-dev]"));
        assert!(block.contains("sso_session = trajector"));
        assert!(block.contains("sso_account_id = 111111111111"));
        assert!(block.contains("sso_role_name = AdministratorAccess"));
        assert!(block.contains("region = us-east-2"));
        // Inline SSO keys would duplicate the session's config.
        assert!(!block.contains("sso_start_url"));
    }

    #[test]
    fn records_where_the_profile_came_from() {
        let block = render_profile("gm-dev", &target(), "us-east-2");
        // Provenance survives a rename, unlike a name prefix.
        assert!(block.contains("# written by pontifex"));
        assert!(block.contains("111111111111 (Global Event Bus Development)"));
        assert!(block.contains("as AdministratorAccess"));
        // The comment must precede the section header, or it would be read as
        // part of the previous profile.
        assert!(block.find("# written by pontifex") < block.find("[profile gm-dev]"));
    }

    #[test]
    fn omits_the_account_name_when_there_is_none() {
        let anonymous = SsoTarget {
            account_name: None,
            ..target()
        };
        let block = render_profile("gm-dev", &anonymous, "us-east-2");
        assert!(block.contains("# written by pontifex — 111111111111 as"));
    }

    #[test]
    fn detects_existing_profiles() {
        let config = "[default]\nregion=us-east-2\n\n[profile gm-dev]\nregion = us-east-2\n";
        assert!(profile_exists(config, "gm-dev"));
        assert!(profile_exists(config, "default"));
        assert!(!profile_exists(config, "gm-stg"));
        // Must not match a substring of another profile's name.
        assert!(!profile_exists(config, "gm"));
    }

    #[test]
    fn rejects_names_that_would_corrupt_the_ini() {
        for bad in ["", "  ", "has space", "brack[et"] {
            assert!(
                export_sso_profile(bad, &target(), "us-east-2").is_err(),
                "should reject {bad:?}"
            );
        }
    }
}
