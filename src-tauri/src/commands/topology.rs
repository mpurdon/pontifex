use crate::aws::clients::map_sdk_error;
use crate::error::{Error, Result};
use crate::logging::cat;
use crate::state::AppState;
use futures::stream::StreamExt;
use regex::Regex;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::OnceLock;
use tauri::State;

/// A bus declared in the repo's `stacks/busConfiguration.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredBus {
    pub source_bus_name: String,
    pub source_arn: String,
    /// "trajector" or "thirdParty" — which map the entry came from.
    pub category: String,
    pub stage: String,
    /// Parsed out of the ARN, so the UI can flag cross-account buses.
    pub account_id: Option<String>,
    pub region: Option<String>,
}

fn account_and_region(arn: &str) -> (Option<String>, Option<String>) {
    // arn:aws:events:<region>:<account>:event-bus/<name>
    let parts: Vec<&str> = arn.split(':').collect();
    let region = parts.get(3).filter(|s| !s.is_empty()).map(|s| s.to_string());
    let account = parts.get(4).filter(|s| !s.is_empty()).map(|s| s.to_string());
    (account, region)
}

fn entry_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        // Matches a `{ sourceBusName: '...', sourceArn: '...' }` literal across
        // line breaks, tolerating single or double quotes and trailing commas.
        Regex::new(
            r#"(?s)sourceBusName\s*:\s*['"]([^'"]+)['"]\s*,\s*sourceArn\s*:\s*\n?\s*['"]([^'"]+)['"]"#,
        )
        .expect("static regex")
    })
}

fn map_pattern(map_name: &str) -> Regex {
    // Captures the body of `trajectorBuses: { ... }` up to the closing brace at
    // the same indentation. The file is a static literal with a stable shape,
    // so this is adequate without a full TS parser.
    Regex::new(&format!(r"(?s){map_name}\s*:\s*\{{(.*?)\n  \}}")).expect("static regex")
}

fn stage_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?s)(sandbox|dev|stg|prd)\s*:\s*\[(.*?)\]").expect("static regex")
    })
}

