//! Sampling a CloudWatch log group across a time window.
//!
//! `FilterLogEvents` paginates newest-first from the start time, so a scan that
//! stops at a cap has read a *contiguous, recent* slice of the window. Over a
//! busy log group that means the last few minutes, and every event type that
//! only fires overnight looks like it was never published at all — the report
//! then says "no traffic" about schemas that are perfectly healthy.
//!
//! Splitting the window into stripes and scanning them concurrently fixes both
//! halves of that: the sample is spread across the whole period rather than
//! bunched at one end, and the wall-clock cost drops because the stripes are
//! independent requests.

use crate::aws::clients::map_sdk_error;
use crate::error::Result;
use crate::events_cache::{parse_event, CachedEvent};
use crate::logging::cat;
use futures::stream::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// How many stripes the window is cut into, and how many run at once.
///
/// CloudWatch throttles `FilterLogEvents` per account, so this is deliberately
/// modest — the win comes from *where* the samples land, not from saturating
/// the API.
pub const DEFAULT_STRIPES: usize = 8;

/// Pages a single stripe may read before yielding to its neighbours.
const MAX_PAGES_PER_STRIPE: usize = 40;

/// A scan's time budget, in seconds. Matches the UI slider.
pub const BUDGET_CHOICES: [u64; 5] = [15, 30, 60, 90, 120];
pub const DEFAULT_BUDGET_SECONDS: u64 = 30;

pub struct ScanRequest {
    pub log_group: String,
    /// CloudWatch filter pattern; `None` scans everything in the window.
    pub pattern: Option<String>,
    pub start_time: i64,
    pub end_time: i64,
    /// Total events to collect across all stripes.
    pub max_events: usize,
    pub budget: Duration,
    pub stripes: usize,
}

pub struct ScanOutcome {
    /// `(source, detail-type, event)` for everything parsed.
    pub events: Vec<(String, String, CachedEvent)>,
    pub scanned: usize,
    pub pages: usize,
    /// True when any stripe stopped before exhausting its slice, so an absence
    /// in this sample is not evidence the event was never published.
    pub truncated: bool,
    /// Why it stopped, when it stopped early.
    pub note: Option<String>,
    pub elapsed_ms: u128,
    pub stripes: usize,
}

/// Cut `[start, end)` into `stripes` contiguous ranges.
///
/// Returned oldest-first. A window too small to divide yields a single range
/// rather than a pile of empty ones.
pub fn stripe_ranges(start: i64, end: i64, stripes: usize) -> Vec<(i64, i64)> {
    let span = end - start;
    if span <= 0 || stripes <= 1 {
        return vec![(start, end.max(start))];
    }
    // Sub-millisecond stripes would be pure overhead.
    let stripes = stripes.min(span.max(1) as usize).max(1);

    let width = span / stripes as i64;
    let mut ranges = Vec::with_capacity(stripes);
    for i in 0..stripes {
        let from = start + width * i as i64;
        // The last stripe absorbs the remainder, so the window is fully covered.
        let to = if i + 1 == stripes {
            end
        } else {
            start + width * (i as i64 + 1)
        };
        ranges.push((from, to));
    }
    ranges
}

