//! Filing producer bugs, from the UI's point of view.
//!
//! Every write goes through a preview the user confirms, and the preview is
//! rendered by the same code that files it — so there is no gap between the
//! ticket you read and the ticket that gets created. The only thing the
//! preview reaches Jira for is the duplicate check, and it renders without
//! that when Jira is unreachable.

use crate::error::{Error, Result};
use crate::jira::client::{JiraClient, Site};
use crate::jira::issues::{self, FiledTicket, JiraProject};
use crate::jira::ticket::{self, TicketContext, TicketDraft};
use crate::jira::{oauth, tokens};
use crate::logging::cat;
use crate::schema::events::Issue;
use crate::state::AppState;
use serde::{Deserialize, Serialize};
use tauri::State;

/// Duplicate checks in flight at once during a bulk run.
const DEDUPE_CONCURRENCY: usize = 8;
use tauri_plugin_opener::OpenerExt;

/// Where the integration stands, in one object the Settings screen can render.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraStatus {
    /// An app is registered — a client id and secret exist.
    pub configured: bool,
    /// There is a live grant.
    pub connected: bool,
    /// The site tickets would be filed on.
    pub site: Option<SiteSummary>,
    /// Why it is not usable, when it is not.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteSummary {
    pub cloud_id: String,
    pub name: Option<String>,
    pub url: Option<String>,
}

/// Read the configured client id and secret, or say which half is missing.
async fn credentials(state: &State<'_, AppState>) -> Result<(String, String)> {
    let settings = state.settings_snapshot().await;
    let client_id = settings.jira.client_id.trim().to_string();
    if client_id.is_empty() {
        return Err(Error::Invalid(
            "No Jira app is configured. Add the client ID and secret from the Atlassian \
             developer console in Settings → Jira."
                .into(),
        ));
    }
    let secret = tokens::load_client_secret()?.ok_or_else(|| {
        Error::Invalid(
            "The Jira client secret is missing from the keychain. Re-enter it in Settings → Jira."
                .into(),
        )
    })?;
    Ok((client_id, secret))
}

async fn client(state: &State<'_, AppState>) -> Result<JiraClient> {
    let (client_id, secret) = credentials(state).await?;
    JiraClient::connect(&client_id, &secret).await
}

/// Where things stand, reporting a keychain refusal rather than failing on it.
///
/// Status is read after every other action, so letting it propagate an error
/// made an unrelated hiccup look like the action failed: saving the client
/// secret *worked*, then the read-back was refused, and the UI reported the
/// save as broken while the item sat in the keychain.
#[tauri::command]
pub async fn jira_status(state: State<'_, AppState>) -> Result<JiraStatus> {
    let settings = state.settings_snapshot().await;

    let secret = tokens::load_client_secret();
    let tokens = tokens::load_tokens();
    let keychain_error = secret
        .as_ref()
        .err()
        .or(tokens.as_ref().err())
        .map(ToString::to_string);

    if let Some(detail) = keychain_error {
        return Ok(JiraStatus {
            // Unknown, not false — but everything downstream needs the secret,
            // so there is nothing to offer until the keychain answers.
            configured: false,
            connected: false,
            site: None,
            detail: Some(detail),
        });
    }

    let configured = settings.jira.is_configured() && secret.unwrap_or(None).is_some();
    let stored = tokens.unwrap_or(None);

    Ok(JiraStatus {
        configured,
        connected: stored.is_some(),
        site: stored.as_ref().and_then(|t| {
            t.cloud_id.as_ref().map(|id| SiteSummary {
                cloud_id: id.clone(),
                name: t.site_name.clone(),
                url: t.site_url.clone(),
            })
        }),
        detail: match (configured, &stored) {
            (false, _) => Some(
                "Register an app in the Atlassian developer console, then add its client ID and \
                 secret here."
                    .into(),
            ),
            (true, None) => Some("Not connected yet.".into()),
            (true, Some(t)) if t.cloud_id.is_none() => {
                Some("Connected, but no site chosen yet.".into())
            }
            _ => None,
        },
    })
}

/// Store the app registration. The secret goes to the keychain, never to disk.
#[tauri::command]
pub async fn set_jira_app(
    state: State<'_, AppState>,
    client_id: String,
    client_secret: Option<String>,
    callback_url: Option<String>,
) -> Result<JiraStatus> {
    // Rejected before it is stored: a callback we cannot listen on would sign
    // in successfully and then hang waiting for a redirect that never arrives.
    let callback = callback_url
        .map(|url| oauth::parse_callback(&url))
        .transpose()?;

    {
        let mut settings = state.settings.write().await;
        settings.jira.client_id = client_id.trim().to_string();
        if let Some(callback) = callback {
            settings.jira.callback_url = callback.url;
        }
    }
    state.persist().await?;

    // An omitted secret means "leave the stored one alone", so re-saving the
    // client id does not wipe a secret the user cannot see to re-enter.
    if let Some(secret) = client_secret {
        let secret = secret.trim();
        if secret.is_empty() {
            tokens::clear_client_secret()?;
        } else {
            tokens::save_client_secret(secret)?;
        }
    }

    jira_status(state).await
}

