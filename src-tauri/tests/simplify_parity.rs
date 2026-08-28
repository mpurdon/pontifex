//! Parity check for the `simplifySchemas.js` port.
//!
//! The global-event-bus repo keeps both the full schemas and the generated
//! simplified ones under version control. That gives us a free, high-coverage
//! oracle: running our Rust port over `schemas/` must reproduce
//! `schemas-simplified/` exactly, for all ~292 real schemas.
//!
//! Skipped (not failed) when the repo is not checked out locally, so the test
//! suite stays green on a machine without it.

use pontifex_lib::schema::simplify::simplify_document;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Locate the global-event-bus checkout, honouring an env override so CI can
/// point at a different path.
fn repo_root() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GLOBAL_EVENT_BUS_PATH") {
        let path = PathBuf::from(path);
        return path.join("schemas").is_dir().then_some(path);
    }
    let candidate = dirs::home_dir()?
        .join("Projects")
        .join("trajector")
        .join("global-event-bus");
    candidate.join("schemas").is_dir().then_some(candidate)
}

fn read_json(path: &Path) -> Value {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

#[test]
fn simplify_reproduces_the_committed_simplified_schemas() {
    let Some(root) = repo_root() else {
        eprintln!(
            "skipping: global-event-bus checkout not found \
             (set GLOBAL_EVENT_BUS_PATH to run this test)"
        );
        return;
    };

    let full_dir = root.join("schemas");
    let simplified_dir = root.join("schemas-simplified");
    assert!(
        simplified_dir.is_dir(),
        "{} does not exist",
        simplified_dir.display()
    );

    let mut compared = 0usize;
    let mut mismatches: Vec<String> = Vec::new();
    // Files present in schemas/ but not schemas-simplified/ are not a failure
    // of the port — they just have not been regenerated in the repo.
    let mut ungenerated: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&full_dir).expect("read schemas/") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let file_name = path.file_name().unwrap().to_string_lossy().into_owned();

        let expected_path = simplified_dir.join(&file_name);
        if !expected_path.exists() {
            ungenerated.push(file_name);
            continue;
        }

        let original = read_json(&path);
        let expected = read_json(&expected_path);

        // The JS simplifies only the `content` document and copies the rest of
        // the file envelope through unchanged.
        let ours = simplify_document(&original["content"]);

        if ours != expected["content"] {
            mismatches.push(file_name);
        }
        compared += 1;
    }

    assert!(compared > 0, "no schema files were compared");

    if !ungenerated.is_empty() {
        eprintln!(
            "note: {} schema(s) have no committed simplified output: {}",
            ungenerated.len(),
            ungenerated.join(", ")
        );
    }

    assert!(
        mismatches.is_empty(),
        "{} of {compared} schemas did not match the committed simplified output: {}",
        mismatches.len(),
        mismatches.join(", ")
    );

    eprintln!("simplify parity verified across {compared} real schemas");
}
