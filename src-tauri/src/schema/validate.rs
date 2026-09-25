use crate::schema::ajv;
use crate::schema::model::{parse_content, EventIdentity, ENVELOPE_REQUIRED};
use crate::schema::openapi;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// How badly a finding should block the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    /// Registration will fail, or the event will not validate at runtime.
    Error,
    /// Legal, but out of step with the conventions in this registry.
    Warning,
}

/// A repair pontifex can apply to the whole document to clear a finding.
///
/// Carried on the finding rather than inferred by the editor: the alternative
/// was matching the message prose, which makes rewording a sentence silently
/// remove a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fix {
    /// Rewrite the JSON Schema spellings the registry refuses into OpenAPI
    /// 3.0 — [`crate::schema::openapi::to_openapi_30`], exposed as the
    /// `openapi_30_schema` command.
    Openapi30,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    pub severity: Severity,
    /// JSON Pointer into the document, so the editor can jump to it.
    pub path: String,
    pub message: String,
    /// Set when this finding has a mechanical whole-document repair.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub valid: bool,
    pub findings: Vec<Finding>,
    /// Identity read out of the document, when it could be determined.
    pub identity: Option<EventIdentity>,
}

impl ValidationReport {
    fn from(findings: Vec<Finding>, identity: Option<EventIdentity>) -> Self {
        let valid = !findings.iter().any(|f| f.severity == Severity::Error);
        ValidationReport {
            valid,
            findings,
            identity,
        }
    }
}

fn error(path: &str, message: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Error,
        path: path.to_string(),
        message: message.into(),
        fix: None,
    }
}

fn warning(path: &str, message: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Warning,
        path: path.to_string(),
        message: message.into(),
        fix: None,
    }
}

/// Validate a schema document before it is written to the registry.
///
/// Three layers of checking:
/// 1. **Structural** — the EventBridge `AWSEvent` envelope contract. AWS
///    accepts documents that violate this, and they then fail silently at
///    event-validation time, so catching it here is the whole point.
/// 2. **JSON Schema** — every component schema must itself be a compilable
///    schema, and internal `$ref`s must resolve.
/// 3. **Ajv parity** — keywords that read as constraints but that the bus's
///    validator does not implement, and so silently enforces nothing. See
///    [`check_ajv_parity`].
///
/// `expected_name` is the registry schema name the document is being saved as;
/// when supplied, the envelope's source/detail-type must agree with it.
pub fn validate(content: &Value, expected_name: Option<&str>) -> ValidationReport {
    let mut findings = Vec::new();

    let doc = match parse_content(content) {
        Ok(d) => d,
        Err(e) => {
            findings.push(error("", e.to_string()));
            return ValidationReport::from(findings, None);
        }
    };

    if !doc.is_object() {
        findings.push(error("", "Schema document must be a JSON object"));
        return ValidationReport::from(findings, None);
    }

    if doc.get("openapi").and_then(Value::as_str).is_none() {
        findings.push(warning(
            "/openapi",
            "Missing `openapi` version string (expected \"3.0.0\")",
        ));
    }

    let Some(schemas) = openapi::component_schemas(&doc) else {
        findings.push(error(
            "/components/schemas",
            "Document has no `components.schemas` object",
        ));
        return ValidationReport::from(findings, None);
    };

    let Some(envelope) = schemas.get("AWSEvent") else {
        findings.push(error(
            "/components/schemas/AWSEvent",
            "Missing the `AWSEvent` envelope schema that EventBridge requires",
        ));
        return ValidationReport::from(findings, None);
    };

    let identity = check_envelope(envelope, expected_name, &mut findings);
    check_detail_ref(envelope, schemas, &mut findings);
    check_component_schemas(schemas, &mut findings);
    check_ajv_parity(&doc, &mut findings);
    check_openapi_30(&doc, &mut findings);
    check_registry_name(expected_name, &mut findings);

    ValidationReport::from(findings, identity)
}

/// Report constraints the bus will not enforce.
///
/// Everything here is legal JSON Schema, or at least legal OpenAPI, and none of
/// it stops a registration. It is reported because the schema *reads* as though
/// it constrains something and does not: the bus compiles with Ajv 8's default
/// export, which is draft-07 and nothing else, under `strict: 'log'` — so a
/// keyword Ajv does not implement is logged once into CloudWatch and then
/// ignored for the life of the process.
///
/// The failure mode this exists to prevent is a schema review that concludes
/// "the field is constrained, we're covered", when in production it is not.
fn check_ajv_parity(document: &Value, findings: &mut Vec<Finding>) {
    let mut parity = Parity::default();

    // The document root is not itself a schema position, so `walk_schemas`
    // does not visit it — but a `$schema` written there is just as fatal.
    if let Some(map) = document.as_object() {
        parity.check_dialect("", map);
    }
    openapi::walk_schemas(document, &mut |pointer, map| parity.visit(pointer, map));

    parity.report(findings);
}

