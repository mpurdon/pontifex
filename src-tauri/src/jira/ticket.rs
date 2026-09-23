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
    /// Who publishes it, when the origin lookup has run for this type.
    #[serde(default)]
    pub origin: Option<TicketOrigin>,
}

/// The publisher, as the origin lookup found it, for the ticket body and
/// for routing by owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketOrigin {
    pub repo: String,
    pub path: String,
    pub url: String,
    #[serde(default)]
    pub owners: Vec<String>,
    #[serde(default)]
    pub introduced_by: Option<String>,
    #[serde(default)]
    pub pull_url: Option<String>,
    /// True when the file puts the event on the bus, as opposed to merely
    /// naming it — which is all the lookup could find for some types.
    #[serde(default)]
    pub publisher: bool,
}

impl TicketOrigin {
    /// The most telling file the lookup found: a publisher, else the first
    /// non-incidental match, else nothing worth putting in a ticket.
    pub fn from_origin(origin: &crate::origin::EventOrigin) -> Option<Self> {
        use crate::origin::Role;
        let best = origin
            .producers
            .iter()
            .filter(|p| !p.incidental)
            .max_by_key(|p| p.role)?;
        let introduced = best.introduced.as_ref();
        Some(TicketOrigin {
            repo: best.repo.clone(),
            path: best.path.clone(),
            url: best.url.clone(),
            owners: best.owners.clone(),
            introduced_by: introduced.map(|a| {
                a.pull_author
                    .clone()
                    .or_else(|| a.login.clone())
                    .unwrap_or_else(|| a.author.clone())
            }),
            pull_url: introduced.and_then(|a| a.pull_url.clone()),
            publisher: best.role == Role::Publisher,
        })
    }
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
    ///
    /// Carried only to find tickets filed before the labels were readable;
    /// nothing is filed with it any more. See [`identifying_labels`].
    pub fingerprint: String,
    /// The labels that identify what this ticket is about, all of which an
    /// open ticket must carry to count as the same problem.
    pub identity: Vec<String>,
    pub assignee_account_id: Option<String>,
    /// Why this project — shown in the preview so routing is auditable.
    pub routed_by: String,
}

/// Label prefix that marks a ticket as one of ours.
pub const TICKET_LABEL: &str = "pontifex";

/// A label Jira will accept, and a person can read.
///
/// Jira allows up to 255 characters and no spaces — a space would be read as
/// the separator between two labels, so `Atomic Forms` would arrive as
/// `Atomic` and `Forms`. Brackets go too: an array path reads better as
/// `callAttemptHistory.result` than `callAttemptHistory[].result`, and the
/// `[]` carries nothing a label needs to say.
pub(crate) fn label(text: &str) -> String {
    let cleaned: String = text
        .trim()
        .replace("[]", "")
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .collect();
    cleaned.chars().take(255).collect()
}

/// The bus these events were read from, named the way the rest of the app
/// names it — stage included, because the same event type on dev and on prd
/// are two different conversations.
fn bus_name(context: &TicketContext) -> String {
    bus_label(&context.environment)
}

/// The bus label for an environment, for a search that spans schemas rather
/// than asking about one.
pub fn bus_label(environment: &str) -> String {
    label(&format!("{environment}-global-bus"))
}

/// The labels every ticket about this event type carries, whatever finding it
/// is about — so "what has been filed about this" is one search.
pub fn event_labels(context: &TicketContext) -> Vec<String> {
    identifying_labels(&[], context)
}

