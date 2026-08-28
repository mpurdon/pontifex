use crate::aws::clients::map_sdk_error;
use crate::error::{Error, Result};
use crate::schema::events::Issue;
use crate::schema::model::{self, EventIdentity};
use crate::schema::repair::{self, Repair};
use crate::schema::validate::{self, ValidationReport};
use crate::settings::BedrockModel;
use crate::state::AppState;
use aws_sdk_bedrockruntime::types::{
    ContentBlock, ConversationRole, InferenceConfiguration, Message, SystemContentBlock,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

/// The contract every generated schema must satisfy, given to the model as a
/// system prompt. Kept in sync with `schema/validate.rs` — if the validator
/// gains a rule, it belongs here too, or the model will keep producing
/// documents that get rejected.
// Note: `r##"..."##` because the prompt contains `"#` in the `$ref` example.
const SYSTEM_PROMPT: &str = r##"You are a schema author for an AWS EventBridge schema registry.

You produce OpenAPI 3.0 documents describing EventBridge events. Every document MUST follow this exact contract:

- Top level: `openapi` ("3.0.0"), `info` (with `version` and `title`), `paths` ({}), and `components.schemas`.
- `components.schemas.AWSEvent` is the event envelope. It MUST have:
  - "type": "object"
  - "required": ["detail-type","resources","detail","id","source","time","region","version","account"]
  - "x-amazon-events-source": the event source (e.g. "milo-medical")
  - "x-amazon-events-detail-type": the event detail-type (e.g. "packetNotification-assigned")
  - "properties" defining detail, account, detail-type, id, region, resources, source, time (format date-time) and version.
  - `properties.detail` MUST be {"$ref": "#/components/schemas/<DetailTitle>"} where <DetailTitle> is the PascalCase form of the detail-type.
- `components.schemas.<DetailTitle>` describes the event payload. It MUST have "type": "object", "additionalProperties": true, a "properties" map, and a "required" array containing ONLY identifier-like fields (names ending in id, ids, uuid or arn, case-insensitive). If there are no such fields, omit "required" entirely.
- `info.title` MUST equal <DetailTitle>.

Naming conventions in this registry: sources are kebab-case service names, detail-types are camelCase words joined by hyphens. Payload fields are camelCase.

Respond with ONLY the JSON document. No prose, no markdown fences, no explanation."##;

const EXPLAIN_PROMPT: &str = r#"You explain AWS EventBridge schemas to engineers.

Given an OpenAPI 3.0 EventBridge schema document, write a short, concrete explanation covering:
- what the event represents and when it is likely emitted
- the source and detail-type it matches
- each payload field, what it holds, and whether it is required
- anything notable: loose typing, missing descriptions, fields that look like they should be required but are not

Use plain prose and short bullet lists. Do not restate the JSON."#;

/// What the AI panel is being asked to do.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AiAction {
    /// Plain-English description -> a complete new schema.
    Generate,
    /// An existing schema + instructions -> a revised schema.
    Refactor,
    /// An existing schema -> prose. The only action that does not return JSON.
    Explain,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRequest {
    pub action: AiAction,
    /// The user's description or instruction.
    pub prompt: String,
    /// Present for Refactor/Explain.
    pub schema: Option<Value>,
    /// Target identity for Generate, when the user has already picked one.
    pub source: Option<String>,
    pub detail_type: Option<String>,
    /// Overrides the configured model.
    pub model_id: Option<String>,
    /// Real schemas to use as few-shot examples, so output matches house style.
    #[serde(default)]
    pub examples: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiResponse {
    /// The raw model output.
    pub text: String,
    /// Parsed JSON, for Generate/Refactor. `None` when the model returned
    /// something we could not parse — `text` still holds it for inspection.
    pub content: Option<Value>,
    /// Validation of `content`, when there is any.
    pub validation: Option<ValidationReport>,
    pub model_id: String,
}

async fn resolve_model(state: &AppState, override_id: Option<String>) -> Result<String> {
    if let Some(id) = override_id.filter(|s| !s.trim().is_empty()) {
        return Ok(id);
    }
    let settings = state.settings.read().await;
    let selected = settings.llm.selected_model_id.as_deref();
    let model = selected
        .and_then(|sel| settings.llm.models.iter().find(|m| m.id == sel))
        .or_else(|| settings.llm.models.first())
        .ok_or_else(|| {
            Error::Invalid(
                "No Bedrock model configured. Import one from your Claude Code settings, or add one in Settings → AI."
                    .into(),
            )
        })?;
    Ok(model.model_id.clone())
}

fn build_user_message(req: &AiRequest) -> String {
    let mut parts: Vec<String> = Vec::new();

    if !req.examples.is_empty() {
        parts.push(
            "Here are existing schemas from this registry. Match their style and structure:"
                .to_string(),
        );
        for example in req.examples.iter().take(3) {
            if let Ok(pretty) = serde_json::to_string_pretty(example) {
                parts.push(pretty);
            }
        }
    }

    match req.action {
        AiAction::Generate => {
            if let (Some(source), Some(detail_type)) = (&req.source, &req.detail_type) {
                let title = EventIdentity {
                    source: source.clone(),
                    detail_type: detail_type.clone(),
                }
                .detail_title();
                parts.push(format!(
                    "Create a schema with source \"{source}\", detail-type \"{detail_type}\", and detail title \"{title}\"."
                ));
            } else {
                parts.push(
                    "Choose an appropriate source and detail-type following the registry conventions."
                        .to_string(),
                );
            }
            parts.push(format!("The event to describe:\n{}", req.prompt));
        }
        AiAction::Refactor => {
            if let Some(schema) = &req.schema {
                parts.push(format!(
                    "Here is the schema to revise:\n{}",
                    serde_json::to_string_pretty(schema).unwrap_or_default()
                ));
            }
            parts.push(format!("Apply these changes:\n{}", req.prompt));
            parts.push(
                "Keep the source and detail-type unchanged unless explicitly told otherwise."
                    .to_string(),
            );
        }
        AiAction::Explain => {
            if let Some(schema) = &req.schema {
                parts.push(format!(
                    "Explain this schema:\n{}",
                    serde_json::to_string_pretty(schema).unwrap_or_default()
                ));
            }
            if !req.prompt.trim().is_empty() {
                parts.push(format!("Focus on: {}", req.prompt));
            }
        }
    }

    parts.join("\n\n")
}

/// Pull a JSON object out of a model response.
///
/// Models wrap output in ```json fences or add a sentence of preamble often
/// enough that failing on it would make the feature feel broken, so we strip
/// fences and, failing that, take the outermost balanced `{...}` span.
fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();

    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }

    // ```json ... ``` or ``` ... ```
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        let after = after.strip_prefix("json").unwrap_or(after);
        if let Some(end) = after.find("```") {
            if let Ok(value) = serde_json::from_str::<Value>(after[..end].trim()) {
                return Some(value);
            }
        }
    }

    // Outermost balanced braces, ignoring braces inside strings.
    let bytes = trimmed.as_bytes();
    let start = bytes.iter().position(|&b| b == b'{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_string {
            if escaped {
                escaped = false;
            } else if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str::<Value>(&trimmed[start..=i]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// Run a Bedrock conversation, streaming deltas to the frontend.
///
/// Emits `bedrock://delta` as text arrives so the panel can render
/// progressively, then returns the assembled result. The model never writes to
/// AWS: output lands in the editor as a validated draft the user must save.
#[tauri::command]
pub async fn ai_generate(
    app: AppHandle,
    state: State<'_, AppState>,
    request: AiRequest,
) -> Result<AiResponse> {
    let model_id = resolve_model(&state, request.model_id.clone()).await?;
    let cfg = state.llm_config().await?;
    let client = aws_sdk_bedrockruntime::Client::new(&cfg);

    let system = match request.action {
        AiAction::Explain => EXPLAIN_PROMPT,
        _ => SYSTEM_PROMPT,
    };

    let message = Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(build_user_message(&request)))
        .build()
        .map_err(|e| Error::Internal(format!("Could not build Bedrock message: {e}")))?;

    let mut stream = client
        .converse_stream()
        .model_id(&model_id)
        .system(SystemContentBlock::Text(system.to_string()))
        .messages(message)
        .inference_config(
            InferenceConfiguration::builder()
                .max_tokens(8192)
                // Schema authoring wants determinism, not flair.
                .temperature(0.2)
                .build(),
        )
        .send()
        .await
        .map_err(map_sdk_error)?;

    let mut text = String::new();
    loop {
        let event = stream
            .stream
            .recv()
            .await
            .map_err(|e| Error::Aws(format!("Bedrock stream failed: {e}")))?;

        let Some(event) = event else { break };

        use aws_sdk_bedrockruntime::types::ConverseStreamOutput as Out;
        if let Out::ContentBlockDelta(delta) = event {
            if let Some(aws_sdk_bedrockruntime::types::ContentBlockDelta::Text(chunk)) =
                delta.delta()
            {
                text.push_str(chunk);
                let _ = app.emit("bedrock://delta", chunk);
            }
        }
    }

    let _ = app.emit("bedrock://done", ());

    let (content, validation) = match request.action {
        AiAction::Explain => (None, None),
        _ => match extract_json(&text) {
            Some(parsed) => {
                let expected = match (&request.source, &request.detail_type) {
                    (Some(s), Some(d)) => Some(
                        EventIdentity {
                            source: s.clone(),
                            detail_type: d.clone(),
                        }
                        .schema_name(),
                    ),
                    // For a refactor, hold the model to the original name.
                    _ => request
                        .schema
                        .as_ref()
                        .and_then(|s| model::parse_content(s).ok())
                        .and_then(|s| model::identity_of(&s))
                        .map(|identity| identity.schema_name()),
                };
                let report = validate::validate(&parsed, expected.as_deref());
                (Some(parsed), Some(report))
            }
            None => (None, None),
        },
    };

    Ok(AiResponse {
        text,
        content,
        validation,
        model_id,
    })
}

/// What a schema version's description may hold.
///
/// EventBridge caps `UpdateSchema`'s description, and a write rejected for
/// length after the diff was reviewed and confirmed is the worst moment to find
/// out. The model is asked for far less than this; the cap is the backstop.
const DESCRIPTION_LIMIT: usize = 256;

const REPAIR_PROMPT: &str = r#"You decide how to repair one field in an AWS EventBridge JSON schema, given what real events actually carry.

You are given: the current declaration for the field, what the validator reported, and example values observed in real traffic.

Choose exactly one repair, or none:
- {"kind":"widenType","types":["string","null"]} — redeclare the field as the types real events carry. Include EVERY type still present in traffic, not only the offending one: a field declared `string` that is null in 7% of events becomes ["string","null"], never ["null"]. Drop a declared type only when the traffic no longer contains it.
- {"kind":"extendEnum","values":["archived"]} — allow values producers already send.
- {"kind":"dropRequired"} — stop requiring a field that events omit.
- {"kind":"declareField","types":["string"]} — declare a field the schema does not describe.
- null — when no mechanical edit is right, e.g. the events look like a producer bug, or the correct shape cannot be told from the sample.

Types must be JSON type names: null, boolean, integer, number, string, array, object.

Prefer the repair that stops events being rejected while narrowing the schema as little as possible, and prefer none over a guess. Say plainly in the rationale what the edit gives up.

Respond with ONLY this JSON, no prose or fences:
{"repair": <one of the above, or null>, "rationale": "<one sentence>"}"#;

const SUMMARY_PROMPT: &str = r#"You write the version description for an AWS EventBridge schema registry.

You are given the previous and new version of a schema document. Describe what changed, for an engineer reading the version history months from now who wants to know whether this version is why their events started or stopped validating.

Rules:
- State the change and its effect on validation, e.g. "homeAddress.country widened to string|null; 3 fields declared; status no longer required."
- Name the fields. "Various fixes" is worthless in a version history.
- If nothing but formatting changed, say so.
- One paragraph, at most 200 characters. No markdown, no preamble, no trailing period-padding to fill space.

Respond with ONLY the description text."#;

/// The repair a model proposes for one issue, with its reasoning.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairSuggestion {
    /// `None` when the model declined — which is a real answer, not a failure.
    pub repair: Option<Repair>,
    pub rationale: String,
    pub model_id: String,
}