/// Report what the registry will refuse to store.
///
/// The registry validates every write as OpenAPI 3.0 and answers a violation
/// with `Content is not valid OpenAPI 3.0: 'components/schemas/X' oneOf
/// failed` — the component, never the field or the keyword. These are the
/// same checks, made before the write, naming the pointer. Every one is an
/// error: the document cannot be saved as it stands.
fn check_openapi_30(document: &Value, findings: &mut Vec<Finding>) {
    let mut type_lists = Sites::default();
    let mut null_types = Sites::default();
    let mut tuple_items = Sites::default();
    let mut numeric_exclusives = Sites::default();
    let mut translatable: BTreeMap<&'static str, Sites> = BTreeMap::new();
    let mut unknown: BTreeMap<String, Sites> = BTreeMap::new();

    openapi::walk_schemas(document, &mut |pointer, map| {
        match map.get("type") {
            Some(Value::Array(_)) => type_lists.add(pointer),
            Some(Value::String(t)) if t == "null" => null_types.add(pointer),
            _ => {}
        }
        if matches!(map.get("items"), Some(Value::Array(_))) {
            tuple_items.add(&format!("{pointer}/items"));
        }
        for keyword in ["exclusiveMinimum", "exclusiveMaximum"] {
            if map.get(keyword).is_some_and(Value::is_number) {
                numeric_exclusives.add(&format!("{pointer}/{keyword}"));
            }
        }
        for key in map.keys() {
            if openapi::is_openapi_30_keyword(key) {
                continue;
            }
            // A foreign dialect is already an error from the parity pass,
            // and one that says the registry refuses it too.
            if key == "$schema"
                && map
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|id| !ajv::is_draft_07(id))
            {
                continue;
            }
            match key.as_str() {
                k @ ("const" | "examples" | "$schema" | "$id" | "id" | "$comment") => {
                    let k: &'static str = match k {
                        "const" => "const",
                        "examples" => "examples",
                        "$schema" => "$schema",
                        "$id" => "$id",
                        "id" => "id",
                        _ => "$comment",
                    };
                    translatable
                        .entry(k)
                        .or_default()
                        .add(&format!("{pointer}/{k}"));
                }
                other => unknown
                    .entry(other.to_string())
                    .or_default()
                    .add(&format!("{pointer}/{other}")),
            }
        }
    });

    aggregate(
        findings,
        Severity::Error,
        &type_lists,
        Some(Fix::Openapi30),
        "`type` is a list, which the registry refuses: an OpenAPI 3.0 schema has one \
         `type`. Spell \"or null\" as `nullable: true` — the bus honours it — and a \
         genuine union as `anyOf`.",
    );
    aggregate(
        findings,
        Severity::Error,
        &null_types,
        Some(Fix::Openapi30),
        "`type: \"null\"` is not an OpenAPI 3.0 type, and the registry refuses it. Use \
         `nullable: true`, with or without another `type`.",
    );
    aggregate(
        findings,
        Severity::Error,
        &tuple_items,
        None,
        "`items` is a list, which OpenAPI 3.0 does not allow — `items` describes every \
         element with one schema. The registry refuses this document.",
    );
    aggregate(
        findings,
        Severity::Error,
        &numeric_exclusives,
        Some(Fix::Openapi30),
        "`exclusiveMinimum`/`exclusiveMaximum` are booleans in OpenAPI 3.0, so the \
         registry refuses a number here — and Ajv refuses the boolean. No spelling \
         satisfies both; use `minimum`/`maximum`.",
    );
    for (keyword, sites) in &translatable {
        let note = match *keyword {
            "const" => "spell it `enum` with one value",
            "examples" => "OpenAPI 3.0 has `example`, singular",
            _ => "drop it; the document's identity is its registry name",
        };
        aggregate(
            findings,
            Severity::Error,
            sites,
            Some(Fix::Openapi30),
            &format!(
                "`{keyword}` is not an OpenAPI 3.0 keyword, and the registry refuses a \
                 document that carries it — {note}."
            ),
        );
    }
    for (keyword, sites) in &unknown {
        aggregate(
            findings,
            Severity::Error,
            sites,
            None,
            &format!(
                "`{keyword}` is not an OpenAPI 3.0 keyword, and the registry refuses a \
                 document that carries it. Only `x-` extensions may be added."
            ),
        );
    }
}

/// How many offending paths to name before summarising.
///
/// One finding per site would mean hundreds of findings on the registry's real
/// documents, which is not a report anyone reads.
const MAX_LISTED: usize = 5;

/// Where one kind of divergence was found, and how often.
///
/// Only the first [`MAX_LISTED`] pointers are kept: the rest are counted and
/// discarded, because the message never shows them and building a `String` per
/// site meant hundreds of allocations per keystroke on the worst documents.
#[derive(Default)]
struct Sites {
    count: usize,
    /// The first offending pointer, so the editor can jump to it.
    path: Option<String>,
    /// Rendered labels, capped at [`MAX_LISTED`].
    shown: Vec<String>,
}

impl Sites {
    fn add(&mut self, pointer: &str) {
        self.add_labelled(pointer, None);
    }

    /// `detail` names what specifically is wrong at this pointer, where the
    /// keyword alone does not say — which keywords a `$ref` discards, say.
    fn add_labelled(&mut self, pointer: &str, detail: Option<&str>) {
        self.count += 1;
        if self.path.is_none() {
            self.path = Some(pointer.to_string());
        }
        if self.shown.len() < MAX_LISTED {
            self.shown.push(match detail {
                Some(detail) => format!("`{pointer}` ({detail})"),
                None => format!("`{pointer}`"),
            });
        }
    }

    fn listed(&self) -> String {
        match self.count.saturating_sub(self.shown.len()) {
            0 => self.shown.join(", "),
            rest => format!("{}, and {rest} more", self.shown.join(", ")),
        }
    }
}