/// The labels that say what a ticket is about, worst-to-least specific.
///
/// These are the ticket's identity as well as its description: the duplicate
/// check searches for an open ticket carrying all of them, so a reader and a
/// JQL query agree on what "the same problem" means. A ticket about one
/// finding names the field; a roll-up covering several names all of their
/// fields, and is found by any one of them.
fn identifying_labels(findings: &[Issue], context: &TicketContext) -> Vec<String> {
    let mut labels = vec![
        TICKET_LABEL.to_string(),
        label(&bus_name(context)),
        label(&context.source),
    ];
    if !context.detail_type.is_empty() {
        labels.push(label(&context.detail_type));
    }
    for finding in findings {
        // The payload as a whole is not a field, and an issue about it is
        // already named by the three labels above.
        if finding.path.is_empty() {
            continue;
        }
        let field = label(&finding.path);
        if !labels.contains(&field) {
            labels.push(field);
        }
    }
    labels
}

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
        IssueKind::EmptyRequired => "sends a required field blank",
        IssueKind::Undeclared => "sends a field the schema does not describe",
        IssueKind::NeverSeen => "never sends a field the schema declares",
        IssueKind::Rejected => "sends events the schema rejects",
        IssueKind::Unregistered => "publishes an event type with no registered schema",
        IssueKind::Concern => "publishes an event type a reviewer has asked to change",
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

/// A file in a repository, as a Jira wiki link.
fn wiki_link(repo: &str, path: &str, url: &str) -> String {
    format!("[{repo}/{path}|{url}]")
}

/// The evidence bullets that are about one finding: how much traffic it
/// affects, which field, what the two sides say, and who downstream cares.
///
/// Separate from [`where_found`] because a roll-up ticket says where it was
/// found once and then repeats these per finding.
fn finding_evidence(issue: &Issue, context: &TicketContext, out: &mut String) {
    // A concern is a person's words, not a validator's finding: there is no
    // count of affected events and no verdict from the schema.
    let concern = issue.kind == IssueKind::Concern;
    if !concern {
        out.push_str(&format!(
            "* Observed in *{} of {} sampled events* over {}\n",
            issue.affected,
            issue.sampled,
            describe_window(context.minutes),
        ));
    }
    if !issue.path.is_empty() {
        out.push_str(&format!("* Field: {{{{{}}}}}\n", issue.path));
    }
    if let Some(declared) = &issue.declared {
        out.push_str(&format!("* Schema declares: {{{{{declared}}}}}\n"));
    }
    if let Some(observed) = &issue.observed {
        out.push_str(&format!("* Events carry: {{{{{observed}}}}}\n"));
    }
    // The lines that decide urgency for whoever picks this up: whether the
    // bus is throwing the events away, and who downstream reads the field.
    if concern {
        return;
    }
    out.push_str(if issue.rejects {
        "* *The registered schema rejects these events.* EventBridge still delivers them; the bus's schema validator alerts on failures rather than blocking.\n"
    } else {
        "* The schema does not reject these events — it is out of date, not broken.\n"
    });
    if let Some(impact) = &issue.impact {
        out.push_str(&match (impact.readers.len(), impact.indirect.len()) {
            (0, 0) => format!(
                "* None of the {} consumer file(s) found reads this field.\n",
                impact.handlers
            ),
            (0, n) => format!(
                "* {n} of the {} consumer file(s) found pass this field's parent along whole, so whether they read it is not visible — listed below.\n",
                impact.handlers
            ),
            (n, _) => format!(
                "* *Read by {n} of the {} consumer file(s) found — listed below.*\n",
                impact.handlers
            ),
        });
    }
}

/// Where the events were sampled from and what they were graded against —
/// true of every finding in the ticket, so written once however many there are.
fn where_found(context: &TicketContext, unregistered: bool, out: &mut String) {
    out.push_str(&format!(
        "* Source: {{{{{}}}}} / detail-type {{{{{}}}}}\n",
        context.source, context.detail_type,
    ));
    if unregistered {
        out.push_str(&format!(
            "* Schema: none registered — it would be named {{{{{}}}}}\n",
            context.schema_name
        ));
    } else {
        out.push_str(&format!(
            "* Schema: {{{{{}}}}}{}\n",
            context.schema_name,
            context
                .type_name
                .as_ref()
                .map(|t| format!(", validated against {{{{{t}}}}}"))
                .unwrap_or_default(),
        ));
    }
    if let Some(registry) = &context.registry {
        out.push_str(&format!("* Registry: {{{{{registry}}}}}\n"));
    }
    if let Some(log_group) = &context.log_group {
        out.push_str(&format!("* Sampled from: {{{{{log_group}}}}}\n"));
    }
    out.push('\n');
}

