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

/// Find an open ticket already filed for this exact problem.
///
/// Open only: a problem that was fixed and closed, then reappeared, deserves a
/// new ticket rather than a comment on a resolved one.
pub async fn find_open_by_fingerprint(
    client: &JiraClient,
    fingerprint: &str,
) -> Result<Option<FiledTicket>> {
    let jql = format!(
        "labels = {} AND statusCategory != Done ORDER BY created DESC",
        jql_quote(fingerprint)
    );
    let response = client
        .get("/search", &[("jql", jql.as_str()), ("maxResults", "1"), ("fields", "summary,status")])
        .await?;

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

pub async fn create(client: &JiraClient, draft: &TicketDraft) -> Result<FiledTicket> {
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
pub fn recurrence_comment(affected: usize, sampled: usize, window: &str) -> String {
    format!(
        "Still happening: {affected} of {sampled} sampled events over {window}, checked from pontifex."
    )
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
        return Err(Error::NotFound(format!(
            "Jira offers no issue types in project {project_key} — check the key, and that you \
             can create issues there."
        )));
    }

    Ok(types.into_iter().map(|(_, name)| name).collect())
}

/// `(id, name)` for each issue type a project offers.
async fn issue_type_list(client: &JiraClient, project_key: &str) -> Result<Vec<(String, String)>> {
    let response = client
        .get(&format!("/issue/createmeta/{project_key}/issuetypes"), &[])
        .await?;

    Ok(response
        .get("values")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|t| {
                    Some((
                        t.get("id")?.as_str()?.to_string(),
                        t.get("name")?.as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default())
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
    pub allowed_values: Vec<AllowedValue>,
}

/// Fields pontifex already fills, or Jira fills itself.
const HANDLED_FIELDS: &[&str] = &[
    "summary",
    "description",
    "issuetype",
    "project",
    "reporter",
    "labels",
];

/// Pull the fields that must be supplied out of a createmeta page.
///
/// Split from the request so the shape Jira returns can be tested without one.
pub fn parse_required_fields(page: &Value) -> Vec<RequiredField> {
    page.get("values")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter(|field| field.get("required").and_then(Value::as_bool) == Some(true))
                .filter_map(|field| {
                    let field_id = field.get("fieldId")?.as_str()?.to_string();
                    if HANDLED_FIELDS.contains(&field_id.as_str()) {
                        return None;
                    }
                    Some(RequiredField {
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
        })
        .unwrap_or_default()
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
            Error::NotFound(format!(
                "Project {project_key} has no issue type called '{issue_type}'. It offers: {}.",
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
    fn finds_the_custom_field_a_project_demands() {
        // The shape that rejected a ticket with "Discovery Environment is
        // required" — a mandatory custom field with a fixed set of values.
        let page = serde_json::json!({
            "values": [
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
        assert_eq!(fields[0].field_id, "customfield_10798");
        assert_eq!(fields[0].name, "Discovery Environment");
        assert_eq!(fields[0].field_type, "option");
        assert_eq!(fields[0].allowed_values.len(), 2);
        assert_eq!(fields[0].allowed_values[0].label, "Production");
        assert_eq!(fields[0].allowed_values[0].id, "10500");
    }

    #[test]
    fn does_not_ask_for_fields_pontifex_already_fills() {
        let page = serde_json::json!({
            "values": [
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
            "values": [{
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
            "values": [{
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
    fn a_project_that_demands_nothing_extra_returns_nothing() {
        assert!(parse_required_fields(&serde_json::json!({ "values": [] })).is_empty());
        assert!(parse_required_fields(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn the_recurrence_comment_carries_the_new_numbers() {
        let body = recurrence_comment(12, 200, "the last 1 day(s)");
        assert!(body.contains("12 of 200"), "{body}");
    }
}
