//! Infers schemas from real unregistered traffic and checks they hold up.
//!
//! The test that matters: a schema inferred from a sample must actually
//! validate the events it came from. If it does not, the drafts handed to users
//! are wrong on arrival.
//!
//! Ignored by default. Run with:
//!   cargo test --test infer_live -- --ignored --nocapture

use aws_config::BehaviorVersion;
use gebman_lib::schema::events::check_events;
use gebman_lib::schema::infer::infer_payload_schema;
use gebman_lib::schema::model::{document_with_detail, EventIdentity};
use gebman_lib::schema::validate;
use serde_json::Value;
use std::collections::BTreeMap;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

#[test]
#[ignore]
fn inferred_schemas_validate_the_events_they_came_from() {
    let profile = env("GEBMAN_LIVE_PROFILE", "global-event-bus");
    let registry = env("GEBMAN_LIVE_REGISTRY", "prd-global-registry");
    let log_group = env("GEBMAN_LIVE_LOG_GROUP", "/aws/events/prd-global-events");
    let region = env("GEBMAN_LIVE_REGION", "us-east-2");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let cfg = aws_config::defaults(BehaviorVersion::latest())
            .profile_name(&profile)
            .region(aws_config::Region::new(region))
            .load()
            .await;
        let logs = aws_sdk_cloudwatchlogs::Client::new(&cfg);
        let schemas = aws_sdk_schemas::Client::new(&cfg);

        // Which event types have no schema?
        let listed = schemas
            .list_schemas()
            .registry_name(&registry)
            .send()
            .await
            .expect("list schemas");
        let registered: std::collections::BTreeSet<String> = listed
            .schemas()
            .iter()
            .filter_map(|s| s.schema_name())
            .map(str::to_string)
            .collect();

        let start = (time::OffsetDateTime::now_utc() - time::Duration::hours(24)).unix_timestamp()
            * 1000;

        let mut buckets: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut req = logs
                .filter_log_events()
                .log_group_name(&log_group)
                .start_time(start)
                .limit(1_000);
            if let Some(t) = &next_token {
                req = req.next_token(t);
            }
            let page = req.send().await.expect("filter log events");

            for event in page.events() {
                let Some(message) = event.message() else { continue };
                let Ok(envelope) = serde_json::from_str::<Value>(message) else {
                    continue;
                };
                let (Some(source), Some(detail_type), Some(detail)) = (
                    envelope.get("source").and_then(Value::as_str),
                    envelope.get("detail-type").and_then(Value::as_str),
                    envelope.get("detail"),
                ) else {
                    continue;
                };
                let bucket = buckets.entry(format!("{source}@{detail_type}")).or_default();
                if bucket.len() < 50 {
                    bucket.push(detail.clone());
                }
            }

            pages += 1;
            next_token = page.next_token().map(str::to_string);
            if next_token.is_none() || pages >= 10 {
                break;
            }
        }

        let unregistered: Vec<(&String, &Vec<Value>)> = buckets
            .iter()
            .filter(|(name, _)| !registered.contains(*name))
            .collect();

        if unregistered.is_empty() {
            eprintln!("skipping: every event type on the bus already has a schema");
            return;
        }

        println!("{} unregistered event types\n", unregistered.len());

        let mut checked = 0usize;
        for (name, payloads) in unregistered.iter().take(8) {
            let Ok(identity) = EventIdentity::from_schema_name(name) else {
                continue;
            };

            let detail = infer_payload_schema(payloads);
            let document = document_with_detail(&identity, detail);

            // 1. The draft must pass our own structural validator.
            let report = validate::validate(&document, Some(name));
            assert!(
                report.valid,
                "inferred schema for {name} failed validation: {:?}",
                report.findings
            );

            // 2. And it must accept the very events it was inferred from.
            let type_name = identity.detail_title();
            let checked_report =
                check_events(&document, &type_name, payloads).expect("analysis");
            assert_eq!(
                checked_report.failed, 0,
                "inferred schema for {name} rejects its own source events: {:?}",
                checked_report.failures
            );
            assert!(
                checked_report.drift.undeclared.is_empty(),
                "inferred schema for {name} left fields undeclared: {:?}",
                checked_report
                    .drift
                    .undeclared
                    .iter()
                    .map(|f| &f.path)
                    .collect::<Vec<_>>()
            );

            let field_count = document
                .pointer(&format!("/components/schemas/{type_name}/properties"))
                .and_then(Value::as_object)
                .map(|m| m.len())
                .unwrap_or(0);
            println!(
                "  {name}: {} events → {field_count} top-level fields, validates cleanly",
                payloads.len()
            );
            checked += 1;
        }

        assert!(checked > 0, "no unregistered type could be inferred");
        println!("\nverified {checked} inferred schemas against their own traffic");
    });
}
