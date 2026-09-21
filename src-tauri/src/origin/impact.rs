//! Severity by who is affected, not only by what the validator says.
//!
//! The validator grades one thing: whether the registered schema rejects the
//! events. That is the right floor — a rejected event reaches nobody — but
//! above it every disagreement looked the same, and a missing field three
//! handlers dereference is not the same problem as one nothing reads.
//!
//! The origin lookup finds the consumer files, and [`super::reads`] finds the
//! fields each one reads off the event. Given both, an issue about a field
//! somebody reads is an error, and one about a field nobody found reads is a
//! note. Without them — no lookup run yet, or no handler found — severity is
//! left as the validator graded it, and says so by carrying no impact at all.
//! The rule itself lives with the validator's, in
//! [`crate::schema::events::severity_for`].

use super::reads::{relation, segments, Relation};
use super::{EventOrigin, ProducerOrigin};
use crate::schema::events::{rank, severity_for, Issue, IssueKind};
use serde::{Deserialize, Serialize};

/// A consumer file, as much of it as an issue needs to name it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reader {
    pub repo: String,
    pub path: String,
    pub url: String,
    pub owners: Vec<String>,
}

impl From<&ProducerOrigin> for Reader {
    fn from(file: &ProducerOrigin) -> Self {
        Reader {
            repo: file.repo.clone(),
            path: file.path.clone(),
            url: file.url.clone(),
            owners: file.owners.clone(),
        }
    }
}

/// What the consumers found say about one issue.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Impact {
    /// Consumer files found reading fields off this event at all — the
    /// denominator, so "nobody reads it" can be read as "none of these".
    pub handlers: usize,
    /// The ones that read this field, or something inside it.
    pub readers: Vec<Reader>,
    /// The ones that take an ancestor of the field along whole — `payload`
    /// handed to another module — and so may read it where this cannot
    /// see. Enough to withhold "nobody reads it"; not enough to say who does.
    #[serde(default)]
    pub indirect: Vec<Reader>,
}

