use crate::aws::clients::map_sdk_error;
use crate::error::{Error, Result};
use crate::logging::cat;
use crate::schema::model::{self, EventIdentity, SchemaFile};
use crate::schema::openapi;
use crate::schema::simplify;
use crate::schema::validate::{self, ValidationReport};
use crate::state::AppState;
use futures::stream::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::State;

/// A schema as it appears in the browser list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaSummary {
    pub name: String,
    /// The `source` half of `source@detail-type`, used to group the list.
    pub source: Option<String>,
    pub detail_type: Option<String>,
    pub version: Option<String>,
    pub last_modified: Option<String>,
    pub arn: Option<String>,
}

fn summarize(
    name: String,
    version: Option<String>,
    last_modified: Option<String>,
    arn: Option<String>,
) -> SchemaSummary {
    // Discovered schemas occasionally land with names that do not follow the
    // convention, so parse leniently rather than failing the whole listing.
    let identity = EventIdentity::from_schema_name(&name).ok();
    SchemaSummary {
        source: identity.as_ref().map(|i| i.source.clone()),
        detail_type: identity.as_ref().map(|i| i.detail_type.clone()),
        name,
        version,
        last_modified,
        arn,
    }
}

fn format_ts(dt: Option<&aws_smithy_types::DateTime>) -> Option<String> {
    dt.and_then(|d| d.fmt(aws_smithy_types::date_time::Format::DateTime).ok())
}

