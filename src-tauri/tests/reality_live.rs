//! End-to-end check of the reality-check pipeline against live AWS data.
//!
//! Exercises the real path: read a schema from the registry, pull matching
//! events out of CloudWatch with the same filter pattern the app builds, and
//! run the analysis. This is where a broken filter pattern shows up — a unit
//! test asserting the pattern string cannot tell you the API rejects it.
//!
//! Ignored by default because it needs live credentials. Run with:
//!   GEBMAN_LIVE_PROFILE=global-event-bus \
//!   GEBMAN_LIVE_REGISTRY=prd-global-registry \
//!   GEBMAN_LIVE_LOG_GROUP=/aws/events/prd-global-events \
//!   cargo test --test reality_live -- --ignored --nocapture

use aws_config::BehaviorVersion;
use gebman_lib::schema::events::check_events;
use serde_json::Value;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

/// The filter pattern the app builds. Kept in sync with `commands/reality.rs`;
/// the point of this test is proving CloudWatch accepts it.
fn filter_pattern(source: &str, detail_type: &str) -> String {
    format!("{{ $.source = \"{source}\" && $.detail-type = \"{detail_type}\" }}")
}

#[test]
#[ignore]
fn checks_a_real_schema_against_real_events() {
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
            .region(aws_config::Region::new(region.clone()))
            .load()
            .await;

        let schemas = aws_sdk_schemas::Client::new(&cfg);
        let logs = aws_sdk_cloudwatchlogs::Client::new(&cfg);

        // Find a schema that actually has traffic by trying several.
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
        assert!(!names.is_empty(), "registry {registry} is empty");

        let start_time =
            (time::OffsetDateTime::now_utc() - time::Duration::days(2)).unix_timestamp() * 1000;

        let mut checked = 0usize;
        for name in names.iter().take(25) {
            let Some((source, detail_type)) = name.split_once('@') else {
                continue;
            };

            let pattern = filter_pattern(source, detail_type);
            let page = logs
                .filter_log_events()
                .log_group_name(&log_group)
                .start_time(start_time)
                .filter_pattern(&pattern)
                .limit(50)
                .send()
                .await;

            // A rejected pattern is the failure this test exists to catch.
            let page = page.unwrap_or_else(|e| {
                panic!("CloudWatch rejected the app's filter pattern `{pattern}`: {e:?}")
            });

            let payloads: Vec<Value> = page
                .events()
                .iter()
                .filter_map(|e| e.message())
                .filter_map(|m| serde_json::from_str::<Value>(m).ok())
                .filter_map(|v| v.get("detail").cloned())
                .collect();

            if payloads.is_empty() {
                continue;
            }

            let described = schemas
                .describe_schema()
                .registry_name(&registry)
                .schema_name(name)
                .send()
                .await
                .expect("describe schema");
            let content: Value = serde_json::from_str(described.content().unwrap_or("{}"))
                .expect("schema content is JSON");

            // The payload type, as the command resolves it.
            let type_name = content
                .pointer("/components/schemas/AWSEvent/properties/detail/$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/components/schemas/"))
                .unwrap_or("")
                .to_string();
            if type_name.is_empty() {
                continue;
            }

            let report = check_events(&content, &type_name, &payloads)
                .expect("analysis should not error on a real schema");

            println!(
                "\n{name}\n  sampled {} · passed {} · failed {}\n  \
                 undeclared {} · unused {} · type mismatches {} · enum drift {}",
                report.sampled,
                report.passed,
                report.failed,
                report.drift.undeclared.len(),
                report.drift.unused.len(),
                report.drift.type_mismatches.len(),
                report.drift.enum_drift.len(),
            );
            for field in report.drift.undeclared.iter().take(5) {
                println!(
                    "    undeclared: {} ({}) in {}/{}",
                    field.path,
                    field.types.join("|"),
                    field.seen_in,
                    report.sampled
                );
            }
            for failure in report.failures.iter().take(3) {
                println!("    failure ×{}: {}", failure.count, failure.message);
            }

            assert_eq!(report.sampled, payloads.len());
            assert_eq!(report.passed + report.failed, report.sampled);
            // Coverage must be reported for every declared field.
            assert!(
                report.coverage.values().all(|c| *c <= report.sampled),
                "coverage cannot exceed the sample size"
            );

            checked += 1;
            if checked >= 3 {
                break;
            }
        }

        assert!(
            checked > 0,
            "no schema in {registry} had matching events in {log_group} — \
             cannot verify the pipeline"
        );
        println!("\nverified the pipeline against {checked} real schemas");
    });
}
