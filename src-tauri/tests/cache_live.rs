//! Proves the event cache does what it exists for: a second check costs
//! nothing, and an overlapping refetch does not duplicate.
//!
//! Ignored by default. Run with:
//!   cargo test --test cache_live -- --ignored --nocapture

use aws_config::BehaviorVersion;
use gebman_lib::events_cache::{cache_key, parse_event, CachedEvent, EventCache};
use std::time::Instant;

fn env(name: &str, fallback: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| fallback.to_string())
}

#[test]
#[ignore]
fn caching_removes_the_second_fetch() {
    let profile = env("GEBMAN_LIVE_PROFILE", "global-event-bus");
    let log_group = env("GEBMAN_LIVE_LOG_GROUP", "/aws/events/prd-global-events");
    let region = env("GEBMAN_LIVE_REGION", "us-east-2");

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async {
        let dir = std::env::temp_dir().join(format!("gebman-cache-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cache = EventCache::load(&dir);

        let cfg = aws_config::defaults(BehaviorVersion::latest())
            .profile_name(&profile)
            .region(aws_config::Region::new(region))
            .load()
            .await;
        let logs = aws_sdk_cloudwatchlogs::Client::new(&cfg);

        let start = (time::OffsetDateTime::now_utc() - time::Duration::hours(6)).unix_timestamp()
            * 1000;

        // --- first fetch, from AWS -----------------------------------------
        let began = Instant::now();
        let page = logs
            .filter_log_events()
            .log_group_name(&log_group)
            .start_time(start)
            .limit(300)
            .send()
            .await
            .expect("filter log events");
        let fetch_ms = began.elapsed().as_millis();

        let mut by_type: std::collections::BTreeMap<String, Vec<CachedEvent>> = Default::default();
        for event in page.events() {
            let Some(message) = event.message() else { continue };
            if let Some((source, detail_type, cached)) = parse_event(
                event.event_id().unwrap_or_default(),
                event.timestamp().unwrap_or(0),
                message,
            ) {
                by_type
                    .entry(format!("{source}@{detail_type}"))
                    .or_default()
                    .push(cached);
            }
        }
        assert!(!by_type.is_empty(), "no events in {log_group} in the last 6h");

        let now = gebman_lib::events_cache::now_ms();
        let (type_key, events) = by_type.iter().next().unwrap();
        let (source, detail_type) = type_key.split_once('@').unwrap();
        let key = cache_key("prd", &log_group, source, detail_type);

        cache
            .merge(&key, events.clone(), start, now, false)
            .await;
        cache.persist().await;

        // --- second read, from cache ---------------------------------------
        let began = Instant::now();
        let sample = cache.get(&key).await.expect("cached");
        let cached_payloads = sample.events_in(start, now);
        let cache_us = began.elapsed().as_micros();

        assert_eq!(cached_payloads.len(), events.len());
        assert!(
            sample.covers(start, now),
            "a clean fetch should mark the window covered"
        );

        println!(
            "{type_key}: {} events\n  AWS fetch {fetch_ms} ms → cache read {cache_us} µs",
            events.len()
        );

        // --- overlapping refetch must not duplicate -------------------------
        cache.merge(&key, events.clone(), start, now, false).await;
        let after = cache.get(&key).await.unwrap();
        assert_eq!(
            after.events.len(),
            events.len(),
            "re-merging the same events must dedupe by id"
        );

        // --- and it survives a restart --------------------------------------
        cache.persist().await;
        let reloaded = EventCache::load(&dir);
        assert_eq!(
            reloaded.get(&key).await.unwrap().events.len(),
            events.len(),
            "cache should survive a reload"
        );

        let (types, total) = reloaded.stats().await;
        println!("  persisted {types} event type(s), {total} events");

        let _ = std::fs::remove_dir_all(&dir);
    });
}
