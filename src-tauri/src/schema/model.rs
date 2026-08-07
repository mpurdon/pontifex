use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The on-disk file format used by the global-event-bus repo's `schemas/` and
/// `schemas-simplified/` directories, and by its download/register scripts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaFile {
    pub schema_name: String,
    pub schema_version: String,
    #[serde(rename = "type")]
    pub schema_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// The OpenApi3 document. EventBridge stores this as a JSON *string*; on
    /// disk the repo stores it as a nested object, so we normalize on read.
    pub content: Value,
}

/// The two identifiers EventBridge derives a schema name from.
///
/// A registry schema is named `<source>@<detail-type>`, and the same pair is
/// duplicated inside the document as `x-amazon-events-source` and
/// `x-amazon-events-detail-type`. Keeping them in sync is the single most
/// common thing to get wrong by hand, so we model it explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventIdentity {
    pub source: String,
    pub detail_type: String,
}

impl EventIdentity {
    pub fn schema_name(&self) -> String {
        format!("{}@{}", self.source, self.detail_type)
    }

    /// Parse `source@detail-type`. Detail types may themselves contain `@`
    /// in theory, so we split on the first separator only.
    pub fn from_schema_name(name: &str) -> Result<Self> {
        let (source, detail_type) = name.split_once('@').ok_or_else(|| {
            Error::Invalid(format!(
                "Schema name '{name}' is not in the expected <source>@<detail-type> form"
            ))
        })?;
        if source.is_empty() || detail_type.is_empty() {
            return Err(Error::Invalid(format!(
                "Schema name '{name}' has an empty source or detail-type"
            )));
        }
        Ok(EventIdentity {
            source: source.to_string(),
            detail_type: detail_type.to_string(),
        })
    }

    /// The PascalCase title EventBridge's own codegen uses for the detail
    /// object, derived from the detail-type: `packetNotification-assigned`
    /// becomes `PacketNotificationAssigned`.
    pub fn detail_title(&self) -> String {
        self.detail_type
            .split(['-', '_', '.', ' '])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect()
    }
}

/// Characters EventBridge accepts in a registry schema name.
///
/// The API enforces `^[a-zA-Z0-9_\.\-\@]+$` and rejects anything else with a
/// `BadRequestException`. Notably it excludes spaces — and real sources on the
/// bus do contain them (`Atomic Forms`), so an inferred draft can carry a name
/// that can never be registered.
pub fn is_valid_schema_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '@')
}

/// The distinct characters in `name` that EventBridge would reject.
pub fn invalid_schema_name_chars(name: &str) -> Vec<char> {
    let mut seen = Vec::new();
    for c in name.chars() {
        if !is_valid_schema_name_char(c) && !seen.contains(&c) {
            seen.push(c);
        }
    }
    seen
}

/// A registrable name, with rejected characters replaced by `-`.
///
/// Only the *name* is rewritten. The document keeps the true
/// `x-amazon-events-source`, so the schema still describes the events as they
/// are actually published — the registry name is an identifier, not the
/// contract.
pub fn sanitize_schema_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if is_valid_schema_name_char(c) { c } else { '-' })
        .collect();
    // Collapse runs so "Atomic  Forms" does not become "Atomic--Forms".
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out
}

/// The `AWSEvent` envelope out of a document, if it has one.
pub fn envelope(document: &Value) -> Option<&Value> {
    crate::schema::openapi::component_schema(document, "AWSEvent")
}

/// Read the identity back out of a document's envelope.
///
/// The counterpart to [`new_schema_document`]: this module knows how the two
/// `x-amazon-events-*` keys are written, so it is also where they are read.
/// Callers that hand-walked the path drifted on what to do when only one key
/// was present.
pub fn identity_of(document: &Value) -> Option<EventIdentity> {
    let envelope = envelope(document)?;
    let source = envelope.get("x-amazon-events-source")?.as_str()?;
    let detail_type = envelope.get("x-amazon-events-detail-type")?.as_str()?;
    Some(EventIdentity {
        source: source.to_string(),
        detail_type: detail_type.to_string(),
    })
}