/// Everything one pass over the document turns up.
///
/// A single accumulator because the four checks used to walk the whole
/// document independently — four traversals of a schema that can run to
/// hundreds of kilobytes, on a path that re-runs while you type.
#[derive(Default)]
struct Parity {
    /// `nullable` with no `type`: Ajv refuses to compile it.
    untyped_nullable: Sites,
    dialect: Sites,
    /// Keyed by the post-draft-07 keyword that was found.
    post_07: BTreeMap<&'static str, Sites>,
    ref_siblings: Sites,
    boolean_exclusives: Sites,
    /// Keyed by the unrecognised format name.
    unknown_formats: BTreeMap<String, Sites>,
    numeric_formats: Sites,
}

/// Keywords introduced after draft-07, and how draft-07 spells the same idea.
///
/// Ajv's draft-07 mode treats each as an unknown keyword: logged, then ignored.
const POST_DRAFT_07: [(&str, &str); 9] = [
    ("$defs", "draft-07 spells this `definitions`"),
    ("dependentRequired", "draft-07 spells this `dependencies`"),
    ("dependentSchemas", "draft-07 spells this `dependencies`"),
    ("prefixItems", "draft-07 spells this `items` with an array"),
    ("unevaluatedProperties", "no draft-07 equivalent"),
    ("unevaluatedItems", "no draft-07 equivalent"),
    ("minContains", "no draft-07 equivalent"),
    ("maxContains", "no draft-07 equivalent"),
    ("$dynamicRef", "no draft-07 equivalent"),
];

impl Parity {
    fn visit(&mut self, pointer: &str, map: &serde_json::Map<String, Value>) {
        self.check_dialect(pointer, map);

        // Ajv: '"nullable" cannot be used without "type"' — a compile error,
        // which would take the whole type down with it.
        if map.contains_key("nullable") && !map.contains_key("type") {
            self.untyped_nullable.add(pointer);
        }

        for (keyword, _) in POST_DRAFT_07 {
            if map.contains_key(keyword) {
                self.post_07.entry(keyword).or_default().add(pointer);
            }
        }

        // Draft-07 evaluates `$ref` alone and discards everything beside it.
        // 2019-09 changed this, which is why it surprises people.
        if map.contains_key("$ref") && map.len() > 1 {
            let ignored: Vec<&str> = map
                .keys()
                .map(String::as_str)
                .filter(|k| *k != "$ref" && !is_annotation(k))
                .collect();
            if !ignored.is_empty() {
                self.ref_siblings
                    .add_labelled(pointer, Some(&ignored.join(", ")));
            }
        }

        // Draft-04 spelled these as booleans modifying `minimum`/`maximum`.
        // Draft-07 wants a number, and Ajv reads a boolean as a type error on
        // the schema rather than as the old meaning.
        for keyword in ["exclusiveMinimum", "exclusiveMaximum"] {
            if map.get(keyword).is_some_and(Value::is_boolean) {
                self.boolean_exclusives.add(&format!("{pointer}/{keyword}"));
            }
        }

        if let Some(Value::String(format)) = map.get("format") {
            let at = format!("{pointer}/format");
            if ajv::is_unreproducible_format(format) {
                self.numeric_formats.add(&at);
            } else if !ajv::is_known_format(format) {
                self.unknown_formats
                    .entry(format.clone())
                    .or_default()
                    .add(&at);
            }
        }
    }

    /// Ajv 8's default export carries the draft-07 meta-schema and no other.
    fn check_dialect(&mut self, pointer: &str, map: &serde_json::Map<String, Value>) {
        if let Some(Value::String(id)) = map.get("$schema") {
            if !ajv::is_draft_07(id) {
                self.dialect.add_labelled(pointer, Some(id));
            }
        }
    }

    fn report(self, findings: &mut Vec<Finding>) {
        aggregate(
            findings,
            Severity::Error,
            &self.untyped_nullable,
            None,
            "`nullable` without a `type` is a schema Ajv refuses to compile (\"nullable\" \
             cannot be used without \"type\"), so nothing of this type would be graded. \
             Give the field a `type`, or drop `nullable` — an untyped field already \
             accepts null.",
        );

        aggregate(
            findings,
            Severity::Error,
            &self.dialect,
            None,
            "`$schema` names a dialect the bus cannot load. It compiles with Ajv's draft-07 \
             meta-schema and would throw on load, so no event of this type would ever be \
             graded. Remove it — the registry refuses `$schema` in any case.",
        );

        for (keyword, note) in POST_DRAFT_07 {
            if let Some(sites) = self.post_07.get(keyword) {
                aggregate(
                    findings,
                    Severity::Warning,
                    sites,
                    None,
                    &format!(
                        "`{keyword}` was added after draft-07, and the bus validates with \
                         draft-07 — it is ignored, so it constrains nothing ({note})."
                    ),
                );
            }
        }

        aggregate(
            findings,
            Severity::Warning,
            &self.ref_siblings,
            None,
            "In draft-07 a `$ref` replaces its whole schema object, so keywords written \
             beside it are discarded. Move them into the referenced type, or wrap the \
             `$ref` in an `allOf`.",
        );

        aggregate(
            findings,
            Severity::Error,
            &self.boolean_exclusives,
            None,
            "`exclusiveMinimum`/`exclusiveMaximum` must be numbers in draft-07; a boolean is \
             the draft-04 spelling and Ajv rejects the schema outright, so nothing of this \
             type would be graded.",
        );

        for (format, sites) in &self.unknown_formats {
            aggregate(
                findings,
                Severity::Warning,
                sites,
                None,
                &format!(
                    "`format: {format}` is not one the bus knows. Ajv logs an unknown format \
                     and moves on, so this documents an intention without enforcing it — any \
                     string passes. Add a `pattern` if it needs to hold."
                ),
            );
        }

        aggregate(
            findings,
            Severity::Warning,
            &self.numeric_formats,
            None,
            "The bus asserts this format against numbers, and pontifex's validator only ever \
             sees strings — so events are graded here without it. The check is real in \
             production; this report just cannot reproduce it.",
        );
    }
}

