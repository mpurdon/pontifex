//! Reading an OpenAPI 3.0 schema document as the JSON Schema the bus compiles.
//!
//! The two are close but not identical, and this module used to paper over the
//! difference: `nullable: true` was rewritten to `type: [T, "null"]` before
//! validation, on the reasoning that the registry uses `nullable` 560 times and
//! flagging all of them would drown the reality check.
//!
//! That was the wrong call, and it made pontifex lie in the direction that costs
//! the most. `nullable` is an OpenAPI keyword; Ajv has no implementation of it,
//! so under `strict: 'log'` the bus logs it once and ignores it forever. A
//! field declared `{"type": "string", "nullable": true}` therefore **rejects
//! `null` at the bus** while pontifex called it fine. Those 560 uses are not a
//! reason to suppress the finding — they are the size of the problem.
//!
//! So the conversion no longer happens on the validation path. What is left is
//! [`widen_nullable`], the same rewrite offered as a *repair*: it produces the
//! `type: [T, "null"]` spelling that draft-07 genuinely honours, so a schema
//! that means to accept nulls can be made to actually accept them.

use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::fmt::Write as _;

/// Rewrite `nullable` into the JSON Schema spelling the bus honours.
///
/// - `nullable: true` alongside `type: T` becomes `type: [T, "null"]`
/// - `nullable: true` alongside `enum` gains an explicit `null` member
/// - `nullable` itself is dropped, since JSON Schema has no such keyword
///
/// This is a document repair, not a validation step — see the module note. It
/// is what [`crate::schema::validate`]'s `nullable` finding is telling you to
/// do, mechanised.
///
/// `$ref`s are left alone: the validator resolves them against the document
/// root, so the whole document travels with the subschema.
pub fn widen_nullable(document: &Value) -> Value {
    // Located through [`walk_schemas`], then rewritten in place. The previous
    // form was a second recursive traversal with its own opinion about where a
    // schema is, and it disagreed with the one that produces the finding: it
    // rewrote a `nullable` key sitting inside an `example` payload, corrupting
    // sample data to fix a constraint that was never there.
    let mut sites = Vec::new();
    walk_schemas(document, &mut |pointer, map| {
        if map.contains_key("nullable") {
            sites.push(pointer.to_string());
        }
    });

    let mut out = document.clone();
    for pointer in sites {
        if let Some(map) = out.pointer_mut(&pointer).and_then(Value::as_object_mut) {
            widen_node(map);
        }
    }
    out
}

/// Apply the rewrite to one schema object.
fn widen_node(map: &mut Map<String, Value>) {
    // Dropped whether true or false: neither does anything at the bus, and
    // leaving `nullable: false` behind would keep the editor's warning firing.
    let nullable = map.remove("nullable").and_then(|v| v.as_bool()) == Some(true);
    if !nullable {
        return;
    }

    match map.get("type").cloned() {
        Some(Value::String(t)) => {
            map.insert("type".into(), json!([t, "null"]));
        }
        Some(Value::Array(mut types)) => {
            if !types.iter().any(|t| t == "null") {
                types.push(json!("null"));
            }
            map.insert("type".into(), Value::Array(types));
        }
        // No declared type: nothing to widen, any value is allowed.
        _ => {}
    }

    if let Some(Value::Array(values)) = map.get_mut("enum") {
        if !values.iter().any(Value::is_null) {
            values.push(Value::Null);
        }
    }
}

/// How every intra-document `$ref` in these schemas is spelled.
///
/// The frontend's `schema-model.ts` owns the same constant; both sides needing
/// it is why it is a named constant rather than a literal at each use.
pub const REF_PREFIX: &str = "#/components/schemas/";

/// The type name a `$ref` string points at, if it is an intra-document ref.
pub fn ref_name(reference: &str) -> Option<&str> {
    reference.strip_prefix(REF_PREFIX)
}

/// A `$ref` string pointing at a named type.
pub fn ref_to(type_name: &str) -> String {
    format!("{REF_PREFIX}{type_name}")
}

/// The document's `components.schemas` map.
pub fn component_schemas(document: &Value) -> Option<&serde_json::Map<String, Value>> {
    document
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(Value::as_object)
}

/// One named type out of `components.schemas`.
pub fn component_schema<'a>(document: &'a Value, type_name: &str) -> Option<&'a Value> {
    component_schemas(document).and_then(|s| s.get(type_name))
}