async fn schemas_client(
    state: &AppState,
    env_id: Option<&str>,
) -> Result<(String, aws_sdk_schemas::Client)> {
    let (env, cfg) = state.env_config(env_id).await?;
    Ok((
        env.registry_name.clone(),
        aws_sdk_schemas::Client::new(&cfg),
    ))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrySummary {
    pub name: String,
    pub arn: Option<String>,
    pub description: Option<String>,
    /// Registry tags. The global-event-bus stack tags each registry with the
    /// stage (or PR number) it belongs to, which is the only way to tell the
    /// ephemeral per-PR sandbox registries apart.
    pub tags: std::collections::HashMap<String, String>,
}

/// List the schema registries visible to an environment's credentials.
///
/// Registry names are conventional (`<stage>-global-registry`) for the
/// long-lived stages, but sandbox deploys create one registry per pull request
/// (`PR-213-global-registry`), so the settings screen needs to discover them
/// rather than assume a name.
#[tauri::command]
pub async fn list_registries(
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<Vec<RegistrySummary>> {
    let (_, cfg) = state.env_config(env_id.as_deref()).await?;
    let client = aws_sdk_schemas::Client::new(&cfg);

    let mut out = Vec::new();
    let mut pages = client.list_registries().into_paginator().send();

    while let Some(page) = pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for registry in page.registries() {
            let Some(name) = registry.registry_name() else {
                continue;
            };
            out.push(RegistrySummary {
                name: name.to_string(),
                arn: registry.registry_arn().map(str::to_string),
                description: None,
                tags: registry.tags().cloned().unwrap_or_default(),
            });
        }
    }

    // Discovered/AWS-managed registries last; the ones the stack owns first.
    out.sort_by(|a, b| {
        let owned =
            |r: &RegistrySummary| !r.name.starts_with("aws.") && r.name != "discovered-schemas";
        owned(b).cmp(&owned(a)).then(a.name.cmp(&b.name))
    });
    Ok(out)
}

/// List every schema in the environment's registry.
///
/// Paginates eagerly: the dev registry holds a few hundred schemas, which is
/// small enough to hold in memory and makes client-side filtering instant.
#[tauri::command]
pub async fn list_schemas(
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<Vec<SchemaSummary>> {
    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;

    let mut out = Vec::new();
    let mut pages = client
        .list_schemas()
        .registry_name(&registry)
        .into_paginator()
        .send();

    while let Some(page) = pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for s in page.schemas() {
            let Some(name) = s.schema_name() else {
                continue;
            };
            out.push(summarize(
                name.to_string(),
                s.version_count().map(|v| v.to_string()),
                format_ts(s.last_modified()),
                s.schema_arn().map(str::to_string),
            ));
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaDetail {
    pub name: String,
    pub version: String,
    pub schema_type: String,
    pub description: Option<String>,
    pub last_modified: Option<String>,
    pub arn: Option<String>,
    /// The OpenApi3 document, parsed. The API returns it as a string; we hand
    /// the frontend real JSON so the editor and diff view get pretty-printing
    /// and folding for free.
    pub content: Value,
    /// Result of running our validator over the live content, so the UI can
    /// flag pre-existing problems the moment a schema is opened.
    pub validation: ValidationReport,
}

#[tauri::command]
pub async fn describe_schema(
    state: State<'_, AppState>,
    name: String,
    version: Option<String>,
    env_id: Option<String>,
) -> Result<SchemaDetail> {
    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;

    let mut req = client
        .describe_schema()
        .registry_name(&registry)
        .schema_name(&name);
    if let Some(v) = &version {
        req = req.schema_version(v);
    }
    let out = req.send().await.map_err(map_sdk_error)?;

    let raw = out
        .content()
        .ok_or_else(|| Error::Aws(format!("Schema '{name}' has no content")))?;
    let content = model::parse_content(&Value::String(raw.to_string()))?;
    let validation = validate::validate(&content, Some(&name));

    Ok(SchemaDetail {
        name: out.schema_name().unwrap_or(&name).to_string(),
        version: out.schema_version().unwrap_or("1").to_string(),
        schema_type: out.r#type().unwrap_or("OpenApi3").to_string(),
        description: out.description().map(str::to_string),
        last_modified: format_ts(out.last_modified()),
        arn: out.schema_arn().map(str::to_string),
        content,
        validation,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaVersionSummary {
    pub version: String,
    pub schema_type: Option<String>,
}

/// Versions, newest first — the order the version dropdown wants.
#[tauri::command]
pub async fn list_schema_versions(
    state: State<'_, AppState>,
    name: String,
    env_id: Option<String>,
) -> Result<Vec<SchemaVersionSummary>> {
    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;

    let mut out = Vec::new();
    let mut pages = client
        .list_schema_versions()
        .registry_name(&registry)
        .schema_name(&name)
        .into_paginator()
        .send();

    while let Some(page) = pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for v in page.schema_versions() {
            if let Some(version) = v.schema_version() {
                out.push(SchemaVersionSummary {
                    version: version.to_string(),
                    schema_type: v.r#type().map(|t| t.as_str().to_string()),
                });
            }
        }
    }

    // Versions are numeric strings; sort numerically so "10" beats "9".
    out.sort_by_key(|v| std::cmp::Reverse(v.version.parse::<u64>().unwrap_or(0)));
    Ok(out)
}

/// One version in a schema's history, with the document as it stood then.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaHistoryEntry {
    pub version: String,
    /// When this version was created, ISO-8601. `None` for versions the
    /// registry does not date.
    pub created_at: Option<String>,
    pub content: Value,
}

/// How many versions back to fetch. Each one is a `DescribeSchema` call, so an
/// unbounded history would be a hundred round trips for a much-edited schema.
const MAX_HISTORY_VERSIONS: usize = 25;

/// Concurrent `DescribeSchema` calls. Matches the registry report's fan-out.
const HISTORY_CONCURRENCY: usize = 8;

/// A schema's versions with their content, newest first.
///
/// `ListSchemaVersions` returns only version numbers, so the dates and the
/// documents have to come from a `DescribeSchema` per version. They are
/// independent calls, so they run concurrently rather than in a slow serial
/// walk back through history.
#[tauri::command]
pub async fn schema_history(
    state: State<'_, AppState>,
    name: String,
    env_id: Option<String>,
) -> Result<Vec<SchemaHistoryEntry>> {
    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;

    let mut versions: Vec<String> = Vec::new();
    let mut pages = client
        .list_schema_versions()
        .registry_name(&registry)
        .schema_name(&name)
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for v in page.schema_versions() {
            if let Some(version) = v.schema_version() {
                versions.push(version.to_string());
            }
        }
    }
    versions.sort_by_key(|v| std::cmp::Reverse(v.parse::<u64>().unwrap_or(0)));

    let total = versions.len();
    if total > MAX_HISTORY_VERSIONS {
        lwarn!(
            cat::SCHEMA,
            "{name} has {total} versions; showing the most recent {MAX_HISTORY_VERSIONS}"
        );
        versions.truncate(MAX_HISTORY_VERSIONS);
    }

    let timer = crate::logging::Timed::start(
        cat::SCHEMA,
        format!("fetching {} versions of {name}", versions.len()),
    );

    let entries: Vec<Result<SchemaHistoryEntry>> =
        futures::stream::iter(versions.into_iter().map(|version| {
            let client = client.clone();
            let registry = registry.clone();
            let name = name.clone();
            async move {
                let out = client
                    .describe_schema()
                    .registry_name(&registry)
                    .schema_name(&name)
                    .schema_version(&version)
                    .send()
                    .await
                    .map_err(map_sdk_error)?;
                let raw = out.content().unwrap_or("{}");
                Ok(SchemaHistoryEntry {
                    version,
                    created_at: format_ts(out.version_created_date()),
                    content: model::parse_content(&Value::String(raw.to_string()))
                        .unwrap_or(Value::Null),
                })
            }
        }))
        .buffer_unordered(HISTORY_CONCURRENCY)
        .collect()
        .await;

    let mut history: Vec<SchemaHistoryEntry> = entries.into_iter().collect::<Result<_>>()?;
    history.sort_by_key(|e| std::cmp::Reverse(e.version.parse::<u64>().unwrap_or(0)));
    timer.done(format!("{} versions", history.len()));
    Ok(history)
}

#[tauri::command]
pub async fn search_schemas(
    state: State<'_, AppState>,
    keywords: String,
    env_id: Option<String>,
) -> Result<Vec<SchemaSummary>> {
    if keywords.trim().is_empty() {
        return Ok(Vec::new());
    }
    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;

    let mut out = Vec::new();
    let mut pages = client
        .search_schemas()
        .registry_name(&registry)
        .keywords(&keywords)
        .into_paginator()
        .send();

    while let Some(page) = pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for s in page.schemas() {
            let Some(name) = s.schema_name() else {
                continue;
            };
            // Search results carry versions rather than a top-level version.
            let latest = s
                .schema_versions()
                .iter()
                .filter_map(|v| v.schema_version())
                .max_by_key(|v| v.parse::<u64>().unwrap_or(0))
                .map(str::to_string);
            out.push(summarize(
                name.to_string(),
                latest,
                None,
                s.schema_arn().map(str::to_string),
            ));
        }
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Result of a write, including the validation that gated it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    pub name: String,
    pub version: String,
    pub created: bool,
    pub validation: ValidationReport,
}

/// Validate a document without touching AWS. Powers the editor's live gutter.
#[tauri::command]
pub fn validate_schema(content: Value, name: Option<String>) -> ValidationReport {
    validate::validate(&content, name.as_deref())
}

/// Create or update a schema.
///
/// Always validates first: AWS accepts documents that violate the EventBridge
/// envelope contract and they then fail silently at event-validation time.
/// `UpdateSchema` creates a new version rather than mutating the current one.
#[tauri::command]
pub async fn put_schema(
    state: State<'_, AppState>,
    name: String,
    content: Value,
    description: Option<String>,
    env_id: Option<String>,
) -> Result<WriteResult> {
    // Checked before the document, because a bad name fails the write no matter
    // how correct the schema is, and the API's own message ("doesn't match
    // pattern ^[a-zA-Z0-9_\.\-\@]+$") does not say which character offended.
    let bad_chars = model::invalid_schema_name_chars(&name);
    if !bad_chars.is_empty() {
        let rendered: Vec<String> = bad_chars
            .iter()
            .map(|c| {
                if *c == ' ' {
                    "space".to_string()
                } else {
                    format!("'{c}'")
                }
            })
            .collect();
        return Err(Error::Invalid(format!(
            "EventBridge will not accept the schema name '{name}' — it contains {}. \
             Rename it to '{}' and try again; the envelope keeps the real source.",
            rendered.join(", "),
            model::sanitize_schema_name(&name)
        )));
    }

    let validation = validate::validate(&content, Some(&name));
    if !validation.valid {
        let first = validation
            .findings
            .iter()
            .find(|f| f.severity == validate::Severity::Error)
            .map(|f| f.message.clone())
            .unwrap_or_else(|| "Schema failed validation".into());
        return Err(Error::Invalid(first));
    }

    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;
    let body = serde_json::to_string(&content)?;

    // Does it already exist? Distinguishes create from update, and lets us
    // report `created` accurately in the UI.
    let exists = match client
        .describe_schema()
        .registry_name(&registry)
        .schema_name(&name)
        .send()
        .await
    {
        Ok(_) => true,
        Err(e) => match map_sdk_error(e) {
            Error::NotFound(_) => false,
            other => return Err(other),
        },
    };

    if exists {
        let mut req = client
            .update_schema()
            .registry_name(&registry)
            .schema_name(&name)
            .r#type(aws_sdk_schemas::types::Type::OpenApi3)
            .content(body);
        if let Some(d) = &description {
            req = req.description(d);
        }
        let out = req.send().await.map_err(map_sdk_error)?;
        let version = out.schema_version().unwrap_or("1").to_string();
        // Registry writes change shared state, so they are recorded at info
        // regardless of level: "who changed prd and when" must be answerable.
        linfo!(cat::REGISTRY, "updated {name} in {registry} -> v{version}");
        Ok(WriteResult {
            version,
            name,
            created: false,
            validation,
        })
    } else {
        let mut req = client
            .create_schema()
            .registry_name(&registry)
            .schema_name(&name)
            .r#type(aws_sdk_schemas::types::Type::OpenApi3)
            .content(body);
        if let Some(d) = &description {
            req = req.description(d);
        }
        let out = req.send().await.map_err(map_sdk_error)?;
        let version = out.schema_version().unwrap_or("1").to_string();
        linfo!(cat::REGISTRY, "created {name} in {registry} -> v{version}");
        Ok(WriteResult {
            version,
            name,
            created: true,
            validation,
        })
    }
}

/// Delete a schema, or a single version of one.
///
/// `confirm_name` must equal `name`. This is a deliberate second gate on top of
/// the UI's typed confirmation: a mis-wired frontend cannot delete by accident.
/// Protected environments (prd) additionally require `confirm_protected`,
/// mirroring the `CONFIRM_CLEAR_PRODUCTION` guard in the repo's clearSchemas.js.
#[tauri::command]
pub async fn delete_schema(
    state: State<'_, AppState>,
    name: String,
    confirm_name: String,
    version: Option<String>,
    confirm_protected: Option<bool>,
    env_id: Option<String>,
) -> Result<()> {
    if confirm_name != name {
        return Err(Error::Invalid(
            "Confirmation does not match the schema name".into(),
        ));
    }

    let env = state.resolve_environment(env_id.as_deref()).await?;
    if env.is_protected() && !confirm_protected.unwrap_or(false) {
        return Err(Error::Forbidden(format!(
            "'{}' is a protected environment. Confirm the protected-environment prompt to delete from it.",
            env.label
        )));
    }

    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;

    // Destructive and irreversible: logged before the call, so a delete that
    // fails halfway still leaves evidence that it was attempted.
    lwarn!(
        cat::REGISTRY,
        "deleting {name}{} from {registry} (environment '{}', protected={})",
        version
            .as_ref()
            .map(|v| format!(" v{v}"))
            .unwrap_or_default(),
        env.label,
        env.is_protected()
    );

    let result = match version {
        Some(v) => client
            .delete_schema_version()
            .registry_name(&registry)
            .schema_name(&name)
            .schema_version(&v)
            .send()
            .await
            .map(|_| ())
            .map_err(map_sdk_error),
        None => client
            .delete_schema()
            .registry_name(&registry)
            .schema_name(&name)
            .send()
            .await
            .map(|_| ())
            .map_err(map_sdk_error),
    };

    match &result {
        Ok(()) => linfo!(cat::REGISTRY, "deleted {name} from {registry}"),
        Err(e) => lerror!(
            cat::REGISTRY,
            "failed to delete {name} from {registry}: {e}"
        ),
    }
    result
}

/// Generate the boilerplate document for a new event.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailProperty {
    pub name: String,
    #[serde(rename = "type")]
    pub prop_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSchemaDraft {
    pub name: String,
    pub content: Value,
    pub file_name: String,
    pub validation: ValidationReport,
}

#[tauri::command]
pub fn new_schema_draft(
    source: String,
    detail_type: String,
    properties: Vec<DetailProperty>,
) -> Result<NewSchemaDraft> {
    if source.trim().is_empty() || detail_type.trim().is_empty() {
        return Err(Error::Invalid(
            "Both a source and a detail-type are required".into(),
        ));
    }

    let identity = EventIdentity {
        source: source.trim().to_string(),
        detail_type: detail_type.trim().to_string(),
    };
    let props: Vec<(String, String)> = properties
        .into_iter()
        .filter(|p| !p.name.trim().is_empty())
        .map(|p| (p.name.trim().to_string(), p.prop_type))
        .collect();

    let content = model::new_schema_document(&identity, &props);
    let name = identity.schema_name();
    let validation = validate::validate(&content, Some(&name));

    Ok(NewSchemaDraft {
        file_name: model::file_name_for(&identity),
        name,
        content,
        validation,
    })
}

/// Preview what simplification would do to a document, without applying it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimplifyPreview {
    pub content: Value,
    pub changes: Vec<simplify::SimplifyChange>,
}

#[tauri::command]
pub fn simplify_schema(content: Value) -> Result<SimplifyPreview> {
    let parsed = model::parse_content(&content)?;
    let simplified = simplify::simplify_document(&parsed);
    let changes = simplify::describe_changes(&parsed, &simplified);
    Ok(SimplifyPreview {
        content: simplified,
        changes,
    })
}

/// What rewriting a document into OpenAPI 3.0 would do, without applying it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Openapi30Repair {
    pub content: Value,
    /// JSON Pointers to the schema objects that were rewritten, for the diff.
    pub changed: Vec<String>,
}

/// Rewrite the JSON Schema spellings the registry refuses — `type` lists,
/// `type: "null"`, `const`, `examples`, `$schema` — into OpenAPI 3.0.
///
/// The repair for the OpenAPI findings in [`crate::schema::validate`]. A
/// preview: the caller decides whether to keep it.
#[tauri::command]
pub fn openapi_30_schema(content: Value) -> Result<Openapi30Repair> {
    let parsed = model::parse_content(&content)?;
    let (content, changed) = openapi::to_openapi_30(&parsed);
    Ok(Openapi30Repair { content, changed })
}

// ---------------------------------------------------------------------------
// Import / export
// ---------------------------------------------------------------------------

/// What importing a directory would do to the registry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlanEntry {
    pub file: String,
    pub name: String,
    /// "create" | "update" | "unchanged" | "error"
    pub action: String,
    pub message: Option<String>,
    pub content: Value,
    pub validation: Option<ValidationReport>,
    pub simplify_changes: Vec<simplify::SimplifyChange>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub directory: String,
    pub entries: Vec<ImportPlanEntry>,
}

fn read_schema_dir(dir: &Path) -> Result<Vec<(String, SchemaFile)>> {
    if !dir.is_dir() {
        return Err(Error::NotFound(format!(
            "{} is not a directory",
            dir.display()
        )));
    }

    let mut files: Vec<(String, SchemaFile)> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let raw = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<SchemaFile>(&raw) {
            Ok(schema) => files.push((name, schema)),
            Err(e) => {
                lwarn!(cat::REGISTRY, "skipping {}: {e}", path.display());
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Build a pre-flight plan for importing a directory of schema files.
///
/// Nothing is written. Every file is validated, optionally simplified, and
/// compared against what is live so the UI can show create/update/unchanged
/// before the user commits.
#[tauri::command]
pub async fn plan_import(
    state: State<'_, AppState>,
    directory: String,
    simplify_on_import: Option<bool>,
    env_id: Option<String>,
) -> Result<ImportPlan> {
    let dir = PathBuf::from(crate::settings::shellexpand_home(&directory));
    let files = read_schema_dir(&dir)?;
    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;
    let do_simplify = simplify_on_import.unwrap_or(false);

    // One describe per file, run concurrently: re-importing a full registry
    // export is 300+ files, and serially that is minutes of round trips before
    // the plan can render.
    const PLAN_CONCURRENCY: usize = 16;

    let mut entries: Vec<ImportPlanEntry> = futures::stream::iter(files)
        .map(|(file_name, schema)| {
            let client = client.clone();
            let registry = registry.clone();
            async move {
                let parsed = match model::parse_content(&schema.content) {
                    Ok(c) => c,
                    Err(e) => {
                        return ImportPlanEntry {
                            file: file_name,
                            name: schema.schema_name,
                            action: "error".into(),
                            message: Some(e.to_string()),
                            content: Value::Null,
                            validation: None,
                            simplify_changes: Vec::new(),
                        }
                    }
                };

                let (content, simplify_changes) = if do_simplify {
                    let simplified = simplify::simplify_document(&parsed);
                    let changes = simplify::describe_changes(&parsed, &simplified);
                    (simplified, changes)
                } else {
                    (parsed, Vec::new())
                };

                let validation = validate::validate(&content, Some(&schema.schema_name));

                // Compare against live. A byte-for-byte match is unlikely, so
                // compare parsed JSON — key order and whitespace should not
                // count as a change.
                let live = client
                    .describe_schema()
                    .registry_name(&registry)
                    .schema_name(&schema.schema_name)
                    .send()
                    .await;

                let (action, message) = match live {
                    Ok(out) => {
                        let live_content = out
                            .content()
                            .and_then(|c| serde_json::from_str::<Value>(c).ok());
                        if live_content.as_ref() == Some(&content) {
                            ("unchanged", None)
                        } else {
                            ("update", None)
                        }
                    }
                    Err(e) => match map_sdk_error(e) {
                        Error::NotFound(_) => ("create", None),
                        other => ("error", Some(other.to_string())),
                    },
                };

                ImportPlanEntry {
                    file: file_name,
                    name: schema.schema_name,
                    action: action.into(),
                    message,
                    content,
                    validation: Some(validation),
                    simplify_changes,
                }
            }
        })
        .buffer_unordered(PLAN_CONCURRENCY)
        .collect()
        .await;

    // Concurrency makes completion order arbitrary; the plan is a reviewable
    // list, so give it a stable one.
    entries.sort_by(|a, b| a.file.cmp(&b.file));

    Ok(ImportPlan {
        directory: dir.to_string_lossy().into_owned(),
        entries,
    })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyImportResult {
    pub applied: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Apply a subset of an import plan. The frontend passes only the entries the
/// user ticked, so an import is always an explicit, reviewed action.
#[tauri::command]
pub async fn apply_import(
    state: State<'_, AppState>,
    entries: Vec<ImportSelection>,
    env_id: Option<String>,
) -> Result<ApplyImportResult> {
    let mut applied = Vec::new();
    let mut failed = Vec::new();

    for entry in entries {
        match put_schema(
            state.clone(),
            entry.name.clone(),
            entry.content,
            None,
            env_id.clone(),
        )
        .await
        {
            Ok(_) => applied.push(entry.name),
            Err(e) => failed.push((entry.name, e.to_string())),
        }
    }

    Ok(ApplyImportResult { applied, failed })
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSelection {
    pub name: String,
    pub content: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub directory: String,
    pub written: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// Write schemas from the registry to a directory in the repo's file format.
///
/// With `names` empty, exports the whole registry — the equivalent of the
/// repo's `downloadSchemas.js`.
#[tauri::command]
pub async fn export_schemas(
    state: State<'_, AppState>,
    directory: String,
    names: Vec<String>,
    simplify_on_export: Option<bool>,
    env_id: Option<String>,
) -> Result<ExportResult> {
    let dir = PathBuf::from(crate::settings::shellexpand_home(&directory));
    std::fs::create_dir_all(&dir)?;

    let names = if names.is_empty() {
        list_schemas(state.clone(), env_id.clone())
            .await?
            .into_iter()
            .map(|s| s.name)
            .collect()
    } else {
        names
    };

    let (registry, client) = schemas_client(&state, env_id.as_deref()).await?;
    let do_simplify = simplify_on_export.unwrap_or(false);

    let mut written = Vec::new();
    let mut failed = Vec::new();

    for name in names {
        let result = async {
            let out = client
                .describe_schema()
                .registry_name(&registry)
                .schema_name(&name)
                .send()
                .await
                .map_err(map_sdk_error)?;

            let raw = out
                .content()
                .ok_or_else(|| Error::Aws(format!("Schema '{name}' has no content")))?;
            let mut content = model::parse_content(&Value::String(raw.to_string()))?;
            if do_simplify {
                content = simplify::simplify_document(&content);
            }

            let file = model::to_schema_file(
                &name,
                out.schema_version().unwrap_or("1"),
                content,
                format_ts(out.last_modified()),
            );

            // Prefer the repo's `<source>_<DetailTitle>.json` convention, but
            // fall back to a sanitized schema name for anything off-convention.
            let file_name = EventIdentity::from_schema_name(&name)
                .map(|id| model::file_name_for(&id))
                .unwrap_or_else(|_| format!("{}.json", name.replace(['@', '/', ':'], "_")));

            std::fs::write(dir.join(&file_name), serde_json::to_string_pretty(&file)?)?;
            Ok::<String, Error>(file_name)
        }
        .await;

        match result {
            Ok(file_name) => written.push(file_name),
            Err(e) => failed.push((name, e.to_string())),
        }
    }

    written.sort();
    Ok(ExportResult {
        directory: dir.to_string_lossy().into_owned(),
        written,
        failed,
    })
}
