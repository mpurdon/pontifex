//! Infer a schema from observed event payloads.
//!
//! An event type flowing on the bus with no schema registered is the most
//! actionable thing the health report finds — and the events themselves already
//! describe the shape. Inferring from them turns "you should document this"
//! into a reviewable draft.
//!
//! The output deliberately matches the conventions the registry already uses:
//! `additionalProperties: true`, and only ID-like fields marked required (see
//! `simplify.rs`, ported from the repo's `simplifySchemas.js`).
//!
//! A sometimes-null field is declared `type: T, nullable: true`: the OpenAPI
//! 3.0 spelling the registry stores, which the bus's Ajv honours. (`type:
//! [T, "null"]` means the same and cannot be saved.)

use crate::schema::simplify::is_id_field;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Recognise the string formats OpenAPI defines that we can detect reliably.
///
/// Only claimed when *every* observed value matches: a format that holds for
/// most values but not all would make the schema reject real traffic.
fn detect_format(values: &[&Value]) -> Option<&'static str> {
    let strings: Vec<&str> = values.iter().filter_map(|v| v.as_str()).collect();
    if strings.is_empty() {
        return None;
    }

    let all = |f: fn(&str) -> bool| strings.iter().all(|s| f(s));

    if all(looks_like_date_time) {
        return Some("date-time");
    }
    if all(looks_like_uuid) {
        return Some("uuid");
    }
    if all(|s| s.contains('@') && s.contains('.') && !s.contains(' ') && s.len() > 5) {
        return Some("email");
    }
    None
}

fn looks_like_date_time(s: &str) -> bool {
    // 2026-01-01T12:00:00Z and friends.
    let bytes = s.as_bytes();
    s.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && (bytes[10] == b'T' || bytes[10] == b' ')
        && bytes[13] == b':'
        && s[..4].chars().all(|c| c.is_ascii_digit())
}

