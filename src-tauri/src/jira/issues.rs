//! Creating, finding and updating the tickets themselves.

use crate::error::{Error, Result};
use crate::jira::client::JiraClient;
use crate::jira::ticket::{self, TicketDraft};
use serde::Serialize;
use serde_json::Value;

/// A ticket that exists in Jira.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiledTicket {
    pub key: String,
    pub url: String,
    /// Absent for a ticket we just created and have not read back.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// A ticket key as a link to the site it lives on.
pub(crate) fn browse_url(site_url: Option<&str>, key: &str) -> String {
    match site_url {
        Some(base) => format!("{}/browse/{key}", base.trim_end_matches('/')),
        // Better a key with no link than a link to the wrong site.
        None => key.to_string(),
    }
}

/// Escape a value for embedding in a JQL string literal.
fn jql_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The JQL that finds a ticket already filed for the same problem.
///
/// A ticket is the same problem when it carries every label identifying this
/// one — the bus, the producer, the event type, and the field where there is
/// one. That is a claim a person reading the ticket can check, which the
/// opaque fingerprint it replaced was not.
///
/// The fingerprint is still searched alongside, so tickets filed before the
/// labels were readable are still found rather than filed a second time.
fn duplicate_jql(draft: &TicketDraft) -> String {
    let identity = draft
        .identity
        .iter()
        .map(|label| format!("labels = {}", jql_quote(label)))
        .collect::<Vec<_>>()
        .join(" AND ");
    format!(
        "(labels = {} OR ({identity})) AND statusCategory != Done ORDER BY created DESC",
        jql_quote(&draft.fingerprint),
    )
}

/// Find an open ticket already filed for this problem.
///
/// Open only: a problem that was fixed and closed, then reappeared, deserves a
/// new ticket rather than a comment on a resolved one.
pub async fn find_open_duplicate(
    client: &JiraClient,
    draft: &TicketDraft,
) -> Result<Option<FiledTicket>> {
    let jql = duplicate_jql(draft);
    let response = client.search(&jql, "summary,status", 1).await?;

    let Some(first) = response
        .get("issues")
        .and_then(Value::as_array)
        .and_then(|issues| issues.first())
    else {
        return Ok(None);
    };

    let key = first
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Internal("Jira returned an issue with no key".into()))?;

    Ok(Some(FiledTicket {
        key: key.to_string(),
        url: browse_url(client.site_url(), key),
        status: first
            .pointer("/fields/status/name")
            .and_then(Value::as_str)
            .map(str::to_string),
        summary: first
            .pointer("/fields/summary")
            .and_then(Value::as_str)
            .map(str::to_string),
    }))
}

/// A ticket already filed about an event type, as the panel shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventTicket {
    pub key: String,
    pub url: String,
    pub summary: String,
    /// The project's own word for it — "In Progress", "Blocked", "Done".
    pub status: String,
    /// Jira's three-way grouping of that word, which is the part worth
    /// colouring by: a project can call its statuses anything.
    pub done: bool,
    pub started: bool,
    /// Findings this ticket covers, by the dotted path each was reported at.
    pub covers: Vec<String>,
    /// Every label on the ticket, so a caller with several event types in
    /// hand can tell which one a ticket belongs to without asking again.
    pub labels: Vec<String>,
    /// Epoch ms of the last change, for "moved 2 days ago".
    pub updated: Option<i64>,
}

/// Every ticket Pontifex has filed about one event type, newest first.
///
/// Open and closed alike: a finding that is still reported with a Done ticket
/// against it is worth seeing as exactly that, and a closed ticket is the
/// answer to "did anyone ever raise this".
pub async fn tickets_for_event(
    client: &JiraClient,
    labels: &[String],
    paths: &[String],
    limit: u32,
) -> Result<Vec<EventTicket>> {
    if labels.is_empty() {
        return Ok(Vec::new());
    }
    let jql = format!(
        "{} ORDER BY created DESC",
        labels
            .iter()
            .map(|label| format!("labels = {}", jql_quote(label)))
            .collect::<Vec<_>>()
            .join(" AND "),
    );
    let response = client
        .search(&jql, "summary,status,labels,updated", limit)
        .await?;

    Ok(response
        .get("issues")
        .and_then(Value::as_array)
        .map(|issues| {
            issues
                .iter()
                .filter_map(|issue| parse_event_ticket(issue, client.site_url(), paths))
                .collect()
        })
        .unwrap_or_default())
}

