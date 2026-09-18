//! Turning a watch into a CloudWatch filter pattern, and matching locally.
//!
//! The server does the heavy lifting: `FilterLogEvents` is sent a pattern
//! built from the watch, so the poller only ever downloads events that
//! matched *some* watch. When several watches share a log group they are
//! folded into one `||` pattern and fetched in a single call, which is why a
//! second local pass exists — to work out which of them each event satisfied.
//!
//! The local matcher mirrors CloudWatch's semantics as far as a watch uses
//! them: exact string match with `*` as a wildcard, numeric equality for
//! unquoted numbers, and `$.a.b[0].c` paths.

use super::{trimmed, Watch, WatchOp};
use crate::commands::logs::{detail_type_clause, escape, source_clause, wrap_clauses};
use crate::error::{Error, Result};
use serde_json::Value;

/// What a watch asks CloudWatch for.
pub enum Compiled {
    /// A pattern written by hand, sent verbatim in a call of its own.
    Raw(String),
    /// Clauses from the structured fields, `&&`-ed together; can be `||`-ed
    /// with other structured watches on the same log group.
    Structured(Vec<String>),
    /// Nothing to filter on: every event on the group.
    Everything,
}

/// Classify a watch and build its clauses, validating the field paths.
pub fn classify(watch: &Watch) -> Result<Compiled> {
    if let Some(raw) = watch.raw() {
        return Ok(Compiled::Raw(raw.to_string()));
    }
    let clauses = clauses(watch)?;
    Ok(if clauses.is_empty() {
        Compiled::Everything
    } else {
        Compiled::Structured(clauses)
    })
}

/// The pattern for one watch, or `None` when it matches everything.
pub fn compile(watch: &Watch) -> Result<Option<String>> {
    Ok(match classify(watch)? {
        Compiled::Raw(raw) => Some(raw),
        Compiled::Structured(clauses) => Some(wrap_clauses(&clauses)),
        Compiled::Everything => None,
    })
}

/// The bare clauses of a structured watch, without the outer braces, so they
/// can be grouped with other watches' clauses.
fn clauses(watch: &Watch) -> Result<Vec<String>> {
    let mut out = Vec::new();
    if let Some(source) = trimmed(watch.source.as_deref()) {
        out.push(source_clause(source));
    }
    if let Some(detail_type) = trimmed(watch.detail_type.as_deref()) {
        out.push(detail_type_clause(detail_type));
    }
    for condition in &watch.conditions {
        let Some(path) = trimmed(Some(&condition.path)) else {
            continue;
        };
        let selector = selector(path)?;
        let op = match condition.op {
            WatchOp::Eq => "=",
            WatchOp::Ne => "!=",
        };
        out.push(format!("{selector} {op} {}", literal(&condition.value)));
    }
    Ok(out)
}

/// One pattern covering every structured watch in a group.
///
/// `{ (a && b) || (c) }`. A watch with no clauses matches everything, so its
/// presence makes the whole group unfiltered — the caller sends no pattern.
pub fn compile_group(watches: &[&Watch]) -> Result<Option<String>> {
    let mut groups: Vec<Vec<String>> = Vec::new();
    for watch in watches {
        let clauses = clauses(watch)?;
        if clauses.is_empty() {
            return Ok(None);
        }
        groups.push(clauses);
    }
    Ok(match groups.as_slice() {
        [] => None,
        [only] => Some(wrap_clauses(only)),
        many => {
            let ored: Vec<String> = many
                .iter()
                .map(|g| format!("({})", g.join(" && ")))
                .collect();
            Some(format!("{{ {} }}", ored.join(" || ")))
        }
    })
}

/// `detail.clientId`, `.detail.clientId` and `$.detail.clientId` all mean the
/// same thing to a person; this is the path with any such root removed.
fn strip_root(path: &str) -> &str {
    path.strip_prefix("$.")
        .or_else(|| path.strip_prefix('$'))
        .or_else(|| path.strip_prefix('.'))
        .unwrap_or(path)
}

/// Normalise a user-typed path to the `$.a.b` form CloudWatch wants.
///
/// The one thing CloudWatch itself rejects is a quoted segment
/// (`$."detail-type"`), so the same check applies here rather than letting
/// the poller discover it on its first call.
fn selector(path: &str) -> Result<String> {
    let body = strip_root(path);
    if body.is_empty() {
        return Err(Error::Invalid(
            "A condition needs a field path, e.g. detail.clientId".into(),
        ));
    }
    if body.contains('"') || body.contains(' ') {
        return Err(Error::Invalid(format!(
            "Field path '{path}' cannot contain quotes or spaces — CloudWatch rejects them"
        )));
    }
    Ok(format!("$.{body}"))
}