/// Escape a property name for use as a JSON Pointer segment.
pub fn escape_segment(name: &str) -> Cow<'_, str> {
    // Borrowed in the overwhelmingly common case: `str::replace` allocates even
    // when it matches nothing, and this runs for every property of every schema
    // on the keystroke path.
    if name.contains(['~', '/']) {
        Cow::Owned(name.replace('~', "~0").replace('/', "~1"))
    } else {
        Cow::Borrowed(name)
    }
}

/// Visit every object in `document` that is positioned as a schema.
///
/// The one traversal policy for the whole module. Structural rather than
/// blind: it descends only the keys whose values are schemas, so an `example`
/// payload's own `format` or `nullable` key is not mistaken for a constraint.
/// Everything that reads or rewrites schema keywords goes through this, which
/// is what keeps the reports and the repairs describing the same set of nodes.
///
/// The visitor is handed a JSON Pointer valid against `document`.
pub fn walk_schemas(document: &Value, visit: &mut impl FnMut(&str, &Map<String, Value>)) {
    let mut pointer = String::new();
    match component_schemas(document) {
        Some(schemas) => {
            for (name, schema) in schemas {
                pointer.clear();
                pointer.push_str("/components/schemas/");
                pointer.push_str(&escape_segment(name));
                walk_schema(schema, &mut pointer, visit);
            }
        }
        // Not an OpenAPI document — a bare JSON Schema, as the JSON view allows
        // mid-edit, and as this module's own tests pass in. Treat it as one.
        None => walk_schema(document, &mut pointer, visit),
    }
}

/// Walk one schema and everything nested inside it.
///
/// `pointer` is grown and truncated in place rather than re-formatted per node:
/// a pointer is O(depth) bytes and there are thousands of nodes per document,
/// so building a fresh `String` per edge was the bulk of this traversal's cost
/// on documents where nothing is ever reported.
fn walk_schema(
    value: &Value,
    pointer: &mut String,
    visit: &mut impl FnMut(&str, &Map<String, Value>),
) {
    let Some(map) = value.as_object() else { return };
    visit(pointer, map);

    const MAPS: [&str; 3] = ["properties", "definitions", "patternProperties"];
    const SINGLE: [&str; 5] = [
        "items",
        "additionalProperties",
        "not",
        "contains",
        "propertyNames",
    ];
    const LISTS: [&str; 3] = ["oneOf", "anyOf", "allOf"];

    // Each descent rewinds to here and appends its own segment, so the pointer
    // is extended in place rather than rebuilt.
    let base = pointer.len();

    for key in MAPS {
        if let Some(Value::Object(children)) = map.get(key) {
            for (name, child) in children {
                pointer.truncate(base);
                let _ = write!(pointer, "/{key}/{}", escape_segment(name));
                walk_schema(child, pointer, visit);
            }
        }
    }
    for key in SINGLE {
        if let Some(child) = map.get(key) {
            pointer.truncate(base);
            let _ = write!(pointer, "/{key}");
            walk_schema(child, pointer, visit);
        }
    }
    for key in LISTS {
        if let Some(Value::Array(branches)) = map.get(key) {
            for (index, branch) in branches.iter().enumerate() {
                pointer.truncate(base);
                let _ = write!(pointer, "/{key}/{index}");
                walk_schema(branch, pointer, visit);
            }
        }
    }
    pointer.truncate(base);
}

/// Every JSON Pointer in `document` at which `nullable: true` appears.
///
/// The bus ignores all of them, so this is the list of places a `null` would be
/// rejected in production despite the schema appearing to allow it.
pub fn nullable_sites(document: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk_schemas(document, &mut |pointer, map| {
        if map.get("nullable").and_then(Value::as_bool) == Some(true) {
            out.push(pointer.to_string());
        }
    });
    out
}

