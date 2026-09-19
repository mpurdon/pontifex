//! Checking a schema against the events that are actually flowing.
//!
//! A schema can be structurally valid and still wrong: too strict for real
//! traffic, or missing fields that producers have started sending. Validation
//! alone answers "would this event be rejected"; the field diff answers the
//! more useful question, "where has the schema drifted from reality".

use crate::error::{Error, Result};
use crate::schema::repair::Repair;
use crate::schema::{ajv, openapi};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Why one sampled event failed validation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureGroup {
    /// JSON Pointer into the event payload.
    pub pointer: String,
    pub message: String,
    /// How many sampled events failed this way.
    pub count: usize,
    /// One offending value, to make the failure concrete.
    pub example: Option<Value>,
}

/// A field observed in real events, described by what was seen.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldObservation {
    /// Dotted path within the payload, e.g. `metadata.trackingId`.
    pub path: String,
    /// Types seen at this path, e.g. `["string"]` or `["string", "null"]`.
    pub types: Vec<String>,
    /// How many sampled events contained it.
    pub seen_in: usize,
    pub example: Option<Value>,
}

/// A field the schema declares but the sample never contained.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnusedField {
    pub path: String,
    pub required: bool,
}

/// A declared field whose observed type contradicts the schema.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeMismatch {
    pub path: String,
    pub declared: String,
    pub observed: Vec<String>,
    /// Events containing the field at all.
    pub seen_in: usize,
    /// Events that carried one of the `observed` types.
    ///
    /// Distinct from `seen_in`, and the number that actually matters: a field
    /// present in all 200 events but wrong in 7 of them was reported as
    /// "200/200 · 100%", which reads as "every event is broken".
    pub mismatched_in: usize,
    pub example: Option<Value>,
    /// The type set that would accept every event in the sample.
    ///
    /// Not `observed`, and the difference is the whole repair. `observed` holds
    /// only the types the declaration *forbids*: for a field declared `string`
    /// that is null in 7% of events, it is `["null"]` alone. Redeclaring the
    /// field as that — which the action line reads as suggesting — would fix the
    /// 7% by rejecting the other 93%.
    ///
    /// So a declared type is kept when the sample still contains it, and dropped
    /// when it does not: a field declared `object` that is a string in every
    /// event becomes `string`, not `object | string`.
    pub suggested: Vec<String>,
}

/// A declared enum that real traffic exceeded.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnumDrift {
    pub path: String,
    pub declared: Vec<Value>,
    pub unexpected: Vec<Value>,
    /// Events containing the field at all.
    pub seen_in: usize,
    /// Events that carried a value outside the enum.
    pub unexpected_in: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftReport {
    /// Present in events, absent from the schema.
    pub undeclared: Vec<FieldObservation>,
    /// Declared in the schema, never present in the sample.
    pub unused: Vec<UnusedField>,
    /// Declared `required`, yet missing from some events.
    pub missing_required: Vec<FieldObservation>,
    pub type_mismatches: Vec<TypeMismatch>,
    pub enum_drift: Vec<EnumDrift>,
}

/// How often each declared field actually appeared, keyed by dotted path.
///
/// Drives the coverage annotations in the structure tree — a field declared
/// but present in 3% of traffic is a different thing from one present in 100%.
pub type Coverage = BTreeMap<String, usize>;

/// What kind of disagreement an [`Issue`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IssueKind {
    /// Events carry a type the schema does not allow.
    WrongType,
    /// Events carry a value outside a declared enum.
    OutsideEnum,
    /// A field marked required is absent from some events.
    MissingRequired,
    /// Producers send a field the schema does not describe.
    Undeclared,
    /// The schema declares a field the sample never contained.
    NeverSeen,
    /// A rejection none of the above explains — a pattern, a format, a bound.
    Rejected,
    /// The event type is on the bus and the registry has no schema for it
    /// at all. Not a disagreement with a schema — the absence of one.
    Unregistered,
}

/// The issue an event type with no schema raises. `seen` is how many of its
/// events were found; `example` is one payload.
pub fn unregistered_issue(
    source: &str,
    detail_type: &str,
    seen: usize,
    example: Option<Value>,
) -> Issue {
    Issue {
        key: "unregistered".into(),
        kind: IssueKind::Unregistered,
        severity: IssueSeverity::Warning,
        path: String::new(),
        summary: format!(
            "`{source}` publishes `{detail_type}` and the registry has no schema for it"
        ),
        action: "Register a schema for this event type — Pontifex can draft one from its \
                 traffic — so consumers have a contract to build against and validation \
                 can catch drift."
            .into(),
        declared: Some("no schema".into()),
        observed: Some(format!(
            "{seen} event{} on the bus",
            if seen == 1 { "" } else { "s" }
        )),
        affected: seen,
        sampled: seen,
        rejects: false,
        example,
        message: None,
        fix: None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IssueSeverity {
    /// Events are being rejected, or would be.
    Error,
    /// The schema is wrong about traffic, but nothing is rejected.
    Warning,
    /// Worth knowing, not worth doing anything about on its own.
    Info,
}

/// One thing wrong between a schema and the traffic it describes.
///
/// The panel used to show validation failures and drift as separate lists,
/// which meant a single problem — a field producers send as a string that the
/// schema declares an integer — appeared twice, under two headings, with two
/// different frequencies, and no statement of what to do about it. An issue is
/// that one problem: what disagrees, how often, whether events are being
/// rejected over it, and the choice it puts in front of you.
// Deserialize as well as Serialize: the Jira integration sends an issue back
// as the subject of a ticket, and it should be the same issue that was shown.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Stable across runs: identifies the same problem in a later sample.
    pub key: String,
    pub kind: IssueKind,
    pub severity: IssueSeverity,
    /// Dotted path, empty for a problem with the payload as a whole.
    pub path: String,
    /// One line: what disagrees.
    pub summary: String,
    /// One line: the choice this puts in front of you.
    pub action: String,
    /// What the schema says, where that is a single comparable thing.
    pub declared: Option<String>,
    /// What the events actually carry.
    pub observed: Option<String>,
    /// Events exhibiting this, out of `sampled`.
    pub affected: usize,
    pub sampled: usize,
    /// Whether validation rejects these events today.
    pub rejects: bool,
    pub example: Option<Value>,
    /// The validator's own message, when one applies.
    pub message: Option<String>,
    /// The edit that would clear this, where one can be computed.
    ///
    /// Carried on the issue rather than worked out in the panel: the operands
    /// live here, and a button driven by them cannot drift from the row it sits
    /// on the way one driven by matching the summary text would.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Repair>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCheckReport {
    pub sampled: usize,
    pub passed: usize,
    pub failed: usize,
    /// Every disagreement, deduplicated and ranked. The panel renders this.
    pub issues: Vec<Issue>,
    /// Raw validation groups, kept for the health report's headline.
    pub failures: Vec<FailureGroup>,
    pub drift: DriftReport,
    pub coverage: Coverage,
    /// The type the payloads were validated against.
    pub type_name: String,
}

fn type_name_of(value: &Value) -> &'static str {
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

/// How many distinct scalar values are remembered per path.
///
/// Enough to describe an enum-shaped field ("saw `pending`, `queued`, `done`")
/// without accumulating a set the size of the sample for a field that holds an
/// id.
const VALUE_SAMPLE_CAP: usize = 32;

/// What one event said about one path.
#[derive(Debug, Default, Clone)]
struct Seen {
    /// JSON type names, in first-seen order.
    types: Vec<String>,
    /// One example per type, so a row's example always illustrates its type.
    examples: BTreeMap<String, Value>,
    /// Distinct scalar values at this path, capped.
    values: Vec<Value>,
}

impl Seen {
    fn record(&mut self, value: &Value) {
        let ty = type_name_of(value).to_string();
        if !self.types.contains(&ty) {
            self.types.push(ty.clone());
        }
        self.examples.entry(ty).or_insert_with(|| value.clone());
        // Only scalars: comparing whole objects would be both expensive and
        // useless, since enums are only declarable on leaves.
        if !value.is_object()
            && !value.is_array()
            && self.values.len() < VALUE_SAMPLE_CAP
            && !self.values.contains(value)
        {
            self.values.push(value.clone());
        }
    }
}

/// Flatten an observed payload into dotted paths.
///
/// Array elements collapse onto the array's own path with a `[]` marker, so a
/// list of 500 items contributes one path rather than 500.
fn observe(value: &Value, prefix: &str, out: &mut BTreeMap<String, Seen>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                let path = join_path(prefix, key);
                // `get_mut` first: the entry API would clone the path on every
                // hit, and most paths are seen on every event.
                match out.get_mut(&path) {
                    Some(seen) => seen.record(child),
                    None => {
                        out.entry(path.clone()).or_default().record(child);
                    }
                }
                observe(child, &path, out);
            }
        }
        Value::Array(items) => {
            let path = format!("{prefix}[]");
            for item in items {
                out.entry(path.clone()).or_default().record(item);
                observe(item, &path, out);
            }
        }
        _ => {}
    }
}