/// The whole sign-in: open the browser, catch the redirect, exchange the code.
///
/// One command rather than begin/complete, because the loopback listener has
/// to outlive the browser trip and nothing else needs to happen in between.
#[tauri::command]
pub async fn jira_connect(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Vec<Site>> {
    let (client_id, secret) = credentials(&state).await?;
    let callback = oauth::parse_callback(&state.settings_snapshot().await.jira.callback_url)?;

    // Bind before opening the browser: a consent that completes instantly must
    // not arrive before anything is listening.
    let listeners = oauth::bind(&callback).await?;
    let csrf = uuid::Uuid::new_v4().to_string();
    let url = oauth::authorize_url(&client_id, &callback.url, &csrf)?;

    log::info!(target: cat::JIRA, "opening Jira consent in the browser");
    app.opener()
        .open_url(url.clone(), None::<&str>)
        .map_err(|e| Error::Internal(format!("Cannot open the browser for Jira sign-in: {e}")))?;

    let code = oauth::listen_for_code(listeners, &csrf).await?;
    let stored = oauth::exchange_code(&client_id, &secret, &code, &callback.url).await?;
    tokens::save_tokens(&stored)?;

    // Sites come back from the grant itself, so the picker never guesses.
    let client = JiraClient::connect(&client_id, &secret).await?;
    let sites = client.sites().await?;

    // One site is not a choice; making the user confirm it would be ceremony.
    if let Some(only) = sites.first().filter(|_| sites.len() == 1) {
        select_site_inner(only.id.clone(), Some(only.clone()))?;
    }

    log::info!(target: cat::JIRA, "connected to Jira, {} site(s) available", sites.len());
    Ok(sites)
}

fn select_site_inner(cloud_id: String, site: Option<Site>) -> Result<()> {
    let mut stored =
        tokens::load_tokens()?.ok_or_else(|| Error::Auth("Not connected to Jira.".into()))?;
    stored.cloud_id = Some(cloud_id);
    stored.site_url = site.as_ref().map(|s| s.url.clone());
    stored.site_name = site.as_ref().map(|s| s.name.clone());
    tokens::save_tokens(&stored)
}

/// Read an authorization URL copied from the developer console.
///
/// Pure — nothing is stored until the values are on screen and the user saves
/// them. The point is to stop the client id and callback being transcribed by
/// hand into fields where a single wrong character fails at consent time with
/// a message about none of this.
#[tauri::command]
pub fn parse_jira_authorize_url(url: String) -> Result<oauth::AuthorizeUrlParts> {
    oauth::parse_authorize_url(&url)
}

#[tauri::command]
pub async fn jira_sites(state: State<'_, AppState>) -> Result<Vec<Site>> {
    client(&state).await?.sites().await
}

#[tauri::command]
pub async fn select_jira_site(state: State<'_, AppState>, cloud_id: String) -> Result<JiraStatus> {
    let sites = client(&state).await?.sites().await?;
    let site = sites.into_iter().find(|s| s.id == cloud_id);
    select_site_inner(cloud_id, site)?;
    jira_status(state).await
}

#[tauri::command]
pub async fn jira_disconnect(state: State<'_, AppState>) -> Result<JiraStatus> {
    tokens::clear_tokens()?;
    jira_status(state).await
}

#[tauri::command]
pub async fn jira_projects(state: State<'_, AppState>) -> Result<Vec<JiraProject>> {
    issues::list_projects(&client(&state).await?).await
}

#[tauri::command]
pub async fn jira_issue_types(
    state: State<'_, AppState>,
    project_key: String,
) -> Result<Vec<String>> {
    issues::issue_types(&client(&state).await?, &project_key).await
}

/// What a project demands before it will accept a ticket.
///
/// Asked of Jira rather than assumed: a project can make any field mandatory,
/// and the first sign of one is otherwise a rejected create naming a custom
/// field id.
#[tauri::command]
pub async fn jira_required_fields(
    state: State<'_, AppState>,
    project_key: String,
    issue_type: String,
) -> Result<Vec<issues::RequiredField>> {
    issues::required_fields(&client(&state).await?, &project_key, &issue_type).await
}

/// Remember what to send for a project's mandatory fields.
///
/// Per project rather than per rule: the requirement belongs to the project,
/// and answering "Discovery Environment" once should cover every ticket that
/// lands there.
#[tauri::command]
pub async fn set_jira_field_defaults(
    state: State<'_, AppState>,
    project_key: String,
    fields: std::collections::BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    {
        let mut settings = state.settings.write().await;
        let entry = settings
            .jira
            .field_defaults
            .entry(project_key.trim().to_string())
            .or_default();
        merge_fields(entry, fields);
    }
    state.persist().await
}

/// Apply field answers, treating a cleared value as a removal.
///
/// Null is not a value Jira accepts as "leave this alone" — it reads as "unset
/// this" and is often refused — so a cleared answer drops the entry instead.
fn merge_fields(
    into: &mut std::collections::BTreeMap<String, serde_json::Value>,
    updates: impl IntoIterator<Item = (String, serde_json::Value)>,
) {
    for (id, value) in updates {
        if value.is_null() {
            into.remove(&id);
        } else {
            into.insert(id, value);
        }
    }
}

/// One issue plus where it was found: everything a ticket needs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketRequest {
    pub issue: Issue,
    pub context: TicketContext,
}

