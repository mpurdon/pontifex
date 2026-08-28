//! Turning an analysis issue into a ticket.
//!
//! Pure: no network, no settings lookups beyond what is handed in. The
//! preview the user confirms and the ticket that gets created come from this
//! one function, so what you see is what is filed.
//!
//! Bodies are Jira wiki markup (REST v2) rather than Atlassian Document
//! Format (v3). ADF would mean assembling a document tree to say "here is a
//! paragraph and here is a code block"; v2 says it in a string.

use crate::error::{Error, Result};
use crate::jira::routing::{self, Routed};
use crate::schema::events::{Issue, IssueKind};
use crate::settings::JiraSettings;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};

/// Where an issue was found. Everything a reader needs to reproduce it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketContext {
    pub schema_name: String,
    /// Environment label, e.g. `prd`.
    pub environment: String,
    #[serde(default)]
    pub registry: Option<String>,
    pub source: String,
    pub detail_type: String,
    #[serde(default)]
    pub log_group: Option<String>,
    /// Sampling window in minutes.
    #[serde(default)]
    pub minutes: Option<u64>,
    /// The type the payloads were validated against.
    #[serde(default)]
    pub type_name: Option<String>,
}

/// A ticket, fully rendered, before anyone has agreed to create it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketDraft {
    pub project_key: String,
    pub issue_type: String,
    pub summary: String,
    pub description: String,
    pub labels: Vec<String>,
    /// Values for fields the target project makes mandatory.
    pub fields: std::collections::BTreeMap<String, Value>,
    /// Identifies this exact problem across runs, as a label.
    pub fingerprint: String,
    pub assignee_account_id: Option<String>,
    /// Why this project — shown in the preview so routing is auditable.
    pub routed_by: String,
}

/// Label prefix that marks a ticket as one of ours.
pub const TICKET_LABEL: &str = "pontifex";

/// Strip the code marks the summaries carry.
///
/// The backticks are there so the panel can render field names as code; a
/// Jira title is plain text and would show them literally.
fn plain(text: &str) -> String {
    text.replace('`', "")
}

/// Identity of a problem, stable across runs and across samples.
///
/// Environment is in the hash because the same drift in dev and prd are two
/// different conversations, and schema is in it because two schemas can have a
/// field of the same name.
pub fn fingerprint(environment: &str, schema_name: &str, issue_key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{environment}|{schema_name}|{issue_key}").as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("{TICKET_LABEL}-{}", &digest[..12])
}

fn kind_phrase(kind: IssueKind) -> &'static str {
    match kind {
        IssueKind::WrongType => "sends a field with the wrong type",
        IssueKind::OutsideEnum => "sends a value the schema does not allow",
        IssueKind::MissingRequired => "omits a required field",
        IssueKind::Undeclared => "sends a field the schema does not describe",
        IssueKind::NeverSeen => "never sends a field the schema declares",
        IssueKind::Rejected => "sends events the schema rejects",
    }
}

/// The sampling window in words, for anything a reader will see.
pub fn describe_window(minutes: Option<u64>) -> String {
    match minutes {
        Some(m) if m % 1440 == 0 => format!("the last {} day(s)", m / 1440),
        Some(m) if m % 60 == 0 => format!("the last {} hour(s)", m / 60),
        Some(m) => format!("the last {m} minute(s)"),
        None => "the sampled window".to_string(),
    }
}

/// A value as evidence, written the way JSON would write it.
///
/// Strings keep their quotes: an empty string rendered raw produced a `{code}`
/// block containing nothing, which reads as "pontifex failed to include the
/// example" rather than "the value is empty" — and an empty value is usually
/// the whole bug.
fn render_example(example: &Value) -> String {
    serde_json::to_string_pretty(example).unwrap_or_else(|_| example.to_string())
}