/// Everything the sample said about one path, across all events.
#[derive(Debug, Default, Clone)]
struct Observation {
    /// Events that contained this path at all.
    seen_in: usize,
    /// Type name → events that carried a value of that type here.
    type_counts: BTreeMap<String, usize>,
    /// Type name → one example value of that type.
    examples: BTreeMap<String, Value>,
    /// Distinct scalar value → events that carried it, capped.
    value_counts: Vec<(Value, usize)>,
    /// The first value seen, for rows that do not care which type it was.
    example: Option<Value>,
}

impl Observation {
    fn types(&self) -> Vec<String> {
        self.type_counts.keys().cloned().collect()
    }

    fn absorb(&mut self, seen: Seen) {
        self.seen_in += 1;
        for ty in &seen.types {
            *self.type_counts.entry(ty.clone()).or_insert(0) += 1;
            if let Some(example) = seen.examples.get(ty) {
                if self.example.is_none() {
                    self.example = Some(example.clone());
                }
                self.examples
                    .entry(ty.clone())
                    .or_insert_with(|| example.clone());
            }
        }
        for value in seen.values {
            match self.value_counts.iter_mut().find(|(v, _)| v == &value) {
                Some((_, count)) => *count += 1,
                None => {
                    if self.value_counts.len() < VALUE_SAMPLE_CAP {
                        self.value_counts.push((value, 1));
                    }
                }
            }
        }
    }
}

/// A declared field in the schema, flattened to the same dotted-path space as
/// [`observe`], so the two can be compared directly.
#[derive(Debug, Clone)]
struct DeclaredField {
    types: Vec<String>,
    required: bool,
    enum_values: Option<Vec<Value>>,
}

fn declared_fields(
    document: &Value,
    type_name: &str,
    prefix: &str,
    stack: &mut Vec<String>,
    out: &mut BTreeMap<String, DeclaredField>,
) {
    // A cyclic schema would otherwise recurse forever.
    if stack.contains(&type_name.to_string()) {
        return;
    }
    stack.push(type_name.to_string());

    let Some(schema) = openapi::component_schema(document, type_name) else {
        stack.pop();
        return;
    };

    declare_object(document, schema, prefix, stack, out);
    stack.pop();
}

fn resolve_ref<'a>(document: &'a Value, schema: &'a Value) -> (Option<String>, &'a Value) {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(name) = openapi::ref_name(reference) {
            if let Some(target) = openapi::component_schema(document, name) {
                return (Some(name.to_string()), target);
            }
        }
    }
    (None, schema)
}

/// The type names a schema node declares.
///
/// `nullable: true` deliberately does **not** add `"null"` here. It reads as
/// though it should, and this function used to do it, which made the drift
/// report quietly disagree with the thing it is reporting on: the bus has no
/// implementation of `nullable`, so a `{"type": "string", "nullable": true}`
/// field that receives `null` is rejected in production. Adding `"null"` hid
/// exactly the mismatch worth showing. The `type: [T, "null"]` spelling below
/// is the one draft-07 honours, and is what `openapi::widen_nullable` produces.
fn declared_types(schema: &Value) -> Vec<String> {
    match schema.get("type") {
        Some(Value::String(t)) => vec![t.clone()],
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ if schema.get("properties").is_some() => vec!["object".to_string()],
        _ => Vec::new(),
    }
}

fn declare_object(
    document: &Value,
    schema: &Value,
    prefix: &str,
    stack: &mut Vec<String>,
    out: &mut BTreeMap<String, DeclaredField>,
) {
    let required: Vec<String> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return;
    };

    for (name, raw) in properties {
        let path = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}.{name}")
        };

        let (ref_name, resolved) = resolve_ref(document, raw);

        out.insert(
            path.clone(),
            DeclaredField {
                types: declared_types(resolved),
                required: required.contains(name),
                enum_values: resolved.get("enum").and_then(Value::as_array).cloned(),
            },
        );

        // Recurse into the shape, following refs by name so cycles terminate.
        if let Some(name) = ref_name {
            declared_fields(document, &name, &path, stack, out);
        } else if resolved.get("properties").is_some() {
            declare_object(document, resolved, &path, stack, out);
        } else if let Some(items) = resolved.get("items") {
            // Observed paths collapse array elements onto `path[]`, so the
            // element type has to be declared under the same name or every
            // array would read as an undeclared field.
            let item_path = format!("{path}[]");
            let (item_ref, item_schema) = resolve_ref(document, items);

            out.insert(
                item_path.clone(),
                DeclaredField {
                    types: declared_types(item_schema),
                    required: false,
                    enum_values: item_schema.get("enum").and_then(Value::as_array).cloned(),
                },
            );

            if let Some(name) = item_ref {
                declared_fields(document, &name, &item_path, stack, out);
            } else if item_schema.get("properties").is_some() {
                declare_object(document, item_schema, &item_path, stack, out);
            }
        }
    }
}