/// Build a standalone JSON Schema that validates against one named type.
///
/// The whole document rides along so internal `$ref`s resolve; the root is just
/// a pointer at the type of interest.
///
/// The document is passed through **unaltered**. Anything OpenAPI-only in it —
/// `nullable`, `discriminator`, `xml`, `example` — is a keyword draft-07 does
/// not define, so both Ajv and this crate ignore it. Rewriting any of it here
/// would grade events under a schema the bus never sees.
pub fn schema_for_type(document: &Value, type_name: &str) -> Value {
    let mut root = json!({ "$ref": ref_to(type_name) });

    // Only `components` is cloned. Cloning the whole document and then removing
    // everything but this subtree copied `openapi`, `info` and `paths` for
    // nothing, on a path that re-runs while you type.
    if let Some(components) = document.get("components") {
        root.as_object_mut()
            .expect("object literal")
            .insert("components".into(), components.clone());
    }
    root
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widens_a_typed_nullable_field() {
        let input = json!({ "type": "string", "nullable": true });
        assert_eq!(widen_nullable(&input), json!({ "type": ["string", "null"] }));
    }

    #[test]
    fn drops_nullable_false_without_widening() {
        let input = json!({ "type": "string", "nullable": false });
        assert_eq!(widen_nullable(&input), json!({ "type": "string" }));
    }

    #[test]
    fn leaves_untyped_nullable_alone() {
        // Nothing to widen; the field already accepts anything.
        let input = json!({ "nullable": true, "description": "x" });
        assert_eq!(widen_nullable(&input), json!({ "description": "x" }));
    }

    #[test]
    fn adds_null_to_a_nullable_enum() {
        let input = json!({ "type": "string", "enum": ["a", "b"], "nullable": true });
        assert_eq!(
            widen_nullable(&input),
            json!({ "type": ["string", "null"], "enum": ["a", "b", null] })
        );
    }

    #[test]
    fn converts_nested_and_array_members() {
        let input = json!({
            "type": "object",
            "properties": {
                "a": { "type": "string", "nullable": true },
                "list": {
                    "type": "array",
                    "items": { "type": "integer", "nullable": true }
                }
            }
        });
        let out = widen_nullable(&input);
        assert_eq!(out["properties"]["a"]["type"], json!(["string", "null"]));
        assert_eq!(
            out["properties"]["list"]["items"]["type"],
            json!(["integer", "null"])
        );
    }

    #[test]
    fn preserves_refs_verbatim() {
        let input = json!({ "$ref": "#/components/schemas/Thing" });
        assert_eq!(widen_nullable(&input), input);
    }

    #[test]
    fn an_example_payload_is_data_and_is_left_alone() {
        // `nullable` inside an `example` is sample data, not a constraint.
        // Reporting it would be noise; rewriting it — which the old blind walk
        // did — corrupted the sample to fix a constraint that was never there.
        let document = json!({
            "components": { "schemas": { "Thing": {
                "type": "object",
                "example": { "nullable": true, "type": "string" },
                "properties": { "real": { "type": "string", "nullable": true } }
            }}}
        });

        assert_eq!(
            nullable_sites(&document),
            vec!["/components/schemas/Thing/properties/real"]
        );

        let repaired = widen_nullable(&document);
        assert_eq!(
            repaired["components"]["schemas"]["Thing"]["example"],
            json!({ "nullable": true, "type": "string" }),
            "the example payload must survive untouched"
        );
        assert_eq!(
            repaired["components"]["schemas"]["Thing"]["properties"]["real"],
            json!({ "type": ["string", "null"] })
        );
    }

    #[test]
    fn a_property_named_with_a_slash_gets_an_escaped_pointer() {
        let document = json!({
            "components": { "schemas": { "Thing": {
                "type": "object",
                "properties": { "a/b": { "type": "string", "nullable": true } }
            }}}
        });
        let sites = nullable_sites(&document);
        assert_eq!(sites, vec!["/components/schemas/Thing/properties/a~1b"]);
        // And the pointer has to be the one `pointer_mut` accepts, or the
        // repair silently skips the field it just reported.
        assert_eq!(
            widen_nullable(&document)["components"]["schemas"]["Thing"]["properties"]["a/b"],
            json!({ "type": ["string", "null"] })
        );
    }

    #[test]
    fn nullable_sites_names_every_offender() {
        let document = json!({
            "components": { "schemas": {
                "Thing": {
                    "type": "object",
                    "properties": {
                        "note": { "type": "string", "nullable": true },
                        "tags": { "type": "array", "items": { "type": "string", "nullable": true } },
                        "id": { "type": "string" },
                        "old": { "type": "string", "nullable": false }
                    }
                }
            }}
        });
        assert_eq!(
            nullable_sites(&document),
            vec![
                "/components/schemas/Thing/properties/note",
                "/components/schemas/Thing/properties/tags/items",
            ]
        );
    }

    #[test]
    fn schema_for_type_carries_the_components_along() {
        let document = json!({
            "components": {
                "schemas": {
                    "Thing": {
                        "type": "object",
                        "properties": { "id": { "type": "string", "nullable": true } }
                    }
                }
            }
        });
        let schema = schema_for_type(&document, "Thing");
        assert_eq!(schema["$ref"], "#/components/schemas/Thing");
        // Verbatim, `nullable` and all: the bus compiles the document as
        // written, so grading against a tidied-up copy would grade the wrong
        // schema. See the module note.
        assert_eq!(
            schema["components"]["schemas"]["Thing"]["properties"]["id"],
            json!({ "type": "string", "nullable": true })
        );
    }
}