/// Extract the declared buses from `busConfiguration.ts`.
///
/// The file is a hand-maintained static object literal, so a targeted regex
/// pass is enough and avoids pulling in a TypeScript parser. If the file's
/// shape changes, this degrades to returning fewer buses rather than failing —
/// the live AWS data still renders, and the drift report will say so.
pub fn parse_bus_configuration(content: &str) -> Vec<DeclaredBus> {
    let mut buses = Vec::new();

    for (map_name, category) in [
        ("trajectorBuses", "trajector"),
        ("thirdPartyBuses", "thirdParty"),
    ] {
        let Some(map_match) = map_pattern(map_name).captures(content) else {
            continue;
        };
        let body = map_match.get(1).map(|m| m.as_str()).unwrap_or("");

        for stage_match in stage_pattern().captures_iter(body) {
            let stage = stage_match.get(1).map(|m| m.as_str()).unwrap_or("");
            let entries = stage_match.get(2).map(|m| m.as_str()).unwrap_or("");

            for entry in entry_pattern().captures_iter(entries) {
                let name = entry.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                let arn = entry.get(2).map(|m| m.as_str()).unwrap_or("").to_string();
                let (account_id, region) = account_and_region(&arn);
                buses.push(DeclaredBus {
                    source_bus_name: name,
                    source_arn: arn,
                    category: category.to_string(),
                    stage: stage.to_string(),
                    account_id,
                    region,
                });
            }
        }
    }

    buses
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleTarget {
    pub id: String,
    pub arn: String,
    /// "lambda", "sqs", "events", ... derived from the ARN's service segment.
    pub service: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveRule {
    pub name: String,
    pub state: Option<String>,
    pub description: Option<String>,
    pub event_pattern: Option<String>,
    pub targets: Vec<RuleTarget>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveBus {
    pub name: String,
    pub arn: Option<String>,
    pub rules: Vec<LiveRule>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopologyDrift {
    /// Declared in busConfiguration.ts but not visible in the account.
    /// Expected for cross-account buses the profile cannot read.
    pub declared_only: Vec<DeclaredBus>,
    /// Present in the account but absent from busConfiguration.ts.
    pub live_only: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Topology {
    pub stage: String,
    pub region: String,
    pub account_id: Option<String>,
    pub declared: Vec<DeclaredBus>,
    pub live: Vec<LiveBus>,
    pub drift: TopologyDrift,
    /// Set when busConfiguration.ts could not be read, so the UI can explain
    /// why the declared side is empty instead of showing a bare graph.
    pub config_warning: Option<String>,
    pub config_path: Option<String>,
}

fn service_of(arn: &str) -> Option<String> {
    arn.split(':').nth(2).map(str::to_string)
}

/// Build the bus topology for an environment: what the repo declares, what is
/// actually deployed, and where the two disagree.
#[tauri::command]
pub async fn get_topology(
    state: State<'_, AppState>,
    env_id: Option<String>,
) -> Result<Topology> {
    let (env, cfg) = state.env_config(env_id.as_deref()).await?;
    let settings = state.settings_snapshot().await;

    // --- declared side -----------------------------------------------------
    let mut config_warning = None;
    let mut config_path = None;
    let mut declared = Vec::new();

    match settings.event_bus_repo_path.as_deref() {
        Some(repo) if !repo.trim().is_empty() => {
            let path = PathBuf::from(crate::settings::shellexpand_home(repo))
                .join("stacks")
                .join("busConfiguration.ts");
            config_path = Some(path.to_string_lossy().into_owned());
            match std::fs::read_to_string(&path) {
                Ok(content) => {
                    let all = parse_bus_configuration(&content);
                    // The environment's label is the stage in the config file.
                    declared = all.into_iter().filter(|b| b.stage == env.label).collect();
                    if declared.is_empty() {
                        config_warning = Some(format!(
                            "No buses declared for stage '{}' in {}",
                            env.label,
                            path.display()
                        ));
                    }
                }
                Err(e) => {
                    config_warning = Some(format!("Could not read {}: {e}", path.display()));
                }
            }
        }
        _ => {
            config_warning = Some(
                "No global-event-bus repo path set. Add one in Settings to see declared buses."
                    .into(),
            );
        }
    }

    // --- live side ---------------------------------------------------------
    let events = aws_sdk_eventbridge::Client::new(&cfg);
    let mut live: Vec<LiveBus> = Vec::new();

    // EventBridge's list operations have no generated paginator, so walk
    // next_token by hand.
    let mut next_token: Option<String> = None;
    loop {
        let mut req = events.list_event_buses();
        if let Some(token) = &next_token {
            req = req.next_token(token);
        }
        let page = req.send().await.map_err(map_sdk_error)?;

        for bus in page.event_buses() {
            let Some(name) = bus.name() else { continue };
            live.push(LiveBus {
                name: name.to_string(),
                arn: bus.arn().map(str::to_string),
                rules: Vec::new(),
            });
        }

        next_token = page.next_token().map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }

    // Rules and their targets, per bus. The comment used to say this was
    // bounded by the handful of buses in an account — but the cost is one
    // request per *rule*, and a stage's global bus routes every event type, so
    // hundreds is normal. List the rules first, then fetch their targets
    // concurrently.
    for bus in &mut live {
        let mut rule_token: Option<String> = None;
        loop {
            let mut req = events.list_rules().event_bus_name(&bus.name);
            if let Some(token) = &rule_token {
                req = req.next_token(token);
            }
            let page = req.send().await.map_err(map_sdk_error)?;

            for rule in page.rules() {
                let Some(rule_name) = rule.name() else { continue };
                bus.rules.push(LiveRule {
                    name: rule_name.to_string(),
                    state: rule.state().map(|s| s.as_str().to_string()),
                    description: rule.description().map(str::to_string),
                    event_pattern: rule.event_pattern().map(str::to_string),
                    targets: Vec::new(),
                });
            }

            rule_token = page.next_token().map(str::to_string);
            if rule_token.is_none() {
                break;
            }
        }
        bus.rules.sort_by(|a, b| a.name.cmp(&b.name));
    }
    live.sort_by(|a, b| a.name.cmp(&b.name));

    const TARGET_CONCURRENCY: usize = 16;
    let pending: Vec<(usize, usize, String, String)> = live
        .iter()
        .enumerate()
        .flat_map(|(bus_index, bus)| {
            bus.rules.iter().enumerate().map(move |(rule_index, rule)| {
                (bus_index, rule_index, bus.name.clone(), rule.name.clone())
            })
        })
        .collect();

    let fetched: Vec<(usize, usize, Vec<RuleTarget>)> = futures::stream::iter(pending)
        .map(|(bus_index, rule_index, bus_name, rule_name)| {
            let events = events.clone();
            async move {
                let targets = match events
                    .list_targets_by_rule()
                    .rule(&rule_name)
                    .event_bus_name(&bus_name)
                    .send()
                    .await
                {
                    Ok(out) => out
                        .targets()
                        .iter()
                        .map(|t| RuleTarget {
                            id: t.id().to_string(),
                            service: service_of(t.arn()),
                            arn: t.arn().to_string(),
                        })
                        .collect(),
                    // A rule we cannot read targets for is still worth showing.
                    Err(e) => {
                        lwarn!(cat::TOPOLOGY, "could not list targets for rule {rule_name}: {e}");
                        Vec::new()
                    }
                };
                (bus_index, rule_index, targets)
            }
        })
        .buffer_unordered(TARGET_CONCURRENCY)
        .collect()
        .await;

    // Indices were taken after both sorts, so they still address the right rule.
    for (bus_index, rule_index, targets) in fetched {
        live[bus_index].rules[rule_index].targets = targets;
    }

    // --- drift -------------------------------------------------------------
    let live_names: std::collections::HashSet<&str> =
        live.iter().map(|b| b.name.as_str()).collect();

    let declared_only: Vec<DeclaredBus> = declared
        .iter()
        .filter(|d| !live_names.contains(d.source_bus_name.as_str()))
        .cloned()
        .collect();

    let declared_names: std::collections::HashSet<&str> =
        declared.iter().map(|d| d.source_bus_name.as_str()).collect();

    // The stage's own buses are created by this stack, not declared as
    // sources, so excluding them keeps the drift report meaningful.
    let stage_prefix = format!("{}-", env.label);
    let live_only: Vec<String> = live
        .iter()
        .map(|b| b.name.clone())
        .filter(|n| {
            !declared_names.contains(n.as_str()) && !n.starts_with(&stage_prefix) && n != "default"
        })
        .collect();

    let account_id = crate::aws::clients::caller_identity(&cfg)
        .await
        .ok()
        .and_then(|i| i.account);

    Ok(Topology {
        stage: env.label.clone(),
        region: env.region.clone(),
        account_id,
        declared,
        live,
        drift: TopologyDrift {
            declared_only,
            live_only,
        },
        config_warning,
        config_path,
    })
}

/// Read the repo path's busConfiguration.ts without hitting AWS. Used by the
/// settings screen to confirm the configured path is the right one.
#[tauri::command]
pub async fn preview_bus_configuration(path: String) -> Result<Vec<DeclaredBus>> {
    let file = PathBuf::from(crate::settings::shellexpand_home(&path))
        .join("stacks")
        .join("busConfiguration.ts");
    let content = std::fs::read_to_string(&file)
        .map_err(|e| Error::NotFound(format!("Could not read {}: {e}", file.display())))?;
    Ok(parse_bus_configuration(&content))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Trimmed from the real stacks/busConfiguration.ts, including the
    // line-wrapped sourceArn the formatter produces.
    const SAMPLE: &str = r#"
const busConfig: BusConfig = {
  trajectorBuses: {
    sandbox: [
      {
        sourceBusName: 'sandbox-bus-1',
        sourceArn: 'arn:aws:events:us-east-2:745266787460:event-bus/sandbox-bus-1',
      },
    ],
    dev: [
      {
        sourceBusName: 'dev-disability-api-event-bus',
        sourceArn:
          'arn:aws:events:us-east-2:362462262482:event-bus/dev-disability-api-event-bus',
      },
      {
        sourceBusName: 'milo-bus',
        sourceArn: 'arn:aws:events:us-east-2:085753910160:event-bus/milo-bus',
      },
    ],
    stg: [],
    prd: [],
  },
  thirdPartyBuses: {
    sandbox: [],
    dev: [
      {
        sourceBusName: 'aws.partner/example',
        sourceArn: 'arn:aws:events:us-east-2:111111111111:event-bus/aws.partner-example',
      },
    ],
    stg: [],
    prd: [],
  },
};
"#;

    #[test]
    fn parses_declared_buses_per_stage_and_category() {
        let buses = parse_bus_configuration(SAMPLE);

        let dev_trajector: Vec<_> = buses
            .iter()
            .filter(|b| b.stage == "dev" && b.category == "trajector")
            .collect();
        assert_eq!(dev_trajector.len(), 2);
        assert_eq!(dev_trajector[0].source_bus_name, "dev-disability-api-event-bus");
        assert_eq!(dev_trajector[1].source_bus_name, "milo-bus");

        assert_eq!(
            buses.iter().filter(|b| b.stage == "sandbox").count(),
            1,
            "sandbox has one trajector bus"
        );
        assert_eq!(
            buses.iter().filter(|b| b.category == "thirdParty").count(),
            1
        );
        // Empty stage arrays contribute nothing.
        assert_eq!(buses.iter().filter(|b| b.stage == "prd").count(), 0);
    }

    #[test]
    fn handles_line_wrapped_arns() {
        let buses = parse_bus_configuration(SAMPLE);
        let wrapped = buses
            .iter()
            .find(|b| b.source_bus_name == "dev-disability-api-event-bus")
            .expect("wrapped entry should parse");
        assert_eq!(
            wrapped.source_arn,
            "arn:aws:events:us-east-2:362462262482:event-bus/dev-disability-api-event-bus"
        );
    }

    #[test]
    fn extracts_account_and_region_from_arns() {
        let buses = parse_bus_configuration(SAMPLE);
        let milo = buses
            .iter()
            .find(|b| b.source_bus_name == "milo-bus")
            .unwrap();
        assert_eq!(milo.account_id.as_deref(), Some("085753910160"));
        assert_eq!(milo.region.as_deref(), Some("us-east-2"));
    }

    #[test]
    fn returns_empty_rather_than_failing_on_unexpected_shapes() {
        assert!(parse_bus_configuration("export const nothing = 1;").is_empty());
        assert!(parse_bus_configuration("").is_empty());
    }

    #[test]
    fn derives_target_service_from_arn() {
        assert_eq!(
            service_of("arn:aws:lambda:us-east-2:1:function:f").as_deref(),
            Some("lambda")
        );
        assert_eq!(
            service_of("arn:aws:events:us-east-2:1:event-bus/b").as_deref(),
            Some("events")
        );
    }
}