/// Emit one finding covering every site of one divergence.
///
/// Every parity finding has the same shape — first offending pointer as the
/// jump target, the sites appended to the prose — and writing that out six
/// times was six chances for the convention to drift. It already had.
fn aggregate(
    findings: &mut Vec<Finding>,
    severity: Severity,
    sites: &Sites,
    fix: Option<Fix>,
    message: &str,
) {
    let Some(path) = &sites.path else { return };
    findings.push(Finding {
        severity,
        path: path.clone(),
        message: format!("{message} At {}.", sites.listed()),
        fix,
    });
}

/// Keywords that are annotations in every draft, so losing them beside a `$ref`
/// costs nothing worth a finding.
fn is_annotation(keyword: &str) -> bool {
    matches!(
        keyword,
        "description" | "title" | "example" | "examples" | "deprecated" | "default" | "$comment"
    )
}

/// The name must be one EventBridge will accept.
///
/// Caught here so it surfaces in the editor rather than as a
/// `BadRequestException` after the write is attempted — the failure that made
/// "Create" look like it did nothing.
fn check_registry_name(expected_name: Option<&str>, findings: &mut Vec<Finding>) {
    let Some(name) = expected_name.filter(|n| !n.is_empty()) else {
        return;
    };
    let bad = crate::schema::model::invalid_schema_name_chars(name);
    if bad.is_empty() {
        return;
    }

    let rendered: Vec<String> = bad
        .iter()
        .map(|c| {
            if *c == ' ' {
                "space".to_string()
            } else {
                format!("'{c}'")
            }
        })
        .collect();

    findings.push(Finding {
        severity: Severity::Error,
        path: "".to_string(),
        message: format!(
            "EventBridge will reject the name '{name}': it allows only letters, digits, \
             `_`, `.`, `-` and `@`, and this contains {}. Register it as '{}' instead — \
             the envelope keeps the real source either way.",
            rendered.join(", "),
            crate::schema::model::sanitize_schema_name(name)
        ),
        fix: None,
    });
}

fn check_envelope(
    envelope: &Value,
    expected_name: Option<&str>,
    findings: &mut Vec<Finding>,
) -> Option<EventIdentity> {
    const BASE: &str = "/components/schemas/AWSEvent";

    if envelope.get("type").and_then(Value::as_str) != Some("object") {
        findings.push(error(
            &format!("{BASE}/type"),
            "`AWSEvent` must have type \"object\"",
        ));
    }

    // Required-field coverage.
    let declared: BTreeSet<&str> = envelope
        .get("required")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let missing: Vec<&str> = ENVELOPE_REQUIRED
        .iter()
        .copied()
        .filter(|f| !declared.contains(f))
        .collect();
    if !missing.is_empty() {
        findings.push(error(
            &format!("{BASE}/required"),
            format!(
                "`AWSEvent.required` is missing EventBridge envelope fields: {}",
                missing.join(", ")
            ),
        ));
    }

    let source = envelope
        .get("x-amazon-events-source")
        .and_then(Value::as_str);
    let detail_type = envelope
        .get("x-amazon-events-detail-type")
        .and_then(Value::as_str);

    if source.is_none() {
        findings.push(error(
            &format!("{BASE}/x-amazon-events-source"),
            "Missing `x-amazon-events-source` — EventBridge cannot match events to this schema without it",
        ));
    }
    if detail_type.is_none() {
        findings.push(error(
            &format!("{BASE}/x-amazon-events-detail-type"),
            "Missing `x-amazon-events-detail-type` — EventBridge cannot match events to this schema without it",
        ));
    }

    let identity = match (source, detail_type) {
        (Some(s), Some(d)) => Some(EventIdentity {
            source: s.to_string(),
            detail_type: d.to_string(),
        }),
        _ => None,
    };

    // The schema's registry name is derived from these two fields, so a
    // mismatch normally means the document would register under a name that
    // contradicts itself.
    //
    // The exception is EventBridge's own schema discovery, which names
    // discovered schemas `<source>@<PascalCaseDetailType>` — so
    // `ai-services.c-file@labels-generated` is registered as
    // `ai-services.c-file@LabelsGenerated`. That is AWS's convention rather
    // than a defect, so it is reported as a warning and does not block a save.
    if let (Some(id), Some(expected)) = (identity.as_ref(), expected_name) {
        let declared = id.schema_name();
        if declared != expected {
            let discovery_naming = EventIdentity::from_schema_name(expected)
                .map(|e| e.source == id.source && e.detail_type == id.detail_title())
                .unwrap_or(false);

            // The other benign case: the declared name contains characters
            // EventBridge forbids in a schema name, so it was sanitized to get
            // the write accepted. Not a contradiction — but not free either,
            // see the message.
            let sanitized_naming = !discovery_naming
                && expected == crate::schema::model::sanitize_schema_name(&declared);

            let severity = if discovery_naming || sanitized_naming {
                Severity::Warning
            } else {
                Severity::Error
            };

            let message = if discovery_naming {
                format!(
                    "Registered as '{expected}' but the envelope declares '{declared}'. \
                     This is how EventBridge names auto-discovered schemas."
                )
            } else if sanitized_naming {
                // The runtime validator in global-event-bus builds its lookup
                // key as `${event.source}@${event['detail-type']}` verbatim
                // (packages/core/src/schemaValidator.ts). It will ask for the
                // *declared* name, which is not what this is registered under,
                // so the schema exists but never matches. Saying so is the
                // whole value of this finding.
                format!(
                    "Registered as '{expected}' because EventBridge rejects '{declared}' \
                     — its schema names cannot contain those characters. Be aware that the \
                     runtime validator looks schemas up by the event's own \
                     `source@detail-type`, so it will ask for '{declared}' and not find this. \
                     Making it match needs the publisher to change the source, or the \
                     lookup to sanitize the same way."
                )
            } else {
                format!(
                    "Envelope declares '{declared}' but the schema is being saved as '{expected}'"
                )
            };

            findings.push(Finding {
                severity,
                path: format!("{BASE}/x-amazon-events-source"),
                message,
                fix: None,
            });
        }
    }

    // Envelope property coverage is a warning: AWS tolerates extras/omissions,
    // but consumers generated from the schema will differ from the rest.
    if let Some(props) = envelope.get("properties").and_then(Value::as_object) {
        let missing_props: Vec<&str> = ENVELOPE_REQUIRED
            .iter()
            .copied()
            .filter(|f| !props.contains_key(*f))
            .collect();
        if !missing_props.is_empty() {
            findings.push(warning(
                &format!("{BASE}/properties"),
                format!(
                    "`AWSEvent.properties` does not define: {}",
                    missing_props.join(", ")
                ),
            ));
        }
    } else {
        findings.push(error(
            &format!("{BASE}/properties"),
            "`AWSEvent` has no `properties` object",
        ));
    }

    identity
}