/// A rendered ticket, and whether one already exists for it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketPreview {
    #[serde(flatten)]
    pub draft: TicketDraft,
    /// An open ticket already filed for this exact problem.
    pub existing: Option<FiledTicket>,
}

/// Fill in who publishes the event, from the origin cache, when a lookup
/// has run for the type. Never a network call: a ticket must file with or
/// without the origin story, and the cache is the only source cheap enough
/// to consult on every preview.
async fn fill_origin(state: &AppState, org: Option<&str>, context: &mut TicketContext) {
    if context.origin.is_some() {
        return;
    }
    context.origin =
        crate::commands::origin::cached_origin(state, org, &context.source, &context.detail_type)
            .await
            .and_then(|origin| ticket::TicketOrigin::from_origin(&origin));
}

/// Render a ticket without filing it. No network unless a dedupe check is possible.
#[tauri::command]
pub async fn preview_jira_ticket(
    state: State<'_, AppState>,
    request: TicketRequest,
) -> Result<TicketPreview> {
    let settings = state.settings_snapshot().await;
    let mut request = request;
    let org = crate::commands::origin::resolve_org(&settings).await;
    fill_origin(&state, org.as_deref(), &mut request.context).await;
    let draft = ticket::draft(&request.issue, &request.context, &settings.jira)?;

    // Best-effort: a preview must still render when Jira is unreachable, or a
    // network blip would block filing rather than just the duplicate check.
    let existing = match client(&state).await {
        Ok(client) => issues::find_open_by_fingerprint(&client, &draft.fingerprint)
            .await
            .unwrap_or(None),
        Err(_) => None,
    };

    Ok(TicketPreview { draft, existing })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTicketRequest {
    #[serde(flatten)]
    pub ticket: TicketRequest,
    /// Edited in the preview, when the user changed something.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub project_key: Option<String>,
    #[serde(default)]
    pub issue_type: Option<String>,
    /// Values for the project's mandatory fields, as answered in the preview.
    #[serde(default)]
    pub fields: Option<std::collections::BTreeMap<String, serde_json::Value>>,
    /// Comment on the existing ticket instead of creating a second one.
    #[serde(default)]
    pub comment_on: Option<String>,
}

/// What happened to one ticket, whether filed alone or as part of a batch.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTicketResult {
    /// Echoed back so a bulk caller can match results to rows.
    pub issue_key: String,
    pub schema_name: String,
    pub outcome: FileOutcome,
    pub ticket: Option<FiledTicket>,
    pub error: Option<Error>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileOutcome {
    Created,
    /// An open ticket already existed; a comment was added instead.
    Commented,
    /// An open ticket already existed and was left alone.
    Skipped,
    Failed,
}

async fn file_one(
    client: &JiraClient,
    settings: &crate::settings::Settings,
    request: &FileTicketRequest,
) -> Result<(FileOutcome, FiledTicket)> {
    let mut draft = ticket::draft(
        &request.ticket.issue,
        &request.ticket.context,
        &settings.jira,
    )?;

    // Edits from the preview win: what was on screen is what gets filed.
    if let Some(summary) = request.summary.as_ref().filter(|s| !s.trim().is_empty()) {
        draft.summary = summary.clone();
    }
    if let Some(description) = request
        .description
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        draft.description = description.clone();
    }
    if let Some(project) = request
        .project_key
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        let project = project.trim();
        if project != draft.project_key {
            // Field defaults belong to the project, not to the routing rule
            // that chose it — carrying the routed project's custom field ids
            // into a different project is a rejection naming a field the user
            // never saw.
            draft.fields = settings
                .jira
                .field_defaults
                .get(project)
                .cloned()
                .unwrap_or_default();
        }
        draft.project_key = project.to_string();
    }
    if let Some(issue_type) = request.issue_type.as_ref().filter(|s| !s.trim().is_empty()) {
        draft.issue_type = issue_type.trim().to_string();
    }
    // Answered in the preview, on top of whatever the project's defaults said.
    if let Some(fields) = &request.fields {
        merge_fields(&mut draft.fields, fields.clone());
    }

    if let Some(key) = &request.comment_on {
        // The same phrasing the ticket body uses, so a ticket and its own
        // follow-up comment do not describe one window two ways.
        let window = ticket::describe_window(request.ticket.context.minutes);
        issues::comment(
            client,
            key,
            &issues::recurrence_comment(
                request.ticket.issue.affected,
                request.ticket.issue.sampled,
                &window,
            ),
        )
        .await?;
        return Ok((
            FileOutcome::Commented,
            FiledTicket {
                key: key.clone(),
                url: issues::browse_url(client.site_url(), key),
                status: None,
                summary: None,
            },
        ));
    }

    let filed = issues::create(client, &draft).await?;
    Ok((FileOutcome::Created, filed))
}