/// Scan a log group across its window, striped and concurrent.
///
/// `on_progress` is called with `(scanned_so_far, elapsed)` as pages land, so
/// callers can report progress without threading a handle through here.
pub async fn scan_striped<F>(
    client: &aws_sdk_cloudwatchlogs::Client,
    request: ScanRequest,
    on_progress: F,
) -> Result<ScanOutcome>
where
    F: Fn(usize) + Send + Sync,
{
    let started = Instant::now();
    let deadline = started + request.budget;
    let ranges = stripe_ranges(request.start_time, request.end_time, request.stripes);
    let stripes = ranges.len();

    // Per-stripe budget, so one busy slice cannot spend the whole allowance and
    // leave the rest of the window unsampled — the entire point of striping.
    let per_stripe = request.max_events.div_ceil(stripes).max(1);

    let scanned = AtomicUsize::new(0);
    let pages_total = AtomicUsize::new(0);
    let capped_stripes = AtomicUsize::new(0);
    let hit_deadline = AtomicUsize::new(0);

    ldebug!(
        cat::EVENTS,
        "striped scan of {} — {stripes} stripes, {} events each, {}s budget",
        request.log_group,
        per_stripe,
        request.budget.as_secs()
    );

    let results: Vec<Result<Vec<(String, String, CachedEvent)>>> =
        futures::stream::iter(ranges.into_iter().enumerate())
            .map(|(index, (from, to))| {
                let client = client.clone();
                let log_group = request.log_group.clone();
                let pattern = request.pattern.clone();
                let scanned = &scanned;
                let pages_total = &pages_total;
                let capped_stripes = &capped_stripes;
                let hit_deadline = &hit_deadline;
                let on_progress = &on_progress;

                async move {
                    let mut collected = Vec::new();
                    let mut next_token: Option<String> = None;
                    let mut pages = 0usize;

                    loop {
                        if Instant::now() >= deadline {
                            hit_deadline.fetch_add(1, Ordering::Relaxed);
                            break;
                        }

                        let mut req = client
                            .filter_log_events()
                            .log_group_name(&log_group)
                            .start_time(from)
                            .end_time(to)
                            .limit(1_000);
                        if let Some(p) = &pattern {
                            req = req.filter_pattern(p);
                        }
                        if let Some(token) = &next_token {
                            req = req.next_token(token);
                        }

                        let page = req.send().await.map_err(map_sdk_error)?;

                        let before = collected.len();
                        for event in page.events() {
                            let Some(message) = event.message() else {
                                continue;
                            };
                            if let Some(parsed) = parse_event(
                                event.event_id().unwrap_or_default(),
                                event.timestamp().unwrap_or(0),
                                message,
                            ) {
                                collected.push(parsed);
                            }
                        }

                        pages += 1;
                        pages_total.fetch_add(1, Ordering::Relaxed);
                        // Only this page's yield, or every page would re-count
                        // everything the stripe had already collected.
                        let added = collected.len() - before;
                        on_progress(scanned.fetch_add(added, Ordering::Relaxed) + added);

                        next_token = page.next_token().map(str::to_string);
                        if next_token.is_none() {
                            break;
                        }
                        if collected.len() >= per_stripe || pages >= MAX_PAGES_PER_STRIPE {
                            capped_stripes.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }

                    ldebug!(
                        cat::EVENTS,
                        "stripe {index} ({from}..{to}) collected {} events over {pages} page(s)",
                        collected.len()
                    );
                    Ok(collected)
                }
            })
            .buffer_unordered(stripes)
            .collect()
            .await;

    let mut events = Vec::new();
    for result in results {
        events.extend(result?);
    }
    // Newest first, matching what every consumer expects.
    events.sort_by_key(|(_, _, e)| std::cmp::Reverse(e.timestamp));
    events.truncate(request.max_events);

    let capped = capped_stripes.load(Ordering::Relaxed);
    let timed_out = hit_deadline.load(Ordering::Relaxed);
    let elapsed_ms = started.elapsed().as_millis();
    let truncated = capped > 0 || timed_out > 0;

    let note = if timed_out > 0 {
        Some(format!(
            "Stopped after {}s: {timed_out} of {stripes} time slices were still reading. \
             This sample is spread across the whole window but is not complete — raise the \
             scan budget, or narrow the window.",
            request.budget.as_secs()
        ))
    } else if capped > 0 {
        Some(format!(
            "{capped} of {stripes} time slices hit their event cap. The sample covers the \
             whole window but not every event in it — raise the event cap for more depth."
        ))
    } else {
        None
    };

    Ok(ScanOutcome {
        // What the scan actually read, not what survived the cap — the
        // difference is exactly how much was thrown away.
        scanned: scanned.load(Ordering::Relaxed),
        events,
        pages: pages_total.load(Ordering::Relaxed),
        truncated,
        note,
        elapsed_ms,
        stripes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_window_into_contiguous_stripes() {
        let ranges = stripe_ranges(0, 800, 8);
        assert_eq!(ranges.len(), 8);
        assert_eq!(ranges[0], (0, 100));
        assert_eq!(ranges[7], (700, 800));
        // No gaps: each stripe starts where the previous ended.
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
    }

    #[test]
    fn the_last_stripe_absorbs_the_remainder() {
        // 1000 / 3 = 333 with 1 left over; dropping it would silently exclude
        // the newest millisecond of the window.
        let ranges = stripe_ranges(0, 1000, 3);
        assert_eq!(ranges.first().unwrap().0, 0);
        assert_eq!(ranges.last().unwrap().1, 1000);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].1, pair[1].0);
        }
    }

    #[test]
    fn covers_the_whole_window_for_every_stripe_count() {
        for stripes in 1..=16 {
            let ranges = stripe_ranges(1_700_000_000_000, 1_700_000_086_400_000, stripes);
            assert_eq!(ranges.first().unwrap().0, 1_700_000_000_000);
            assert_eq!(ranges.last().unwrap().1, 1_700_000_086_400_000);
            for pair in ranges.windows(2) {
                assert_eq!(pair[0].1, pair[1].0, "gap with {stripes} stripes");
            }
        }
    }

    #[test]
    fn a_single_stripe_is_the_whole_window() {
        assert_eq!(stripe_ranges(100, 900, 1), vec![(100, 900)]);
    }

    #[test]
    fn does_not_produce_empty_stripes_for_a_tiny_window() {
        // Asking for 8 stripes of a 3ms window would otherwise yield stripes of
        // width zero, each costing a request that can return nothing.
        let ranges = stripe_ranges(0, 3, 8);
        assert!(ranges.len() <= 3, "got {} stripes", ranges.len());
        assert!(ranges.iter().all(|(a, b)| b > a), "empty stripe in {ranges:?}");
        assert_eq!(ranges.last().unwrap().1, 3);
    }

    #[test]
    fn an_inverted_or_empty_window_yields_one_range() {
        assert_eq!(stripe_ranges(500, 500, 8), vec![(500, 500)]);
        assert_eq!(stripe_ranges(500, 100, 8), vec![(500, 500)]);
    }

    #[test]
    fn budget_choices_are_ascending_and_include_the_default() {
        assert!(BUDGET_CHOICES.windows(2).all(|w| w[0] < w[1]));
        assert!(BUDGET_CHOICES.contains(&DEFAULT_BUDGET_SECONDS));
    }
}
