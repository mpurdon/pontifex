use crate::aws::clients::map_sdk_error;
use crate::error::{Error, Result};
use crate::schema::model::{self, EventIdentity};
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
}