/// Who publishes this event type, when the origin lookup found them.
fn publisher_block(context: &TicketContext, level: &str, out: &mut String) {
    let Some(origin) = &context.origin else {
        return;
    };
    out.push_str(&format!(
        "{level}. {}\n",
        if origin.publisher {
            "Publisher"
        } else {
            "Where it appears"
        }
    ));
    out.push_str(&format!(
        "* {}\n",
        wiki_link(&origin.repo, &origin.path, &origin.url)
    ));
    if !origin.owners.is_empty() {
        out.push_str(&format!("* Owned by {}\n", origin.owners.join(", ")));
    }
    if let Some(by) = &origin.introduced_by {
        match &origin.pull_url {
            Some(url) => out.push_str(&format!("* First published by {by} in [{url}]\n")),
            None => out.push_str(&format!("* First published by {by}\n")),
        }
    }
    out.push('\n');
}

/// The consumer files behind the impact line, the example value, and the
/// validator's own words — everything a finding carries below its evidence.
fn finding_detail(issue: &Issue, out: &mut String) {
    if let Some(impact) = &issue.impact {
        for (heading, files) in [
            ("h3. Consumers that read this field\n", &impact.readers),
            (
                "h3. Consumers that pass its parent along\n",
                &impact.indirect,
            ),
        ] {
            if files.is_empty() {
                continue;
            }
            out.push_str(heading);
            for file in files {
                out.push_str(&format!(
                    "* {}{}\n",
                    wiki_link(&file.repo, &file.path, &file.url),
                    if file.owners.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", file.owners.join(", "))
                    }
                ));
            }
            out.push('\n');
        }
    }

    if let Some(example) = &issue.example {
        out.push_str("h3. Example value\n");
        out.push_str(&format!(
            "{{code}}\n{}\n{{code}}\n\n",
            render_example(example)
        ));
    }

    if let Some(message) = &issue.message {
        out.push_str("h3. Validator\n");
        out.push_str(&format!("{{quote}}{}{{quote}}\n\n", message));
    }
}

/// How a ticket signs off, which depends on who found the problem.
fn footer(findings: &[Issue]) -> &'static str {
    if findings.iter().all(|i| i.kind == IssueKind::Concern) {
        "----\nFiled from Pontifex by someone reviewing this schema against real events on the bus.\n"
    } else {
        "----\nFiled from Pontifex, which sampled real events off the bus and compared them with the registered schema.\n"
    }
}

/// The ticket body, in Jira wiki markup.
///
/// One finding or several. A ticket about several says the shared part once —
/// who publishes this event type, which schema, which sample — and then each
/// finding in full, in the sections it would have had as a ticket of its own.
/// That is the difference between one ticket a producer team can act on and
/// eight tickets that arrive together and repeat each other.
pub fn render_description(findings: &[Issue], context: &TicketContext) -> String {
    match findings {
        [] => String::new(),
        [single] => single_description(single, context),
        many => rollup_description(many, context),
    }
}

fn single_description(issue: &Issue, context: &TicketContext) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "The producer of {} {} on the {} event bus.\n\n",
        context.source,
        kind_phrase(issue.kind),
        context.environment,
    ));

    out.push_str("h3. What is wrong\n");
    out.push_str(&format!("{}\n\n", plain(&issue.summary)));

    out.push_str(if issue.kind == IssueKind::Concern {
        "h3. Details\n"
    } else {
        "h3. What to do\n"
    });
    out.push_str(&format!("{}\n\n", plain(&issue.action)));

    out.push_str("h3. Evidence\n");
    finding_evidence(issue, context, &mut out);
    where_found(context, issue.kind == IssueKind::Unregistered, &mut out);

    publisher_block(context, "h3", &mut out);
    finding_detail(issue, &mut out);
    out.push_str(footer(std::slice::from_ref(issue)));
    out
}

