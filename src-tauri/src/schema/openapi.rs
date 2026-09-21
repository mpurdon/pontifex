//! Reading an OpenAPI 3.0 schema document as the JSON Schema the bus compiles.
//!
//! The two are close but not identical, and the registry and the bus each
//! enforce their own side of the difference:
//!
//! - **The registry** stores the document as OpenAPI 3.0 and validates it as
//!   such on every write. A Schema Object there has one `type`, never a list
//!   and never `null`; "or null" is spelled `nullable: true`. A document with
//!   `type: ["string", "null"]` is refused with `BadRequestException: Content
//!   is not valid OpenAPI 3.0`, and the message names only the component, not
//!   the field. [`to_openapi_30`] rewrites the JSON Schema spellings into the
//!   OpenAPI ones, and `validate` reports what it cannot.
//! - **The bus** compiles with Ajv 8, which honours `nullable` (its docs:
//!   "the nullable keyword is supported by default"): `{"type": "string",
//!   "nullable": true}` accepts `null`, exactly as `["string", "null"]` would.
//!   The Rust validator here does not know the keyword, so [`widen_nullable`]
//!   rewrites it into the draft-07 spelling before anything is graded — on the
//!   validation path only, never into a document.
//!
//! This module used to hold the opposite position — that Ajv ignored
//! `nullable` and the type list was the only spelling that worked — and wrote
//! type lists into drafts, which the registry then refused to save. The
//! correction is verified against the bus's own Ajv configuration.

use serde_json::{json, Map, Value};
use std::borrow::Cow;
use std::fmt::Write as _;

/// Rewrite `nullable` into the JSON Schema spelling the Rust validator knows,
/// so it grades the way Ajv does.
///
/// - `nullable: true` alongside `type: T` becomes `type: [T, "null"]`
/// - `nullable: true` alongside `enum` gains an explicit `null` member
/// - `nullable` itself is dropped, since JSON Schema has no such keyword
///
/// For the validator only: a document written this way cannot be saved.
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

/// Keywords an OpenAPI 3.0 Schema Object may carry. Anything else — bar
/// `x-` extensions — fails the registry's validation of the document.
pub const OPENAPI_30_KEYWORDS: [&str; 36] = [
    "$ref",
    "title",
    "multipleOf",
    "maximum",
    "exclusiveMaximum",
    "minimum",
    "exclusiveMinimum",
    "maxLength",
    "minLength",
    "pattern",
    "maxItems",
    "minItems",
    "uniqueItems",
    "maxProperties",
    "minProperties",
    "required",
    "enum",
    "type",
    "allOf",
    "oneOf",
    "anyOf",
    "not",
    "items",
    "properties",
    "additionalProperties",
    "description",
    "format",
    "default",
    "nullable",
    "discriminator",
    "readOnly",
    "writeOnly",
    "xml",
    "externalDocs",
    "example",
    "deprecated",
];

/// Whether a keyword may appear in an OpenAPI 3.0 Schema Object.
pub fn is_openapi_30_keyword(keyword: &str) -> bool {
    keyword.starts_with("x-") || OPENAPI_30_KEYWORDS.contains(&keyword)
}

/// Rewrite the JSON Schema spellings the registry refuses into the OpenAPI
/// 3.0 ones it stores, and say where.
///
/// - `type: [T, "null"]` becomes `type: T, nullable: true`
/// - `type: [T, U, …]` becomes `anyOf: [{type: T}, {type: U}, …]`, with
///   `nullable: true` when `null` was among them
/// - `type: "null"` becomes `nullable: true` with no `type` — nothing in
///   OpenAPI 3.0 asserts a value is always null
/// - `const: v` becomes `enum: [v]`; `examples: [v, …]` becomes `example: v`
/// - `$schema`, `$id`, `id` and `$comment` are dropped
///
/// Everything else the registry would refuse — a tuple `items`, a keyword
/// OpenAPI 3.0 does not have — is left for `validate` to name, since there is
/// no rewrite that keeps its meaning.
pub fn to_openapi_30(document: &Value) -> (Value, Vec<String>) {
    let mut sites = Vec::new();
    walk_schemas(document, &mut |pointer, map| {
        if needs_openapi_30(map) {
            sites.push(pointer.to_string());
        }
    });

    let mut out = document.clone();
    for pointer in &sites {
        if let Some(map) = out.pointer_mut(pointer).and_then(Value::as_object_mut) {
            openapi_30_node(map);
        }
    }
    (out, sites)
}

fn needs_openapi_30(map: &Map<String, Value>) -> bool {
    matches!(map.get("type"), Some(Value::Array(_)))
        || map.get("type").and_then(Value::as_str) == Some("null")
        || map.contains_key("const")
        || map.contains_key("examples")
        || ["$schema", "$id", "id", "$comment"]
            .iter()
            .any(|k| map.contains_key(*k))
}