/// Validate a sample of event payloads against a type and diff the observed
/// fields against what the schema declares.
///
/// `payloads` are event *details*, not whole envelopes.
pub fn check_events(
    document: &Value,
    type_name: &str,
    payloads: &[Value],
) -> Result<EventCheckReport> {
    let compiled_schema = openapi::schema_for_type(document, type_name);
    // Not `jsonschema::validator_for`: that compiles under 2020-12 with the
    // spec's format set, and the bus compiles under draft-07 with
    // `ajv-formats`. See `schema::ajv`.
    let validator = ajv::validator_for(&compiled_schema)
        .map_err(|e| Error::Invalid(format!("Schema cannot be compiled for validation: {e}")))?;

    let mut passed = 0usize;
    let mut grouped: BTreeMap<(String, String), (FailureGroup, FailureClass)> = BTreeMap::new();

    let mut observed: BTreeMap<String, Observation> = BTreeMap::new();

    for payload in payloads {
        // One traversal: `is_valid` followed by `iter_errors` validated every
        // failing event twice, and a schema worth grading is usually one with
        // failures.
        let mut valid = true;
        for error in validator.iter_errors(payload) {
            valid = false;
            let pointer = error.instance_path.to_string();
            // Group on *where in the schema* the check failed, not on the
            // rendered message: the message embeds the offending value, so
            // grouping by text would split one recurring problem into one
            // row per event.
            let key = (pointer.clone(), error.schema_path.to_string());
            match grouped.get_mut(&key) {
                Some(entry) => entry.0.count += 1,
                None => {
                    // Only the first occurrence keeps its message and example,
                    // so rendering them for all 5000 was work thrown away.
                    let class = classify_failure(&error, &pointer);
                    grouped.insert(
                        key,
                        (
                            FailureGroup {
                                pointer,
                                message: error.to_string(),
                                count: 1,
                                example: Some(error.instance.clone().into_owned()),
                            },
                            class,
                        ),
                    );
                }
            }
        }
        if valid {
            passed += 1;
        }

        let mut per_event: BTreeMap<String, Seen> = BTreeMap::new();
        observe(payload, "", &mut per_event);
        for (path, seen) in per_event {
            observed.entry(path).or_default().absorb(seen);
        }
    }

    let mut declared: BTreeMap<String, DeclaredField> = BTreeMap::new();
    declared_fields(document, type_name, "", &mut Vec::new(), &mut declared);

    let mut drift = DriftReport::default();
    let sampled = payloads.len();

    for (path, observation) in &observed {
        let types = observation.types();
        let seen_in = &observation.seen_in;
        let example = observation.example.clone().unwrap_or(Value::Null);
        match declared.get(path) {
            None => drift.undeclared.push(FieldObservation {
                path: path.clone(),
                types: types.clone(),
                seen_in: *seen_in,
                example: Some(example.clone()),
            }),
            Some(field) => {
                // Only flag a contradiction when the schema actually declares a
                // type. A `null` against a `nullable: true` field *is* a
                // contradiction — the bus ignores `nullable` — so it is
                // reported like any other wrong type; `validate::check_ajv_parity`
                // supplies the explanation.
                if !field.types.is_empty() {
                    let unexpected: Vec<String> = types
                        .iter()
                        .filter(|t| !field.types.contains(t))
                        // `integer` satisfies a `number` declaration.
                        .filter(|t| {
                            !(t.as_str() == "integer" && field.types.iter().any(|d| d == "number"))
                        })
                        .cloned()
                        .collect();
                    if !unexpected.is_empty() {
                        // Count the events that actually carried a wrong type,
                        // and show an example *of* that type — not whichever
                        // value happened to be seen first, which for a field
                        // that is usually fine was usually a fine one.
                        let mismatched_in = unexpected
                            .iter()
                            .filter_map(|t| observation.type_counts.get(t))
                            .sum();
                        let example = unexpected
                            .iter()
                            .find_map(|t| observation.examples.get(t))
                            .cloned()
                            .unwrap_or_else(|| example.clone());
                        // Keep a declared type only while the sample still
                        // contains it — `integer` counting as `number`, the
                        // same equivalence the mismatch itself is judged by.
                        let mut suggested: Vec<String> = field
                            .types
                            .iter()
                            .filter(|declared| {
                                types.iter().any(|seen| {
                                    seen == *declared
                                        || (declared.as_str() == "number" && seen == "integer")
                                })
                            })
                            .cloned()
                            .collect();
                        for seen in &unexpected {
                            if !suggested.contains(seen) {
                                suggested.push(seen.clone());
                            }
                        }

                        drift.type_mismatches.push(TypeMismatch {
                            path: path.clone(),
                            declared: field.types.join(" | "),
                            observed: unexpected,
                            seen_in: *seen_in,
                            mismatched_in,
                            example: Some(example),
                            suggested,
                        });
                    }
                }

                if let Some(allowed) = &field.enum_values {
                    // Enum drift is only detectable for scalar leaves.
                    let offending: Vec<&(Value, usize)> = observation
                        .value_counts
                        .iter()
                        .filter(|(value, _)| !allowed.contains(value) && !value.is_null())
                        .collect();
                    if !offending.is_empty() {
                        drift.enum_drift.push(EnumDrift {
                            path: path.clone(),
                            declared: allowed.clone(),
                            // Every offending value, not just the first one
                            // seen: "saw `retry`" is a different conversation
                            // from "saw `retry`, `deferred`, `cancelled`".
                            unexpected: offending.iter().map(|(v, _)| v.clone()).collect(),
                            seen_in: *seen_in,
                            // One event can carry several offending values in
                            // an array, so the sum can exceed the sample.
                            unexpected_in: offending
                                .iter()
                                .map(|(_, n)| *n)
                                .sum::<usize>()
                                .min(*seen_in),
                        });
                    }
                }

                if field.required && *seen_in < sampled {
                    drift.missing_required.push(FieldObservation {
                        path: path.clone(),
                        types: types.clone(),
                        seen_in: *seen_in,
                        example: Some(example.clone()),
                    });
                }
            }
        }
    }

    for (path, field) in &declared {
        if !observed.contains_key(path) {
            drift.unused.push(UnusedField {
                path: path.clone(),
                required: field.required,
            });
        }
    }

    // Most-frequent first: the drift worth acting on is the drift that happens.
    drift
        .undeclared
        .sort_by(|a, b| b.seen_in.cmp(&a.seen_in).then(a.path.cmp(&b.path)));
    drift
        .type_mismatches
        .sort_by_key(|m| std::cmp::Reverse(m.mismatched_in));
    drift.missing_required.sort_by_key(|f| f.seen_in);
    drift.unused.sort_by(|a, b| a.path.cmp(&b.path));

    let classified: Vec<(FailureGroup, FailureClass)> = grouped.into_values().collect();
    let issues = build_issues(&drift, &classified, sampled);

    let mut failures: Vec<FailureGroup> = classified.into_iter().map(|(f, _)| f).collect();
    failures.sort_by_key(|f| std::cmp::Reverse(f.count));

    let coverage: Coverage = declared
        .keys()
        .map(|path| {
            (
                path.clone(),
                observed.get(path).map(|o| o.seen_in).unwrap_or(0),
            )
        })
        .collect();

    Ok(EventCheckReport {
        sampled,
        passed,
        failed: sampled - passed,
        issues,
        failures,
        drift,
        coverage,
        type_name: type_name.to_string(),
    })
}

/// What a validation failure is *about*, so it can be matched to the drift item
/// describing the same problem.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FailureClass {
    /// Wrong JSON type at `path`.
    WrongType { path: String },
    /// Value outside the declared enum at `path`.
    OutsideEnum { path: String },
    /// Required property missing; `path` is the property, not the object.
    MissingRequired { path: String },
    /// Properties the schema forbids.
    ///
    /// Plural: the validator reports every unexpected property of an object in
    /// one error, so a schema that forbids extras and three fields producers
    /// send is a single failure explaining three issues.
    Undeclared { paths: Vec<String> },
    /// Anything else — a pattern, a format, a bound.
    Other { path: String },
}

impl FailureClass {
    /// The tag and paths this failure explains, for matching against drift.
    fn keys(&self) -> (&'static str, Vec<String>) {
        match self {
            Self::WrongType { path } => ("wrongType", vec![path.clone()]),
            Self::OutsideEnum { path } => ("outsideEnum", vec![path.clone()]),
            Self::MissingRequired { path } => ("missingRequired", vec![path.clone()]),
            Self::Undeclared { paths } => ("undeclared", paths.clone()),
            Self::Other { path } => ("other", vec![path.clone()]),
        }
    }

    /// Where to say this happened, for a failure nothing else explains.
    fn path(&self) -> &str {
        match self {
            Self::WrongType { path }
            | Self::OutsideEnum { path }
            | Self::MissingRequired { path }
            | Self::Other { path } => path,
            Self::Undeclared { paths } => paths.first().map(String::as_str).unwrap_or_default(),
        }
    }
}

/// Rewrite a validator JSON Pointer as the dotted path the drift report uses.
///
/// `/callAttemptHistory/3/result` becomes `callAttemptHistory[].result`, which
/// is how [`observe`] names it — array indices collapse, so one bad element in
/// event 3 and another in event 40 are the same problem.
fn pointer_to_path(pointer: &str) -> String {
    let mut path = String::new();
    for raw in pointer.split('/').skip(1).filter(|s| !s.is_empty()) {
        let segment = raw.replace("~1", "/").replace("~0", "~");
        if segment.chars().all(|c| c.is_ascii_digit()) {
            path.push_str("[]");
        } else if path.is_empty() {
            path.push_str(&segment);
        } else {
            path.push('.');
            path.push_str(&segment);
        }
    }
    path
}

fn join_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}.{name}")
    }
}

fn classify_failure(error: &jsonschema::ValidationError<'_>, pointer: &str) -> FailureClass {
    use jsonschema::error::ValidationErrorKind;
    let path = pointer_to_path(pointer);
    match &error.kind {
        ValidationErrorKind::Type { .. } => FailureClass::WrongType { path },
        ValidationErrorKind::Enum { .. } | ValidationErrorKind::Constant { .. } => {
            FailureClass::OutsideEnum { path }
        }
        // The instance path is the *object* that lacks the property, so the
        // property name has to come off the error itself for this to line up
        // with the drift entry, which is keyed by the field.
        ValidationErrorKind::Required { property } => FailureClass::MissingRequired {
            path: join_path(&path, property.as_str().unwrap_or_default()),
        },
        ValidationErrorKind::AdditionalProperties { unexpected }
        | ValidationErrorKind::UnevaluatedProperties { unexpected } => FailureClass::Undeclared {
            paths: unexpected
                .iter()
                .map(|name| join_path(&path, name))
                .collect(),
        },
        _ => FailureClass::Other { path },
    }
}

