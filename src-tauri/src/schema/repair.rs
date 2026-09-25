//! Turning a drift issue into the edit that clears it.
//!
//! The analysis panel could always say what was wrong and how much traffic it
//! affected, and then left you to make the edit by hand — for a schema with
//! forty drifted fields, forty times. A repair is that edit, computed from the
//! same observation that produced the issue.
//!
//! Each repair carries its own operands rather than being re-derived from the
//! issue's prose. The summary and action lines are written for a reader and are
//! lossy on purpose (`render_value` quotes values, types are joined with
//! `" | "`); parsing them back to decide what to change would make rewording a
//! sentence silently change what a button does. Commit `0bf7ba5` established
//! the same rule for the editor's `Finding.fix`, and for the same reason.

use crate::error::{Error, Result};
use crate::schema::events;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The edit that clears one issue, with everything it needs to apply itself.
///
/// Deserialize as well as Serialize: this makes the round trip out to the panel
/// and back as the subject of a click, and it should be the same repair that
/// was offered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Repair {
    /// Redeclare a field as the types real events carry.
    WidenType { types: Vec<String> },
    /// Admit values the producers already send.
    ExtendEnum { values: Vec<Value> },
    /// Stop requiring a field that events omit.
    DropRequired,
    /// Refuse a blank string in a field that producers send empty.
    RequireNonEmpty,
    /// Pin a field to the shape its values actually have.
    ///
    /// The repair for a rejection the events cannot satisfy — a `format` no
    /// producer honours — where the values still have a structure worth
    /// declaring. Replaces the constraint that rejects rather than removing
    /// it: a plain string would accept the empty string and a sentence.
    ConstrainPattern { pattern: String },
    /// Declare a field that events send and the schema does not describe.
    DeclareField {
        types: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        example: Option<Value>,
    },
}

/// Keywords that only describe an object, and mean nothing once it is not one.
///
/// Left behind, they are not merely noise: `additionalProperties: false` beside
/// `type: string, nullable: true` still rejects nothing, but the next reader
/// has to work out which half of the declaration is live.
const OBJECT_ONLY: [&str; 5] = [
    "properties",
    "additionalProperties",
    "required",
    "minProperties",
    "maxProperties",
];

/// Keywords that only describe an array.
const ARRAY_ONLY: [&str; 4] = ["items", "minItems", "maxItems", "uniqueItems"];

/// Apply a repair to the type that owns `path`.
///
/// Returns a new document; the caller decides whether to keep it. Nothing here
/// touches AWS — the result is a draft, reviewed as a diff like any other edit.
pub fn apply(document: &Value, type_name: &str, path: &str, repair: &Repair) -> Result<Value> {
    let (pointer, leaf) = events::resolve_owner(document, type_name, path).ok_or_else(|| {
        Error::Invalid(format!(
            "Could not work out which type owns `{path}` — edit it by hand in the JSON view"
        ))
    })?;

    let mut next = document.clone();

    match repair {
        Repair::DropRequired => drop_required(&mut next, &pointer, &leaf, path),
        Repair::RequireNonEmpty => require_non_empty(&mut next, &pointer, &leaf, path),
        Repair::WidenType { types } => widen_type(&mut next, &pointer, &leaf, path, types),
        Repair::ExtendEnum { values } => extend_enum(&mut next, &pointer, &leaf, path, values),
        Repair::ConstrainPattern { pattern } => {
            constrain_pattern(&mut next, &pointer, &leaf, path, pattern)
        }
        Repair::DeclareField { types, .. } => declare_field(&mut next, &pointer, &leaf, types),
    }?;

    Ok(next)
}

/// The declaration currently standing at a path, if there is one.
///
/// Given to a model alongside the finding so it reasons about the document
/// rather than about the summary's paraphrase of it — the summary says
/// "declared object", which does not distinguish an inline object from a `$ref`
/// to a shared one, and the two want different repairs.
pub fn declaration_at(document: &Value, type_name: &str, path: &str) -> Option<Value> {
    let (pointer, leaf) = events::resolve_owner(document, type_name, path)?;
    document
        .pointer(&pointer)?
        .get("properties")?
        .get(&leaf)
        .cloned()
}