/// The name of the type the envelope's `detail` points at.
///
/// Returns `None` for a document whose `detail` is inlined rather than
/// `$ref`ed, which the registry's schemas never are but a hand-edited draft
/// can be.
pub fn detail_type_name(document: &Value) -> Option<String> {
    let reference = envelope(document)?
        .get("properties")?
        .get("detail")?
        .get("$ref")?
        .as_str()?;
    crate::schema::openapi::ref_name(reference).map(str::to_string)
}

/// Parse a schema document from either representation: a JSON string (what the
/// EventBridge API returns) or an already-parsed object (what the repo stores).
pub fn parse_content(content: &Value) -> Result<Value> {
    match content {
        Value::String(s) => serde_json::from_str(s)
            .map_err(|e| Error::Invalid(format!("Schema content is not valid JSON: {e}"))),
        other => Ok(other.clone()),
    }
}

/// The `properties` block of the EventBridge event envelope. Every schema in
/// the registry carries an identical copy of this.
fn envelope_properties(detail_title: &str) -> Value {
    json!({
        "detail": { "$ref": crate::schema::openapi::ref_to(detail_title) },
        "account": { "type": "string" },
        "detail-type": { "type": "string" },
        "id": { "type": "string" },
        "region": { "type": "string" },
        "resources": { "type": "array", "items": { "type": "object" } },
        "source": { "type": "string" },
        "time": { "type": "string", "format": "date-time" },
        "version": { "type": "string" }
    })
}

/// Fields the `AWSEvent` envelope must declare as required.
pub const ENVELOPE_REQUIRED: [&str; 9] = [
    "detail-type",
    "resources",
    "detail",
    "id",
    "source",
    "time",
    "region",
    "version",
    "account",
];

/// Build a complete, registry-ready schema document for a new event.
///
/// This is the boilerplate the repo README asks people to copy by hand; the
/// New Schema wizard generates it instead so source, detail-type and detail
/// title can never drift apart.
pub fn new_schema_document(
    identity: &EventIdentity,
    detail_properties: &[(String, String)],
) -> Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<Value> = Vec::new();
    for (name, ty) in detail_properties {
        properties.insert(name.clone(), json!({ "type": ty }));
        // Mirror the registry's simplified convention: only ID-like fields are
        // required. See `simplify.rs`.
        if crate::schema::simplify::is_id_field(name) {
            required.push(Value::String(name.clone()));
        }
    }

    let mut detail = serde_json::Map::new();
    detail.insert("type".into(), json!("object"));
    if !required.is_empty() {
        detail.insert("required".into(), Value::Array(required));
    }
    detail.insert("properties".into(), Value::Object(properties));
    detail.insert("additionalProperties".into(), json!(true));

    document_with_detail(identity, Value::Object(detail))
}

/// Build a document around a ready-made detail schema.
///
/// Used when the payload shape comes from somewhere other than the field
/// wizard — inferred from real events, for instance — so the envelope is still
/// generated consistently rather than assembled by the caller.
pub fn document_with_detail(identity: &EventIdentity, detail: Value) -> Value {
    let title = identity.detail_title();

    json!({
        "openapi": "3.0.0",
        "info": { "version": "1.0.0", "title": title },
        "paths": {},
        "components": {
            "schemas": {
                "AWSEvent": {
                    "type": "object",
                    "required": ENVELOPE_REQUIRED,
                    "x-amazon-events-detail-type": identity.detail_type,
                    "x-amazon-events-source": identity.source,
                    "properties": envelope_properties(&title),
                },
                title: detail,
            }
        }
    })
}

/// Wrap a document in the repo's file format, ready to write to disk.
pub fn to_schema_file(
    schema_name: &str,
    version: &str,
    content: Value,
    last_modified: Option<String>,
) -> SchemaFile {
    SchemaFile {
        schema_name: schema_name.to_string(),
        schema_version: version.to_string(),
        schema_type: "OpenApi3".to_string(),
        last_modified,
        content,
    }
}