/// A share of the sample, never rounding a real occurrence away.
///
/// 1 event in 224 rounds to 0%, and "0% of events are rejected" reads as
/// nothing happening — the opposite of what a rejection means. Anything that
/// happened at all is at least 1%.
fn percent(count: usize, total: usize) -> String {
    if total == 0 || count == 0 {
        return "0%".to_string();
    }
    let exact = count as f64 / total as f64 * 100.0;
    format!("{}%", (exact.round() as usize).max(1))
}

fn field_label(path: &str) -> String {
    if path.is_empty() {
        "the payload".to_string()
    } else {
        format!("`{path}`")
    }
}

/// Fold validation failures and drift into one ranked list of problems.
///
/// The two views overlap almost entirely — a type mismatch on a declared field
/// is also a rejection — so presenting both meant reading the same problem
/// twice with two different denominators. Here the rejection is a *property* of
/// the problem rather than a second entry: `rejects` says whether events are
/// being thrown away over it.
/// What a matched validation failure says about an issue.
///
/// The same four conclusions were drawn in each of the blocks below, which
/// left the parts that genuinely differ — the wording — buried among the parts
/// that do not.
fn from_failure(
    matched: Option<(usize, String)>,
    fallback: usize,
) -> (IssueSeverity, usize, bool, Option<String>) {
    match matched {
        Some((count, message)) => (IssueSeverity::Error, count, true, Some(message)),
        None => (IssueSeverity::Warning, fallback, false, None),
    }
}

/// Whether a path names a property a repair could edit.
///
/// A bare array element is not a named property, and the payload as a whole is
/// not a field — offering a button for either would only produce an error on
/// click, which is a worse answer than no button.
fn repairable(path: &str) -> bool {
    !path.is_empty() && !path.ends_with("[]")
}

fn build_issues(
    drift: &DriftReport,
    failures: &[(FailureGroup, FailureClass)],
    sampled: usize,
) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();

    // Indexed rather than scanned: one failure can explain several issues (an
    // `additionalProperties` rejection names every offending field at once),
    // and every failure has to be accounted for exactly once at the end.
    let mut index: BTreeMap<(&'static str, String), usize> = BTreeMap::new();
    for (position, (_, class)) in failures.iter().enumerate() {
        let (tag, paths) = class.keys();
        for path in paths {
            index.entry((tag, path)).or_insert(position);
        }
    }
    let mut used: BTreeSet<usize> = BTreeSet::new();

    let take = |tag: &'static str, path: &str, used: &mut BTreeSet<usize>| {
        let position = *index.get(&(tag, path.to_string()))?;
        used.insert(position);
        let failure = &failures[position].0;
        Some((failure.count, failure.message.clone()))
    };

    for mismatch in &drift.type_mismatches {
        let (severity, affected, rejects, message) = from_failure(
            take("wrongType", &mismatch.path, &mut used),
            mismatch.mismatched_in,
        );
        let observed = mismatch.observed.join(" | ");
        issues.push(Issue {
            key: format!("wrongType:{}", mismatch.path),
            kind: IssueKind::WrongType,
            severity,
            path: mismatch.path.clone(),
            summary: format!(
                "{} is declared {} but {} of events send {}",
                field_label(&mismatch.path),
                mismatch.declared,
                percent(mismatch.mismatched_in, sampled),
                observed,
            ),
            action: format!(
                "Fix the producer, or redeclare {} as {}.",
                field_label(&mismatch.path),
                observed,
            ),
            declared: Some(mismatch.declared.clone()),
            observed: Some(observed),
            affected,
            sampled,
            rejects,
            example: mismatch.example.clone(),
            message,
            fix: (repairable(&mismatch.path) && !mismatch.suggested.is_empty()).then(|| {
                Repair::WidenType {
                    types: mismatch.suggested.clone(),
                }
            }),
        });
    }

    for entry in &drift.enum_drift {
        let (severity, affected, rejects, message) = from_failure(
            take("outsideEnum", &entry.path, &mut used),
            entry.unexpected_in,
        );
        let unexpected = entry
            .unexpected
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", ");
        issues.push(Issue {
            key: format!("outsideEnum:{}", entry.path),
            kind: IssueKind::OutsideEnum,
            severity,
            path: entry.path.clone(),
            summary: format!(
                "{} carries {} the declared values do not allow",
                field_label(&entry.path),
                unexpected,
            ),
            action: format!(
                "Add {} to the allowed values, or fix the producer.",
                unexpected
            ),
            declared: Some(
                entry
                    .declared
                    .iter()
                    .map(render_value)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            observed: Some(unexpected),
            affected,
            sampled,
            rejects,
            example: entry.unexpected.first().cloned(),
            message,
            fix: (repairable(&entry.path) && !entry.unexpected.is_empty()).then(|| {
                Repair::ExtendEnum {
                    values: entry.unexpected.clone(),
                }
            }),
        });
    }

    for field in &drift.missing_required {
        let absent = sampled.saturating_sub(field.seen_in);
        let (severity, affected, rejects, message) =
            from_failure(take("missingRequired", &field.path, &mut used), absent);
        issues.push(Issue {
            key: format!("missingRequired:{}", field.path),
            kind: IssueKind::MissingRequired,
            severity,
            path: field.path.clone(),
            summary: format!(
                "{} is required but absent from {} of events",
                field_label(&field.path),
                percent(absent, sampled),
            ),
            action: format!(
                "Make {} optional, or fix the producers that omit it.",
                field_label(&field.path)
            ),
            declared: Some("required".to_string()),
            observed: Some(format!("present in {}/{}", field.seen_in, sampled)),
            affected,
            sampled,
            rejects,
            example: field.example.clone(),
            message,
            fix: repairable(&field.path).then_some(Repair::DropRequired),
        });
    }

    for field in &drift.undeclared {
        // Affected is always the number of events carrying the field, whether
        // or not the schema rejects it, so the fallback count is unused here.
        let (severity, _, rejects, message) =
            from_failure(take("undeclared", &field.path, &mut used), field.seen_in);
        let types = field.types.join(" | ");
        issues.push(Issue {
            key: format!("undeclared:{}", field.path),
            kind: IssueKind::Undeclared,
            severity,
            path: field.path.clone(),
            summary: format!(
                "{} of events send {}, which the schema does not describe",
                percent(field.seen_in, sampled),
                field_label(&field.path),
            ),
            action: if rejects {
                format!(
                    "The schema forbids extra fields, so these events are rejected. Declare {}.",
                    field_label(&field.path)
                )
            } else {
                format!(
                    "Declare {} as {} — nothing validates it today.",
                    field_label(&field.path),
                    types
                )
            },
            declared: None,
            observed: Some(types),
            affected: field.seen_in,
            sampled,
            rejects,
            example: field.example.clone(),
            message,
            fix: (repairable(&field.path) && !field.types.is_empty()).then(|| {
                Repair::DeclareField {
                    types: field.types.clone(),
                    example: field.example.clone(),
                }
            }),
        });
    }

    for field in &drift.unused {
        issues.push(Issue {
            key: format!("neverSeen:{}", field.path),
            kind: IssueKind::NeverSeen,
            severity: IssueSeverity::Info,
            path: field.path.clone(),
            summary: format!("{} never appeared in this sample", field_label(&field.path)),
            action: if field.required {
                "Declared required, yet nothing sends it — check the producer before trusting it."
                    .to_string()
            } else {
                "Possibly dead, possibly just rare. Widen the window before removing it."
                    .to_string()
            },
            declared: Some(
                if field.required {
                    "required"
                } else {
                    "optional"
                }
                .to_string(),
            ),
            observed: None,
            affected: 0,
            sampled,
            rejects: false,
            example: None,
            message: None,
            // Nothing mechanical to do: whether a field that went quiet is dead
            // or merely rare is a question about the producer, not the document.
            fix: None,
        });
    }

    // Whatever is left is a rejection no drift category explains: a pattern, a
    // format, a bound. Dropping these would hide real failures.
    for (index, (failure, class)) in failures.iter().enumerate() {
        if used.contains(&index) {
            continue;
        }
        let path = class.path().to_string();
        issues.push(Issue {
            key: format!("rejected:{}:{}", path, failure.message),
            kind: IssueKind::Rejected,
            severity: IssueSeverity::Error,
            path: path.clone(),
            // The validator's message carries the whole finding for this kind
            // — which constraint, and the value that broke it. A summary of
            // "11% of events are rejected at X" without it says only that
            // something is wrong, which is not a thing anyone can act on.
            summary: format!(
                "{} rejects {} of events: {}",
                field_label(&path),
                percent(failure.count, sampled),
                clip(&failure.message, 140),
            ),
            action: "Relax the constraint the events break, or fix the producer.".to_string(),
            declared: None,
            observed: None,
            affected: failure.count,
            sampled,
            rejects: true,
            example: failure.example.clone(),
            message: Some(failure.message.clone()),
            // A pattern, a format, a bound. Which constraint to relax, and to
            // what, is a judgement the sample does not contain.
            fix: None,
        });
    }

    // Rejections first, then by how much traffic each affects: the ranking is
    // the recommendation about what to look at.
    issues.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then(b.affected.cmp(&a.affected))
            .then(a.path.cmp(&b.path))
    });
    issues
}