/// The envelope's `detail` must `$ref` a schema that actually exists in the
/// document — a dangling ref is the most common breakage after a rename.
fn check_detail_ref(
    envelope: &Value,
    schemas: &serde_json::Map<String, Value>,
    findings: &mut Vec<Finding>,
) {
    const PATH: &str = "/components/schemas/AWSEvent/properties/detail";

    let Some(detail) = envelope.get("properties").and_then(|p| p.get("detail")) else {
        findings.push(error(PATH, "`AWSEvent.properties.detail` is missing"));
        return;
    };

    let Some(reference) = detail.get("$ref").and_then(Value::as_str) else {
        findings.push(warning(
            PATH,
            "`detail` is inlined rather than a `$ref` to a named schema",
        ));
        return;
    };

    let Some(target) = openapi::ref_name(reference) else {
        findings.push(error(
            PATH,
            format!(
                "`detail.$ref` must point into `{}`, got '{reference}'",
                openapi::REF_PREFIX
            ),
        ));
        return;
    };

    if !schemas.contains_key(target) {
        findings.push(error(
            PATH,
            format!("`detail.$ref` points at '{target}', which is not defined in this document"),
        ));
    }
}

/// Compile each component schema so malformed keywords are caught here rather
/// than by whatever consumes the schema later.
///
/// Compiled the way the bus compiles — draft-07, `ajv-formats` — so "this does
/// not compile" means the same thing in both places. Under 2020-12 a document
/// could compile here and throw there.
fn check_component_schemas(schemas: &serde_json::Map<String, Value>, findings: &mut Vec<Finding>) {
    for (name, schema) in schemas {
        // `$ref`s are document-relative and the validator would try to fetch
        // them, so validate a copy with the refs stripped to their targets.
        let standalone = strip_internal_refs(schema);
        // `compile`, not `validator_for`: the latter re-scans for a foreign
        // dialect, which `check_ajv_parity` already reports once for the whole
        // document — so this loop used to produce a second error saying the
        // same thing, at a different path, per component.
        if let Err(e) = ajv::compile(&standalone) {
            findings.push(error(
                &format!("/components/schemas/{}", openapi::escape_segment(name)),
                format!("Not a valid JSON Schema: {e}"),
            ));
        }
    }
}