/// The repo names files `<source>_<DetailTitle>.json`, e.g.
/// `milo-medical_PacketNotificationAssigned.json`.
pub fn file_name_for(identity: &EventIdentity) -> String {
    format!("{}_{}.json", identity.source, identity.detail_title())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_characters_eventbridge_allows() {
        assert!(invalid_schema_name_chars("milo-medical@packet_v1.2").is_empty());
        assert!(invalid_schema_name_chars("ABC123@xyz").is_empty());
    }

    #[test]
    fn reports_a_space_in_a_source_name() {
        // Real sources on the bus contain spaces ("Atomic Forms"), so an
        // inferred draft can carry a name AWS will always reject.
        assert_eq!(
            invalid_schema_name_chars("Atomic Forms@CASE_STATUS_CHANGED"),
            vec![' ']
        );
    }

    #[test]
    fn lists_each_offending_character_once() {
        assert_eq!(invalid_schema_name_chars("a b/c d/e"), vec![' ', '/']);
    }

    #[test]
    fn sanitizing_produces_a_name_eventbridge_accepts() {
        let name = sanitize_schema_name("Atomic Forms@CASE_STATUS_CHANGED");
        assert_eq!(name, "Atomic-Forms@CASE_STATUS_CHANGED");
        assert!(invalid_schema_name_chars(&name).is_empty());
    }

    #[test]
    fn sanitizing_does_not_leave_runs_of_separators() {
        assert_eq!(sanitize_schema_name("Atomic   Forms@x"), "Atomic-Forms@x");
    }

    #[test]
    fn sanitizing_leaves_a_valid_name_untouched() {
        let name = "milo-medical@packetNotification-assigned";
        assert_eq!(sanitize_schema_name(name), name);
    }

    #[test]
    fn round_trips_schema_names() {
        let id = EventIdentity::from_schema_name("milo-medical@packetNotification-assigned").unwrap();
        assert_eq!(id.source, "milo-medical");
        assert_eq!(id.detail_type, "packetNotification-assigned");
        assert_eq!(id.schema_name(), "milo-medical@packetNotification-assigned");
    }

    #[test]
    fn derives_detail_title_like_eventbridge_codegen() {
        let id = EventIdentity::from_schema_name("milo-medical@packetNotification-assigned").unwrap();
        // Matches the real schema in the repo: title "PacketNotificationAssigned".
        assert_eq!(id.detail_title(), "PacketNotificationAssigned");
    }

    #[test]
    fn derives_file_name_like_the_repo() {
        let id = EventIdentity::from_schema_name("milo-medical@packetNotification-assigned").unwrap();
        assert_eq!(
            file_name_for(&id),
            "milo-medical_PacketNotificationAssigned.json"
        );
    }

    #[test]
    fn rejects_malformed_schema_names() {
        assert!(EventIdentity::from_schema_name("no-separator").is_err());
        assert!(EventIdentity::from_schema_name("@detail").is_err());
        assert!(EventIdentity::from_schema_name("source@").is_err());
    }

    #[test]
    fn parses_content_from_either_representation() {
        let as_object = json!({ "openapi": "3.0.0" });
        let as_string = Value::String(r#"{"openapi":"3.0.0"}"#.into());
        assert_eq!(parse_content(&as_object).unwrap(), as_object);
        assert_eq!(parse_content(&as_string).unwrap(), as_object);
    }

    #[test]
    fn generated_document_is_self_consistent() {
        let id = EventIdentity::from_schema_name("my-service@thing-happened").unwrap();
        let doc = new_schema_document(
            &id,
            &[
                ("veteranId".into(), "string".into()),
                ("note".into(), "string".into()),
            ],
        );

        let schemas = &doc["components"]["schemas"];
        assert_eq!(schemas["AWSEvent"]["x-amazon-events-source"], "my-service");
        assert_eq!(
            schemas["AWSEvent"]["x-amazon-events-detail-type"],
            "thing-happened"
        );
        // The envelope's $ref must point at the generated detail schema.
        assert_eq!(
            schemas["AWSEvent"]["properties"]["detail"]["$ref"],
            "#/components/schemas/ThingHappened"
        );
        assert!(schemas.get("ThingHappened").is_some());
        // Only the ID-like field is required.
        assert_eq!(
            schemas["ThingHappened"]["required"],
            json!(["veteranId"])
        );
        assert_eq!(schemas["ThingHappened"]["additionalProperties"], json!(true));
    }
}
