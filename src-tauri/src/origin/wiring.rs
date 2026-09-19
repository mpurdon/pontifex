//! The bus repo's side of the story, from the local checkout.
//!
//! `git log -S<literal>` (the pickaxe) finds the commits that changed the
//! number of occurrences of a string; the oldest is where the event type was
//! first wired into the bus configuration. The repo's schema directories are
//! excluded: they are an export of the registry, not history.

use super::{pull_number_from_subject, Authorship};
use crate::error::Result;
use std::path::{Path, PathBuf};

/// The oldest commit in the checkout that mentions `literal`, outside the
/// schema exports. `None` when the checkout is missing or has no mention.
pub async fn first_mention(repo: &Path, literal: &str) -> Result<Option<Authorship>> {
    if !repo.join(".git").exists() {
        return Ok(None);
    }
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args([
            "log",
            "--reverse",
            "-S",
            literal,
            "--format=%H%x1f%an%x1f%ae%x1f%aI%x1f%s",
            "--",
            ".",
            ":!schemas",
            ":!schemas-simplified",
        ])
        .output()
        .await?;
    if !output.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let Some(line) = text.lines().next() else {
        return Ok(None);
    };
    let mut parts = line.split('\x1f');
    let (Some(sha), Some(author), Some(email), Some(date), Some(subject)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Ok(None);
    };
    Ok(Some(Authorship {
        author: author.to_string(),
        email: Some(email.to_string()),
        login: None,
        date: date.to_string(),
        sha: sha.to_string(),
        subject: subject.to_string(),
        commit_url: None,
        pull_number: pull_number_from_subject(subject),
        pull_title: None,
        pull_url: None,
        pull_author: None,
    }))
}

/// `owner/repo` from the checkout's `origin` remote, for building links.
///
/// Remembered per checkout: the remote of a clone does not change under a
/// running app, and this is asked on every graded check, so the `git`
/// spawn is paid once rather than on every keystroke of a live re-check.
pub async fn github_slug(repo: &Path) -> Option<String> {
    static SLUGS: std::sync::Mutex<Option<std::collections::HashMap<PathBuf, Option<String>>>> =
        std::sync::Mutex::new(None);
    if let Some(known) = SLUGS
        .lock()
        .ok()
        .and_then(|s| s.as_ref()?.get(repo).cloned())
    {
        return known;
    }
    let slug = read_github_slug(repo).await;
    if let Ok(mut slugs) = SLUGS.lock() {
        slugs
            .get_or_insert_with(Default::default)
            .insert(repo.to_path_buf(), slug.clone());
    }
    slug
}

async fn read_github_slug(repo: &Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["remote", "get-url", "origin"])
        .output()
        .await
        .ok()?;
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    slug_from_remote(&url)
}

/// `git@github.com:org/repo.git` or `https://github.com/org/repo(.git)` → `org/repo`.
pub fn slug_from_remote(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let slug = rest.trim_end_matches('/').trim_end_matches(".git");
    (slug.matches('/').count() == 1).then(|| slug.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls_become_slugs() {
        assert_eq!(
            slug_from_remote("git@github.com:team-and-tech/global-event-bus.git").as_deref(),
            Some("team-and-tech/global-event-bus")
        );
        assert_eq!(
            slug_from_remote("https://github.com/team-and-tech/global-event-bus").as_deref(),
            Some("team-and-tech/global-event-bus")
        );
        assert_eq!(slug_from_remote("https://gitlab.com/x/y"), None);
    }
}
