use regex::Regex;
use serde_json::{Map, Value};
use std::sync::OnceLock;

/// Fields the registry keeps as required. A direct port of the `ID_PATTERN` in
/// the global-event-bus repo's `scripts/simplifySchemas.js`.
fn id_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?i)(id|ids|uuid|arn)$").expect("static regex"))
}

pub fn is_id_field(name: &str) -> bool {
    id_pattern().is_match(name)
}

/// Relax a single detail-object schema:
/// - allow additional properties
/// - keep only ID-like fields in `required`, dropping `required` entirely if
///   none survive
///
/// Non-object schemas pass through untouched, matching the JS behaviour.
fn simplify_object_schema(schema: &Value) -> Value {
    let Some(obj) = schema.as_object() else {
        return schema.clone();
    };
    if obj.get("type").and_then(Value::as_str) != Some("object") {
        return schema.clone();
    }

    let mut simplified = obj.clone();
    simplified.insert("additionalProperties".into(), Value::Bool(true));

    // The JS only rewrites `required` when `properties` is also present.
    if obj.contains_key("required") && obj.contains_key("properties") {
        let id_required: Vec<Value> = obj
            .get("required")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter(|f| f.as_str().map(is_id_field).unwrap_or(false))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        if id_required.is_empty() {
            simplified.remove("required");
        } else {
            simplified.insert("required".into(), Value::Array(id_required));
        }
    }

    Value::Object(simplified)
}

/// Simplify a whole OpenApi3 schema document.
///
/// The `AWSEvent` envelope is deliberately left untouched — EventBridge relies
/// on its required fields — while every other component schema is relaxed.
/// Documents without `components.schemas` pass through unchanged.
pub fn simplify_document(content: &Value) -> Value {
    let Some(schemas) = content
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object)
    else {
        return content.clone();
    };

    let mut simplified_schemas = Map::new();
    for (name, schema) in schemas {
        if name == "AWSEvent" {
            simplified_schemas.insert(name.clone(), schema.clone());
        } else {
            simplified_schemas.insert(name.clone(), simplify_object_schema(schema));
        }
    }

    let mut components = content
        .get("components")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    components.insert("schemas".into(), Value::Object(simplified_schemas));

    let mut out = content.as_object().cloned().unwrap_or_default();
    out.insert("components".into(), Value::Object(components));
    Value::Object(out)
}

/// A human-readable summary of what simplification changed, for the import
/// pre-flight UI.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SimplifyChange {
    pub schema: String,
    pub required_before: usize,
    pub required_after: usize,
}

pub fn describe_changes(original: &Value, simplified: &Value) -> Vec<SimplifyChange> {
    let empty = Map::new();
    let orig = original
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let simp = simplified
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object)
        .unwrap_or(&empty);

    let count_required = |v: Option<&Value>| {
        v.and_then(|s| s.get("required"))
            .and_then(Value::as_array)
            .map(|a| a.len())
            .unwrap_or(0)
    };

    orig.iter()
        .filter(|(name, _)| name.as_str() != "AWSEvent")
        .filter_map(|(name, schema)| {
            let before = count_required(Some(schema));
            let after = count_required(simp.get(name));
            (before != after).then(|| SimplifyChange {
                schema: name.clone(),
                required_before: before,
                required_after: after,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_the_js_id_pattern() {
        for name in ["veteranId", "caseRecordIds", "someUuid", "resourceArn", "ID", "ARN"] {
            assert!(is_id_field(name), "{name} should be treated as an ID field");
        }
        for name in ["identity", "name", "status", "idle", "arnold"] {
            assert!(!is_id_field(name), "{name} should not be an ID field");
        }
    }

    #[test]
    fn leaves_the_envelope_untouched() {
        let doc = json!({
            "components": { "schemas": {
                "AWSEvent": {
                    "type": "object",
                    "required": ["detail-type", "resources", "detail"],
                    "properties": { "detail": {} }
                }
            }}
        });
        let out = simplify_document(&doc);
        assert_eq!(
            out["components"]["schemas"]["AWSEvent"],
            doc["components"]["schemas"]["AWSEvent"]
        );
        // Crucially, additionalProperties is NOT injected into the envelope.
        assert!(out["components"]["schemas"]["AWSEvent"]
            .get("additionalProperties")
            .is_none());
    }

    #[test]
    fn keeps_only_id_fields_required() {
        let doc = json!({
            "components": { "schemas": {
                "ThingHappened": {
                    "type": "object",
                    "required": ["veteranId", "caseRecordId", "someRequiredField"],
                    "properties": {
                        "veteranId": { "type": "string" },
                        "caseRecordId": { "type": "string" },
                        "someRequiredField": { "type": "string" }
                    }
                }
            }}
        });
        let out = simplify_document(&doc);
        let thing = &out["components"]["schemas"]["ThingHappened"];
        assert_eq!(thing["required"], json!(["veteranId", "caseRecordId"]));
        assert_eq!(thing["additionalProperties"], json!(true));
        // Properties are preserved, only the constraint is relaxed.
        assert!(thing["properties"]["someRequiredField"].is_object());
    }

    #[test]
    fn drops_required_when_no_id_fields_survive() {
        let doc = json!({
            "components": { "schemas": {
                "Thing": {
                    "type": "object",
                    "required": ["name", "status"],
                    "properties": { "name": {}, "status": {} }
                }
            }}
        });
        let out = simplify_document(&doc);
        assert!(out["components"]["schemas"]["Thing"].get("required").is_none());
    }

    #[test]
    fn ignores_required_without_properties() {
        // The JS guards on `simplified.required && simplified.properties`.
        let doc = json!({
            "components": { "schemas": {
                "Thing": { "type": "object", "required": ["name"] }
            }}
        });
        let out = simplify_document(&doc);
        assert_eq!(out["components"]["schemas"]["Thing"]["required"], json!(["name"]));
    }

    #[test]
    fn passes_through_documents_without_component_schemas() {
        let doc = json!({ "openapi": "3.0.0", "paths": {} });
        assert_eq!(simplify_document(&doc), doc);
    }

    #[test]
    fn describes_required_count_changes() {
        let original = json!({
            "components": { "schemas": {
                "Thing": {
                    "type": "object",
                    "required": ["aId", "b", "c"],
                    "properties": { "aId": {}, "b": {}, "c": {} }
                }
            }}
        });
        let simplified = simplify_document(&original);
        let changes = describe_changes(&original, &simplified);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].schema, "Thing");
        assert_eq!(changes[0].required_before, 3);
        assert_eq!(changes[0].required_after, 1);
    }
}