fn openapi_30_node(map: &mut Map<String, Value>) {
    for key in ["$schema", "$id", "id", "$comment"] {
        map.remove(key);
    }
    if let Some(value) = map.remove("const") {
        map.entry("enum").or_insert_with(|| json!([value]));
    }
    if let Some(Value::Array(examples)) = map.remove("examples") {
        if let Some(first) = examples.into_iter().next() {
            map.entry("example").or_insert(first);
        }
    }

    let types: Vec<String> = match map.get("type") {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(t)) if t == "null" => vec!["null".into()],
        _ => return,
    };
    let nullable = types.iter().any(|t| t == "null");
    let named: Vec<&String> = types.iter().filter(|t| *t != "null").collect();
    match named.as_slice() {
        [] => {
            map.remove("type");
        }
        [single] => {
            map.insert("type".into(), Value::String((*single).clone()));
        }
        many => {
            map.remove("type");
            map.insert(
                "anyOf".into(),
                Value::Array(many.iter().map(|t| json!({ "type": t })).collect()),
            );
        }
    }
    if nullable {
        map.insert("nullable".into(), Value::Bool(true));
    }
}

/// Build a standalone JSON Schema that validates against one named type.
///
/// The whole document rides along so internal `$ref`s resolve; the root is just
/// a pointer at the type of interest.
///
/// The document is passed through unaltered here; `ajv::validator_for` applies
/// [`widen_nullable`] before compiling, which is the one OpenAPI keyword Ajv
/// honours. The rest — `discriminator`, `xml`, `example` — draft-07 does not
/// define, so both Ajv and this crate ignore them.
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
    fn type_lists_become_nullable_or_any_of() {
        let (out, changed) = to_openapi_30(&json!({ "components": { "schemas": { "T": {
            "type": "object",
            "properties": {
                "a": { "type": ["string", "null"], "maxLength": 3 },
                "b": { "type": ["string", "integer"] },
                "c": { "type": ["string", "integer", "null"] },
                "d": { "type": "null" },
                "e": { "type": "string" }
            }
        }}}}));
        let props = &out["components"]["schemas"]["T"]["properties"];
        assert_eq!(
            props["a"],
            json!({ "type": "string", "nullable": true, "maxLength": 3 })
        );
        assert_eq!(
            props["b"],
            json!({ "anyOf": [{ "type": "string" }, { "type": "integer" }] })
        );
        assert_eq!(
            props["c"],
            json!({ "anyOf": [{ "type": "string" }, { "type": "integer" }], "nullable": true })
        );
        assert_eq!(props["d"], json!({ "nullable": true }));
        assert_eq!(props["e"], json!({ "type": "string" }));
        assert_eq!(changed.len(), 4);
        assert!(changed.contains(&"/components/schemas/T/properties/a".to_string()));
    }

    #[test]
    fn draft_only_keywords_are_translated_or_dropped() {
        let (out, _) = to_openapi_30(&json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "type": "object",
            "properties": {
                "k": { "const": "x", "examples": ["x", "y"], "$comment": "c" }
            }
        }));
        assert!(out.get("$schema").is_none());
        assert_eq!(
            out["properties"]["k"],
            json!({ "enum": ["x"], "example": "x" })
        );
    }

    #[test]
    fn widens_a_typed_nullable_field() {
        let input = json!({ "type": "string", "nullable": true });
        assert_eq!(
            widen_nullable(&input),
            json!({ "type": ["string", "null"] })
        );
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
                "properties": { "a/b": { "type": ["string", "null"] } }
            }}}
        });
        let (repaired, sites) = to_openapi_30(&document);
        assert_eq!(sites, vec!["/components/schemas/Thing/properties/a~1b"]);
        // And the pointer has to be the one `pointer_mut` accepts, or the
        // repair silently skips the field it just reported.
        assert_eq!(
            repaired["components"]["schemas"]["Thing"]["properties"]["a/b"],
            json!({ "type": "string", "nullable": true })
        );
    }

    #[test]
    fn the_openapi_rewrite_names_every_site_including_nested_ones() {
        let document = json!({
            "components": { "schemas": {
                "Thing": {
                    "type": "object",
                    "properties": {
                        "note": { "type": ["string", "null"] },
                        "tags": { "type": "array", "items": { "type": ["string", "null"] } },
                        "id": { "type": "string" },
                        "fine": { "type": "string", "nullable": true }
                    }
                }
            }}
        });
        let (_, sites) = to_openapi_30(&document);
        assert_eq!(
            sites,
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