/// Re-grade `issues` by the consumers in `origin`, and rank them again.
///
/// Idempotent: severity is recomputed from the issue and the lookup, so a
/// list graded against an older lookup gets the answer the new one gives.
pub fn grade(issues: &mut [Issue], origin: &EventOrigin) {
    // Which files count was decided when they were examined: a file has
    // reads only if it is one whose reads say something about breakage.
    let handlers: Vec<(&ProducerOrigin, Vec<Vec<String>>)> = origin
        .producers
        .iter()
        .filter(|p| !p.reads.is_empty())
        .map(|p| (p, p.reads.iter().map(|r| segments(r)).collect()))
        .collect();

    for issue in issues.iter_mut() {
        // A problem with the event as a whole, or a person's concern, is
        // not about a field anyone reads.
        let about_a_field = !issue.path.is_empty()
            && !matches!(issue.kind, IssueKind::Unregistered | IssueKind::Concern);
        issue.impact = (about_a_field && !handlers.is_empty()).then(|| {
            let path = segments(&issue.path);
            let mut readers = Vec::new();
            let mut indirect = Vec::new();
            for (file, reads) in &handlers {
                let closest = reads.iter().map(|read| relation(read, &path)).fold(
                    Relation::Unrelated,
                    |best, r| match (best, r) {
                        (Relation::Reads, _) | (_, Relation::Reads) => Relation::Reads,
                        (Relation::PassesAlong, _) | (_, Relation::PassesAlong) => {
                            Relation::PassesAlong
                        }
                        _ => Relation::Unrelated,
                    },
                );
                match closest {
                    Relation::Reads => readers.push(Reader::from(*file)),
                    Relation::PassesAlong => indirect.push(Reader::from(*file)),
                    Relation::Unrelated => {}
                }
            }
            Impact {
                handlers: handlers.len(),
                readers,
                indirect,
            }
        });
        issue.severity = severity_for(issue.kind, issue.rejects, issue.impact.as_ref());
    }
    rank(issues);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::{GithubOutcome, Role};
    use crate::schema::events::{check_events, IssueSeverity};
    use serde_json::json;

    fn file(path: &str, reads: &[&str]) -> ProducerOrigin {
        ProducerOrigin {
            repo: "org/consumer".into(),
            path: path.into(),
            url: format!("https://github.com/org/consumer/blob/main/{path}"),
            introduced: None,
            owners: vec!["@org/team".into()],
            incidental: false,
            role: Role::Consumer,
            reads: reads.iter().map(|r| r.to_string()).collect(),
        }
    }

    fn origin(producers: Vec<ProducerOrigin>) -> EventOrigin {
        EventOrigin {
            source: "s".into(),
            detail_type: "t".into(),
            wiring: None,
            wiring_repo_url: None,
            producers,
            github: GithubOutcome::failed("ok", None, String::new()),
            cached_at: 0,
            version: super::super::ORIGIN_VERSION,
        }
    }

    fn issues() -> Vec<Issue> {
        let doc = json!({ "components": { "schemas": { "T": {
            "type": "object",
            "required": ["matterId"],
            "properties": {
                "matterId": { "type": "string" },
                "count": { "type": "integer" },
                "note": { "type": "string" }
            }
        }}}});
        let events = vec![
            json!({ "matterId": "m", "count": "1", "extra": true }),
            json!({ "matterId": "m", "count": 2, "extra": true }),
        ];
        check_events(&doc, "T", &events).unwrap().issues
    }

    fn find<'a>(issues: &'a [Issue], key: &str) -> &'a Issue {
        issues.iter().find(|i| i.key == key).unwrap()
    }

    fn severity_of(issues: &[Issue], key: &str) -> IssueSeverity {
        find(issues, key).severity
    }

    #[test]
    fn without_a_handler_the_validator_grade_stands_and_no_impact_is_claimed() {
        let mut list = issues();
        grade(&mut list, &origin(vec![file("rule.yml", &[])]));
        assert!(list.iter().all(|i| i.impact.is_none()));
        assert_eq!(severity_of(&list, "wrongType:count"), IssueSeverity::Error);
        assert_eq!(
            severity_of(&list, "undeclared:extra"),
            IssueSeverity::Warning
        );
        assert_eq!(severity_of(&list, "neverSeen:note"), IssueSeverity::Info);
    }

    #[test]
    fn a_field_somebody_reads_is_an_error_and_one_nobody_reads_is_a_note() {
        let mut list = issues();
        grade(
            &mut list,
            &origin(vec![
                file("src/handler.ts", &["matterId", "extra"]),
                file("src/other.ts", &["matterId"]),
            ]),
        );
        // A rejected field somebody reads is an error on both counts.
        let mut read = issues();
        grade(&mut read, &origin(vec![file("h.ts", &["count"])]));
        assert_eq!(severity_of(&read, "wrongType:count"), IssueSeverity::Error);
        let extra = find(&list, "undeclared:extra");
        assert_eq!(extra.severity, IssueSeverity::Error);
        let impact = extra.impact.as_ref().unwrap();
        assert_eq!(impact.handlers, 2);
        assert_eq!(impact.readers.len(), 1);
        assert_eq!(impact.readers[0].path, "src/handler.ts");

        let note = find(&list, "neverSeen:note");
        assert_eq!(note.severity, IssueSeverity::Info);
        assert!(note.impact.as_ref().unwrap().readers.is_empty());

        // A rejection nobody found depends on is a broken contract, not a
        // broken consumer: one step down, never a note.
        assert_eq!(
            severity_of(&list, "wrongType:count"),
            IssueSeverity::Warning
        );
        // Errors first, so the ranking follows the new grade.
        assert_eq!(list[0].severity, IssueSeverity::Error);
    }

    #[test]
    fn a_field_passed_along_whole_is_neither_promoted_nor_demoted() {
        let doc = json!({ "components": { "schemas": { "T": {
            "type": "object",
            "properties": { "payload": { "type": "object", "properties": {
                "matterId": { "type": "string" }
            }}}
        }}}});
        let events = vec![json!({ "payload": { "matterId": 1, "extra": true } })];
        let mut list = check_events(&doc, "T", &events).unwrap().issues;
        // The handler unwraps `payload` and hands it to another module.
        grade(&mut list, &origin(vec![file("h.ts", &["payload"])]));
        let extra = find(&list, "undeclared:payload.extra");
        assert_eq!(extra.severity, IssueSeverity::Warning);
        let impact = extra.impact.as_ref().unwrap();
        assert!(impact.readers.is_empty());
        assert_eq!(impact.indirect.len(), 1);
        assert_eq!(impact.indirect[0].path, "h.ts");

        // A read of a field inside it is a read of it, and settles the
        // others: nothing touches `extra` any more.
        grade(
            &mut list,
            &origin(vec![file("h.ts", &["payload.matterId"])]),
        );
        assert_eq!(
            severity_of(&list, "undeclared:payload.extra"),
            IssueSeverity::Info
        );
        let wrong = find(&list, "wrongType:payload.matterId");
        assert_eq!(wrong.impact.as_ref().unwrap().readers.len(), 1);
    }

    #[test]
    fn a_declared_field_never_sent_but_read_is_worth_a_warning() {
        let mut list = issues();
        grade(&mut list, &origin(vec![file("h.py", &["note"])]));
        assert_eq!(severity_of(&list, "neverSeen:note"), IssueSeverity::Warning);
    }

    #[test]
    fn grading_twice_against_different_lookups_does_not_compound() {
        let mut list = issues();
        grade(&mut list, &origin(vec![file("h.ts", &["extra"])]));
        assert_eq!(severity_of(&list, "undeclared:extra"), IssueSeverity::Error);
        grade(&mut list, &origin(vec![file("h.ts", &["matterId"])]));
        assert_eq!(severity_of(&list, "undeclared:extra"), IssueSeverity::Info);
        grade(&mut list, &origin(vec![]));
        assert_eq!(
            severity_of(&list, "undeclared:extra"),
            IssueSeverity::Warning
        );
        assert!(list.iter().all(|i| i.impact.is_none()));
    }
}