/// One search result as a ticket, or nothing if Jira returned something
/// without a key — which would be a row nobody could click.
fn parse_event_ticket(
    issue: &Value,
    site_url: Option<&str>,
    paths: &[String],
) -> Option<EventTicket> {
    let key = issue.get("key")?.as_str()?.to_string();
    let labels: Vec<&str> = issue
        .pointer("/fields/labels")
        .and_then(Value::as_array)
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let category = issue
        .pointer("/fields/status/statusCategory/key")
        .and_then(Value::as_str)
        .unwrap_or_default();

    Some(EventTicket {
        url: browse_url(site_url, &key),
        key,
        summary: issue
            .pointer("/fields/summary")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        status: issue
            .pointer("/fields/status/name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        done: category == "done",
        started: category == "indeterminate",
        // Matched on the label the field would have been given, so this agrees
        // with what was filed rather than guessing from the summary text.
        covers: paths
            .iter()
            .filter(|path| labels.contains(&crate::jira::ticket::label(path).as_str()))
            .cloned()
            .collect(),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        updated: issue
            .pointer("/fields/updated")
            .and_then(Value::as_str)
            .and_then(parse_jira_time),
    })
}

/// Jira's timestamps are ISO 8601 with a `+0000`-style offset, which
/// `time`'s RFC 3339 parser refuses — so read it as the format Jira sends.
fn parse_jira_time(text: &str) -> Option<i64> {
    const FORMAT: &[time::format_description::FormatItem<'_>] = time::macros::format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3][offset_hour sign:mandatory][offset_minute]"
    );
    time::OffsetDateTime::parse(text, FORMAT)
        .ok()
        .map(|t| t.unix_timestamp() * 1000 + i64::from(t.millisecond()))
}

pub async fn create(client: &JiraClient, draft: &TicketDraft) -> Result<FiledTicket> {
    let draft = &off_screen_fields_dropped(client, draft).await;
    let created = client.post("/issue", &ticket::to_fields(draft)).await?;
    let key = created
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Internal("Jira created an issue but returned no key".into()))?;

    Ok(FiledTicket {
        key: key.to_string(),
        url: browse_url(client.site_url(), key),
        status: None,
        summary: Some(draft.summary.clone()),
    })
}

/// The draft without an offered field its issue type's create screen lacks.
///
/// A priority answered for one issue type, or saved as the project's default,
/// is sent again for a type whose screen has no Priority — and Jira refuses
/// the whole create over it. Only checked when the draft carries one, so
/// most creates cost no extra call; if the screen cannot be read, the draft
/// goes as it is and Jira's own answer stands.
async fn off_screen_fields_dropped(client: &JiraClient, draft: &TicketDraft) -> TicketDraft {
    let mut draft = draft.clone();
    if !draft
        .fields
        .keys()
        .any(|id| OFFERED_FIELDS.contains(&id.as_str()))
    {
        return draft;
    }
    if let Ok(screen) = required_fields(client, &draft.project_key, &draft.issue_type).await {
        drop_off_screen(&mut draft.fields, &screen);
    }
    draft
}

/// Drop the offered fields `screen` does not list. Required ones are left for
/// Jira to judge: those are what the project said it needs.
fn drop_off_screen(fields: &mut std::collections::BTreeMap<String, Value>, screen: &[RequiredField]) {
    fields.retain(|id, _| {
        !OFFERED_FIELDS.contains(&id.as_str()) || screen.iter().any(|f| &f.field_id == id)
    });
}

/// Add a comment saying the problem is still happening, with today's numbers.
pub async fn comment(client: &JiraClient, key: &str, body: &str) -> Result<()> {
    client
        .post(
            &format!("/issue/{key}/comment"),
            &serde_json::json!({ "body": body }),
        )
        .await?;
    Ok(())
}