/// The ticket body, in Jira wiki markup.
pub fn render_description(issue: &Issue, context: &TicketContext) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "The producer of {} {} on the {} event bus.\n\n",
        context.source,
        kind_phrase(issue.kind),
        context.environment,
    ));

    out.push_str("h3. What is wrong\n");
    out.push_str(&format!("{}\n\n", plain(&issue.summary)));

    out.push_str("h3. What to do\n");
    out.push_str(&format!("{}\n\n", plain(&issue.action)));

    out.push_str("h3. Evidence\n");
    out.push_str(&format!(
        "* Observed in *{} of {} sampled events* over {}\n",
        issue.affected,
        issue.sampled,
        describe_window(context.minutes),
    ));
    out.push_str(&format!(
        "* Source: {{{{{}}}}} / detail-type {{{{{}}}}}\n",
        context.source, context.detail_type,
    ));
    if !issue.path.is_empty() {
        out.push_str(&format!("* Field: {{{{{}}}}}\n", issue.path));
    }
    if let Some(declared) = &issue.declared {
        out.push_str(&format!("* Schema declares: {{{{{declared}}}}}\n"));
    }
    if let Some(observed) = &issue.observed {
        out.push_str(&format!("* Events carry: {{{{{observed}}}}}\n"));
    }
    // The line that decides urgency for whoever picks this up.
    out.push_str(if issue.rejects {
        "* *These events are being rejected by schema validation today.*\n"
    } else {
        "* Not currently rejected — the schema is out of date, not blocking.\n"
    });
    out.push_str(&format!(
        "* Schema: {{{{{}}}}}{}\n",
        context.schema_name,
        context
            .type_name
            .as_ref()
            .map(|t| format!(", validated against {{{{{t}}}}}"))
            .unwrap_or_default(),
    ));
    if let Some(registry) = &context.registry {
        out.push_str(&format!("* Registry: {{{{{registry}}}}}\n"));
    }
    if let Some(log_group) = &context.log_group {
        out.push_str(&format!("* Sampled from: {{{{{log_group}}}}}\n"));
    }
    out.push('\n');

    if let Some(example) = &issue.example {
        out.push_str("h3. Example value\n");
        out.push_str(&format!("{{code}}\n{}\n{{code}}\n\n", render_example(example)));
    }

    if let Some(message) = &issue.message {
        out.push_str("h3. Validator\n");
        out.push_str(&format!("{{quote}}{}{{quote}}\n\n", message));
    }

    out.push_str("----\nFiled from pontifex, which sampled real events off the bus and compared them with the registered schema.\n");
    out
}

/// The one-line title: names the producer first, because that is who this is for.
pub fn render_summary(issue: &Issue, context: &TicketContext) -> String {
    format!("[{}] {}", context.source, plain(&issue.summary))
}

/// Build the ticket, or explain why it cannot be routed.
pub fn draft(issue: &Issue, context: &TicketContext, settings: &JiraSettings) -> Result<TicketDraft> {
    let Routed {
        project_key,
        issue_type,
        labels: route_labels,
        assignee_account_id,
        reason,
    } = routing::route_for(settings, &context.source).ok_or_else(|| {
        Error::Invalid(format!(
            "No Jira project is mapped to the source '{}'. Add a rule — or a default project — \
             in Settings → Jira.",
            context.source
        ))
    })?;

    let fingerprint = fingerprint(&context.environment, &context.schema_name, &issue.key);

    // Deduplicated and ordered: the fingerprint has to be present exactly once
    // for the dedupe search to be reliable.
    let mut labels = vec![TICKET_LABEL.to_string(), fingerprint.clone()];
    labels.push(format!("{TICKET_LABEL}-{}", context.environment));
    for label in settings.labels.iter().chain(route_labels.iter()) {
        let cleaned = label.trim().replace(' ', "-");
        if !cleaned.is_empty() && !labels.contains(&cleaned) {
            labels.push(cleaned);
        }
    }

    // Whatever this project demands, as configured. Missing entries surface as
    // Jira's own rejection, which names the field.
    let fields = settings
        .field_defaults
        .get(&project_key)
        .cloned()
        .unwrap_or_default();

    Ok(TicketDraft {
        project_key,
        issue_type,
        summary: render_summary(issue, context),
        description: render_description(issue, context),
        labels,
        fields,
        fingerprint,
        assignee_account_id,
        routed_by: reason,
    })
}

