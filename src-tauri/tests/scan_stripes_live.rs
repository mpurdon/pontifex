//! Does striping actually widen the sample?
//!
//! The claim behind the change is that a capped *linear* scan reads a
//! contiguous, recent slice of the window — so rare or overnight event types
//! read as "no traffic" — while a striped scan of the same budget spreads its
//! reads across the whole period. That is a claim about real log-group
//! behaviour and cannot be tested against a fixture.
//!
//! Uses the same in-app SSO credential resolution the app does, so no AWS
//! profile has to exist.
//!
//! Ignored by default. Run with:
//!   cargo test --test scan_stripes_live -- --ignored --nocapture

use aws_config::BehaviorVersion;
use pontifex_lib::aws::log_scan::{scan_striped, ScanRequest};
use pontifex_lib::aws::sso_credentials::{SsoRoleProvider, SsoTarget};
use std::collections::BTreeSet;
use std::time::Duration;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

/// Span between the oldest and newest event, in minutes.
fn coverage_minutes(timestamps: &[i64]) -> i64 {
    match (timestamps.iter().min(), timestamps.iter().max()) {
        (Some(lo), Some(hi)) => (hi - lo) / 60_000,
        _ => 0,
    }
}

#[test]
#[ignore]
fn striping_samples_more_of_the_window_than_a_linear_scan() {
    let session = env("PONTIFEX_LIVE_SESSION", "trajector");
    let account = env("PONTIFEX_LIVE_ACCOUNT", "333333333333");
    let role = env("PONTIFEX_LIVE_ROLE", "ReadOnlyAccess");
    let log_group = env("PONTIFEX_LIVE_LOG_GROUP", "/aws/events/prd-global-events");
    let region = env("PONTIFEX_LIVE_REGION", "us-east-2");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let cfg = aws_config::defaults(BehaviorVersion::latest())
            .region(aws_config::Region::new(region))
            .credentials_provider(SsoRoleProvider::new(SsoTarget {
                session,
                account_id: account,
                role_name: role,
                account_name: None,
            }))
            .load()
            .await;

        let logs = aws_sdk_cloudwatchlogs::Client::new(&cfg);

        let window_minutes = 24 * 60i64;
        let end = time::OffsetDateTime::now_utc().unix_timestamp() * 1000;
        let start = end - window_minutes * 60_000;
        // Deliberately small, so both scans are forced to stop early — the
        // whole question is *which* events they got, not how many.
        let budget_events: usize = env("PONTIFEX_LIVE_MAX_EVENTS", "1500")
            .parse()
            .expect("PONTIFEX_LIVE_MAX_EVENTS");

        // --- linear: one stripe, which is the old behaviour ----------------
        let linear = scan_striped(
            &logs,
            ScanRequest {
                log_group: log_group.clone(),
                pattern: None,
                start_time: start,
                end_time: end,
                max_events: budget_events,
                budget: Duration::from_secs(120),
                stripes: 1,
            },
            |_| {},
        )
        .await
        .expect("linear scan");

        // --- striped -------------------------------------------------------
        let striped = scan_striped(
            &logs,
            ScanRequest {
                log_group: log_group.clone(),
                pattern: None,
                start_time: start,
                end_time: end,
                max_events: budget_events,
                budget: Duration::from_secs(120),
                stripes: 8,
            },
            |_| {},
        )
        .await
        .expect("striped scan");

        let types_of = |events: &[(String, String, pontifex_lib::events_cache::CachedEvent)]| {
            events
                .iter()
                .map(|(s, d, _)| format!("{s}@{d}"))
                .collect::<BTreeSet<String>>()
        };
        let stamps_of = |events: &[(String, String, pontifex_lib::events_cache::CachedEvent)]| {
            events.iter().map(|(_, _, e)| e.timestamp).collect::<Vec<_>>()
        };

        let linear_types = types_of(&linear.events);
        let striped_types = types_of(&striped.events);
        let linear_cover = coverage_minutes(&stamps_of(&linear.events));
        let striped_cover = coverage_minutes(&stamps_of(&striped.events));

        println!("window: {window_minutes} minutes, cap {budget_events} events\n");
        println!(
            "linear : {:5} events, {:3} types, spans {:5} min, {:2} pages, {:5}ms",
            linear.events.len(),
            linear_types.len(),
            linear_cover,
            linear.pages,
            linear.elapsed_ms
        );
        println!(
            "striped: {:5} events, {:3} types, spans {:5} min, {:2} pages, {:5}ms",
            striped.events.len(),
            striped_types.len(),
            striped_cover,
            striped.pages,
            striped.elapsed_ms
        );

        let only_striped: Vec<&String> = striped_types.difference(&linear_types).collect();
        println!(
            "\nevent types the linear scan missed ({}):",
            only_striped.len()
        );
        for name in only_striped.iter().take(15) {
            println!("  {name}");
        }

        assert!(!striped.events.is_empty(), "expected live traffic to sample");
        assert!(
            striped_cover >= linear_cover,
            "striping should cover at least as much of the window: \
             striped {striped_cover} min vs linear {linear_cover} min"
        );
    });
}
