//! Reproduces the "Inferring a shape…" hang against the real payloads that
//! triggered it.
//!
//! The UI sat on that stage for 600+ seconds. Everything after the fetch is
//! synchronous, so either it is pathologically slow or it panics — and a panic
//! inside a `#[tauri::command]` leaves the IPC promise pending forever, which
//! looks exactly like a hang.

use gebman_lib::schema::{infer, model, validate};
use serde_json::Value;
use std::time::Instant;

fn payloads() -> Vec<Value> {
    let raw = include_str!("fixtures/atomic_payloads.json");
    serde_json::from_str(raw).expect("fixture parses")
}

#[test]
fn inferring_from_a_realistic_payload_shape_finishes_promptly() {
    let payloads = payloads();
    assert!(!payloads.is_empty(), "fixture should have events");

    let started = Instant::now();
    let detail = infer::infer_payload_schema(&payloads);
    let elapsed = started.elapsed();
    println!("infer over {} payloads took {elapsed:?}", payloads.len());
    assert!(
        elapsed.as_secs() < 5,
        "inference should be near-instant, took {elapsed:?}"
    );

    let identity = model::EventIdentity {
        source: "Atomic Forms".into(),
        detail_type: "CASE_STATUS_CHANGED".into(),
    };

    let started = Instant::now();
    let content = model::document_with_detail(&identity, detail);
    let report = validate::validate(&content, Some(&identity.schema_name()));
    let elapsed = started.elapsed();
    println!("document + validate took {elapsed:?}");
    println!("valid: {}, findings: {}", report.valid, report.findings.len());
    for f in &report.findings {
        println!("  [{:?}] {} — {}", f.severity, f.path, f.message);
    }
    assert!(
        elapsed.as_secs() < 5,
        "validation should be near-instant, took {elapsed:?}"
    );
}

#[test]
fn a_source_with_a_space_still_produces_a_usable_file_name() {
    let identity = model::EventIdentity {
        source: "Atomic Forms".into(),
        detail_type: "CASE_STATUS_CHANGED".into(),
    };
    let name = model::file_name_for(&identity);
    println!("file name: {name}");
    assert!(!name.is_empty());
}