fn looks_like_uuid(s: &str) -> bool {
    s.len() == 36
        && s.as_bytes()[8] == b'-'
        && s.as_bytes()[13] == b'-'
        && s.as_bytes()[18] == b'-'
        && s.as_bytes()[23] == b'-'
        && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Build a schema describing every value observed at one position.
///
/// Presence counting happens in the object branch, where it is meaningful:
/// "required" is a statement about a field's parent, not about the field.
fn infer_position(values: &[&Value]) -> Value {
    let non_null: Vec<&&Value> = values.iter().filter(|v| !v.is_null()).collect();
    let saw_null = values.iter().any(|v| v.is_null());

    let mut schema = Map::new();

    if non_null.is_empty() {
        // Only ever null. `type: ["null"]` would be a real constraint asserting
        // the field is *always* null, which the sample does not support — an
        // untyped schema accepts null and everything else, which is the honest
        // reading.
        return Value::Object(schema);
    }

    let mut types: Vec<&'static str> = Vec::new();
    for value in &non_null {
        let t = json_type(value);
        if !types.contains(&t) {
            types.push(t);
        }
    }

    // An integer among numbers is still a number.
    if types.len() == 2 && types.contains(&"integer") && types.contains(&"number") {
        types = vec!["number"];
    }

    if types.len() != 1 {
        // Genuinely mixed: leave it open rather than pick a type that would
        // reject half the traffic.
        return Value::Object(schema);
    }

    let ty = types[0];
    schema.insert("type".into(), Value::String(ty.to_string()));
    if saw_null {
        schema.insert("nullable".into(), Value::Bool(true));
    }

    match ty {
        "object" => {
            let total = non_null.len();
            let mut fields: BTreeMap<String, (Vec<&Value>, usize)> = BTreeMap::new();
            for value in &non_null {
                let Some(map) = value.as_object() else {
                    continue;
                };
                for (key, child) in map {
                    let entry = fields.entry(key.clone()).or_insert((Vec::new(), 0));
                    entry.0.push(child);
                    entry.1 += 1;
                }
            }

            let mut properties = Map::new();
            let mut required: Vec<Value> = Vec::new();
            for (name, (child_values, present_in)) in fields {
                // Match the registry's convention: only ID-like fields are
                // required, and only when every payload actually carried them.
                if is_id_field(&name) && present_in == total {
                    required.push(Value::String(name.clone()));
                }
                properties.insert(name, infer_position(&child_values));
            }

            if !required.is_empty() {
                schema.insert("required".into(), Value::Array(required));
            }
            schema.insert("properties".into(), Value::Object(properties));
            // The registry stores relaxed schemas; a strict one inferred from a
            // sample would reject the first event carrying a new field.
            schema.insert("additionalProperties".into(), Value::Bool(true));
        }
        "array" => {
            let elements: Vec<&Value> = non_null
                .iter()
                .filter_map(|v| v.as_array())
                .flatten()
                .collect();
            schema.insert("items".into(), infer_position(&elements));
        }
        "string" => {
            if let Some(format) = detect_format(&non_null.iter().map(|v| **v).collect::<Vec<_>>()) {
                schema.insert("format".into(), Value::String(format.to_string()));
            }
        }
        _ => {}
    }

    Value::Object(schema)
}

/// Infer a schema for a set of event payloads.
///
/// Returns the schema for the payload object itself — the caller wraps it in
/// the EventBridge envelope.
pub fn infer_payload_schema(payloads: &[Value]) -> Value {
    let refs: Vec<&Value> = payloads.iter().collect();
    infer_position(&refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn infers_scalar_types_from_the_sample() {
        let payloads = vec![
            json!({ "name": "a", "count": 1, "ok": true, "ratio": 1.5 }),
            json!({ "name": "b", "count": 2, "ok": false, "ratio": 2.5 }),
        ];
        let schema = infer_payload_schema(&payloads);
        let props = &schema["properties"];
        assert_eq!(props["name"]["type"], "string");
        assert_eq!(props["count"]["type"], "integer");
        assert_eq!(props["ok"]["type"], "boolean");
        assert_eq!(props["ratio"]["type"], "number");
    }

    #[test]
    fn follows_the_registry_convention_for_required_and_extras() {
        let payloads = vec![
            json!({ "veteranId": "v1", "note": "x" }),
            json!({ "veteranId": "v2", "note": "y" }),
        ];
        let schema = infer_payload_schema(&payloads);
        // Only the ID-like field is required, and extras stay allowed.
        assert_eq!(schema["required"], json!(["veteranId"]));
        assert_eq!(schema["additionalProperties"], json!(true));
    }

    #[test]
    fn does_not_require_an_id_field_that_is_sometimes_absent() {
        let payloads = vec![json!({ "caseId": "c1" }), json!({ "other": 1 })];
        let schema = infer_payload_schema(&payloads);
        assert!(schema.get("required").is_none());
        // It is still declared, just optional.
        assert_eq!(schema["properties"]["caseId"]["type"], "string");
    }

    #[test]
    fn merges_fields_seen_across_different_payloads() {
        let payloads = vec![json!({ "a": "x" }), json!({ "b": 2 })];
        let props = &infer_payload_schema(&payloads)["properties"];
        assert_eq!(props["a"]["type"], "string");
        assert_eq!(props["b"]["type"], "integer");
    }

    #[test]
    fn a_sometimes_null_field_is_nullable_in_the_registrys_spelling() {
        let payloads = vec![json!({ "note": "x" }), json!({ "note": null })];
        let note = &infer_payload_schema(&payloads)["properties"]["note"];
        assert_eq!(note["type"], json!("string"));
        assert_eq!(note["nullable"], json!(true));
    }

    #[test]
    fn a_never_typed_field_carries_no_constraint_at_all() {
        let payloads = vec![json!({ "unknown": null })];
        let field = &infer_payload_schema(&payloads)["properties"]["unknown"];
        assert!(field.get("type").is_none());
        assert!(field.get("nullable").is_none());
    }

    #[test]
    fn an_inferred_draft_accepts_the_events_it_was_inferred_from() {
        // The property this whole change exists to restore, asserted end to end
        // against the bus's own validator rather than by inspecting keywords.
        let payloads = vec![
            json!({ "id": "a", "note": "x", "count": 1 }),
            json!({ "id": "b", "note": null, "count": null }),
        ];
        let schema = infer_payload_schema(&payloads);
        let validator = crate::schema::ajv::validator_for(&schema).unwrap();
        for payload in &payloads {
            assert!(validator.is_valid(payload), "rejected {payload}");
        }
    }

    #[test]
    fn leaves_genuinely_mixed_types_open() {
        // Picking one would reject half the real traffic.
        let payloads = vec![json!({ "v": "text" }), json!({ "v": 42 })];
        let field = &infer_payload_schema(&payloads)["properties"]["v"];
        assert!(field.get("type").is_none());
    }

    #[test]
    fn widens_integers_to_number_when_both_appear() {
        let payloads = vec![json!({ "amount": 1 }), json!({ "amount": 1.5 })];
        assert_eq!(
            infer_payload_schema(&payloads)["properties"]["amount"]["type"],
            "number"
        );
    }

    #[test]
    fn infers_nested_objects() {
        let payloads = vec![
            json!({ "metadata": { "trackingId": "t1", "attempt": 1 } }),
            json!({ "metadata": { "trackingId": "t2", "attempt": 2 } }),
        ];
        let metadata = &infer_payload_schema(&payloads)["properties"]["metadata"];
        assert_eq!(metadata["type"], "object");
        assert_eq!(metadata["properties"]["trackingId"]["type"], "string");
        assert_eq!(metadata["properties"]["attempt"]["type"], "integer");
        assert_eq!(metadata["required"], json!(["trackingId"]));
    }

    #[test]
    fn infers_arrays_from_all_elements() {
        let payloads = vec![json!({ "lines": [{ "sku": "a" }, { "sku": "b", "qty": 2 }] })];
        let lines = &infer_payload_schema(&payloads)["properties"]["lines"];
        assert_eq!(lines["type"], "array");
        assert_eq!(lines["items"]["type"], "object");
        assert_eq!(lines["items"]["properties"]["sku"]["type"], "string");
        assert_eq!(lines["items"]["properties"]["qty"]["type"], "integer");
    }

    #[test]
    fn handles_an_empty_array_without_inventing_an_item_type() {
        let payloads = vec![json!({ "tags": [] })];
        let tags = &infer_payload_schema(&payloads)["properties"]["tags"];
        assert_eq!(tags["type"], "array");
        assert!(tags["items"].get("type").is_none());
    }

    #[test]
    fn detects_date_time_and_uuid_formats() {
        let payloads = vec![
            json!({
                "at": "2026-01-01T12:00:00Z",
                "ref": "3f2504e0-4f89-11d3-9a0c-0305e82c3301"
            }),
            json!({
                "at": "2026-02-03T04:05:06.789Z",
                "ref": "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
            }),
        ];
        let props = &infer_payload_schema(&payloads)["properties"];
        assert_eq!(props["at"]["format"], "date-time");
        assert_eq!(props["ref"]["format"], "uuid");
    }

    #[test]
    fn only_claims_a_format_when_every_value_matches() {
        // One plain string is enough to disqualify the format, or the schema
        // would reject real traffic.
        let payloads = vec![
            json!({ "at": "2026-01-01T12:00:00Z" }),
            json!({ "at": "whenever" }),
        ];
        let at = &infer_payload_schema(&payloads)["properties"]["at"];
        assert_eq!(at["type"], "string");
        assert!(at.get("format").is_none());
    }

    #[test]
    fn infers_an_empty_object_from_no_payloads() {
        let schema = infer_payload_schema(&[]);
        // Nothing observed, so nothing claimed.
        assert!(schema.get("type").is_none());
    }

    #[test]
    fn output_passes_our_own_validator_once_wrapped() {
        use crate::schema::model::{document_with_detail, EventIdentity};
        use crate::schema::validate;

        let payloads = vec![
            json!({ "veteranId": "v1", "at": "2026-01-01T12:00:00Z", "meta": { "a": 1 } }),
            json!({ "veteranId": "v2", "at": "2026-01-02T12:00:00Z", "meta": { "a": 2 } }),
        ];
        let identity = EventIdentity::from_schema_name("my-service@thing-happened").unwrap();
        let document = document_with_detail(&identity, infer_payload_schema(&payloads));

        let report = validate::validate(&document, Some("my-service@thing-happened"));
        assert!(
            report.valid,
            "inferred schema must validate: {:?}",
            report.findings
        );
    }
}
