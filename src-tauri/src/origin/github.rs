//! The producer's side of the story, from GitHub.
//!
//! A code search across the organisation for the detail-type literal finds
//! the files that publish (or consume) the event. For each, the commit that
//! first put the literal there is found by binary search over the file's
//! history — GitHub has no pickaxe — and the pull request it landed in and
//! the repo's CODEOWNERS entry for the path say who to talk to.
//!
//! Authentication: a token stored in the keychain, or, failing that, the
//! GitHub CLI's own login (`gh auth token`), so someone with `gh` set up
//! needs no configuration at all.

use super::{Authorship, GithubOutcome, ProducerOrigin, Role, SkippedMatch};
use crate::error::{Error, Result};
use crate::keychain::Keychain;
use crate::logging::cat;
use futures::stream::StreamExt;
use serde_json::Value;

const SERVICE: &str = "pontifex.github";
const TOKEN_ENTRY: &str = "token";
const API: &str = "https://api.github.com";

/// The most a single lookup will read, however many matched. Everything the
/// ignore list lets through is read up to this; each file costs a dozen
/// calls and a couple of seconds, so a type named in hundreds of places is
/// read forty at a time.
pub const MAX_FILES: usize = 40;
/// The most a lookup will ever read, when asked to examine every match: one
/// search page, which is all one search returns.
pub const MAX_EXAMINE: usize = 100;
/// Commits read per file when looking for the introducing one.
const MAX_HISTORY: usize = 300;

// --- token ---------------------------------------------------------------

const KEYCHAIN: Keychain = Keychain::new(SERVICE);

pub fn store_token(token: &str) -> Result<()> {
    KEYCHAIN.write(TOKEN_ENTRY, token.trim())
}

pub fn clear_token() -> Result<()> {
    KEYCHAIN.clear(TOKEN_ENTRY)
}

fn stored_token() -> Result<Option<String>> {
    KEYCHAIN.read(TOKEN_ENTRY)
}

/// The GitHub CLI's token, when it is installed and logged in. A GUI app's
/// PATH is minimal, so the Homebrew locations are tried outright.
async fn gh_cli_token() -> Option<String> {
    for path in ["/opt/homebrew/bin/gh", "/usr/local/bin/gh", "gh"] {
        let Ok(output) = tokio::process::Command::new(path)
            .args(["auth", "token"])
            .output()
            .await
        else {
            continue;
        };
        if output.status.success() {
            let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !token.is_empty() {
                return Some(token);
            }
        }
    }
    None
}

/// Where a usable token comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TokenSource {
    Keychain,
    GhCli,
    None,
}

pub async fn token() -> Result<(Option<String>, TokenSource)> {
    if let Some(t) = stored_token()? {
        return Ok((Some(t), TokenSource::Keychain));
    }
    if let Some(t) = gh_cli_token().await {
        return Ok((Some(t), TokenSource::GhCli));
    }
    Ok((None, TokenSource::None))
}

// --- client --------------------------------------------------------------

pub struct Client {
    token: String,
}

impl Client {
    pub fn new(token: String) -> Self {
        Client { token }
    }