/// Every finding about one event type, in one ticket.
fn rollup_description(findings: &[Issue], context: &TicketContext) -> String {
    let mut out = String::new();
    let rejecting = findings.iter().filter(|i| i.rejects).count();

    out.push_str(&format!(
        "The producer of {} publishes {} events that disagree with the registered schema in {} ways on the {} event bus.{}\n\n",
        context.source,
        context.detail_type,
        findings.len(),
        context.environment,
        match rejecting {
            0 => String::new(),
            n => format!(
                " *{n} of them {} events the bus's validator rejects.*",
                if n == 1 { "produces" } else { "produce" }
            ),
        },
    ));

    // Numbered, in the order the panel ranked them, so the list doubles as a
    // table of contents for the sections below.
    out.push_str("h2. What is wrong\n");
    for issue in findings {
        out.push_str(&format!(
            "# {}{}\n",
            plain(&issue.summary),
            if issue.kind == IssueKind::Concern {
                String::new()
            } else {
                format!(
                    " — {} of {} events{}",
                    issue.affected,
                    issue.sampled,
                    if issue.rejects { ", rejected" } else { "" }
                )
            },
        ));
    }
    out.push('\n');

    out.push_str("h2. Where this was found\n");
    where_found(
        context,
        findings.iter().any(|i| i.kind == IssueKind::Unregistered),
        &mut out,
    );
    publisher_block(context, "h2", &mut out);

    // Each finding in full, so this ticket says everything the eight separate
    // ones would have said.
    for (position, issue) in findings.iter().enumerate() {
        out.push_str(&format!(
            "h2. {} of {}: {}\n\n",
            position + 1,
            findings.len(),
            plain(&issue.summary),
        ));
        out.push_str(if issue.kind == IssueKind::Concern {
            "h3. Details\n"
        } else {
            "h3. What to do\n"
        });
        out.push_str(&format!("{}\n\n", plain(&issue.action)));
        out.push_str("h3. Evidence\n");
        finding_evidence(issue, context, &mut out);
        out.push('\n');
        finding_detail(issue, &mut out);
    }

    out.push_str(footer(findings));
    out
}

/// The one-line title: names the producer first, because that is who this is for.
///
/// A roll-up cannot lead with one finding's summary without misrepresenting
/// the other seven, so it counts them and names the event type instead.
pub fn render_summary(findings: &[Issue], context: &TicketContext) -> String {
    match findings {
        [single] => format!("[{}] {}", context.source, plain(&single.summary)),
        many => {
            let rejecting = many.iter().filter(|i| i.rejects).count();
            format!(
                "[{}] {} schema findings on {}{}",
                context.source,
                many.len(),
                if context.detail_type.is_empty() {
                    context.schema_name.as_str()
                } else {
                    context.detail_type.as_str()
                },
                match rejecting {
                    0 => String::new(),
                    // Grammatical either way: one finding causes rejections,
                    // three of them cause rejections.
                    n => format!(" ({n} {} rejections)", if n == 1 { "causes" } else { "cause" }),
                },
            )
        }
    }
}

/// The issue-key component a roll-up fingerprints under.
///
/// Constant, so every roll-up filed for the same event type in the same
/// environment is the same ticket: findings come and go between samples, and
/// "the state of this contract" is one conversation, not a new ticket each
/// time the list changes. No [`Issue::key`] can collide with it — they are all
/// either `kind:path` or the bare word `unregistered`.
const ROLLUP_KEY: &str = "rollup";