/// The property object a repair edits, created only where that makes sense.
fn property_mut<'a>(
    document: &'a mut Value,
    pointer: &str,
    leaf: &str,
    path: &str,
) -> Result<&'a mut Map<String, Value>> {
    let owner = document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::Invalid(format!("{pointer} is not an object")))?;

    let property = owner
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut(leaf))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "`{path}` is not declared, so there is nothing to change"
            ))
        })?;

    // A `$ref` names a type shared with every other field pointing at it.
    // Rewriting through it would repair this field by changing all of them, so
    // it is refused rather than done quietly.
    if property.get("$ref").is_some() {
        return Err(Error::Invalid(format!(
            "`{path}` is declared by reference to a shared type. Change that type directly, \
             so the effect on everything else pointing at it is visible."
        )));
    }

    // Rendered before the mutable borrow, so the message can name what it found.
    let rendered = property.to_string();
    property.as_object_mut().ok_or_else(|| {
        Error::Invalid(format!(
            "`{path}` is declared as {rendered}, which is not an object"
        ))
    })
}

/// Declare the shape a field's values have, in place of the check they fail.
fn constrain_pattern(
    document: &mut Value,
    pointer: &str,
    leaf: &str,
    path: &str,
    pattern: &str,
) -> Result<()> {
    if pattern.is_empty() {
        return Err(Error::Invalid(format!(
            "No pattern for `{path}` to be constrained to"
        )));
    }
    let property = property_mut(document, pointer, leaf, path)?;

    // The format is what rejects the events, so leaving it beside the pattern
    // would repair nothing: `format: uuid` and a pattern the uuids do not
    // match reject exactly what they rejected before.
    property.remove("format");
    property.insert("pattern".into(), json!(pattern));
    // A pattern only means anything on a string, and the field is one — these
    // are values the producers sent.
    property
        .entry("type".to_string())
        .or_insert_with(|| json!("string"));
    Ok(())
}

/// Rewrite a field's type to what events actually carry.
fn widen_type(
    document: &mut Value,
    pointer: &str,
    leaf: &str,
    path: &str,
    types: &[String],
) -> Result<()> {
    if types.is_empty() {
        return Err(Error::Invalid(format!(
            "No observed type for `{path}` to widen to"
        )));
    }

    let property = property_mut(document, pointer, leaf, path)?;

    declare_types(property, types);

    if !types.iter().any(|t| t == "object") {
        for keyword in OBJECT_ONLY {
            property.remove(keyword);
        }
    }
    if !types.iter().any(|t| t == "array") {
        for keyword in ARRAY_ONLY {
            property.remove(keyword);
        }
    }

    Ok(())
}

/// `minLength: 1`, so `""` no longer satisfies a required string.
///
/// A stricter bound already there is left alone: a field that must be at
/// least four characters is already non-empty.
fn require_non_empty(
    document: &mut Value,
    pointer: &str,
    leaf: &str,
    path: &str,
) -> Result<()> {
    let property = property_mut(document, pointer, leaf, path)?;
    let current = property.get("minLength").and_then(Value::as_u64).unwrap_or(0);
    if current < 1 {
        property.insert("minLength".to_string(), json!(1));
    }
    Ok(())
}

/// Add observed values to a declared enum.
fn extend_enum(
    document: &mut Value,
    pointer: &str,
    leaf: &str,
    path: &str,
    values: &[Value],
) -> Result<()> {
    let property = property_mut(document, pointer, leaf, path)?;

    let existing = property
        .get("enum")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut widened = existing;
    for value in values {
        // Comparing whole JSON values, so `"1"` and `1` stay distinct — which
        // for an enum drift is frequently the entire finding.
        if !widened.contains(value) {
            widened.push(value.clone());
        }
    }

    property.insert("enum".into(), Value::Array(widened));
    Ok(())
}