/// The create-issue payload for REST v2.
pub fn to_fields(draft: &TicketDraft) -> Value {
    let mut fields = serde_json::json!({
        "project": { "key": draft.project_key },
        "issuetype": { "name": draft.issue_type },
        "summary": draft.summary,
        "description": draft.description,
        "labels": draft.labels,
    });

    if let Some(account_id) = &draft.assignee_account_id {
        fields["assignee"] = serde_json::json!({ "accountId": account_id });
    }

    // Last, so a project's own required fields cannot be shadowed by the ones
    // above — and can deliberately override them.
    for (id, value) in &draft.fields {
        fields[id.as_str()] = value.clone();
    }

    serde_json::json!({ "fields": fields })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::events::IssueSeverity;
    use crate::settings::SourceRoute;
    use serde_json::json;

    fn issue() -> Issue {
        Issue {
            key: "wrongType:callAttemptCount".into(),
            kind: IssueKind::WrongType,
            severity: IssueSeverity::Error,
            path: "callAttemptCount".into(),
            summary: "`callAttemptCount` is declared integer but 4% of events send string".into(),
            action: "Fix the producer, or redeclare `callAttemptCount` as string.".into(),
            declared: Some("integer".into()),
            observed: Some("string".into()),
            affected: 7,
            sampled: 200,
            rejects: true,
            example: Some(json!("14")),
            message: Some("\"14\" is not of type \"integer\"".into()),
            fix: Some(crate::schema::repair::Repair::WidenType {
                types: vec!["integer".into(), "string".into()],
            }),
        }
    }

    fn context() -> TicketContext {
        TicketContext {
            schema_name: "orders-fulfilment@lead-unreached".into(),
            environment: "prd".into(),
            registry: Some("prd-global-registry".into()),
            source: "orders-fulfilment".into(),
            detail_type: "lead-unreached".into(),
            log_group: Some("/aws/events/prd-global-events".into()),
            minutes: Some(1440),
            type_name: Some("LeadUnreached".into()),
        }
    }

    fn settings() -> JiraSettings {
        JiraSettings {
            routes: vec![SourceRoute {
                id: "1".into(),
                pattern: "orders-*".into(),
                project_key: "IPP".into(),
                issue_type: None,
                labels: vec!["producer-bug".into()],
                assignee_account_id: None,
            }],
            ..JiraSettings::default()
        }
    }

    #[test]
    fn the_title_names_the_producer_and_drops_the_code_marks() {
        let summary = render_summary(&issue(), &context());
        assert_eq!(
            summary,
            "[orders-fulfilment] callAttemptCount is declared integer but 4% of events send string",
        );
        assert!(!summary.contains('`'));
    }

    #[test]
    fn the_body_carries_what_a_producer_team_needs_to_act() {
        let body = render_description(&issue(), &context());
        assert!(body.contains("7 of 200 sampled events"), "{body}");
        assert!(body.contains("the last 1 day(s)"), "{body}");
        assert!(body.contains("orders-fulfilment"), "{body}");
        assert!(body.contains("lead-unreached"), "{body}");
        assert!(body.contains("/aws/events/prd-global-events"), "{body}");
        assert!(body.contains("LeadUnreached"), "{body}");
        // The example is a code block, not a sentence — and a string keeps its
        // quotes, so the reader can tell "14" from 14.
        assert!(body.contains("{code}\n\"14\"\n{code}"), "{body}");
        // The validator's own words survive.
        assert!(body.contains("is not of type"), "{body}");
    }

    #[test]
    fn an_empty_string_example_is_visible_rather_than_an_empty_code_block() {
        let empty = Issue {
            example: Some(json!("")),
            ..issue()
        };
        let body = render_description(&empty, &context());
        assert!(body.contains("{code}\n\"\"\n{code}"), "{body}");
    }

    #[test]
    fn the_body_says_whether_events_are_being_thrown_away() {
        let rejecting = render_description(&issue(), &context());
        assert!(rejecting.contains("being rejected by schema validation today"));

        let drifting = Issue {
            rejects: false,
            ..issue()
        };
        let body = render_description(&drifting, &context());
        assert!(body.contains("not blocking"), "{body}");
    }

    #[test]
    fn routes_to_the_owning_project_and_labels_it_for_dedupe() {
        let draft = draft(&issue(), &context(), &settings()).unwrap();
        assert_eq!(draft.project_key, "IPP");
        assert_eq!(draft.issue_type, "Bug");
        assert!(draft.labels.contains(&"pontifex".to_string()));
        assert!(draft.labels.contains(&"pontifex-prd".to_string()));
        assert!(draft.labels.contains(&"producer-bug".to_string()));
        // `<label>-<12 hex chars>` — the fingerprint label.
        let fingerprint_len = TICKET_LABEL.len() + 1 + 12;
        assert!(draft
            .labels
            .iter()
            .any(|l| l.starts_with("pontifex-") && l.len() == fingerprint_len));
        assert!(draft.routed_by.contains("orders-*"), "{}", draft.routed_by);
    }

    #[test]
    fn refuses_to_guess_when_nothing_routes_the_source() {
        let unrouted = JiraSettings::default();
        let error = draft(&issue(), &context(), &unrouted).unwrap_err();
        assert!(error.to_string().contains("orders-fulfilment"), "{error}");
        assert!(error.to_string().contains("Settings → Jira"), "{error}");
    }

    #[test]
    fn the_same_problem_fingerprints_the_same_and_a_different_one_does_not() {
        let a = fingerprint("prd", "schema-a", "wrongType:count");
        assert_eq!(a, fingerprint("prd", "schema-a", "wrongType:count"));
        // Environment and schema both participate: the same drift in dev is a
        // different ticket.
        assert_ne!(a, fingerprint("dev", "schema-a", "wrongType:count"));
        assert_ne!(a, fingerprint("prd", "schema-b", "wrongType:count"));
        assert_ne!(a, fingerprint("prd", "schema-a", "undeclared:count"));
    }

    #[test]
    fn labels_with_spaces_cannot_reach_jira_as_two_labels() {
        let settings = JiraSettings {
            labels: vec!["event bus".into()],
            default_project: Some("TRIAGE".into()),
            ..JiraSettings::default()
        };
        let draft = draft(&issue(), &context(), &settings).unwrap();
        assert!(draft.labels.contains(&"event-bus".to_string()));
    }

    #[test]
    fn the_create_payload_is_shaped_the_way_rest_v2_wants_it() {
        let draft = draft(&issue(), &context(), &settings()).unwrap();
        let fields = to_fields(&draft);
        assert_eq!(fields["fields"]["project"]["key"], "IPP");
        assert_eq!(fields["fields"]["issuetype"]["name"], "Bug");
        assert!(fields["fields"]["summary"].as_str().unwrap().starts_with("[orders-fulfilment]"));
        assert!(fields["fields"]["assignee"].is_null());
    }
}
