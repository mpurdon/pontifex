//! Which team owns an event source.
//!
//! Pods are Jira projects here, so routing is a map from the event `source` —
//! the thing a producer is named by on the bus — to a project key. Patterns
//! rather than exact names because sources come in families: `billing-*` is
//! one team's worth of producers, and listing them individually goes stale the
//! first time someone adds a service.

use crate::settings::{JiraSettings, SourceRoute};

/// Where a ticket is going, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routed {
    pub project_key: String,
    pub issue_type: String,
    pub labels: Vec<String>,
    pub assignee_account_id: Option<String>,
    /// The rule that decided this, for the preview: routing that cannot be
    /// explained is routing nobody trusts.
    pub reason: String,
}

/// How specific a pattern is, for picking between two that both match.
///
/// Literal characters count; wildcards do not. `billing-invoices` beats
/// `billing-*` beats `*`, which is what anyone writing the second rule meant.
fn specificity(pattern: &str) -> usize {
    pattern.chars().filter(|c| *c != '*').count()
}

/// Case-insensitive glob with `*` as the only metacharacter.
pub fn matches(pattern: &str, source: &str) -> bool {
    let pattern = pattern.trim().to_lowercase();
    let source = source.to_lowercase();
    if pattern.is_empty() {
        return false;
    }

    let mut remaining = source.as_str();
    let segments: Vec<&str> = pattern.split('*').collect();
    let last = segments.len() - 1;

    for (i, segment) in segments.iter().enumerate() {
        if segment.is_empty() {
            continue;
        }
        match (i, remaining.find(segment)) {
            // A leading segment before any `*` has to sit at the very start.
            (0, Some(0)) => remaining = &remaining[segment.len()..],
            (0, _) => return false,
            (_, Some(at)) => remaining = &remaining[at + segment.len()..],
            (_, None) => return false,
        }
        // Likewise a trailing segment has to reach the end.
        if i == last && !pattern.ends_with('*') && !remaining.is_empty() {
            return false;
        }
    }

    // A pattern with no wildcards at all is an exact match, not a prefix.
    if !pattern.contains('*') {
        return remaining.is_empty();
    }
    true
}

/// The best matching route for a source, or the configured fallback.
pub fn route_for(settings: &JiraSettings, source: &str) -> Option<Routed> {
    let best: Option<&SourceRoute> = settings
        .routes
        .iter()
        .filter(|route| !route.project_key.trim().is_empty() && matches(&route.pattern, source))
        .max_by_key(|route| specificity(&route.pattern));

    match best {
        Some(route) => Some(Routed {
            project_key: route.project_key.trim().to_string(),
            issue_type: route
                .issue_type
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| settings.issue_type.clone()),
            labels: route.labels.clone(),
            assignee_account_id: route.assignee_account_id.clone(),
            reason: format!("`{}` matches the rule {}", source, route.pattern),
        }),
        None => settings
            .default_project
            .as_ref()
            .filter(|key| !key.trim().is_empty())
            .map(|key| Routed {
                project_key: key.trim().to_string(),
                issue_type: settings.issue_type.clone(),
                labels: Vec::new(),
                assignee_account_id: None,
                reason: format!("no rule matches `{source}`, so the default project is used"),
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(pattern: &str, key: &str) -> SourceRoute {
        SourceRoute {
            id: pattern.to_string(),
            pattern: pattern.to_string(),
            project_key: key.to_string(),
            issue_type: None,
            labels: Vec::new(),
            assignee_account_id: None,
        }
    }

    fn settings(routes: Vec<SourceRoute>, default: Option<&str>) -> JiraSettings {
        JiraSettings {
            routes,
            default_project: default.map(str::to_string),
            ..JiraSettings::default()
        }
    }

    #[test]
    fn matches_prefixes_suffixes_and_exact_names() {
        assert!(matches("billing*", "billing-invoices"));
        assert!(matches("*-fulfilment", "orders-fulfilment"));
        assert!(matches("orders-fulfilment", "orders-fulfilment"));
        assert!(matches("*", "anything"));
        assert!(matches("ais-*-legal", "ais-outreach-legal"));

        assert!(!matches("billing*", "prebilling"));
        assert!(!matches("orders-fulfilment", "orders-fulfilment-v2"));
        assert!(!matches("", "orders"));
    }

    #[test]
    fn ignores_case_because_sources_are_not_consistent_about_it() {
        assert!(matches("Billing*", "billing-invoices"));
        assert!(matches("billing*", "Billing-Invoices"));
    }

    #[test]
    fn the_most_specific_rule_wins() {
        let settings = settings(
            vec![
                route("*", "CATCHALL"),
                route("billing*", "IPP"),
                route("billing-invoices", "INV"),
            ],
            None,
        );
        assert_eq!(
            route_for(&settings, "billing-invoices").unwrap().project_key,
            "INV",
        );
        assert_eq!(route_for(&settings, "billing-ledger").unwrap().project_key, "IPP");
        assert_eq!(route_for(&settings, "orders-fulfilment").unwrap().project_key, "CATCHALL");
    }

    #[test]
    fn falls_back_to_the_default_project_and_says_so() {
        let settings = settings(vec![route("billing*", "IPP")], Some("TRIAGE"));
        let routed = route_for(&settings, "orders-fulfilment").unwrap();
        assert_eq!(routed.project_key, "TRIAGE");
        assert!(routed.reason.contains("no rule matches"), "{}", routed.reason);
    }

    #[test]
    fn without_a_rule_or_a_default_there_is_nowhere_to_file() {
        let settings = settings(vec![route("billing*", "IPP")], None);
        assert!(route_for(&settings, "orders-fulfilment").is_none());
    }

    #[test]
    fn a_rule_with_no_project_key_is_ignored_rather_than_routed_to_nothing() {
        let settings = settings(vec![route("orders*", "  ")], Some("TRIAGE"));
        assert_eq!(
            route_for(&settings, "orders-fulfilment").unwrap().project_key,
            "TRIAGE",
        );
    }
}