#[tauri::command]
pub async fn file_jira_ticket(
    state: State<'_, AppState>,
    request: FileTicketRequest,
) -> Result<FileTicketResult> {
    let client = client(&state).await?;
    let settings = state.settings_snapshot().await;
    let issue_key = request.ticket.issue.key.clone();
    let schema_name = request.ticket.context.schema_name.clone();

    // A single filing surfaces its failure as an error, so the dialog can show
    // it where the button was pressed — unlike a bulk run, which reports per row.
    let mut request = request;
    let org = crate::commands::origin::resolve_org(&settings).await;
    fill_origin(&state, org.as_deref(), &mut request.ticket.context).await;
    let (outcome, ticket) = file_one(&client, &settings, &request).await?;
    log::info!(target: cat::JIRA, "filed {} for {schema_name} ({issue_key})", ticket.key);

    Ok(FileTicketResult {
        issue_key,
        schema_name,
        outcome,
        ticket: Some(ticket),
        error: None,
    })
}

/// File several tickets, skipping the ones already open.
///
/// Failures are per-row rather than fatal: one project the user cannot write
/// to should not abandon the other nine.
#[tauri::command]
pub async fn file_jira_tickets(
    state: State<'_, AppState>,
    requests: Vec<FileTicketRequest>,
) -> Result<Vec<FileTicketResult>> {
    let client = client(&state).await?;
    let settings = state.settings_snapshot().await;
    let mut requests = requests;
    let org = crate::commands::origin::resolve_org(&settings).await;
    for request in &mut requests {
        fill_origin(&state, org.as_deref(), &mut request.ticket.context).await;
    }
    // The duplicate checks are independent reads, so they run together; a
    // twenty-schema run spent most of its wall clock waiting for them one at a
    // time. The writes stay sequential — creating twenty tickets in parallel
    // is not a courtesy to anyone's Jira.
    let fingerprints: Vec<Option<String>> = requests
        .iter()
        .map(|request| {
            ticket::draft(
                &request.ticket.issue,
                &request.ticket.context,
                &settings.jira,
            )
            .ok()
            .map(|draft| draft.fingerprint)
        })
        .collect();

    let mut existing: Vec<Option<FiledTicket>> = Vec::with_capacity(fingerprints.len());
    for batch in fingerprints.chunks(DEDUPE_CONCURRENCY) {
        existing.extend(
            futures::future::join_all(batch.iter().map(|fingerprint| async {
                match fingerprint {
                    Some(fingerprint) => issues::find_open_by_fingerprint(&client, fingerprint)
                        .await
                        .unwrap_or(None),
                    None => None,
                }
            }))
            .await,
        );
    }

    let mut results = Vec::with_capacity(requests.len());

    for (request, already) in requests.iter().zip(existing) {
        let issue_key = request.ticket.issue.key.clone();
        let schema_name = request.ticket.context.schema_name.clone();

        if already.is_some() && request.comment_on.is_none() {
            results.push(FileTicketResult {
                issue_key,
                schema_name,
                outcome: FileOutcome::Skipped,
                ticket: already,
                error: None,
            });
            continue;
        }

        match file_one(&client, &settings, request).await {
            Ok((outcome, ticket)) => results.push(FileTicketResult {
                issue_key,
                schema_name,
                outcome,
                ticket: Some(ticket),
                error: None,
            }),
            Err(e) => results.push(FileTicketResult {
                issue_key,
                schema_name,
                outcome: FileOutcome::Failed,
                ticket: None,
                error: Some(e),
            }),
        }
    }

    Ok(results)
}