    async fn get(
        &self,
        path: &str,
        query: &[(&str, &str)],
        accept: &str,
    ) -> Result<reqwest::Response> {
        let url = if path.starts_with("http") {
            path.to_string()
        } else {
            format!("{API}{path}")
        };
        crate::jira::client::http()
            .get(&url)
            .query(query)
            .header("Accept", accept)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "pontifex")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|e| Error::Internal(format!("GitHub request failed: {e}")))
    }

    async fn json(&self, path: &str, query: &[(&str, &str)], context: &str) -> Result<Value> {
        let response = self.get(path, query, "application/vnd.github+json").await?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let message = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("message")?.as_str().map(str::to_string))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(match status.as_u16() {
                401 => Error::Auth(format!(
                    "GitHub rejected the token while {context}: {message}"
                )),
                403 | 429 => Error::Forbidden(format!("GitHub refused while {context}: {message}")),
                404 => Error::NotFound(format!("GitHub found nothing while {context}: {message}")),
                _ => Error::Internal(format!(
                    "GitHub failed while {context} ({status}): {message}"
                )),
            });
        }
        serde_json::from_str(&text)
            .map_err(|e| Error::Internal(format!("GitHub returned a response we cannot read: {e}")))
    }

    /// Who the token belongs to, for the settings check.
    pub async fn viewer(&self) -> Result<String> {
        let user = self.json("/user", &[], "checking the token").await?;
        Ok(user
            .get("login")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string())
    }

    /// Files in `org` containing the exact literal, best matches first.
    async fn search_code(
        &self,
        org: &str,
        literal: &str,
        wanted: usize,
    ) -> Result<(usize, Vec<(String, String, String)>)> {
        let q = format!("\"{literal}\" org:{org}");
        // One page holds a hundred; the ignore list and the bus repo's own
        // matches are dropped after, so ask for more than will be read.
        let per_page = (wanted * 2 + 8).min(100).to_string();
        let result = self
            .json(
                "/search/code",
                &[("q", q.as_str()), ("per_page", per_page.as_str())],
                "searching code",
            )
            .await?;
        let total = result
            .get("total_count")
            .and_then(Value::as_u64)
            .unwrap_or(0) as usize;
        let items = result
            .get("items")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let files = items
            .iter()
            .filter_map(|item| {
                let repo = item.get("repository")?.get("full_name")?.as_str()?;
                let path = item.get("path")?.as_str()?;
                let url = item.get("html_url")?.as_str()?;
                Some((repo.to_string(), path.to_string(), url.to_string()))
            })
            .collect();
        Ok((total, files))
    }

    /// The file's commits, oldest first, up to `MAX_HISTORY`.
    async fn history(&self, repo: &str, path: &str) -> Result<Vec<Value>> {
        let mut commits = Vec::new();
        for page in 1..=3 {
            let page_s = page.to_string();
            let batch = self
                .json(
                    &format!("/repos/{repo}/commits"),
                    &[
                        ("path", path),
                        ("per_page", "100"),
                        ("page", page_s.as_str()),
                    ],
                    "reading file history",
                )
                .await?;
            let items = batch.as_array().cloned().unwrap_or_default();
            let n = items.len();
            commits.extend(items);
            if n < 100 || commits.len() >= MAX_HISTORY {
                break;
            }
        }
        commits.reverse();
        Ok(commits)
    }

    /// The file's text at `sha`, or `None` when it did not exist there: the
    /// commit may predate the path (a rename).
    async fn content_at(&self, repo: &str, path: &str, sha: &str) -> Result<Option<String>> {
        let response = self
            .get(
                &format!("/repos/{repo}/contents/{path}"),
                &[("ref", sha)],
                "application/vnd.github.raw+json",
            )
            .await?;
        if response.status().as_u16() == 404 {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Error::Internal(format!(
                "GitHub failed while reading {path} at {sha} ({})",
                response.status()
            )));
        }
        Ok(Some(response.text().await.unwrap_or_default()))
    }

    async fn contains_at(&self, repo: &str, path: &str, sha: &str, literal: &str) -> Result<bool> {
        Ok(self
            .content_at(repo, path, sha)
            .await?
            .is_some_and(|text| text.contains(literal)))
    }

    /// The oldest commit in `history` whose file content carries the
    /// literal. Binary search: once added, a literal tends to stay, and the
    /// newest commit is known to have it because the search matched.
    async fn introducing_commit(
        &self,
        repo: &str,
        path: &str,
        history: &[Value],
        literal: &str,
    ) -> Result<Option<Value>> {
        if history.is_empty() {
            return Ok(None);
        }
        let sha_of = |c: &Value| {
            c.get("sha")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        // The newest commit is known to contain it: the caller read it.
        let (mut lo, mut hi) = (0usize, history.len() - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self
                .contains_at(repo, path, &sha_of(&history[mid]), literal)
                .await?
            {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        Ok(Some(history[lo].clone()))
    }

    async fn pull_for(&self, repo: &str, sha: &str) -> Result<Option<Pull>> {
        let pulls = self
            .json(
                &format!("/repos/{repo}/commits/{sha}/pulls"),
                &[],
                "finding the pull request",
            )
            .await?;
        Ok(pulls
            .as_array()
            .and_then(|list| list.first())
            .and_then(|pr| {
                Some(Pull {
                    number: pr.get("number")?.as_u64()?,
                    title: pr.get("title")?.as_str()?.to_string(),
                    url: pr.get("html_url")?.as_str()?.to_string(),
                    author: pr
                        .get("user")
                        .and_then(|u| u.get("login"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            }))
    }

    /// The repo's CODEOWNERS file, from the places GitHub looks.
    async fn codeowners(&self, repo: &str) -> Result<Option<String>> {
        for path in [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"] {
            let response = self
                .get(
                    &format!("/repos/{repo}/contents/{path}"),
                    &[],
                    "application/vnd.github.raw+json",
                )
                .await?;
            if response.status().is_success() {
                return Ok(Some(response.text().await.unwrap_or_default()));
            }
        }
        Ok(None)
    }
}

/// A pull request a commit landed in.
struct Pull {
    number: u64,
    title: String,
    url: String,
    author: Option<String>,
}

fn authorship_from_commit(commit: &Value, pull: Option<Pull>) -> Authorship {
    let inner = commit.get("commit").cloned().unwrap_or(Value::Null);
    let author = inner.get("author").cloned().unwrap_or(Value::Null);
    let subject = inner
        .get("message")
        .and_then(Value::as_str)
        .and_then(|m| m.lines().next())
        .unwrap_or("")
        .to_string();
    let login = commit
        .get("author")
        .and_then(|a| a.get("login"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let (pull_number, pull_title, pull_url, pull_author) = match pull {
        Some(p) => (Some(p.number), Some(p.title), Some(p.url), p.author),
        None => (super::pull_number_from_subject(&subject), None, None, None),
    };
    // A commit's author name is whatever git was told, and some machines
    // were told nonsense (`=`). Prefer a real name, then the account that
    // made the commit, then whoever opened the pull request.
    let name = author
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|n| n.chars().filter(|c| c.is_alphanumeric()).count() >= 2)
        .map(str::to_string)
        .or_else(|| login.clone())
        .or_else(|| pull_author.clone())
        .unwrap_or_else(|| "unknown".to_string());
    Authorship {
        author: name,
        email: author
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string),
        login,
        date: author
            .get("date")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        sha: commit
            .get("sha")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        subject,
        commit_url: commit
            .get("html_url")
            .and_then(Value::as_str)
            .map(str::to_string),
        pull_number,
        pull_title,
        pull_url,
        pull_author,
    }
}

/// Files that carry the literal without being where it is produced.
fn is_incidental(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("/test")
        || lower.contains("__tests__")
        || lower.contains("/mock")
        || lower.contains("fixture")
        || lower.ends_with(".md")
        || lower.contains("/docs/")
        || lower.ends_with(".snap")
}

/// CODEOWNERS entries for a path: the last matching rule wins, as on GitHub.
pub fn owners_for(codeowners: &str, path: &str) -> Vec<String> {
    let mut owners: Vec<String> = Vec::new();
    for line in codeowners.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(pattern) = parts.next() else {
            continue;
        };
        if codeowners_matches(pattern, path) {
            owners = parts.map(str::to_string).collect();
        }
    }
    owners
}

/// A workable subset of CODEOWNERS pattern rules: `*` and `**` globs, a
/// leading `/` anchoring to the repo root, a trailing `/` meaning "and
/// everything under it", and an unanchored pattern matching at any depth.
fn codeowners_matches(pattern: &str, path: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // As in gitignore: a leading slash, or a slash anywhere but the end,
    // anchors the pattern to the repo root; otherwise it matches at any depth.
    let anchored = pattern.starts_with('/')
        || pattern
            .trim_start_matches('/')
            .trim_end_matches('/')
            .contains('/');
    let mut pat = pattern.trim_start_matches('/').to_string();
    if pat.ends_with('/') {
        pat.push_str("**");
    }
    if !anchored && !pat.contains('*') {
        // A bare name: any path component.
        return path.split('/').any(|c| c == pat);
    }
    let candidates: Vec<&str> = if anchored {
        vec![path]
    } else {
        // Match against every suffix that starts at a component boundary.
        path.match_indices('/')
            .map(|(i, _)| &path[i + 1..])
            .chain(std::iter::once(path))
            .collect()
    };
    candidates
        .into_iter()
        .any(|candidate| glob_path(&pat, candidate) || glob_path(&format!("{pat}/**"), candidate))
}

/// Path glob where `*` stays within one component and `**` spans any.
fn glob_path(pattern: &str, path: &str) -> bool {
    fn go(p: &[&str], s: &[&str]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(&"**"), _) => go(&p[1..], s) || (!s.is_empty() && go(p, &s[1..])),
            (Some(seg), Some(part)) => {
                crate::watch::pattern::glob(seg, part) && go(&p[1..], &s[1..])
            }
            _ => false,
        }
    }
    let p: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let s: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    go(&p, &s)
}

/// What a file is doing with the event type, from its text and its place.
///
/// Publishing leaves a mark — a `PutEvents` call, a publish or emit helper.
/// Subscribing leaves another — a rule, an event pattern, a handler, a bus
/// configuration. A file with neither is a mention: a schema, a spec, a
/// constants table. The event source names the publishing service, so a
/// repo or path that carries the source's name tips a mention towards
/// publisher.
pub fn classify_role(repo: &str, path: &str, content: &str, source: &str) -> Role {
    let lower_path = path.to_lowercase();
    let publishes = [
        "putevents",
        "put_events",
        "puteventscommand",
        "publishevent",
        "publish_event",
        ".publish(",
        "emitevent",
        "emit_event",
        "sendevent",
        "send_event",
        "eventbridgeclient",
        "eventbridge.putevents",
    ];
    let consumes = [
        "eventpattern",
        "event_pattern",
        "addrule",
        "rule(",
        "rules:",
        "subscri",
        "handler",
        "onevent",
        "targets",
        "eventbus",
        "event_bus",
        "listener",
        "consumer",
    ];
    // A test or a README quoting `PutEvents` is not a publisher.
    if is_incidental(path) {
        return Role::Mention;
    }
    let text = content.to_lowercase();
    let mentions_in = |needles: &[&str], hay: &str| needles.iter().any(|n| hay.contains(n));
    let looks_like_rule_config = lower_path.ends_with(".yml")
        || lower_path.ends_with(".yaml")
        || lower_path.contains("event_bus")
        || lower_path.contains("eventbus")
        || lower_path.contains("busconfiguration");
    // Infrastructure code that wires rules for a type is subscribing to it,
    // even where it also grants something permission to put events.
    let infrastructure =
        lower_path.contains("stack") || lower_path.contains("/infra") || lower_path.contains("cdk");
    if looks_like_rule_config || (infrastructure && mentions_in(&consumes, &text)) {
        return Role::Consumer;
    }
    if mentions_in(&publishes, &text) {
        return Role::Publisher;
    }
    if mentions_in(&consumes, &text) {
        return Role::Consumer;
    }
    // The source's leading word — `messageCenter` of `messageCenter-outreachLegal`
    // — is the service that owns the event.
    let service: String = source
        .split(['-', '_', '.', ' '])
        .next()
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let squash = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect()
    };
    if service.len() >= 4 && (squash(repo).contains(&service) || squash(path).contains(&service)) {
        return Role::Publisher;
    }
    Role::Mention
}

/// The full producer lookup for one event type.
///
/// Files are independent of each other, so a few are examined at once;
/// GitHub's secondary limits are on concurrency, not on requests, so the
/// width stays small.
pub async fn lookup(
    client: &Client,
    org: &str,
    source: &str,
    detail_type: &str,
    skip_repo: Option<&str>,
    ignore: &[String],
    max_files: usize,
) -> (Vec<ProducerOrigin>, GithubOutcome) {
    const WIDTH: usize = 4;
    let max_files = max_files.clamp(1, MAX_EXAMINE);
    let (total, files) = match client.search_code(org, detail_type, max_files).await {
        Ok(found) => found,
        Err(e) => {
            return (
                Vec::new(),
                GithubOutcome::failed("error", Some(org.to_string()), e.to_string()),
            )
        }
    };
    // The ignore list is applied before anything is read: a test that names
    // the type costs as much to examine as the producer and says nothing.
    let mut skipped_files: Vec<SkippedMatch> = Vec::new();
    let files: Vec<(String, String, String)> = files
        .into_iter()
        .filter(|(repo, path, url)| {
            if Some(repo.as_str()) == skip_repo {
                return false;
            }
            match ignore
                .iter()
                .find(|pattern| codeowners_matches(pattern, path))
            {
                Some(pattern) => {
                    skipped_files.push(SkippedMatch {
                        repo: repo.clone(),
                        path: path.clone(),
                        url: url.clone(),
                        pattern: pattern.clone(),
                    });
                    false
                }
                None => true,
            }
        })
        .collect();
    let skipped = skipped_files.len();
    let files: Vec<(String, String, String)> = files.into_iter().take(max_files).collect();

    // CODEOWNERS once per repository, all at once, before the files.
    let repos: std::collections::BTreeSet<String> =
        files.iter().map(|(repo, _, _)| repo.clone()).collect();
    let codeowners_by_repo: std::collections::HashMap<String, Option<String>> =
        futures::stream::iter(repos)
            .map(|repo| async move {
                let owners = client.codeowners(&repo).await.unwrap_or(None);
                (repo, owners)
            })
            .buffer_unordered(WIDTH)
            .collect()
            .await;

    let mut producers: Vec<(usize, ProducerOrigin)> = futures::stream::iter(files)
        .enumerate()
        .map(|(index, (repo, path, url))| {
            let codeowners = codeowners_by_repo.get(&repo).cloned().flatten();
            async move {
                let producer = examine(
                    client,
                    repo,
                    path,
                    url,
                    source,
                    detail_type,
                    codeowners.as_deref(),
                )
                .await;
                (index, producer)
            }
        })
        .buffer_unordered(WIDTH)
        .collect()
        .await;
    // Back into search order, so equal sort keys come out the same way twice.
    producers.sort_by_key(|(index, _)| *index);
    let mut producers: Vec<ProducerOrigin> = producers.into_iter().map(|(_, p)| p).collect();
    // Producers first, then by when the literal first appeared.
    producers.sort_by(|a, b| {
        a.incidental.cmp(&b.incidental).then_with(|| {
            let da = a
                .introduced
                .as_ref()
                .map(|i| i.date.as_str())
                .unwrap_or("~");
            let db = b
                .introduced
                .as_ref()
                .map(|i| i.date.as_str())
                .unwrap_or("~");
            da.cmp(db)
        })
    });
    (
        producers,
        GithubOutcome {
            status: "ok".into(),
            message: None,
            org: Some(org.to_string()),
            total_matches: total,
            skipped,
            skipped_files,
        },
    )
}

/// One matching file: what it does with the type, who first put the literal
/// there, and who owns it now.
async fn examine(
    client: &Client,
    repo: String,
    path: String,
    url: String,
    source: &str,
    detail_type: &str,
    codeowners: Option<&str>,
) -> ProducerOrigin {
    // The file as it is now: for classifying its role, and as the
    // known-good end of the binary search over its history.
    let head = client
        .content_at(&repo, &path, "HEAD")
        .await
        .unwrap_or(None)
        .unwrap_or_default();
    let role = classify_role(&repo, &path, &head, source);
    let introduced = if !head.contains(detail_type) {
        // The search index is ahead of or behind the default branch.
        None
    } else {
        match client.history(&repo, &path).await {
            Ok(history) => match client
                .introducing_commit(&repo, &path, &history, detail_type)
                .await
            {
                Ok(Some(commit)) => {
                    let sha = commit
                        .get("sha")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let pull = client.pull_for(&repo, &sha).await.unwrap_or(None);
                    Some(authorship_from_commit(&commit, pull))
                }
                Ok(None) => None,
                Err(e) => {
                    lwarn!(
                        cat::ORIGIN,
                        "{repo}/{path}: could not find the introducing commit: {e}"
                    );
                    None
                }
            },
            Err(e) => {
                lwarn!(cat::ORIGIN, "{repo}/{path}: could not read history: {e}");
                None
            }
        }
    };
    let owners = codeowners.map(|c| owners_for(c, &path)).unwrap_or_default();
    let incidental = is_incidental(&path);
    // What a consumer does with the event, from the same text its role was
    // read from. A publisher's accesses are of the payload it is building,
    // and a test's are of a fixture; neither says what breaks downstream,
    // so neither gets reads — which is how the grader knows who counts.
    let reads = if role == Role::Publisher || incidental {
        Vec::new()
    } else {
        super::reads::field_reads(&head)
    };
    ProducerOrigin {
        incidental,
        role,
        repo,
        path,
        url,
        introduced,
        owners,
        reads,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codeowners_last_match_wins_with_github_pattern_rules() {
        let file = "\
# comment
* @org/everyone
/packages/functions/ @org/functions
*.py @org/python
docs/ @org/writers
";
        assert_eq!(owners_for(file, "README.md"), vec!["@org/everyone"]);
        assert_eq!(
            owners_for(file, "packages/functions/src/events/eventType.ts"),
            vec!["@org/functions"]
        );
        assert_eq!(owners_for(file, "apps/api/main.py"), vec!["@org/python"]);
        assert_eq!(owners_for(file, "apps/docs/guide.md"), vec!["@org/writers"]);
        assert!(owners_for("", "x").is_empty());
    }

    #[test]
    fn roles_are_read_off_the_code_and_the_source_name() {
        assert_eq!(
            classify_role(
                "org/x",
                "src/a.ts",
                "await client.send(new PutEventsCommand(..))",
                "s-y"
            ),
            Role::Publisher
        );
        assert_eq!(
            classify_role(
                "org/x",
                "stacks/EventBusStack.ts",
                "new Rule(this, 'r', { eventPattern })",
                "s-y"
            ),
            Role::Consumer
        );
        assert_eq!(
            classify_role(
                "org/milo-ols-bus",
                "event_bus_configs/milo.yml",
                "detail-type: [x]",
                "s-y"
            ),
            Role::Consumer
        );
        assert_eq!(
            classify_role(
                "org/message-center-service",
                "src/constants.ts",
                "export const T = 'x'",
                "messageCenter-outreachLegal"
            ),
            Role::Publisher
        );
        assert_eq!(
            classify_role(
                "org/other",
                "openapi.yaml.json",
                "{}",
                "messageCenter-outreachLegal"
            ),
            Role::Mention
        );
        assert_eq!(
            classify_role(
                "org/comms",
                "README.md",
                "call PutEvents with it",
                "messageCenter-x"
            ),
            Role::Mention
        );
        assert_eq!(
            classify_role(
                "org/b2b",
                "stacks/EventBusStack.ts",
                "new Rule(this, 'r', { eventPattern }); grant putEvents",
                "s-y"
            ),
            Role::Consumer
        );
    }

    #[test]
    fn the_default_ignore_list_catches_tests_specs_and_docs_but_not_producers() {
        let ignore = crate::settings::default_origin_ignore();
        let hit = |path: &str| ignore.iter().any(|p| codeowners_matches(p, path));
        assert!(hit("packages/functions/src/events/eventType.test.ts"));
        assert!(hit("apps/api/handler.spec.ts"));
        assert!(hit("apps/message-center-service/scripts/openapi-spec.ts"));
        assert!(hit("apps/x/packages/tests/functions/processor/test_a.py"));
        assert!(hit("README.md"));
        assert!(!hit("packages/functions/src/events/eventType.ts"));
        assert!(!hit("event_bus_configs/milo_outreachlegal.yml"));
        assert!(!hit(
            "apps/message-center-service/packages/functions/src/helpers/constants.ts"
        ));
    }

    #[test]
    fn incidental_files_are_recognised() {
        assert!(is_incidental("tests/api/test_comms.py"));
        assert!(is_incidental("README.md"));
        assert!(!is_incidental("packages/functions/src/events/eventType.ts"));
    }
}