/// What the model is asked to return, before it is trusted.
#[derive(Debug, Deserialize)]
struct ProposedRepair {
    #[serde(default)]
    repair: Option<Repair>,
    #[serde(default)]
    rationale: String,
}

/// The JSON type names a repair may name.
const JSON_TYPES: [&str; 7] = [
    "null", "boolean", "integer", "number", "string", "array", "object",
];

/// Reject a proposal that names something that is not a JSON type.
///
/// A model that invents `"uuid"` or `"date-time"` produces a schema that
/// compiles to nothing and silently validates everything — the one failure mode
/// worth spending a check on.
fn plausible(repair: &Repair) -> bool {
    let types = match repair {
        Repair::WidenType { types } | Repair::DeclareField { types, .. } => types,
        Repair::ExtendEnum { values } => return !values.is_empty(),
        Repair::DropRequired => return true,
    };
    !types.is_empty() && types.iter().all(|t| JSON_TYPES.contains(&t.as_str()))
}

/// Prefer a small, fast model for the short classification calls.
///
/// These run one per click and answer a bounded question; spending the
/// schema-authoring model on them is slower and dearer for no better answer.
/// Falls back to the configured model when nothing cheaper is set up.
async fn resolve_fast_model(state: &AppState, override_id: Option<String>) -> Result<String> {
    if let Some(id) = override_id.filter(|s| !s.trim().is_empty()) {
        return Ok(id);
    }
    {
        let settings = state.settings.read().await;
        let fast = settings.llm.models.iter().find(|m| {
            let haystack = format!("{} {}", m.id, m.label).to_lowercase();
            haystack.contains("haiku")
        });
        if let Some(model) = fast {
            return Ok(model.model_id.clone());
        }
    }
    resolve_model(state, None).await
}