/// Shorten a message to fit a title, on a word boundary where possible.
fn clip(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_string();
    }
    let truncated: String = trimmed.chars().take(limit).collect();
    let cut = truncated.rfind(' ').unwrap_or(truncated.len());
    format!("{}…", &truncated[..cut])
}

/// A value as it should read inside a sentence.
fn render_value(value: &Value) -> String {
    match value {
        Value::String(s) => format!("`{s}`"),
        other => format!("`{other}`"),
    }
}

/// Resolve a dotted observation path to the schema that should own it.
///
/// `metadata.extra` belongs on whatever type `metadata` refs, not on the root —
/// so adding an observed field has to walk the same ref graph the analysis did.
/// Returns the owning schema's JSON Pointer and the leaf property name.
pub(crate) fn resolve_owner(
    document: &Value,
    type_name: &str,
    path: &str,
) -> Option<(String, String)> {
    let segments: Vec<&str> = path.split('.').collect();
    let (leaf, parents) = segments.split_last()?;
    // An array element is not a named property, so it cannot be added to.
    if leaf.ends_with("[]") || leaf.is_empty() {
        return None;
    }

    let mut pointer = format!("/components/schemas/{type_name}");
    let mut current = document.pointer(&pointer)?.clone();

    for segment in parents {
        let (name, through_array) = match segment.strip_suffix("[]") {
            Some(base) => (base, true),
            None => (*segment, false),
        };

        let property = current.get("properties")?.get(name)?.clone();
        let mut next_pointer = format!("{pointer}/properties/{name}");
        let mut next = property;

        if through_array {
            next_pointer = format!("{next_pointer}/items");
            next = next.get("items")?.clone();
        }

        // Follow a ref to where the definition actually lives.
        if let Some(reference) = next.get("$ref").and_then(Value::as_str) {
            let target = openapi::ref_name(reference)?;
            next_pointer = format!("/components/schemas/{target}");
            next = document.pointer(&next_pointer)?.clone();
        }

        pointer = next_pointer;
        current = next;
    }

    Some((pointer, (*leaf).to_string()))
}

/// Declare one observed field on whichever type owns its path.
///
/// The type is taken from what was actually seen rather than guessed, and a
/// field seen as both a value and `null` is declared nullable.
pub fn add_observed_field(
    document: &Value,
    type_name: &str,
    field: &FieldObservation,
) -> Result<Value> {
    let (pointer, leaf) = resolve_owner(document, type_name, &field.path).ok_or_else(|| {
        Error::Invalid(format!(
            "Could not work out which type owns `{}` — add it by hand in the JSON view",
            field.path
        ))
    })?;

    let mut next = document.clone();
    let owner = next
        .pointer_mut(&pointer)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::Invalid(format!("{pointer} is not an object")))?;

    let properties = owner
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let properties = properties
        .as_object_mut()
        .ok_or_else(|| Error::Invalid("`properties` is not an object".into()))?;

    if properties.contains_key(&leaf) {
        return Err(Error::Invalid(format!(
            "`{}` is already declared",
            field.path
        )));
    }

    properties.insert(leaf, declaration_for(field));
    Ok(next)
}

/// The schema snippet describing an observed field.
///
/// A field seen as both a value and `null` is declared `type: [T, "null"]`
/// rather than `nullable: true`. Both spellings say the same thing to a reader;
/// only the first says it to Ajv, and this snippet exists precisely to stop the
/// events that were observed from being rejected.
fn declaration_for(field: &FieldObservation) -> Value {
    let non_null: Vec<&String> = field.types.iter().filter(|t| *t != "null").collect();
    let saw_null = field.types.iter().any(|t| t == "null");
    let mut declaration = Map::new();

    // Mixed or unknown types: leave it untyped rather than guessing.
    if let [single] = non_null.as_slice() {
        let single = single.as_str();
        declaration.insert(
            "type".into(),
            if saw_null {
                json!([single, "null"])
            } else {
                Value::String(single.to_string())
            },
        );

        // An observed object or array conveys nothing about its inner shape,
        // so leave it open rather than inventing one.
        if single == "object" {
            declaration.insert("additionalProperties".into(), Value::Bool(true));
        }
    }
    Value::Object(declaration)
}