/// How a value is written into the pattern.
///
/// Unquoted numbers and `true`/`false`/`null` go in raw, so `count = 5`
/// compares numerically. Everything else is a string. Wrapping the value in
/// quotes forces a string, for ids that happen to look like numbers.
fn literal(value: &str) -> String {
    let value = value.trim();
    if let Some(forced) = quoted(value) {
        return format!("\"{}\"", escape(forced));
    }
    if is_scalar_keyword(value) {
        return value.to_string();
    }
    format!("\"{}\"", escape(value))
}

fn quoted(value: &str) -> Option<&str> {
    (value.len() >= 2 && value.starts_with('"') && value.ends_with('"'))
        .then(|| &value[1..value.len() - 1])
}

fn is_scalar_keyword(value: &str) -> bool {
    matches!(value, "true" | "false" | "null") || value.parse::<f64>().is_ok()
}

// --- local matching -------------------------------------------------------

/// Whether an event envelope satisfies a watch.
///
/// Raw-pattern watches are always fetched in their own call, so the server's
/// answer stands and this returns `true`. Structured watches are re-evaluated
/// field by field.
pub fn matches(watch: &Watch, envelope: &Value) -> bool {
    if watch.raw().is_some() {
        return true;
    }
    if let Some(source) = trimmed(watch.source.as_deref()) {
        if !envelope
            .get("source")
            .and_then(Value::as_str)
            .is_some_and(|s| glob(source, s))
        {
            return false;
        }
    }
    if let Some(detail_type) = trimmed(watch.detail_type.as_deref()) {
        if !envelope
            .get("detail-type")
            .and_then(Value::as_str)
            .is_some_and(|s| glob(detail_type, s))
        {
            return false;
        }
    }
    watch.conditions.iter().all(|condition| {
        let Some(path) = trimmed(Some(&condition.path)) else {
            return true;
        };
        let actual = lookup(envelope, path);
        let equal = actual.is_some_and(|v| value_equals(v, &condition.value));
        match condition.op {
            WatchOp::Eq => equal,
            // CloudWatch's `!=` is true when the field is present and differs,
            // and false when the field is absent.
            WatchOp::Ne => actual.is_some() && !equal,
        }
    })
}

fn value_equals(actual: &Value, expected: &str) -> bool {
    let expected = expected.trim();
    if let Some(forced) = quoted(expected) {
        return actual.as_str().is_some_and(|s| glob(forced, s));
    }
    match expected {
        "true" => return actual == &Value::Bool(true),
        "false" => return actual == &Value::Bool(false),
        "null" => return actual.is_null(),
        _ => {}
    }
    if let Ok(n) = expected.parse::<f64>() {
        return actual
            .as_f64()
            .is_some_and(|a| (a - n).abs() < f64::EPSILON);
    }
    actual.as_str().is_some_and(|s| glob(expected, s))
}

/// Walk `$.a.b[0].c` (or `a.b[0].c`) through a JSON value.
///
/// The dotted path becomes a JSON pointer (`/a/b/0/c`), and serde does the
/// walking; only the syntax conversion lives here.
pub fn lookup<'v>(value: &'v Value, path: &str) -> Option<&'v Value> {
    let mut pointer = String::new();
    for segment in strip_root(path).split('.').filter(|s| !s.is_empty()) {
        let (name, indexes) = segment.split_once('[').unwrap_or((segment, ""));
        if !name.is_empty() {
            pointer.push('/');
            pointer.push_str(&name.replace('~', "~0").replace('/', "~1"));
        }
        for index in indexes.split(']').filter(|s| !s.is_empty()) {
            pointer.push('/');
            pointer.push_str(index.trim_start_matches('['));
        }
    }
    value.pointer(&pointer)
}

