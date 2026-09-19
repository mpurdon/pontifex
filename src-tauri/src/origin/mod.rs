//! Where an event type came from: who first published it, in which repo,
//! in which pull request, and who owns that code now.
//!
//! The registry says *what* an event looks like and nothing about *who* — so
//! when a payload is wrong, the schema is silent on whom to ask. Two sources
//! answer that:
//!
//! - [`github`]: a code search across the organisation for the detail-type
//!   literal finds the producer's source; the first commit that introduced
//!   the literal in that file, its pull request, and the repo's CODEOWNERS
//!   give the person, the moment and the team.
//! - [`wiring`]: the local `global-event-bus` checkout, searched with git's
//!   pickaxe for the same literal, gives the commit and pull request that
//!   wired the type into the bus.
//!
//! Results change slowly and cost API calls, so they are cached on disk for
//! a month; a lookup can be forced fresh.

pub mod github;
pub mod wiring;

use crate::ttl_cache::{TtlCache, THIRTY_DAYS_MS};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A person and a moment, from a commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Authorship {
    pub author: String,
    pub email: Option<String>,
    /// GitHub login, when the commit is linked to an account.
    pub login: Option<String>,
    /// RFC 3339.
    pub date: String,
    pub sha: String,
    pub subject: String,
    pub commit_url: Option<String>,
    pub pull_number: Option<u64>,
    pub pull_title: Option<String>,
    pub pull_url: Option<String>,
    /// The pull request's author, when it differs from the commit's.
    #[serde(default)]
    pub pull_author: Option<String>,
}

/// One place in the organisation's code that carries the event type.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProducerOrigin {
    pub repo: String,
    pub path: String,
    pub url: String,
    /// The commit that first put the literal in this file, when it could be
    /// found within the history read.
    pub introduced: Option<Authorship>,
    /// CODEOWNERS entries for the path, e.g. `@team-and-tech/client-profile`.
    pub owners: Vec<String>,
    /// True when the file looks like a test, fixture or document rather than
    /// the producer itself; shown last.
    pub incidental: bool,
    /// What the file does with the event type.
    #[serde(default)]
    pub role: Role,
}

/// What a file that carries the event type is doing with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Names it without acting on it: a schema, a spec, a constants table.
    #[default]
    Mention,
    /// Subscribes to it: a rule, an event pattern, a handler.
    Consumer,
    /// Puts it on the bus.
    Publisher,
}

/// How the GitHub side went.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubOutcome {
    /// `ok`, `unconfigured` (no organisation or token), or `error`.
    pub status: String,
    pub message: Option<String>,
    pub org: Option<String>,
    /// Matches the search reported, before the ignore list.
    pub total_matches: usize,
    /// Matches the ignore list kept from being examined.
    #[serde(default)]
    pub skipped: usize,
    /// Which ones, and by which pattern — so a pattern that is too broad
    /// can be seen catching something it should not.
    #[serde(default)]
    pub skipped_files: Vec<SkippedMatch>,
}

impl GithubOutcome {
    /// Nothing was searched: `status` says whether that is `unconfigured`
    /// or an `error`, and `message` says what to do about it.
    pub fn failed(status: &str, org: Option<String>, message: String) -> Self {
        GithubOutcome {
            status: status.into(),
            message: Some(message),
            org,
            total_matches: 0,
            skipped: 0,
            skipped_files: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedMatch {
    pub repo: String,
    pub path: String,
    pub url: String,
    /// The ignore-list entry that matched.
    pub pattern: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventOrigin {
    pub source: String,
    pub detail_type: String,
    /// The commit that wired the type into the bus repo, from the local
    /// checkout; `None` when the checkout is unset or has no mention.
    pub wiring: Option<Authorship>,
    pub wiring_repo_url: Option<String>,
    pub producers: Vec<ProducerOrigin>,
    pub github: GithubOutcome,
    /// When this answer was computed, epoch ms.
    pub cached_at: i64,
}

/// Cached answers, keyed by organisation and event type.
pub struct OriginCache(TtlCache<EventOrigin>);

impl OriginCache {
    pub fn load(cache_dir: &Path) -> Self {
        OriginCache(TtlCache::load(
            cache_dir.join("origins.json"),
            THIRTY_DAYS_MS,
            |o| o.cached_at,
            "origins",
        ))
    }

    fn key(org: Option<&str>, source: &str, detail_type: &str) -> String {
        format!("{}|{source}@{detail_type}", org.unwrap_or(""))
    }

    pub async fn get(
        &self,
        org: Option<&str>,
        source: &str,
        detail_type: &str,
    ) -> Option<EventOrigin> {
        self.0.get(&Self::key(org, source, detail_type)).await
    }

    pub async fn put(&self, org: Option<&str>, origin: EventOrigin) {
        self.0
            .put(Self::key(org, &origin.source, &origin.detail_type), origin)
            .await
    }
}

/// Pull-request number out of a merge subject or a squash subject.
///
/// `Merge pull request #181 from …` and `feat: add thing (#42)` are the two
/// shapes GitHub writes; anything else has no number.
pub fn pull_number_from_subject(subject: &str) -> Option<u64> {
    if let Some(rest) = subject.strip_prefix("Merge pull request #") {
        return rest
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse().ok());
    }
    // Squash merges end the subject with `(#N)`; a number anywhere else is
    // a reference, not the pull request this commit came from.
    let trimmed = subject.trim_end().strip_suffix(')')?;
    let open = trimmed.rfind("(#")?;
    trimmed[open + 2..].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pull_numbers_from_both_subject_shapes() {
        assert_eq!(
            pull_number_from_subject("Merge pull request #181 from team-and-tech/revert-180"),
            Some(181)
        );
        assert_eq!(
            pull_number_from_subject("feat: add cadence events (#42)"),
            Some(42)
        );
        assert_eq!(
            pull_number_from_subject("fix: rename window for clarity"),
            None
        );
        assert_eq!(pull_number_from_subject("chore (#12) and more"), None);
    }
}