/// What a re-observation says on an existing ticket.
///
/// A roll-up cannot report one pair of numbers without picking a finding to
/// speak for the rest, so it counts them and quotes the worst.
pub fn recurrence_comment(findings: &[crate::schema::events::Issue], window: &str) -> String {
    match findings {
        [single] => format!(
            "Still happening: {} of {} sampled events over {window}, checked from Pontifex.",
            single.affected, single.sampled
        ),
        many => {
            let worst = many.iter().max_by_key(|i| i.affected);
            format!(
                "Still happening: {} findings on this event type over {window}, checked from \
                 Pontifex.{}",
                many.len(),
                worst
                    .map(|i| format!(
                        " The largest affects {} of {} sampled events.",
                        i.affected, i.sampled
                    ))
                    .unwrap_or_default(),
            )
        }
    }
}

/// Projects the signed-in user can see, for the routing UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JiraProject {
    pub key: String,
    pub name: String,
}

pub async fn list_projects(client: &JiraClient) -> Result<Vec<JiraProject>> {
    // `/project/search` paginates; 200 is more projects than a routing table
    // will ever reasonably name, and asking for everything is slower for no
    // gain.
    let response = client
        .get("/project/search", &[("maxResults", "200"), ("orderBy", "key")])
        .await?;

    let values = response
        .get("values")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    Ok(values
        .iter()
        .filter_map(|project| {
            Some(JiraProject {
                key: project.get("key")?.as_str()?.to_string(),
                name: project
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .collect())
}

/// Issue type names a project actually offers.
///
/// Every project can name its types differently, so "Bug" is a guess until
/// this says otherwise — and a wrong guess is a 400 at create time, which is
/// the worst moment to discover it.
pub async fn issue_types(client: &JiraClient, project_key: &str) -> Result<Vec<String>> {
    let types = issue_type_list(client, project_key).await?;

    if types.is_empty() {
        return Err(no_issue_types(project_key));
    }

    Ok(types.into_iter().map(|(_, name)| name).collect())
}

/// `(id, name)` for each issue type a project offers.
async fn issue_type_list(client: &JiraClient, project_key: &str) -> Result<Vec<(String, String)>> {
    let response = client
        .get(
            &format!("/issue/createmeta/{project_key}/issuetypes"),
            &[("maxResults", "200")],
        )
        .await?;

    Ok(parse_issue_types(&response))
}

/// `(id, name)` out of a createmeta issue-types page.
///
/// Split from the request, like [`parse_required_fields`], so the shape Jira
/// returns is something a test can hold.
fn parse_issue_types(page: &Value) -> Vec<(String, String)> {
    page_items(page, "issueTypes")
        .iter()
        .filter_map(|t| {
            Some((
                t.get("id")?.as_str()?.to_string(),
                t.get("name")?.as_str()?.to_string(),
            ))
        })
        .collect()
}

/// What an empty list of issue types means, which is never "this project has
/// none" — a project cannot exist without one. Either the key names nothing
/// this account can see, or it cannot create issues there.
fn no_issue_types(project_key: &str) -> Error {
    Error::NotFound(format!(
        "Jira offers no issue types in project {project_key} — check the key, and that you \
         can create issues there."
    ))
}

/// A value a field will accept, as Jira describes it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AllowedValue {
    pub id: String,
    pub label: String,
}

/// A field this project demands and pontifex does not already set.
///
/// Projects are free to make anything mandatory — "Discovery Environment",
/// "Team", a component — and a create call that omits one is rejected with a
/// message naming a custom field id. Asking Jira what it wants beats guessing.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequiredField {
    pub field_id: String,
    pub name: String,
    /// `schema.type`: `string`, `option`, `array`, `number`, `user`, …
    pub field_type: String,
    /// Element type, when `field_type` is `array`.
    pub items: Option<String>,
    /// Jira claims it supplies this one.
    ///
    /// Kept rather than filtered out, because the claim is not always true of
    /// a REST create: a select with a configured default can still be demanded
    /// on the API. Shown, but never blocking.
    pub has_default: bool,
    /// Whether the project demands it, as opposed to merely offering it.
    pub required: bool,
    pub allowed_values: Vec<AllowedValue>,
}

/// Offered rather than demanded, and asked about anyway.
///
/// Priority is the field a producer team triages by, and a ticket filed
/// without one lands as "Unassigned" at the bottom of a backlog. It is
/// almost never required, so asking Jira only about required fields never
/// surfaced it — and sending one unasked would be rejected by any project
/// that does not have it on the create screen.
const OFFERED_FIELDS: &[&str] = &["priority"];

/// Fields pontifex already fills, or Jira fills itself.
const HANDLED_FIELDS: &[&str] = &[
    "summary",
    "description",
    "issuetype",
    "project",
    "reporter",
    "labels",
];

/// The array out of one of Jira's paginated create-meta responses.
///
/// Named per endpoint — `issueTypes` for the types a project offers, `fields`
/// for what one of them demands — and not `values`, which is what most of
/// Jira's other paginated responses use. Reading `values` here found nothing
/// every time, and an empty page is indistinguishable from a project that
/// offers nothing: the issue-type picker in Settings stayed empty, and every
/// ticket reported that its project "has no issue type called 'Bug'. It
/// offers: ." `values` is still accepted, because Jira's pagination is not
/// consistent about this and a second spelling costs one line.
fn page_items<'a>(page: &'a Value, name: &str) -> &'a [Value] {
    page.get(name)
        .or_else(|| page.get("values"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// Pull the fields that must be supplied out of a createmeta page.
///
/// Split from the request so the shape Jira returns can be tested without one.
pub fn parse_required_fields(page: &Value) -> Vec<RequiredField> {
    page_items(page, "fields")
                .iter()
                .filter_map(|field| {
                    let field_id = field.get("fieldId")?.as_str()?.to_string();
                    if HANDLED_FIELDS.contains(&field_id.as_str()) {
                        return None;
                    }
                    let required = field.get("required").and_then(Value::as_bool) == Some(true);
                    if !required && !OFFERED_FIELDS.contains(&field_id.as_str()) {
                        return None;
                    }
                    Some(RequiredField {
                        required,
                        name: field
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(&field_id)
                            .to_string(),
                        field_type: field
                            .pointer("/schema/type")
                            .and_then(Value::as_str)
                            .unwrap_or("string")
                            .to_string(),
                        items: field
                            .pointer("/schema/items")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        has_default: field
                            .get("hasDefaultValue")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        allowed_values: field
                            .get("allowedValues")
                            .and_then(Value::as_array)
                            .map(|allowed| {
                                allowed
                                    .iter()
                                    .filter_map(|value| {
                                        Some(AllowedValue {
                                            id: value.get("id")?.as_str()?.to_string(),
                                            // Option fields carry `value`,
                                            // most everything else `name`.
                                            label: value
                                                .get("value")
                                                .or_else(|| value.get("name"))
                                                .and_then(Value::as_str)
                                                .unwrap_or_default()
                                                .to_string(),
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        field_id,
                    })
                })
                .collect()
}

/// What this project demands before it will accept a ticket.
pub async fn required_fields(
    client: &JiraClient,
    project_key: &str,
    issue_type: &str,
) -> Result<Vec<RequiredField>> {
    let types = issue_type_list(client, project_key).await?;
    let (type_id, _) = types
        .iter()
        .find(|(_, name)| name.eq_ignore_ascii_case(issue_type))
        .ok_or_else(|| {
            // An empty list is a different problem from a name that does not
            // match one, and "It offers: ." told nobody either of them.
            if types.is_empty() {
                return no_issue_types(project_key);
            }
            Error::NotFound(format!(
                "Project {project_key} has no issue type called '{issue_type}'. It offers: {}. \
                 Choose one in Settings → Jira.",
                types
                    .iter()
                    .map(|(_, name)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
        })?;

    let page = client
        .get(
            &format!("/issue/createmeta/{project_key}/issuetypes/{type_id}"),
            &[("maxResults", "200")],
        )
        .await?;

    Ok(parse_required_fields(&page))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_key_becomes_a_link_to_the_site_it_lives_on() {
        assert_eq!(
            browse_url(Some("https://example.atlassian.net/"), "PROJ-42"),
            "https://example.atlassian.net/browse/PROJ-42",
        );
    }

    #[test]
    fn without_a_site_the_key_stands_alone_rather_than_linking_somewhere_wrong() {
        assert_eq!(browse_url(None, "PROJ-42"), "PROJ-42");
    }

    #[test]
    fn jql_literals_survive_a_quote() {
        assert_eq!(jql_quote("pontifex-abc123"), "\"pontifex-abc123\"");
        assert_eq!(jql_quote("od\"d"), "\"od\\\"d\"");
    }

    #[test]
    fn reads_the_issue_types_a_project_offers() {
        // Verbatim from Atlassian's own example for
        // `GET /issue/createmeta/{projectIdOrKey}/issuetypes`: the array is
        // `issueTypes`, not the `values` most of Jira's paginated responses
        // use. Reading `values` returned nothing for every project — so the
        // type picker in Settings was empty, and every ticket said its project
        // "has no issue type called 'Bug'. It offers: ."
        let page = serde_json::json!({
            "issueTypes": [
                {
                    "description": "An error in the code",
                    "iconUrl": "https://your-domain.atlassian.net/images/icons/issuetypes/bug.png",
                    "id": "1",
                    "name": "Bug",
                    "self": "https://your-domain.atlassian.net/rest/api/3/issueType/1",
                    "subtask": false
                },
                {
                    "description": "A task",
                    "id": "3",
                    "name": "Task",
                    "subtask": false
                }
            ],
            "maxResults": 2,
            "startAt": 0,
            "total": 2
        });

        let types = parse_issue_types(&page);
        assert_eq!(
            types,
            vec![
                ("1".to_string(), "Bug".to_string()),
                ("3".to_string(), "Task".to_string()),
            ],
        );
    }

    #[test]
    fn reads_the_fields_page_atlassian_documents() {
        // Likewise for `.../issuetypes/{issueTypeId}`, whose array is `fields`.
        let page = serde_json::json!({
            "fields": [
                {
                    "fieldId": "assignee",
                    "hasDefaultValue": false,
                    "key": "assignee",
                    "name": "Assignee",
                    "operations": ["set"],
                    "required": true
                }
            ],
            "maxResults": 1,
            "startAt": 0,
            "total": 1
        });

        let fields = parse_required_fields(&page);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field_id, "assignee");
        // No schema on this one: a field with no stated type is text.
        assert_eq!(fields[0].field_type, "string");
    }

    #[test]
    fn a_page_that_calls_its_array_values_is_still_read() {
        // Deliberate, not incidental: Jira's pagination is not consistent
        // about the name, and this is the spelling that was assumed before.
        assert_eq!(
            parse_issue_types(&serde_json::json!({
                "values": [{ "id": "7", "name": "Defect" }]
            })),
            vec![("7".to_string(), "Defect".to_string())],
        );
    }

    #[test]
    fn finds_the_custom_field_a_project_demands() {
        // The shape that rejected a ticket with "Discovery Environment is
        // required" — a mandatory custom field with a fixed set of values.
        let page = serde_json::json!({
            "fields": [
                {
                    "fieldId": "customfield_10798",
                    "name": "Discovery Environment",
                    "required": true,
                    "hasDefaultValue": false,
                    "schema": { "type": "option", "custom": "select", "customId": 10798 },
                    "allowedValues": [
                        { "id": "10500", "value": "Production" },
                        { "id": "10501", "value": "Staging" }
                    ]
                }
            ]
        });

        let fields = parse_required_fields(&page);
        assert_eq!(fields.len(), 1);
        assert!(fields[0].required);
        assert_eq!(fields[0].field_id, "customfield_10798");
        assert_eq!(fields[0].name, "Discovery Environment");
        assert_eq!(fields[0].field_type, "option");
        assert_eq!(fields[0].allowed_values.len(), 2);
        assert_eq!(fields[0].allowed_values[0].label, "Production");
        assert_eq!(fields[0].allowed_values[0].id, "10500");
    }

    #[test]
    fn a_priority_the_screen_lacks_is_not_sent() {
        let mut fields: std::collections::BTreeMap<String, Value> = [
            ("priority".to_string(), serde_json::json!({ "id": "2" })),
            ("customfield_1".to_string(), serde_json::json!("prod")),
        ]
        .into();
        // A screen with no Priority: it goes, the project's own demand stays.
        drop_off_screen(&mut fields, &[]);
        assert!(!fields.contains_key("priority"));
        assert!(fields.contains_key("customfield_1"));

        let mut fields: std::collections::BTreeMap<String, Value> =
            [("priority".to_string(), serde_json::json!({ "id": "2" }))].into();
        let screen = parse_required_fields(&serde_json::json!({
            "fields": [{ "fieldId": "priority", "name": "Priority", "required": false }]
        }));
        drop_off_screen(&mut fields, &screen);
        assert!(fields.contains_key("priority"));
    }

    #[test]
    fn does_not_ask_for_fields_pontifex_already_fills() {
        let page = serde_json::json!({
            "fields": [
                { "fieldId": "summary", "name": "Summary", "required": true,
                  "hasDefaultValue": false, "schema": { "type": "string" } },
                { "fieldId": "issuetype", "name": "Issue Type", "required": true,
                  "hasDefaultValue": false, "schema": { "type": "issuetype" } },
                { "fieldId": "project", "name": "Project", "required": true,
                  "hasDefaultValue": false, "schema": { "type": "project" } },
                // Not required at all.
                { "fieldId": "customfield_1", "name": "Optional", "required": false,
                  "hasDefaultValue": false, "schema": { "type": "string" } }
            ]
        });

        assert!(parse_required_fields(&page).is_empty());
    }

    #[test]
    fn keeps_a_required_field_jira_claims_to_default() {
        // Jira's claim is not always true of a REST create, and a field
        // filtered out here is one the user cannot fill when it is demanded
        // anyway — which is exactly how a create fails with a message naming a
        // field that never appeared on screen.
        let page = serde_json::json!({
            "fields": [{
                "fieldId": "customfield_10798",
                "name": "Discovery Environment",
                "required": true,
                "hasDefaultValue": true,
                "schema": { "type": "option" },
                "allowedValues": [{ "id": "10500", "value": "Production" }]
            }]
        });

        let fields = parse_required_fields(&page);
        assert_eq!(fields.len(), 1);
        assert!(fields[0].has_default);
    }

    #[test]
    fn reads_a_multi_value_field_and_its_element_type() {
        let page = serde_json::json!({
            "fields": [{
                "fieldId": "components",
                "name": "Components",
                "required": true,
                "hasDefaultValue": false,
                "schema": { "type": "array", "items": "component" },
                "allowedValues": [{ "id": "10010", "name": "billing" }]
            }]
        });

        let fields = parse_required_fields(&page);
        assert_eq!(fields[0].field_type, "array");
        assert_eq!(fields[0].items.as_deref(), Some("component"));
        // Components carry `name` where option fields carry `value`.
        assert_eq!(fields[0].allowed_values[0].label, "billing");
    }

    #[test]
    fn offers_priority_even_though_no_project_demands_it() {
        // A ticket filed without one lands as "Unassigned" at the bottom of a
        // backlog, and priority is almost never a required field — so asking
        // only about required fields never saw it.
        let page = serde_json::json!({
            "fields": [
                {
                    "fieldId": "priority",
                    "name": "Priority",
                    "required": false,
                    "hasDefaultValue": true,
                    "schema": { "type": "priority" },
                    "allowedValues": [
                        { "id": "1", "name": "Highest" },
                        { "id": "2", "name": "High" },
                        { "id": "3", "name": "Medium" }
                    ]
                },
                // Still ignored: offered, not demanded, and not one we ask for.
                {
                    "fieldId": "customfield_999", "name": "Sprint", "required": false,
                    "hasDefaultValue": false, "schema": { "type": "array" }
                }
            ]
        });

        let fields = parse_required_fields(&page);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].field_id, "priority");
        assert!(!fields[0].required);
        assert_eq!(fields[0].allowed_values[1].label, "High");
    }

    #[test]
    fn a_project_that_demands_nothing_extra_returns_nothing() {
        assert!(parse_required_fields(&serde_json::json!({ "fields": [] })).is_empty());
        assert!(parse_required_fields(&serde_json::json!({})).is_empty());
    }

    /// A finding with nothing on it but the numbers a comment quotes.
    fn issue(affected: usize, sampled: usize) -> crate::schema::events::Issue {
        use crate::schema::events::{IssueKind, IssueSeverity};
        crate::schema::events::Issue {
            key: format!("wrongType:field{affected}"),
            kind: IssueKind::WrongType,
            severity: IssueSeverity::Error,
            path: "field".into(),
            summary: "`field` is declared integer but events send string".into(),
            action: "Fix the producer.".into(),
            declared: None,
            observed: None,
            affected,
            sampled,
            rejects: true,
            example: None,
            message: None,
            fix: None,
            impact: None,
        }
    }

    #[test]
    fn reads_a_filed_ticket_out_of_a_search_result() {
        // The shape `/search` returns, with the fields the panel asks for.
        let issue = serde_json::json!({
            "key": "DPIPP-42",
            "fields": {
                "summary": "[billing-medical] 5 schema findings on payment-authorized",
                "status": {
                    "name": "In Progress",
                    "statusCategory": { "key": "indeterminate", "name": "In Progress" }
                },
                "labels": [
                    "pontifex",
                    "prd-global-bus",
                    "billing-medical",
                    "payment-authorized",
                    "paymentAmount",
                    "callAttemptHistory.result"
                ],
                "updated": "2026-09-22T18:26:59.123+0000"
            }
        });

        let paths = vec![
            "paymentAmount".to_string(),
            // Labelled with the array marker stripped, so the match has to go
            // through the same rule that wrote the label.
            "callAttemptHistory[].result".to_string(),
            "clientId".to_string(),
        ];
        let ticket = parse_event_ticket(&issue, Some("https://x.atlassian.net"), &paths).unwrap();

        assert_eq!(ticket.key, "DPIPP-42");
        assert_eq!(ticket.url, "https://x.atlassian.net/browse/DPIPP-42");
        assert_eq!(ticket.status, "In Progress");
        assert!(ticket.started && !ticket.done);
        assert_eq!(
            ticket.covers,
            vec!["paymentAmount", "callAttemptHistory[].result"],
        );
        assert_eq!(ticket.updated, Some(1790101619123));
    }

    #[test]
    fn a_ticket_jira_calls_done_is_marked_done() {
        let issue = serde_json::json!({
            "key": "DPIPP-7",
            "fields": {
                "summary": "fixed",
                // The name is the project's own; the category is Jira's.
                "status": { "name": "Shipped", "statusCategory": { "key": "done" } },
                "labels": ["pontifex"]
            }
        });
        let ticket = parse_event_ticket(&issue, None, &[]).unwrap();
        assert!(ticket.done && !ticket.started);
        assert_eq!(ticket.status, "Shipped");
        // No site to link to, so the key stands alone rather than linking
        // somewhere wrong.
        assert_eq!(ticket.url, "DPIPP-7");
        assert_eq!(ticket.updated, None);
    }

    #[test]
    fn reads_the_offset_jira_stamps_rather_than_rfc_3339() {
        // `+0000`, not `+00:00` — the spelling RFC 3339 parsers refuse.
        assert_eq!(
            parse_jira_time("2026-09-22T18:26:59.000+0000"),
            Some(1790101619000),
        );
        assert_eq!(parse_jira_time("yesterday"), None);
    }

    #[test]
    fn the_duplicate_search_asks_for_every_identifying_label() {
        let draft = TicketDraft {
            project_key: "IPP".into(),
            issue_type: "Bug".into(),
            summary: "[orders-fulfilment] something".into(),
            description: String::new(),
            labels: vec!["pontifex".into(), "prd-global-bus".into(), "extra".into()],
            fields: Default::default(),
            fingerprint: "pontifex-abc123def456".into(),
            identity: vec![
                "pontifex".into(),
                "prd-global-bus".into(),
                "orders-fulfilment".into(),
                "lead-unreached".into(),
                "callAttemptCount".into(),
            ],
            assignee_account_id: None,
            routed_by: "rule".into(),
        };

        let jql = duplicate_jql(&draft);
        // Every identifying label, and only those — a label the routing rule
        // added is not part of what the ticket is about.
        for label in &draft.identity {
            assert!(jql.contains(&format!("labels = \"{label}\"")), "{jql}");
        }
        assert!(!jql.contains("\"extra\""), "{jql}");
        // The old fingerprint too, so tickets filed before the labels were
        // readable are still found rather than filed again.
        assert!(jql.contains("\"pontifex-abc123def456\""), "{jql}");
        assert!(jql.contains("statusCategory != Done"), "{jql}");
    }

    #[test]
    fn the_recurrence_comment_carries_the_new_numbers() {
        let body = recurrence_comment(&[issue(12, 200)], "the last 1 day(s)");
        assert!(body.contains("12 of 200"), "{body}");
    }

    #[test]
    fn a_roll_up_recurrence_counts_the_findings_and_quotes_the_worst() {
        let body = recurrence_comment(&[issue(12, 200), issue(180, 200)], "the last 1 day(s)");
        assert!(body.contains("2 findings"), "{body}");
        assert!(body.contains("180 of 200"), "{body}");
    }
}