/// Build the patch that would add every undeclared field to a type.
///
/// Only top-level scalars are added: a nested path implies a shape decision
/// (inline object or a new named type) that the user should make deliberately.
pub fn suggest_additions(
    document: &Value,
    type_name: &str,
    undeclared: &[FieldObservation],
) -> Value {
    let mut next = document.clone();

    let Some(schema) = next
        .pointer_mut(&format!("/components/schemas/{type_name}"))
        .and_then(Value::as_object_mut)
    else {
        return next;
    };

    let properties = schema
        .entry("properties".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let Some(properties) = properties.as_object_mut() else {
        return next;
    };

    for field in undeclared {
        if field.path.contains('.') || field.path.contains("[]") {
            continue;
        }
        if properties.contains_key(&field.path) {
            continue;
        }

        // The same builder the one-at-a-time path uses, so declaring a field
        // in bulk and declaring it singly cannot produce different schemas —
        // they differed over `additionalProperties` on an observed object.
        properties.insert(field.path.clone(), declaration_for(field));
    }

    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn document() -> Value {
        json!({
            "components": {
                "schemas": {
                    "AWSEvent": {
                        "type": "object",
                        "properties": { "detail": { "$ref": "#/components/schemas/Sync" } }
                    },
                    "Sync": {
                        "type": "object",
                        "required": ["clientId"],
                        "properties": {
                            "clientId": { "type": "string" },
                            "status": { "type": "string", "enum": ["queued", "done"] },
                            "note": { "type": "string", "nullable": true },
                            "count": { "type": "integer" },
                            "metadata": { "$ref": "#/components/schemas/Metadata" },
                            "retired": { "type": "string" }
                        }
                    },
                    "Metadata": {
                        "type": "object",
                        "properties": { "trackingId": { "type": "string" } }
                    }
                }
            }
        })
    }

    #[test]
    fn passes_events_that_match() {
        let events = vec![
            json!({ "clientId": "a", "status": "queued", "count": 1 }),
            json!({ "clientId": "b", "status": "done", "count": 2 }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();
        assert_eq!(report.sampled, 2);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 0);
        assert!(report.failures.is_empty());
    }

    #[test]
    fn a_null_against_nullable_is_a_failure_because_the_bus_rejects_it() {
        // `note` is declared `{"type": "string", "nullable": true}`. pontifex used
        // to widen that to `["string", "null"]` before validating and report a
        // pass — but Ajv has no `nullable`, so the bus rejects this event. The
        // pass was the bug: it hid a real rejection behind a tidy report.
        let events = vec![json!({ "clientId": "a", "note": null })];
        let report = check_events(&document(), "Sync", &events).unwrap();
        assert_eq!(report.failed, 1, "failures: {:?}", report.failures);
        assert!(
            report.failures.iter().any(|f| f.pointer == "/note"),
            "{:?}",
            report.failures
        );
    }

    #[test]
    fn the_widened_spelling_does_accept_a_null() {
        // The other half of the same story: once the document is repaired the
        // event passes, so the finding is actionable rather than just true.
        let repaired = openapi::widen_nullable(&document());
        let events = vec![json!({ "clientId": "a", "note": null })];
        let report = check_events(&repaired, "Sync", &events).unwrap();
        assert_eq!(report.passed, 1, "failures: {:?}", report.failures);
    }

    #[test]
    fn reports_a_missing_required_field_as_a_failure() {
        let events = vec![json!({ "status": "queued" })];
        let report = check_events(&document(), "Sync", &events).unwrap();
        assert_eq!(report.failed, 1);
        assert!(
            report
                .failures
                .iter()
                .any(|f| f.message.contains("clientId")),
            "{:?}",
            report.failures
        );
    }

    #[test]
    fn groups_identical_failures_across_events() {
        let events = vec![
            json!({ "clientId": 1 }),
            json!({ "clientId": 2 }),
            json!({ "clientId": 3 }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();
        assert_eq!(report.failed, 3);
        // One group, counted three times — not three separate rows.
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].count, 3);
    }

    #[test]
    fn finds_fields_the_schema_does_not_declare() {
        let events = vec![
            json!({ "clientId": "a", "newField": "x" }),
            json!({ "clientId": "b", "newField": "y" }),
            json!({ "clientId": "c" }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();
        let undeclared = &report.drift.undeclared;
        assert_eq!(undeclared.len(), 1);
        assert_eq!(undeclared[0].path, "newField");
        assert_eq!(undeclared[0].types, vec!["string"]);
        assert_eq!(undeclared[0].seen_in, 2);
    }

    #[test]
    fn finds_undeclared_fields_nested_under_a_ref() {
        let events = vec![json!({
            "clientId": "a",
            "metadata": { "trackingId": "t", "extra": 5 }
        })];
        let report = check_events(&document(), "Sync", &events).unwrap();
        assert!(report
            .drift
            .undeclared
            .iter()
            .any(|f| f.path == "metadata.extra" && f.types == vec!["integer"]));
    }

    #[test]
    fn reports_declared_fields_that_never_appear() {
        let events = vec![json!({ "clientId": "a" })];
        let report = check_events(&document(), "Sync", &events).unwrap();
        let unused: Vec<&str> = report
            .drift
            .unused
            .iter()
            .map(|u| u.path.as_str())
            .collect();
        assert!(unused.contains(&"retired"));
        assert!(unused.contains(&"metadata.trackingId"));
    }

    #[test]
    fn reports_a_required_field_missing_from_some_events() {
        // Passing validation is not the same as being consistently present:
        // an optional-in-practice required field is worth surfacing.
        let doc = json!({
            "components": { "schemas": { "T": {
                "type": "object",
                "required": ["a"],
                "properties": { "a": { "type": "string" }, "b": { "type": "string" } }
            }}}
        });
        let events = vec![json!({ "a": "x", "b": "y" }), json!({ "b": "y" })];
        let report = check_events(&doc, "T", &events).unwrap();
        let missing = &report.drift.missing_required;
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].path, "a");
        assert_eq!(missing[0].seen_in, 1);
    }

    #[test]
    fn reports_a_type_that_contradicts_the_schema() {
        let events = vec![json!({ "clientId": "a", "count": "twelve" })];
        let report = check_events(&document(), "Sync", &events).unwrap();
        let mismatch = report
            .drift
            .type_mismatches
            .iter()
            .find(|m| m.path == "count")
            .expect("count mismatch");
        assert_eq!(mismatch.declared, "integer");
        assert_eq!(mismatch.observed, vec!["string"]);
    }

    #[test]
    fn accepts_an_integer_where_a_number_is_declared() {
        let doc = json!({
            "components": { "schemas": { "T": {
                "type": "object",
                "properties": { "amount": { "type": "number" } }
            }}}
        });
        let report = check_events(&doc, "T", &[json!({ "amount": 3 })]).unwrap();
        assert!(report.drift.type_mismatches.is_empty());
    }

    #[test]
    fn reports_enum_values_real_traffic_exceeded() {
        let events = vec![json!({ "clientId": "a", "status": "cancelled" })];
        let report = check_events(&document(), "Sync", &events).unwrap();
        assert_eq!(report.drift.enum_drift.len(), 1);
        assert_eq!(report.drift.enum_drift[0].path, "status");
        assert_eq!(
            report.drift.enum_drift[0].unexpected,
            vec![json!("cancelled")]
        );
    }

    #[test]
    fn counts_a_mismatch_by_the_events_that_have_it_not_the_ones_that_have_the_field() {
        // 8 events carry `count`; only 2 carry it as a string. Reporting 8/8
        // read as "every event is broken" when 75% of them were fine.
        let mut events: Vec<Value> = (0..6)
            .map(|i| json!({ "clientId": "a", "count": i }))
            .collect();
        events.push(json!({ "clientId": "a", "count": "14" }));
        events.push(json!({ "clientId": "a", "count": "15" }));

        let report = check_events(&document(), "Sync", &events).unwrap();
        let mismatch = &report.drift.type_mismatches[0];
        assert_eq!(mismatch.seen_in, 8);
        assert_eq!(mismatch.mismatched_in, 2);
        // The example illustrates the problem, not the majority that is fine.
        assert_eq!(mismatch.example, Some(json!("14")));
    }

    #[test]
    fn reports_a_wrong_type_once_rather_than_as_a_failure_and_a_mismatch() {
        let events = vec![
            json!({ "clientId": "a", "count": 1 }),
            json!({ "clientId": "a", "count": "14" }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();

        let about_count: Vec<&Issue> = report.issues.iter().filter(|i| i.path == "count").collect();
        assert_eq!(about_count.len(), 1, "one problem, one issue");

        let issue = about_count[0];
        assert_eq!(issue.kind, IssueKind::WrongType);
        assert_eq!(issue.severity, IssueSeverity::Error);
        assert!(issue.rejects, "these events are thrown away today");
        assert_eq!(issue.affected, 1);
        assert_eq!(issue.sampled, 2);
        assert_eq!(issue.declared.as_deref(), Some("integer"));
        assert_eq!(issue.observed.as_deref(), Some("string"));
        assert!(
            issue.action.contains("Fix the producer"),
            "{}",
            issue.action
        );
        // The validator's wording survives, since it is what a producer team
        // will recognise.
        assert!(issue.message.is_some());
    }

    #[test]
    fn ranks_rejections_above_drift_and_dead_fields_last() {
        let events = vec![json!({ "clientId": "a", "count": "14", "extra": true })];
        let report = check_events(&document(), "Sync", &events).unwrap();

        let kinds: Vec<IssueKind> = report.issues.iter().map(|i| i.kind).collect();
        assert_eq!(kinds.first(), Some(&IssueKind::WrongType));
        assert!(kinds.contains(&IssueKind::Undeclared));
        assert_eq!(kinds.last(), Some(&IssueKind::NeverSeen));
    }

    #[test]
    fn one_rejection_naming_several_fields_marks_all_of_them_rejected() {
        // A closed schema reports every unexpected property in a single error.
        // Matching only the first left the others reading "not rejected" while
        // their events were being thrown away.
        let closed = json!({
            "components": {
                "schemas": {
                    "Sync": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": { "clientId": { "type": "string" } }
                    }
                }
            }
        });
        let events = vec![json!({ "clientId": "a", "extraOne": 1, "extraTwo": 2 })];
        let report = check_events(&closed, "Sync", &events).unwrap();

        let undeclared: Vec<&Issue> = report
            .issues
            .iter()
            .filter(|i| i.kind == IssueKind::Undeclared)
            .collect();
        assert_eq!(undeclared.len(), 2);
        assert!(undeclared.iter().all(|i| i.rejects), "{undeclared:#?}");
        // And the failure is not also reported as an unexplained rejection.
        assert!(!report.issues.iter().any(|i| i.kind == IssueKind::Rejected));
    }

    #[test]
    fn a_rejection_says_which_constraint_broke_not_just_that_one_did() {
        let document = json!({
            "components": {
                "schemas": {
                    "Sync": {
                        "type": "object",
                        "properties": { "clientId": { "type": "string", "minLength": 4 } }
                    }
                }
            }
        });
        // An empty string is the case that used to read as nothing at all: the
        // example rendered blank and the message was never shown.
        let events = vec![json!({ "clientId": "" })];
        let report = check_events(&document, "Sync", &events).unwrap();

        let issue = report
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::Rejected)
            .expect("the minLength rejection survives");
        assert!(issue.summary.contains("clientId"), "{}", issue.summary);
        // The validator's wording is in the summary, so a Jira title made from
        // it is actionable on its own.
        assert!(issue.summary.contains("shorter than"), "{}", issue.summary);
        assert_eq!(issue.example, Some(json!("")));
        assert!(issue.message.is_some());
    }

    #[test]
    fn a_single_affected_event_never_rounds_down_to_no_events() {
        // 1 in 224 is 0.4%, and "0% of events are rejected" describes nothing
        // happening.
        assert_eq!(percent(1, 224), "1%");
        assert_eq!(percent(2, 224), "1%");
        assert_eq!(percent(24, 224), "11%");
        // Genuinely none is still none.
        assert_eq!(percent(0, 224), "0%");
        assert_eq!(percent(0, 0), "0%");
    }

    #[test]
    fn a_long_validator_message_is_clipped_rather_than_filling_the_title() {
        let long = "x".repeat(400);
        let clipped = clip(&long, 140);
        assert_eq!(clipped.chars().count(), 141, "140 plus the ellipsis");
        assert!(clipped.ends_with('…'));
        // Short messages are left exactly as they are.
        assert_eq!(clip("already short", 140), "already short");
    }

    #[test]
    fn keeps_a_rejection_no_drift_category_explains() {
        let document = json!({
            "components": {
                "schemas": {
                    "Sync": {
                        "type": "object",
                        "properties": { "clientId": { "type": "string", "minLength": 4 } }
                    }
                }
            }
        });
        let events = vec![json!({ "clientId": "ab" })];
        let report = check_events(&document, "Sync", &events).unwrap();

        let issue = report
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::Rejected)
            .expect("the minLength rejection survives");
        assert_eq!(issue.path, "clientId");
        assert!(issue.rejects);
        assert_eq!(issue.affected, 1);
    }

    #[test]
    fn matches_a_missing_required_field_to_its_rejection() {
        let events = vec![
            json!({ "count": 1 }),
            json!({ "clientId": "a", "count": 2 }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();

        // The validator reports this against the *object*; the drift report
        // keys it by the field. They still have to line up.
        let issue = report
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::MissingRequired)
            .expect("missing required issue");
        assert_eq!(issue.path, "clientId");
        assert!(issue.rejects);
        assert_eq!(issue.affected, 1);
        assert_eq!(
            report
                .issues
                .iter()
                .filter(|i| i.kind == IssueKind::Rejected)
                .count(),
            0,
            "the rejection is folded into the missing-required issue",
        );
    }

    #[test]
    fn names_every_value_that_exceeded_an_enum_and_counts_them() {
        let events = vec![
            json!({ "clientId": "a", "status": "queued" }),
            json!({ "clientId": "a", "status": "cancelled" }),
            json!({ "clientId": "a", "status": "retry" }),
            json!({ "clientId": "a", "status": "retry" }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();

        let entry = &report.drift.enum_drift[0];
        assert_eq!(entry.seen_in, 4);
        assert_eq!(entry.unexpected_in, 3);
        assert_eq!(entry.unexpected, vec![json!("cancelled"), json!("retry")]);
    }

    #[test]
    fn collapses_array_indices_when_matching_a_failure_to_a_path() {
        assert_eq!(pointer_to_path("/callAttemptCount"), "callAttemptCount");
        assert_eq!(
            pointer_to_path("/callAttemptHistory/3/result"),
            "callAttemptHistory[].result",
        );
        assert_eq!(pointer_to_path(""), "");
        // Pointer escapes: `~1` is a literal slash in a property name.
        assert_eq!(pointer_to_path("/a~1b"), "a/b");
    }

    #[test]
    fn counts_coverage_per_declared_field() {
        let events = vec![
            json!({ "clientId": "a", "status": "queued" }),
            json!({ "clientId": "b" }),
            json!({ "clientId": "c" }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();
        assert_eq!(report.coverage.get("clientId"), Some(&3));
        assert_eq!(report.coverage.get("status"), Some(&1));
        assert_eq!(report.coverage.get("retired"), Some(&0));
    }

    #[test]
    fn collapses_array_elements_onto_one_path() {
        let doc = json!({
            "components": { "schemas": { "T": {
                "type": "object",
                "properties": { "tags": { "type": "array", "items": { "type": "string" } } }
            }}}
        });
        let events = vec![json!({ "tags": ["a", "b", "c"] })];
        let report = check_events(&doc, "T", &events).unwrap();
        // Three elements must not become three undeclared paths.
        assert!(
            report.drift.undeclared.is_empty(),
            "{:?}",
            report.drift.undeclared
        );
    }

    #[test]
    fn handles_a_cyclic_schema_without_hanging() {
        let doc = json!({
            "components": { "schemas": { "Node": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "child": { "$ref": "#/components/schemas/Node" }
                }
            }}}
        });
        let report = check_events(&doc, "Node", &[json!({ "name": "a" })]).unwrap();
        assert_eq!(report.passed, 1);
    }

    #[test]
    fn suggests_declarations_for_top_level_undeclared_fields() {
        let events = vec![
            json!({ "clientId": "a", "newField": "x", "maybe": null }),
            json!({ "clientId": "b", "newField": "y", "maybe": "here" }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();
        let patched = suggest_additions(&document(), "Sync", &report.drift.undeclared);

        let props = &patched["components"]["schemas"]["Sync"]["properties"];
        assert_eq!(props["newField"], json!({ "type": "string" }));
        // Declared exactly as the one-at-a-time path would declare it.
        assert_eq!(
            props["newField"],
            declaration_for(
                report
                    .drift
                    .undeclared
                    .iter()
                    .find(|f| f.path == "newField")
                    .unwrap()
            ),
        );
        // Seen as both null and string, so it is declared with both types —
        // the spelling Ajv honours, not `nullable: true`.
        assert_eq!(props["maybe"], json!({ "type": ["string", "null"] }));
        // Existing declarations are untouched.
        assert_eq!(props["clientId"], json!({ "type": "string" }));
    }

    #[test]
    fn adds_a_top_level_field_to_the_root_type() {
        let field = FieldObservation {
            path: "newField".into(),
            types: vec!["string".into()],
            seen_in: 2,
            example: None,
        };
        let next = add_observed_field(&document(), "Sync", &field).unwrap();
        assert_eq!(
            next["components"]["schemas"]["Sync"]["properties"]["newField"],
            json!({ "type": "string" })
        );
    }

    #[test]
    fn adds_a_nested_field_to_the_type_that_owns_it() {
        // `metadata` refs Metadata, so the field belongs there — not on Sync.
        let field = FieldObservation {
            path: "metadata.extra".into(),
            types: vec!["integer".into()],
            seen_in: 1,
            example: None,
        };
        let next = add_observed_field(&document(), "Sync", &field).unwrap();
        assert_eq!(
            next["components"]["schemas"]["Metadata"]["properties"]["extra"],
            json!({ "type": "integer" })
        );
        // The root type is left alone.
        assert!(next["components"]["schemas"]["Sync"]["properties"]
            .get("metadata.extra")
            .is_none());
    }

    #[test]
    fn adds_a_field_nested_under_an_array_of_refs() {
        let doc = json!({
            "components": { "schemas": {
                "Root": {
                    "type": "object",
                    "properties": {
                        "lines": {
                            "type": "array",
                            "items": { "$ref": "#/components/schemas/Line" }
                        }
                    }
                },
                "Line": { "type": "object", "properties": { "sku": { "type": "string" } } }
            }}
        });
        let field = FieldObservation {
            path: "lines[].qty".into(),
            types: vec!["integer".into()],
            seen_in: 3,
            example: None,
        };
        let next = add_observed_field(&doc, "Root", &field).unwrap();
        assert_eq!(
            next["components"]["schemas"]["Line"]["properties"]["qty"],
            json!({ "type": "integer" })
        );
    }

    #[test]
    fn declares_a_sometimes_null_field_with_both_types() {
        let field = FieldObservation {
            path: "maybe".into(),
            types: vec!["string".into(), "null".into()],
            seen_in: 2,
            example: None,
        };
        let next = add_observed_field(&document(), "Sync", &field).unwrap();
        assert_eq!(
            next["components"]["schemas"]["Sync"]["properties"]["maybe"],
            json!({ "type": ["string", "null"] })
        );

        // And the point of it: the nulls that prompted the suggestion now pass.
        let events = vec![json!({ "clientId": "a", "maybe": null })];
        let report = check_events(&next, "Sync", &events).unwrap();
        assert_eq!(report.passed, 1, "failures: {:?}", report.failures);
    }

    #[test]
    fn leaves_an_observed_object_open_rather_than_inventing_a_shape() {
        let field = FieldObservation {
            path: "blob".into(),
            types: vec!["object".into()],
            seen_in: 1,
            example: None,
        };
        let next = add_observed_field(&document(), "Sync", &field).unwrap();
        assert_eq!(
            next["components"]["schemas"]["Sync"]["properties"]["blob"],
            json!({ "type": "object", "additionalProperties": true })
        );
    }

    #[test]
    fn refuses_to_redeclare_an_existing_field() {
        let field = FieldObservation {
            path: "clientId".into(),
            types: vec!["string".into()],
            seen_in: 1,
            example: None,
        };
        assert!(add_observed_field(&document(), "Sync", &field).is_err());
    }

    #[test]
    fn explains_itself_when_the_owning_type_cannot_be_resolved() {
        for path in ["ghost.child", "items[]", ""] {
            let field = FieldObservation {
                path: path.into(),
                types: vec!["string".into()],
                seen_in: 1,
                example: None,
            };
            assert!(
                add_observed_field(&document(), "Sync", &field).is_err(),
                "should refuse {path:?}"
            );
        }
    }

    #[test]
    fn does_not_mutate_the_document_when_adding() {
        let original = document();
        let snapshot = serde_json::to_string(&original).unwrap();
        let field = FieldObservation {
            path: "newField".into(),
            types: vec!["string".into()],
            seen_in: 1,
            example: None,
        };
        add_observed_field(&original, "Sync", &field).unwrap();
        assert_eq!(serde_json::to_string(&original).unwrap(), snapshot);
    }

    #[test]
    fn does_not_guess_at_nested_additions() {
        let undeclared = vec![FieldObservation {
            path: "metadata.extra".into(),
            types: vec!["string".into()],
            seen_in: 1,
            example: None,
        }];
        let patched = suggest_additions(&document(), "Sync", &undeclared);
        assert_eq!(patched, document());
    }

    #[test]
    fn reports_zero_sampled_without_erroring() {
        let report = check_events(&document(), "Sync", &[]).unwrap();
        assert_eq!(report.sampled, 0);
        assert_eq!(report.passed, 0);
        assert!(report.drift.undeclared.is_empty());
        // Every declared field is trivially unused with no sample.
        assert!(!report.drift.unused.is_empty());
    }

    #[test]
    fn errors_clearly_when_the_type_cannot_compile() {
        let broken = json!({
            "components": { "schemas": { "T": { "type": 42 } } }
        });
        assert!(check_events(&broken, "T", &[]).is_err());
    }

    /// The repair offered for a wrong type, by path.
    fn offered(report: &EventCheckReport, path: &str) -> Option<Repair> {
        report
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::WrongType && i.path == path)
            .and_then(|i| i.fix.clone())
    }

    #[test]
    fn a_partly_null_field_keeps_the_type_the_rest_of_the_traffic_uses() {
        // The everflowId case. `observed` holds only `null`, and the action line
        // reads "redeclare as null" — doing that literally would repair 7% of
        // the traffic by starting to reject the other 93%.
        let events = vec![
            json!({ "clientId": "a", "note": "hi" }),
            json!({ "clientId": "b", "note": "there" }),
            json!({ "clientId": "c", "note": null }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();

        assert_eq!(
            offered(&report, "note"),
            Some(Repair::WidenType {
                types: vec!["string".into(), "null".into()]
            })
        );
    }

    #[test]
    fn a_type_nothing_sends_any_more_is_dropped_rather_than_kept() {
        // The screenshot case: declared object, every event a string or null.
        // Keeping `object` in the union would leave the declaration describing
        // traffic that no longer exists.
        let events = vec![
            json!({ "clientId": "a", "metadata": "W_3M43Y" }),
            json!({ "clientId": "b", "metadata": null }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();

        let Some(Repair::WidenType { types }) = offered(&report, "metadata") else {
            panic!(
                "expected a widen repair for metadata, got {:?}",
                offered(&report, "metadata")
            );
        };
        assert!(
            !types.contains(&"object".to_string()),
            "kept a dead type: {types:?}"
        );
        assert!(types.contains(&"string".to_string()));
        assert!(types.contains(&"null".to_string()));
    }

    #[test]
    fn an_integer_still_satisfies_a_number_declaration_in_the_repair() {
        let base = json!({
            "components": {"schemas": {"Sync": {
                "type": "object",
                "properties": {"amount": {"type": "number"}}
            }}}
        });
        let events = vec![json!({"amount": 3}), json!({"amount": "3"})];
        let report = check_events(&base, "Sync", &events).unwrap();

        let Some(Repair::WidenType { types }) = offered(&report, "amount") else {
            panic!("expected a widen repair for amount");
        };
        // `number` covers the integer traffic, so it survives; `string` joins it.
        assert!(
            types.contains(&"number".to_string()),
            "dropped number: {types:?}"
        );
        assert!(types.contains(&"string".to_string()));
    }

    #[test]
    fn an_undeclared_field_carries_the_repair_that_declares_it() {
        let events = vec![json!({ "clientId": "a", "ringbaId": "W_3M43Y" })];
        let report = check_events(&document(), "Sync", &events).unwrap();

        let issue = report
            .issues
            .iter()
            .find(|i| i.kind == IssueKind::Undeclared && i.path == "ringbaId")
            .expect("undeclared issue");
        assert!(matches!(issue.fix, Some(Repair::DeclareField { .. })));
    }

    #[test]
    fn issues_with_no_mechanical_repair_offer_none() {
        let events = vec![json!({ "clientId": "a" })];
        let report = check_events(&document(), "Sync", &events).unwrap();

        for issue in &report.issues {
            if issue.kind == IssueKind::NeverSeen {
                assert!(issue.fix.is_none(), "{} offered a repair", issue.path);
            }
        }
    }

    #[test]
    fn a_repair_offered_for_a_wrong_type_actually_clears_it() {
        // The property worth testing: applying what the row offers makes the
        // row go away. A repair that does not is worse than no button.
        let events = vec![
            json!({ "clientId": "a", "note": "hi" }),
            json!({ "clientId": "b", "note": null }),
        ];
        let report = check_events(&document(), "Sync", &events).unwrap();
        let fix = offered(&report, "note").expect("a repair for note");

        let repaired = crate::schema::repair::apply(&document(), "Sync", "note", &fix).unwrap();
        let after = check_events(&repaired, "Sync", &events).unwrap();

        assert_eq!(
            after.failed, 0,
            "events still rejected: {:?}",
            after.failures
        );
        assert!(
            offered(&after, "note").is_none(),
            "the issue survived its own repair"
        );
    }
}
