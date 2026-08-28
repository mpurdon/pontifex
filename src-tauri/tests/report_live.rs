//! Exercises the registry-wide health report against live AWS.
//!
//! The report's whole design rests on one claim: a single log scan bucketed by
//! `source@detail-type` grades every schema, and cheaply. That is only
//! verifiable against a real registry with real traffic.
//!
//! Ignored by default. Run with:
//!   cargo test --test report_live -- --ignored --nocapture

use aws_config::BehaviorVersion;
use pontifex_lib::schema::events::check_events;
use serde_json::Value;
use std::collections::BTreeMap;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

#[test]
#[ignore]
fn grades_the_whole_registry_from_one_scan() {
    let profile = env("PONTIFEX_LIVE_PROFILE", "global-event-bus");
    let registry = env("PONTIFEX_LIVE_REGISTRY", "prd-global-registry");
    let log_group = env("PONTIFEX_LIVE_LOG_GROUP", "/aws/events/prd-global-events");
    let region = env("PONTIFEX_LIVE_REGION", "us-east-2");

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

        // --- one scan, bucketed --------------------------------------------
        let start_time =
            (time::OffsetDateTime::now_utc() - time::Duration::hours(24)).unix_timestamp() * 1000;

        let mut buckets: BTreeMap<String, Vec<Value>> = BTreeMap::new();
        let mut scanned = 0usize;
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;

        loop {
            let mut req = logs
                .filter_log_events()
                .log_group_name(&log_group)
                .start_time(start_time)
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
                let (Some(source), Some(detail_type)) = (
                    envelope.get("source").and_then(Value::as_str),
                    envelope.get("detail-type").and_then(Value::as_str),
                ) else {
                    continue;
                };
                scanned += 1;
                let bucket = buckets.entry(format!("{source}@{detail_type}")).or_default();
                if bucket.len() < 50 {
                    if let Some(detail) = envelope.get("detail") {
                        bucket.push(detail.clone());
                    }
                }
            }

            pages += 1;
            next_token = page.next_token().map(str::to_string);
            if next_token.is_none() || scanned >= 3_000 || pages >= 40 {
                break;
            }
        }

        println!(
            "scanned {scanned} events across {} event types in {pages} pages",
            buckets.len()
        );
        assert!(scanned > 0, "no events in {log_group} in the last 24h");

        // --- grade ----------------------------------------------------------
        let listed = schemas
            .list_schemas()
            .registry_name(&registry)
            .send()
            .await
            .expect("list schemas");
        let names: Vec<String> = listed
            .schemas()
            .iter()
            .filter_map(|s| s.schema_name())
            .map(str::to_string)
            .collect();

        let mut failing = 0usize;
        let mut drifting = 0usize;
        let mut ok = 0usize;
        let mut no_traffic = 0usize;
        let mut matched: Vec<String> = Vec::new();

        for name in names.iter().take(60) {
            let Some(payloads) = buckets.get(name) else {
                no_traffic += 1;
                continue;
            };
            matched.push(name.clone());

            let described = schemas
                .describe_schema()
                .registry_name(&registry)
                .schema_name(name)
                .send()
                .await
                .expect("describe schema");
            let content: Value =
                serde_json::from_str(described.content().unwrap_or("{}")).expect("schema JSON");

            let Some(type_name) = content
                .pointer("/components/schemas/AWSEvent/properties/detail/$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/components/schemas/"))
            else {
                continue;
            };

            let report = check_events(&content, type_name, payloads).expect("analysis");
            let drift = &report.drift;

            if report.failed > 0 {
                failing += 1;
                println!(
                    "  FAILING {name}: {}/{} events rejected — {}",
                    report.failed,
                    report.sampled,
                    report
                        .failures
                        .first()
                        .map(|f| f.message.as_str())
                        .unwrap_or("")
                );
            } else if !drift.undeclared.is_empty()
                || !drift.type_mismatches.is_empty()
                || !drift.enum_drift.is_empty()
            {
                drifting += 1;
                println!(
                    "  DRIFT   {name}: {} undeclared, {} type, {} enum",
                    drift.undeclared.len(),
                    drift.type_mismatches.len(),
                    drift.enum_drift.len()
                );
            } else {
                ok += 1;
            }
        }

        println!(
            "\ngraded: {failing} failing · {drifting} drifting · {ok} ok · {no_traffic} no traffic"
        );
        assert!(
            !matched.is_empty(),
            "no schema matched any bucketed event type — the bucketing key is wrong"
        );
    });
}