/// One non-streaming turn. Used by the short calls that have no partial output
/// worth rendering — a two-line answer does not benefit from a typewriter.
async fn converse_once(
    state: &AppState,
    model_id: &str,
    system: &str,
    user: String,
    max_tokens: i32,
) -> Result<String> {
    let cfg = state.llm_config().await?;
    let client = aws_sdk_bedrockruntime::Client::new(&cfg);

    let message = Message::builder()
        .role(ConversationRole::User)
        .content(ContentBlock::Text(user))
        .build()
        .map_err(|e| Error::Internal(format!("Could not build Bedrock message: {e}")))?;

    let out = client
        .converse()
        .model_id(model_id)
        .system(SystemContentBlock::Text(system.to_string()))
        .messages(message)
        .inference_config(
            InferenceConfiguration::builder()
                .max_tokens(max_tokens)
                .temperature(0.1)
                .build(),
        )
        .send()
        .await
        .map_err(map_sdk_error)?;

    let text = out
        .output()
        .and_then(|o| o.as_message().ok())
        .map(|m| {
            m.content()
                .iter()
                .filter_map(|block| block.as_text().ok())
                .cloned()
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();

    Ok(text)
}

/// Ask a model what to do about an issue the deterministic planner cannot decide.
///
/// The four mechanical repairs are computed locally and cost nothing; this is
/// for the rest — a rejected pattern, a bound, a field whose right shape is a
/// judgement about the sample. The answer comes back as the same `Repair` the
/// panel already knows how to apply, so an AI-chosen edit and a computed one
/// travel the identical path and get the same diff review.
#[tauri::command]
pub async fn ai_suggest_repair(
    state: State<'_, AppState>,
    content: Value,
    type_name: String,
    issue: Issue,
    examples: Vec<Value>,
    model_id: Option<String>,
) -> Result<RepairSuggestion> {
    let model_id = resolve_fast_model(&state, model_id).await?;
    let document = model::parse_content(&content)?;

    // The declaration as it stands, so the model is reasoning about the real
    // document rather than the summary's paraphrase of it.
    let declaration = repair::declaration_at(&document, &type_name, &issue.path)
        .map(|d| serde_json::to_string_pretty(&d).unwrap_or_default())
        .unwrap_or_else(|| "(not declared)".to_string());

    let observed: Vec<String> = examples
        .iter()
        .take(10)
        .map(|v| serde_json::to_string(v).unwrap_or_default())
        .collect();

    let user = format!(
        "Field: {path}\n\
         Current declaration:\n{declaration}\n\n\
         Reported: {summary}\n\
         Validator message: {message}\n\
         Affected: {affected} of {sampled} sampled events{rejecting}\n\
         Observed values: {observed}",
        path = issue.path,
        summary = issue.summary,
        message = issue.message.as_deref().unwrap_or("(none)"),
        affected = issue.affected,
        sampled = issue.sampled,
        rejecting = if issue.rejects {
            ", which are being rejected today"
        } else {
            ""
        },
        observed = if observed.is_empty() {
            "(none captured)".to_string()
        } else {
            observed.join(", ")
        },
    );

    let text = converse_once(&state, &model_id, REPAIR_PROMPT, user, 512).await?;

    let proposed: ProposedRepair = extract_json(&text)
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(|| {
            Error::Internal(format!(
                "The model did not answer with a repair it could apply. It said: {}",
                text.trim()
            ))
        })?;

    // A proposal naming a type that does not exist is dropped rather than
    // offered: applying it would produce a schema that validates nothing.
    let repair = proposed.repair.filter(plausible);

    Ok(RepairSuggestion {
        repair,
        rationale: proposed.rationale,
        model_id,
    })
}

/// Describe what a draft changed, for the version's description field.
///
/// EventBridge stores a description per version, so this is the changelog the
/// registry already has a place for and nothing has ever written to.
#[tauri::command]
pub async fn ai_summarize_changes(
    state: State<'_, AppState>,
    before: Option<Value>,
    after: Value,
    model_id: Option<String>,
) -> Result<String> {
    let model_id = resolve_fast_model(&state, model_id).await?;

    let user = match &before {
        Some(before) => format!(
            "Previous version:\n{}\n\nNew version:\n{}",
            serde_json::to_string_pretty(before).unwrap_or_default(),
            serde_json::to_string_pretty(&after).unwrap_or_default(),
        ),
        // A first version has nothing to diff against, so describe the event.
        None => format!(
            "This is the first version of a new schema. Describe what it represents, in one line.\n\n{}",
            serde_json::to_string_pretty(&after).unwrap_or_default(),
        ),
    };

    let text = converse_once(&state, &model_id, SUMMARY_PROMPT, user, 400).await?;
    Ok(clamp_description(&text))
}

/// Trim a description to what the registry will accept, on a word boundary.
fn clamp_description(text: &str) -> String {
    let trimmed = text.trim().trim_matches('"').trim();
    if trimmed.chars().count() <= DESCRIPTION_LIMIT {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(DESCRIPTION_LIMIT - 1).collect();
    let cut = truncated.rfind(' ').unwrap_or(truncated.len());
    format!("{}…", &truncated[..cut])
}

/// Inference profiles available to the configured LLM profile.
///
/// Lets the user pick a model without hand-copying ARNs, and is the fallback
/// when no Claude Code settings file is present to import from.
#[tauri::command]
pub async fn list_bedrock_models(state: State<'_, AppState>) -> Result<Vec<BedrockModel>> {
    let cfg = state.llm_config().await?;
    let client = aws_sdk_bedrock::Client::new(&cfg);

    let mut models = Vec::new();

    let mut pages = client
        .list_inference_profiles()
        .into_paginator()
        .send();
    while let Some(page) = pages.next().await {
        let page = page.map_err(map_sdk_error)?;
        for p in page.inference_profile_summaries() {
            let arn = p.inference_profile_arn();
            models.push(BedrockModel {
                id: p.inference_profile_id().to_string(),
                label: p.inference_profile_name().to_string(),
                model_id: arn.to_string(),
            });
        }
    }

    // Foundation models are the fallback when no inference profiles exist.
    if models.is_empty() {
        let out = client
            .list_foundation_models()
            .by_provider("anthropic")
            .send()
            .await
            .map_err(map_sdk_error)?;
        for m in out.model_summaries() {
            models.push(BedrockModel {
                id: m.model_id().to_string(),
                label: m.model_name().unwrap_or(m.model_id()).to_string(),
                model_id: m.model_id().to_string(),
            });
        }
    }

    models.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(models)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_bare_json() {
        let value = extract_json(r#"{"openapi":"3.0.0"}"#).unwrap();
        assert_eq!(value["openapi"], "3.0.0");
    }

    #[test]
    fn extracts_json_from_markdown_fences() {
        let text = "Here you go:\n```json\n{\"openapi\": \"3.0.0\"}\n```\nHope that helps!";
        assert_eq!(extract_json(text).unwrap()["openapi"], "3.0.0");
    }

    #[test]
    fn extracts_json_from_unlabelled_fences() {
        let text = "```\n{\"a\": 1}\n```";
        assert_eq!(extract_json(text).unwrap()["a"], 1);
    }

    #[test]
    fn extracts_json_after_prose_without_fences() {
        let text = "Sure. {\"a\": {\"b\": 2}} That's the schema.";
        assert_eq!(extract_json(text).unwrap()["a"]["b"], 2);
    }

    #[test]
    fn is_not_confused_by_braces_inside_strings() {
        let text = r#"{"note": "a } brace", "ok": true}"#;
        let value = extract_json(text).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["note"], "a } brace");
    }

    #[test]
    fn returns_none_when_there_is_no_json() {
        assert!(extract_json("I cannot help with that.").is_none());
        assert!(extract_json("{ unterminated").is_none());
    }

    #[test]
    fn generate_message_names_the_expected_detail_title() {
        let req = AiRequest {
            action: AiAction::Generate,
            prompt: "A packet was assigned to a reviewer".into(),
            schema: None,
            source: Some("milo-medical".into()),
            detail_type: Some("packetNotification-assigned".into()),
            model_id: None,
            examples: vec![json!({"openapi": "3.0.0"})],
        };
        let msg = build_user_message(&req);
        assert!(msg.contains("PacketNotificationAssigned"));
        assert!(msg.contains("milo-medical"));
        assert!(msg.contains("existing schemas from this registry"));
    }

    #[test]
    fn a_repair_naming_a_type_that_is_not_a_json_type_is_dropped() {
        // `uuid` and `date-time` are formats, not types. A schema declaring one
        // as a type constrains nothing, so it would validate everything and
        // report the drift as cleared.
        assert!(!plausible(&Repair::WidenType {
            types: vec!["uuid".into()]
        }));
        assert!(!plausible(&Repair::DeclareField {
            types: vec!["string".into(), "date-time".into()],
            example: None,
        }));
    }

    #[test]
    fn a_repair_naming_real_json_types_is_kept() {
        assert!(plausible(&Repair::WidenType {
            types: vec!["string".into(), "null".into()]
        }));
        assert!(plausible(&Repair::DropRequired));
    }

    #[test]
    fn an_empty_repair_is_dropped_rather_than_applied_as_a_no_op() {
        assert!(!plausible(&Repair::WidenType { types: vec![] }));
        assert!(!plausible(&Repair::ExtendEnum { values: vec![] }));
    }

    #[test]
    fn a_proposal_parses_out_of_the_shape_the_prompt_asks_for() {
        let parsed: ProposedRepair = extract_json(
            r#"{"repair": {"kind": "widenType", "types": ["string", "null"]}, "rationale": "7% are null."}"#,
        )
        .and_then(|v| serde_json::from_value(v).ok())
        .expect("should parse");

        assert_eq!(
            parsed.repair,
            Some(Repair::WidenType {
                types: vec!["string".into(), "null".into()]
            })
        );
        assert!(parsed.rationale.contains("null"));
    }

    #[test]
    fn declining_to_repair_parses_as_an_answer_rather_than_an_error() {
        // "None of these is right" is a useful verdict, and must not read as a
        // malformed response — otherwise the panel reports a model failure for
        // the one case where the model was being careful.
        let parsed: ProposedRepair =
            extract_json(r#"{"repair": null, "rationale": "Looks like a producer bug."}"#)
                .and_then(|v| serde_json::from_value(v).ok())
                .expect("should parse");

        assert!(parsed.repair.is_none());
        assert!(!parsed.rationale.is_empty());
    }

    #[test]
    fn a_description_within_the_limit_is_left_alone() {
        let text = "homeAddress.country widened to string|null.";
        assert_eq!(clamp_description(text), text);
    }

    #[test]
    fn an_over_long_description_is_cut_to_what_the_registry_accepts() {
        let long = "field ".repeat(200);
        let clamped = clamp_description(&long);

        assert!(
            clamped.chars().count() <= DESCRIPTION_LIMIT,
            "still {} chars",
            clamped.chars().count()
        );
        assert!(clamped.ends_with('…'));
    }

    #[test]
    fn a_description_the_model_wrapped_in_quotes_is_unwrapped() {
        assert_eq!(clamp_description("  \"Declared 3 fields.\"  "), "Declared 3 fields.");
    }
}