/// Stop requiring a field, leaving its declaration alone.
fn drop_required(document: &mut Value, pointer: &str, leaf: &str, path: &str) -> Result<()> {
    let owner = document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::Invalid(format!("{pointer} is not an object")))?;

    let Some(required) = owner.get_mut("required").and_then(Value::as_array_mut) else {
        return Err(Error::Invalid(format!(
            "`{path}` is not in a `required` list, so it is already optional"
        )));
    };

    let before = required.len();
    required.retain(|entry| entry.as_str() != Some(leaf));
    if required.len() == before {
        return Err(Error::Invalid(format!("`{path}` is already optional")));
    }

    // An empty `required: []` validates the same as no `required` at all, and
    // reads as though someone meant something by it.
    if required.is_empty() {
        owner.remove("required");
    }

    Ok(())
}

/// Declare a field the schema does not describe.
fn declare_field(document: &mut Value, pointer: &str, leaf: &str, types: &[String]) -> Result<()> {
    let owner = document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::Invalid(format!("{pointer} is not an object")))?;

    let properties = owner
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("`properties` is not an object".into()))?;

    let mut declaration = Map::new();
    if !types.is_empty() {
        declare_types(&mut declaration, types);
        // An observed object conveys nothing about its inner shape, so leave it
        // open rather than inventing one. Same choice `declaration_for` makes.
        if types.iter().any(|t| t == "object") {
            declaration.insert("additionalProperties".into(), Value::Bool(true));
        }
    }

    properties.insert(leaf.to_string(), Value::Object(declaration));
    Ok(())
}