/// Replace `{"$ref": "#/components/schemas/X"}` with `{}` so a schema can be
/// compiled in isolation. Ref *targets* are checked separately by
/// [`check_detail_ref`]; this only stops the compiler from trying to resolve
/// them.
fn strip_internal_refs(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            if map.len() == 1 && map.contains_key("$ref") {
                return Value::Object(serde_json::Map::new());
            }
            Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), strip_internal_refs(v)))
                    .collect(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(strip_internal_refs).collect()),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    fn atomic_forms_doc() -> serde_json::Value {
        super::super::model::document_with_detail(
            &super::EventIdentity {
                source: "Atomic Forms".into(),
                detail_type: "CASE_ARCHIVE_STATUS_UPDATE".into(),
            },
            serde_json::json!({ "type": "object", "additionalProperties": true }),
        )
    }

    #[test]
    fn a_sanitized_name_does_not_block_the_save() {
        // The name had to be rewritten for EventBridge to accept it at all, so
        // treating the resulting mismatch as an error would make the schema
        // unsavable by any route.
        let report = super::validate(
            &atomic_forms_doc(),
            Some("Atomic-Forms@CASE_ARCHIVE_STATUS_UPDATE"),
        );
        assert!(report.valid, "sanitized naming must not be an error");

        let finding = report
            .findings
            .iter()
            .find(|f| f.message.contains("because EventBridge rejects"))
            .expect("expected a sanitized-naming finding");
        assert_eq!(finding.severity, super::Severity::Warning);
    }

    #[test]
    fn the_sanitized_warning_states_the_runtime_consequence() {
        // The runtime looks schemas up by the event's own source, so a
        // sanitized name is registered but never matched. A warning that did
        // not say so would read as "harmless".
        let report = super::validate(
            &atomic_forms_doc(),
            Some("Atomic-Forms@CASE_ARCHIVE_STATUS_UPDATE"),
        );
        let message = report
            .findings
            .iter()
            .map(|f| f.message.clone())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(message.contains("runtime validator"), "{message}");
        assert!(
            message.contains("Atomic Forms@CASE_ARCHIVE_STATUS_UPDATE"),
            "should name what the runtime will actually ask for: {message}"
        );
    }

    #[test]
    fn an_unrelated_name_is_still_an_error() {
        // Only the sanitization of *this* name is benign; a genuinely
        // contradictory name must still block.
        let report = super::validate(&atomic_forms_doc(), Some("something-else@Entirely"));
        assert!(!report.valid);
    }

    #[test]
    fn a_name_with_a_space_is_flagged_before_the_write() {
        // AWS answers this with "doesn't match pattern ^[a-zA-Z0-9_\.\-\@]+$",
        // which does not say which character was wrong.
        let doc = super::super::model::document_with_detail(
            &super::EventIdentity {
                source: "Atomic Forms".into(),
                detail_type: "CASE_STATUS_CHANGED".into(),
            },
            serde_json::json!({ "type": "object", "additionalProperties": true }),
        );
        let report = super::validate(&doc, Some("Atomic Forms@CASE_STATUS_CHANGED"));
        assert!(!report.valid, "an unregistrable name must block the save");

        let finding = report
            .findings
            .iter()
            .find(|f| f.message.contains("EventBridge will reject the name"))
            .expect("expected a name finding");
        assert_eq!(finding.severity, super::Severity::Error);
        assert!(
            finding.message.contains("space"),
            "should name the character"
        );
        assert!(
            finding.message.contains("Atomic-Forms@CASE_STATUS_CHANGED"),
            "should offer a registrable alternative: {}",
            finding.message
        );
    }

    #[test]
    fn a_registrable_name_produces_no_name_finding() {
        let doc = super::super::model::document_with_detail(
            &super::EventIdentity {
                source: "orders-api".into(),
                detail_type: "order-assigned".into(),
            },
            serde_json::json!({ "type": "object", "additionalProperties": true }),
        );
        let report = super::validate(&doc, Some("orders-api@order-assigned"));
        assert!(
            !report
                .findings
                .iter()
                .any(|f| f.message.contains("EventBridge will reject")),
            "valid names must not be flagged"
        );
    }

    use super::*;
    use crate::schema::model::{new_schema_document, EventIdentity};
    use serde_json::json;

    fn good_doc() -> Value {
        let id = EventIdentity::from_schema_name("my-service@thing-happened").unwrap();
        new_schema_document(&id, &[("veteranId".into(), "string".into())])
    }

    #[test]
    fn accepts_a_generated_document() {
        let report = validate(&good_doc(), Some("my-service@thing-happened"));
        assert!(
            report.valid,
            "expected valid, findings: {:?}",
            report.findings
        );
        assert_eq!(report.identity.unwrap().source, "my-service");
    }

    #[test]
    fn accepts_content_supplied_as_a_json_string() {
        // This is how the EventBridge API hands schemas back.
        let as_string = Value::String(serde_json::to_string(&good_doc()).unwrap());
        assert!(validate(&as_string, None).valid);
    }

    #[test]
    fn rejects_missing_source_extension() {
        let mut doc = good_doc();
        doc["components"]["schemas"]["AWSEvent"]
            .as_object_mut()
            .unwrap()
            .remove("x-amazon-events-source");
        let report = validate(&doc, None);
        assert!(!report.valid);
        assert!(report
            .findings
            .iter()
            .any(|f| f.message.contains("x-amazon-events-source")));
    }

    #[test]
    fn rejects_dangling_detail_ref() {
        let mut doc = good_doc();
        doc["components"]["schemas"]["AWSEvent"]["properties"]["detail"]["$ref"] =
            json!("#/components/schemas/DoesNotExist");
        let report = validate(&doc, None);
        assert!(!report.valid);
        assert!(report
            .findings
            .iter()
            .any(|f| f.message.contains("DoesNotExist")));
    }

    #[test]
    fn rejects_name_that_contradicts_the_envelope() {
        let report = validate(&good_doc(), Some("other-service@thing-happened"));
        assert!(!report.valid);
        assert!(report
            .findings
            .iter()
            .any(|f| f.message.contains("other-service@thing-happened")));
    }

    #[test]
    fn tolerates_eventbridge_discovery_naming() {
        // Auto-discovered schemas are registered under the PascalCase detail
        // type, which legitimately differs from the envelope's detail-type.
        let report = validate(&good_doc(), Some("my-service@ThingHappened"));
        assert!(
            report.valid,
            "discovery naming should not block a save: {:?}",
            report.findings
        );
        let finding = report
            .findings
            .iter()
            .find(|f| f.message.contains("auto-discovered"))
            .expect("should still be reported");
        assert_eq!(finding.severity, Severity::Warning);
    }

    #[test]
    fn still_rejects_a_wrong_source_even_with_discovery_casing() {
        // Same PascalCase detail type, but a different source is a real error.
        let report = validate(&good_doc(), Some("other-service@ThingHappened"));
        assert!(!report.valid);
    }

    #[test]
    fn rejects_incomplete_envelope_required_list() {
        let mut doc = good_doc();
        doc["components"]["schemas"]["AWSEvent"]["required"] = json!(["detail", "id"]);
        let report = validate(&doc, None);
        assert!(!report.valid);
        assert!(report.findings.iter().any(|f| f.message.contains("source")));
    }

    #[test]
    fn rejects_non_object_and_unparseable_input() {
        assert!(!validate(&json!("nonsense"), None).valid);
        assert!(!validate(&Value::String("{ not json".into()), None).valid);
        assert!(!validate(&json!({ "openapi": "3.0.0" }), None).valid);
    }

    #[test]
    fn rejects_malformed_component_schema() {
        let mut doc = good_doc();
        // `type` must be a string or array of strings.
        doc["components"]["schemas"]["ThingHappened"]["type"] = json!(42);
        let report = validate(&doc, None);
        assert!(!report.valid);
    }

    #[test]
    fn warns_but_accepts_a_missing_openapi_version() {
        let mut doc = good_doc();
        doc.as_object_mut().unwrap().remove("openapi");
        let report = validate(&doc, None);
        assert!(report.valid);
        assert!(report
            .findings
            .iter()
            .any(|f| f.severity == Severity::Warning && f.path == "/openapi"));
    }

    // -- Ajv parity ---------------------------------------------------------

    /// The payload type of [`good_doc`], for hanging a keyword off.
    fn with_payload_field(field: Value) -> Value {
        let mut doc = good_doc();
        doc["components"]["schemas"]["ThingHappened"]["properties"]["veteranId"] = field;
        doc
    }

    fn message_matching(report: &ValidationReport, needle: &str) -> Option<String> {
        report
            .findings
            .iter()
            .find(|f| f.message.contains(needle))
            .map(|f| f.message.clone())
    }

    #[test]
    fn nullable_is_the_registry_spelling_and_raises_nothing() {
        let doc = with_payload_field(json!({ "type": "string", "nullable": true }));
        let report = validate(&doc, None);
        assert!(report.valid, "findings: {:?}", report.findings);
        assert!(
            message_matching(&report, "nullable").is_none(),
            "{:?}",
            report.findings
        );
    }

    #[test]
    fn a_type_list_blocks_the_save_and_names_the_field() {
        // The registry answers this with "'components/schemas/X' oneOf failed",
        // which names the component and not the field. This does.
        let doc = with_payload_field(json!({ "type": ["string", "null"] }));
        let report = validate(&doc, None);
        assert!(!report.valid);
        let finding = report
            .findings
            .iter()
            .find(|f| f.message.contains("`type` is a list"))
            .expect("a type-list finding");
        assert_eq!(finding.severity, Severity::Error);
        assert_eq!(finding.fix, Some(Fix::Openapi30));
        assert!(finding
            .message
            .contains("`/components/schemas/ThingHappened/properties/veteranId`"));

        // And the fix clears it, which is the button's contract.
        let (repaired, changed) = openapi::to_openapi_30(&doc);
        assert_eq!(
            changed,
            vec!["/components/schemas/ThingHappened/properties/veteranId"]
        );
        let report = validate(&repaired, None);
        assert!(report.valid, "findings: {:?}", report.findings);
    }

    #[test]
    fn keywords_openapi_30_lacks_are_errors_with_a_fix_where_one_exists() {
        let doc = with_payload_field(json!({
            "type": "string",
            "const": "x",
            "patternProperties": { "^a": {} }
        }));
        let report = validate(&doc, None);
        assert!(!report.valid);
        let by_keyword = |k: &str| {
            report
                .findings
                .iter()
                .find(|f| f.message.starts_with(&format!("`{k}`")))
                .unwrap_or_else(|| panic!("no finding for {k}: {:?}", report.findings))
        };
        assert_eq!(by_keyword("const").fix, Some(Fix::Openapi30));
        assert_eq!(by_keyword("patternProperties").fix, None);
    }

    #[test]
    fn nullable_without_a_type_is_the_one_nullable_ajv_refuses() {
        let doc = with_payload_field(json!({ "nullable": true, "description": "x" }));
        let report = validate(&doc, None);
        assert!(!report.valid);
        assert!(message_matching(&report, "cannot be used without").is_some());
    }
    #[test]
    fn a_plain_typed_field_says_nothing_about_null() {
        let doc = with_payload_field(json!({ "type": "string" }));
        let report = validate(&doc, None);
        assert!(
            message_matching(&report, "null").is_none(),
            "{:?}",
            report.findings
        );
    }

    #[test]
    fn a_foreign_dialect_blocks_the_save_and_is_reported_once() {
        // Ajv would throw on load, so nothing of this type would ever be
        // graded — a worse outcome than a rejected event, and silent.
        let mut doc = good_doc();
        doc["components"]["schemas"]["ThingHappened"]["$schema"] =
            json!("https://json-schema.org/draft/2020-12/schema");
        let report = validate(&doc, None);
        assert!(!report.valid);

        // Once, not twice. The parity pass and the per-component compile both
        // used to derive this verdict independently and each emit an error, at
        // different paths, saying the same thing.
        let errors: Vec<&Finding> = report
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].message.contains("draft-07"), "{:?}", errors[0]);
    }

    #[test]
    fn a_dialect_at_the_document_root_is_caught_too() {
        // `walk_schemas` starts at the component schemas, so the root needs
        // its own look — and a `$schema` is most often written there.
        let mut doc = good_doc();
        doc["$schema"] = json!("http://json-schema.org/draft-04/schema#");
        assert!(!validate(&doc, None).valid);
    }

    #[test]
    fn post_draft_07_keywords_are_reported_as_inert_and_unsaveable() {
        let mut doc = good_doc();
        doc["components"]["schemas"]["ThingHappened"]["dependentRequired"] =
            json!({ "veteranId": ["claimId"] });
        let report = validate(&doc, None);

        // Two facts about one keyword: Ajv would ignore it, and the registry
        // will not store it at all.
        let message =
            message_matching(&report, "constrains nothing").expect("an inert-keyword finding");
        assert!(message.contains("`dependencies`"), "{message}");
        assert!(!report.valid);
        assert!(message_matching(&report, "registry refuses").is_some());
    }

    #[test]
    fn ref_siblings_are_reported_but_annotations_are_not() {
        let mut doc = good_doc();
        doc["components"]["schemas"]["Nested"] = json!({ "type": "string" });
        doc["components"]["schemas"]["ThingHappened"]["properties"]["a"] = json!({
            "$ref": "#/components/schemas/Nested",
            "maxLength": 3
        });
        // A description beside a `$ref` is lost too, but losing an annotation
        // changes no verdict, so reporting it would be noise.
        doc["components"]["schemas"]["ThingHappened"]["properties"]["b"] = json!({
            "$ref": "#/components/schemas/Nested",
            "description": "fine"
        });

        let report = validate(&doc, None);
        let message = message_matching(&report, "$ref` replaces").expect("a ref-sibling finding");
        assert!(message.contains("maxLength"), "{message}");
        assert!(!message.contains("/properties/b"), "{message}");
    }

    #[test]
    fn a_boolean_exclusive_minimum_blocks_the_save() {
        // Draft-04's spelling. Ajv rejects the schema itself, so every event of
        // this type stops being graded.
        let doc = with_payload_field(json!({
            "type": "number",
            "minimum": 0,
            "exclusiveMinimum": true
        }));
        let report = validate(&doc, None);
        assert!(!report.valid);
        assert!(message_matching(&report, "draft-04 spelling").is_some());
    }

    #[test]
    fn an_unknown_format_is_reported_as_enforcing_nothing() {
        let doc = with_payload_field(json!({ "type": "string", "format": "phone-number" }));
        let report = validate(&doc, None);

        assert!(report.valid);
        let message = message_matching(&report, "phone-number").expect("an unknown-format finding");
        assert!(message.contains("any string passes"), "{message}");
    }

    #[test]
    fn a_format_ajv_knows_is_not_reported() {
        for format in ["uuid", "date-time", "duration", "email", "byte"] {
            let doc = with_payload_field(json!({ "type": "string", "format": format }));
            let report = validate(&doc, None);
            assert!(
                message_matching(&report, "not one the bus knows").is_none(),
                "{format} should be known"
            );
        }
    }

    #[test]
    fn a_numeric_format_states_the_gap_rather_than_implying_a_check() {
        let doc = with_payload_field(json!({ "type": "integer", "format": "int32" }));
        let report = validate(&doc, None);

        assert!(report.valid);
        let message = message_matching(&report, "int32")
            .or_else(|| message_matching(&report, "against numbers"))
            .expect("a numeric-format finding");
        assert!(message.contains("cannot reproduce"), "{message}");
    }

    #[test]
    fn keywords_inside_an_example_are_not_mistaken_for_constraints() {
        // An `example` is a payload, not a schema. Walking it would report the
        // sample's own `format` key as an unenforced constraint.
        let doc = with_payload_field(json!({
            "type": "object",
            "example": { "format": "not-a-real-format", "nullable": true }
        }));
        let report = validate(&doc, None);
        assert!(message_matching(&report, "not-a-real-format").is_none());
    }

    #[test]
    fn constraints_nested_in_arrays_and_composition_are_found() {
        let doc = with_payload_field(json!({
            "type": "array",
            "items": {
                "oneOf": [
                    { "type": "string", "format": "made-up" },
                    { "type": "number" }
                ]
            }
        }));
        let report = validate(&doc, None);
        let message = message_matching(&report, "made-up").expect("a nested format finding");
        assert!(
            message.contains("/properties/veteranId/items/oneOf/0/format"),
            "{message}"
        );
    }
}