/// Build the ticket, or explain why it cannot be routed.
///
/// `findings` is everything the ticket is about: one entry for a row's own
/// File button, several for the roll-up the Analysis tab files when an event
/// type has more than one thing wrong with it.
pub fn draft(
    findings: &[Issue],
    context: &TicketContext,
    settings: &JiraSettings,
) -> Result<TicketDraft> {
    let [first, rest @ ..] = findings else {
        return Err(Error::Invalid(
            "A ticket needs at least one finding to be about.".into(),
        ));
    };
    let Routed {
        project_key,
        issue_type,
        labels: route_labels,
        assignee_account_id,
        reason,
    } = routing::route_for(settings, &context.source, context.origin.as_ref()).ok_or_else(
        || {
            Error::Invalid(format!(
            "No Jira project is mapped to the source '{}'. Add a rule — or a default project — \
             in Settings → Jira.",
            context.source
        ))
        },
    )?;

    let key = if rest.is_empty() {
        first.key.as_str()
    } else {
        ROLLUP_KEY
    };

    let identity = identifying_labels(findings, context);
    let mut labels = identity.clone();
    // Whatever the routing rule and the settings add on top — deduplicated,
    // because a label repeated is a label Jira will reject or fold.
    for extra in settings.labels.iter().chain(route_labels.iter()) {
        let cleaned = label(extra);
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
        summary: render_summary(findings, context),
        description: render_description(findings, context),
        labels,
        fields,
        fingerprint: fingerprint(&context.environment, &context.schema_name, key),
        identity,
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
            impact: None,
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
            origin: None,
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
        let summary = render_summary(&[issue()], &context());
        assert_eq!(
            summary,
            "[orders-fulfilment] callAttemptCount is declared integer but 4% of events send string",
        );
        assert!(!summary.contains('`'));
    }

    #[test]
    fn the_body_carries_what_a_producer_team_needs_to_act() {
        let body = render_description(&[issue()], &context());
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
        let body = render_description(std::slice::from_ref(&empty), &context());
        assert!(body.contains("{code}\n\"\"\n{code}"), "{body}");
    }

    #[test]
    fn the_body_says_whether_events_are_being_thrown_away() {
        let rejecting = render_description(&[issue()], &context());
        assert!(rejecting.contains("The registered schema rejects these events"));
        assert!(rejecting.contains("still delivers"), "{rejecting}");

        let drifting = Issue {
            rejects: false,
            ..issue()
        };
        let body = render_description(std::slice::from_ref(&drifting), &context());
        assert!(body.contains("does not reject"), "{body}");
    }

    #[test]
    fn the_body_names_the_consumers_that_read_the_field() {
        use crate::origin::impact::{Impact, Reader};
        let read = Issue {
            impact: Some(Impact {
                handlers: 3,
                indirect: vec![],
                readers: vec![Reader {
                    repo: "org/billing".into(),
                    path: "src/handler.ts".into(),
                    url: "https://github.com/org/billing/blob/main/src/handler.ts".into(),
                    owners: vec!["@org/billing".into()],
                }],
            }),
            ..issue()
        };
        let body = render_description(std::slice::from_ref(&read), &context());
        assert!(
            body.contains("Read by 1 of the 3 consumer file(s)"),
            "{body}"
        );
        assert!(
            body.contains("h3. Consumers that read this field"),
            "{body}"
        );
        assert!(
            body.contains("[org/billing/src/handler.ts|https://github.com/org/billing/blob/main/src/handler.ts] — @org/billing"),
            "{body}"
        );

        let unread = Issue {
            impact: Some(Impact {
                handlers: 3,
                readers: vec![],
                indirect: vec![],
            }),
            ..issue()
        };
        let body = render_description(std::slice::from_ref(&unread), &context());
        assert!(
            body.contains("None of the 3 consumer file(s) found reads this field"),
            "{body}"
        );
        assert!(!body.contains("h3. Consumers"), "{body}");
    }

    /// A second, different finding about the same event type.
    fn blank_field() -> Issue {
        Issue {
            key: "emptyRequired:clientId".into(),
            kind: IssueKind::EmptyRequired,
            severity: IssueSeverity::Warning,
            path: "clientId".into(),
            summary: "`clientId` is required but blank in 31% of events".into(),
            action: "Require at least one character in `clientId`.".into(),
            declared: Some("required".into()),
            observed: Some("blank in 62/200".into()),
            affected: 62,
            sampled: 200,
            rejects: false,
            example: Some(json!("")),
            message: None,
            fix: None,
            impact: None,
        }
    }

    #[test]
    fn a_roll_up_carries_every_finding_in_full() {
        let body = render_description(&[issue(), blank_field()], &context());

        // The list up top, then a section per finding.
        assert!(body.contains("h2. What is wrong"), "{body}");
        assert!(body.contains("# callAttemptCount is declared integer"), "{body}");
        assert!(body.contains("# clientId is required but blank"), "{body}");
        assert!(body.contains("h2. 1 of 2:"), "{body}");
        assert!(body.contains("h2. 2 of 2:"), "{body}");

        // Everything the two separate tickets would have said about each.
        assert!(body.contains("Fix the producer, or redeclare"), "{body}");
        assert!(body.contains("Require at least one character"), "{body}");
        assert!(body.contains("7 of 200 sampled events"), "{body}");
        assert!(body.contains("62 of 200 sampled events"), "{body}");
        assert!(body.contains("{code}\n\"14\"\n{code}"), "{body}");
        assert!(body.contains("{code}\n\"\"\n{code}"), "{body}");
        assert!(body.contains("is not of type"), "{body}");
        assert!(
            body.contains("The registered schema rejects these events"),
            "{body}"
        );
        assert!(body.contains("does not reject these events"), "{body}");
    }

    #[test]
    fn a_roll_up_says_the_shared_part_once() {
        let body = render_description(&[issue(), blank_field()], &context());
        for shared in [
            "* Source: {{orders-fulfilment}} / detail-type {{lead-unreached}}",
            "* Schema: {{orders-fulfilment@lead-unreached}}, validated against {{LeadUnreached}}",
            "* Registry: {{prd-global-registry}}",
            "* Sampled from: {{/aws/events/prd-global-events}}",
        ] {
            assert_eq!(
                body.matches(shared).count(),
                1,
                "expected `{shared}` exactly once in:\n{body}"
            );
        }
        // And the count of findings, so the first line says what this is.
        assert!(body.contains("in 2 ways"), "{body}");
        assert!(body.contains("*1 of them produces events"), "{body}");
    }

    #[test]
    fn the_roll_up_title_counts_the_findings_rather_than_picking_one() {
        let summary = render_summary(&[issue(), blank_field()], &context());
        assert_eq!(
            summary,
            "[orders-fulfilment] 2 schema findings on lead-unreached (1 causes rejections)",
        );
        // One finding still speaks for itself.
        assert!(render_summary(&[issue()], &context()).contains("callAttemptCount"));
    }

    #[test]
    fn a_roll_up_names_every_field_it_covers_once() {
        // Each covered field is how a reader finds this ticket, and how the
        // duplicate check finds it when one of those fields is filed alone.
        let rollup = draft(&[issue(), blank_field()], &context(), &settings()).unwrap();
        assert_eq!(
            rollup.labels,
            vec![
                "pontifex",
                "prd-global-bus",
                "orders-fulfilment",
                "lead-unreached",
                "callAttemptCount",
                "clientId",
                "producer-bug",
            ],
        );

        // Two problems with the same field are one label, not two.
        let same_field = Issue {
            key: "emptyRequired:callAttemptCount".into(),
            kind: IssueKind::EmptyRequired,
            path: "callAttemptCount".into(),
            ..issue()
        };
        let repeated = draft(&[issue(), same_field], &context(), &settings()).unwrap();
        assert_eq!(
            repeated
                .labels
                .iter()
                .filter(|l| *l == "callAttemptCount")
                .count(),
            1,
            "{:?}",
            repeated.labels,
        );
    }

    #[test]
    fn a_label_is_something_jira_will_accept_and_a_person_can_read() {
        let awkward = TicketContext {
            // Registered as `Atomic-Forms`, but the events carry the space.
            source: "Atomic Forms".into(),
            ..context()
        };
        let nested = Issue {
            path: "callAttemptHistory[].result".into(),
            ..issue()
        };
        // The routing rule matches `orders-*`, so this source needs the
        // default project rather than a rule.
        let routed = JiraSettings {
            default_project: Some("TRIAGE".into()),
            ..settings()
        };
        let draft = draft(&[nested], &awkward, &routed).unwrap();

        // A space would arrive at Jira as two labels, and the array marker
        // says nothing a reader needs.
        assert!(draft.labels.contains(&"Atomic-Forms".to_string()), "{:?}", draft.labels);
        assert!(
            draft.labels.contains(&"callAttemptHistory.result".to_string()),
            "{:?}",
            draft.labels,
        );
        assert!(draft.labels.iter().all(|l| !l.contains(' ') && l.len() <= 255));
    }

    #[test]
    fn a_problem_with_no_field_is_labelled_by_its_event_type_alone() {
        let whole_payload = Issue {
            key: "unregistered".into(),
            kind: IssueKind::Unregistered,
            path: String::new(),
            ..issue()
        };
        let draft = draft(&[whole_payload], &context(), &settings()).unwrap();
        assert_eq!(
            draft.identity,
            vec![
                "pontifex",
                "prd-global-bus",
                "orders-fulfilment",
                "lead-unreached",
            ],
        );
        assert!(draft.labels.iter().all(|l| !l.is_empty()));
    }

    #[test]
    fn a_ticket_about_nothing_is_refused() {
        let error = draft(&[], &context(), &settings()).unwrap_err();
        assert!(error.to_string().contains("at least one finding"), "{error}");
    }

    #[test]
    fn routes_to_the_owning_project_and_says_what_the_ticket_is_about() {
        let draft = draft(&[issue()], &context(), &settings()).unwrap();
        assert_eq!(draft.project_key, "IPP");
        assert_eq!(draft.issue_type, "Bug");
        // The bus (stage included), the producer, the event type, the field —
        // and the rule's own label last.
        assert_eq!(
            draft.labels,
            vec![
                "pontifex",
                "prd-global-bus",
                "orders-fulfilment",
                "lead-unreached",
                "callAttemptCount",
                "producer-bug",
            ],
        );
        // Every one of them readable: no hashes reach a producer's backlog.
        assert!(
            !draft.labels.iter().any(|l| l.len() == TICKET_LABEL.len() + 13
                && l.trim_start_matches("pontifex-").chars().all(|c| c.is_ascii_hexdigit())),
            "{:?}",
            draft.labels,
        );
        assert!(draft.routed_by.contains("orders-*"), "{}", draft.routed_by);
    }

    #[test]
    fn refuses_to_guess_when_nothing_routes_the_source() {
        let unrouted = JiraSettings::default();
        let error = draft(&[issue()], &context(), &unrouted).unwrap_err();
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
        let draft = draft(&[issue()], &context(), &settings).unwrap();
        assert!(draft.labels.contains(&"event-bus".to_string()));
    }

    #[test]
    fn the_create_payload_is_shaped_the_way_rest_v2_wants_it() {
        let draft = draft(&[issue()], &context(), &settings()).unwrap();
        let fields = to_fields(&draft);
        assert_eq!(fields["fields"]["project"]["key"], "IPP");
        assert_eq!(fields["fields"]["issuetype"]["name"], "Bug");
        assert!(fields["fields"]["summary"]
            .as_str()
            .unwrap()
            .starts_with("[orders-fulfilment]"));
        assert!(fields["fields"]["assignee"].is_null());
    }
}