/// Write a set of JSON types onto a schema object in OpenAPI 3.0's spelling,
/// which is what the registry stores and what the bus's Ajv honours.
///
/// One type is `type: T`; "or null" is `nullable: true`; a genuine union is
/// `anyOf` of single types, since OpenAPI 3.0 has no type list. Whatever
/// spelling was there before — a list, a stale `nullable` — is replaced.
pub(crate) fn declare_types(schema: &mut Map<String, Value>, types: &[String]) {
    let nullable = types.iter().any(|t| t == "null");
    let named: Vec<&String> = types.iter().filter(|t| *t != "null").collect();
    schema.remove("type");
    schema.remove("anyOf");
    match named.as_slice() {
        [] => {}
        [single] => {
            schema.insert("type".into(), Value::String((*single).clone()));
        }
        many => {
            schema.insert(
                "anyOf".into(),
                Value::Array(many.iter().map(|t| json!({ "type": t })).collect()),
            );
        }
    }
    if nullable {
        schema.insert("nullable".into(), Value::Bool(true));
    } else {
        schema.remove("nullable");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "components": {"schemas": {
                "Event": {
                    "type": "object",
                    "required": ["id", "status"],
                    "properties": {
                        "id": {"type": "string"},
                        "status": {"type": "string", "enum": ["open", "closed"]},
                        "country": {
                            "type": "object",
                            "properties": {"code": {"type": "string"}},
                            "additionalProperties": false,
                            "nullable": true
                        },
                        "address": {"$ref": "#/components/schemas/Address"}
                    }
                },
                "Address": {"type": "object", "properties": {"street": {"type": "object"}}}
            }}
        })
    }

    fn apply_to(document: &Value, path: &str, repair: &Repair) -> Result<Value> {
        apply(document, "Event", path, repair)
    }

    fn property<'a>(document: &'a Value, pointer: &str) -> &'a Value {
        document.pointer(pointer).expect("property missing")
    }

    #[test]
    fn constraining_to_a_pattern_replaces_the_format_that_rejects() {
        let doc = json!({
            "components": { "schemas": { "T": {
                "type": "object",
                "properties": { "claimId": { "type": "string", "format": "uuid" } }
            }}}
        });

        let next = apply(
            &doc,
            "T",
            "claimId",
            &Repair::ConstrainPattern {
                pattern: r"^\d{7}-[0-9a-f]{16}$".into(),
            },
        )
        .unwrap();

        let field = &next["components"]["schemas"]["T"]["properties"]["claimId"];
        assert_eq!(field["pattern"], r"^\d{7}-[0-9a-f]{16}$");
        assert_eq!(field["type"], "string");
        // Left in place, the format would reject exactly what it rejected
        // before and the repair would have changed nothing.
        assert!(field.get("format").is_none(), "{field}");
    }

    #[test]
    fn widening_a_type_replaces_it_with_what_was_observed() {
        let repair = Repair::WidenType {
            types: vec!["string".into(), "null".into()],
        };
        let next = apply_to(&document(), "country", &repair).unwrap();

        let country = property(&next, "/components/schemas/Event/properties/country");
        assert_eq!(country["type"], json!("string"));
        assert_eq!(country["nullable"], json!(true));
    }

    #[test]
    fn a_genuine_union_is_written_as_any_of() {
        // OpenAPI 3.0 has no type list; the registry refuses one.
        let repair = Repair::WidenType {
            types: vec!["string".into(), "integer".into(), "null".into()],
        };
        let next = apply_to(&document(), "id", &repair).unwrap();
        let id = property(&next, "/components/schemas/Event/properties/id");
        assert!(id.get("type").is_none());
        assert_eq!(
            id["anyOf"],
            json!([{ "type": "string" }, { "type": "integer" }])
        );
        assert_eq!(id["nullable"], json!(true));
    }

    #[test]
    fn widening_clears_the_object_keywords_it_leaves_behind() {
        // The stale half of the declaration is the part that makes a repaired
        // schema unreadable: `additionalProperties: false` beside a string type.
        let repair = Repair::WidenType {
            types: vec!["string".into()],
        };
        let next = apply_to(&document(), "country", &repair).unwrap();
        let country = property(&next, "/components/schemas/Event/properties/country");

        assert_eq!(country.get("type"), Some(&json!("string")));
        assert!(country.get("properties").is_none());
        assert!(country.get("additionalProperties").is_none());
        // Observed as a string only, so the old `nullable` goes too.
        assert!(country.get("nullable").is_none());
    }

    #[test]
    fn a_single_observed_type_is_written_as_a_string_not_a_list() {
        let repair = Repair::WidenType {
            types: vec!["string".into()],
        };
        let next = apply_to(&document(), "id", &repair).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Event/properties/id/type"),
            &json!("string")
        );
    }

    #[test]
    fn widening_a_shared_type_is_refused_rather_than_done_quietly() {
        // Repairing one field by silently changing every field that refs the
        // same type is the one outcome nobody could have predicted from the row.
        let repair = Repair::WidenType {
            types: vec!["string".into()],
        };
        let error = apply_to(&document(), "address", &repair).unwrap_err();

        assert!(
            error.to_string().contains("shared type"),
            "expected a refusal naming the shared type, got: {error}"
        );
    }

    #[test]
    fn widening_reaches_through_a_ref_to_the_type_that_owns_the_field() {
        // `address.street` lives on Address, not Event. Following the ref is the
        // difference between repairing the field and inventing a new one.
        let repair = Repair::WidenType {
            types: vec!["string".into()],
        };
        let next = apply_to(&document(), "address.street", &repair).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Address/properties/street/type"),
            &json!("string")
        );
    }

    #[test]
    fn extending_an_enum_keeps_the_declared_values() {
        let repair = Repair::ExtendEnum {
            values: vec![json!("archived")],
        };
        let next = apply_to(&document(), "status", &repair).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Event/properties/status/enum"),
            &json!(["open", "closed", "archived"])
        );
    }

    #[test]
    fn extending_an_enum_does_not_duplicate_a_value_already_allowed() {
        let repair = Repair::ExtendEnum {
            values: vec![json!("open"), json!("archived")],
        };
        let next = apply_to(&document(), "status", &repair).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Event/properties/status/enum"),
            &json!(["open", "closed", "archived"])
        );
    }

    #[test]
    fn a_string_and_a_number_of_the_same_digits_stay_distinct() {
        let repair = Repair::ExtendEnum {
            values: vec![json!(1)],
        };
        let base = json!({"components": {"schemas": {"Event": {"properties": {
            "code": {"enum": ["1"]}
        }}}}});
        let next = apply_to(&base, "code", &repair).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Event/properties/code/enum"),
            &json!(["1", 1])
        );
    }

    #[test]
    fn requiring_non_empty_sets_a_minimum_length() {
        let next = apply_to(&document(), "status", &Repair::RequireNonEmpty).unwrap();
        let status = property(&next, "/components/schemas/Event/properties/status");
        assert_eq!(status.get("minLength"), Some(&json!(1)));
        // Still required, still an enum — only no longer satisfiable by "".
        assert!(status.get("enum").is_some());
        assert_eq!(
            property(&next, "/components/schemas/Event/required"),
            &json!(["id", "status"])
        );
    }

    #[test]
    fn requiring_non_empty_keeps_a_stricter_bound() {
        let mut base = document();
        base["components"]["schemas"]["Event"]["properties"]["status"]["minLength"] = json!(4);
        let next = apply_to(&base, "status", &Repair::RequireNonEmpty).unwrap();
        assert_eq!(
            property(&next, "/components/schemas/Event/properties/status").get("minLength"),
            Some(&json!(4))
        );
    }

    #[test]
    fn dropping_required_leaves_the_declaration_alone() {
        let next = apply_to(&document(), "status", &Repair::DropRequired).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Event/required"),
            &json!(["id"])
        );
        // Still declared, still an enum — only no longer mandatory.
        assert!(
            property(&next, "/components/schemas/Event/properties/status")
                .get("enum")
                .is_some()
        );
    }

    #[test]
    fn an_emptied_required_list_is_removed_rather_than_left_saying_nothing() {
        let base = json!({"components": {"schemas": {"Event": {
            "required": ["id"],
            "properties": {"id": {"type": "string"}}
        }}}});
        let next = apply_to(&base, "id", &Repair::DropRequired).unwrap();

        assert!(property(&next, "/components/schemas/Event")
            .get("required")
            .is_none());
    }

    #[test]
    fn dropping_required_on_an_optional_field_says_so() {
        let error = apply_to(&document(), "country", &Repair::DropRequired).unwrap_err();
        assert!(
            error.to_string().contains("already optional"),
            "got: {error}"
        );
    }

    #[test]
    fn declaring_adds_the_field_with_its_observed_type() {
        let repair = Repair::DeclareField {
            types: vec!["string".into(), "null".into()],
            example: Some(json!("W_3M43Y")),
        };
        let next = apply_to(&document(), "ringbaId", &repair).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Event/properties/ringbaId"),
            &json!({ "type": "string", "nullable": true })
        );
    }

    #[test]
    fn declaring_an_observed_object_leaves_its_shape_open() {
        let repair = Repair::DeclareField {
            types: vec!["object".into()],
            example: None,
        };
        let next = apply_to(&document(), "metadata", &repair).unwrap();

        assert_eq!(
            property(&next, "/components/schemas/Event/properties/metadata"),
            &json!({"type": "object", "additionalProperties": true})
        );
    }

    #[test]
    fn repairing_an_undeclared_field_says_so_rather_than_creating_one() {
        let repair = Repair::WidenType {
            types: vec!["string".into()],
        };
        let error = apply_to(&document(), "nope", &repair).unwrap_err();
        assert!(error.to_string().contains("not declared"), "got: {error}");
    }

    #[test]
    fn an_array_element_has_no_property_to_repair() {
        let repair = Repair::WidenType {
            types: vec!["string".into()],
        };
        let error = apply_to(&document(), "tags[]", &repair).unwrap_err();
        assert!(
            error.to_string().contains("edit it by hand"),
            "got: {error}"
        );
    }

    #[test]
    fn a_repair_leaves_the_document_it_was_given_untouched() {
        let before = document();
        let repair = Repair::WidenType {
            types: vec!["string".into()],
        };
        let _ = apply_to(&before, "country", &repair).unwrap();

        assert_eq!(before, document());
    }
}