/// `*` matches any run of characters, including none; everything else is literal.
pub fn glob(pattern: &str, text: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == text;
    }
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let mut parts: Vec<&str> = parts.collect();
    let last = parts.pop().unwrap_or("");
    for part in parts.into_iter().filter(|p| !p.is_empty()) {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::WatchCondition;
    use serde_json::json;

    fn watch() -> Watch {
        Watch::blank("dev")
    }

    fn condition(path: &str, op: WatchOp, value: &str) -> WatchCondition {
        WatchCondition {
            path: path.into(),
            op,
            value: value.into(),
        }
    }

    #[test]
    fn compiles_every_field_into_one_pattern() {
        let mut w = watch();
        w.source = Some("client-profile*".into());
        w.detail_type = Some("*sync".into());
        w.conditions = vec![
            condition("detail.clientId", WatchOp::Eq, "abc-123"),
            condition("$.detail.count", WatchOp::Ne, "5"),
            condition("detail.code", WatchOp::Eq, "\"007\""),
        ];
        assert_eq!(
            compile(&w).unwrap().unwrap(),
            r#"{ $.source = "client-profile*" && $.detail-type = "*sync" && $.detail.clientId = "abc-123" && $.detail.count != 5 && $.detail.code = "007" }"#
        );
    }

    #[test]
    fn an_empty_watch_matches_everything() {
        assert_eq!(compile(&watch()).unwrap(), None);
        assert!(matches(&watch(), &json!({"source": "x"})));
    }

    #[test]
    fn a_raw_pattern_wins() {
        let mut w = watch();
        w.source = Some("ignored".into());
        w.raw_pattern = Some(" { $.detail.x = 1 } ".into());
        assert_eq!(compile(&w).unwrap().unwrap(), "{ $.detail.x = 1 }");
    }

    #[test]
    fn groups_or_together_and_go_unfiltered_when_one_member_is_open() {
        let mut a = watch();
        a.source = Some("a".into());
        let mut b = watch();
        b.detail_type = Some("b".into());
        assert_eq!(
            compile_group(&[&a, &b]).unwrap().unwrap(),
            r#"{ ($.source = "a") || ($.detail-type = "b") }"#
        );
        assert_eq!(
            compile_group(&[&a]).unwrap().unwrap(),
            r#"{ $.source = "a" }"#
        );
        assert_eq!(compile_group(&[&a, &watch()]).unwrap(), None);
    }

    #[test]
    fn rejects_paths_cloudwatch_would_reject() {
        let mut w = watch();
        w.conditions = vec![condition("detail.\"odd\"", WatchOp::Eq, "1")];
        assert!(compile(&w).is_err());
        w.conditions = vec![condition("   ", WatchOp::Eq, "1")];
        assert_eq!(compile(&w).unwrap(), None);
    }

    #[test]
    fn local_matching_mirrors_the_pattern() {
        let mut w = watch();
        w.source = Some("client-*".into());
        w.detail_type = Some("*sync".into());
        w.conditions = vec![
            condition("detail.clientId", WatchOp::Eq, "abc-*"),
            condition("detail.count", WatchOp::Eq, "5"),
            condition("detail.items[0].id", WatchOp::Ne, "zzz"),
            condition("detail.flag", WatchOp::Eq, "true"),
        ];
        let event = json!({
            "source": "client-profile",
            "detail-type": "profile-sync",
            "detail": {"clientId": "abc-123", "count": 5, "items": [{"id": "one"}], "flag": true}
        });
        assert!(matches(&w, &event));

        let mut other = event.clone();
        other["detail"]["count"] = json!("5");
        assert!(
            !matches(&w, &other),
            "an unquoted 5 is numeric, so a string does not match"
        );

        let mut missing = event.clone();
        missing["detail"].as_object_mut().unwrap().remove("items");
        assert!(
            !matches(&w, &missing),
            "!= against an absent field is false, as in CloudWatch"
        );
    }

    #[test]
    fn a_quoted_value_forces_a_string_match() {
        let mut w = watch();
        w.conditions = vec![condition("detail.code", WatchOp::Eq, "\"007\"")];
        assert!(matches(&w, &json!({"detail": {"code": "007"}})));
        assert!(!matches(&w, &json!({"detail": {"code": 7}})));
    }

    #[test]
    fn globbing() {
        assert!(glob("orders*", "orders-fulfilment"));
        assert!(glob("*assigned", "lead-assigned"));
        assert!(glob("*Notification*", "aNotificationB"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exactly"));
        assert!(glob("*", ""));
    }

    #[test]
    fn lookup_walks_arrays() {
        let v = json!({"detail": {"items": [{"id": "x"}, {"id": "y"}]}});
        assert_eq!(lookup(&v, "$.detail.items[1].id"), Some(&json!("y")));
        assert_eq!(lookup(&v, "detail.items[5].id"), None);
        assert_eq!(lookup(&v, "detail.nope"), None);
    }
}
