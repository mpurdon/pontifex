//! Run the validator over the real registry, when a copy is available.
//!
//! Fixtures answer "does the rule fire"; only the actual 292 documents answer
//! "is the report readable". A parity pass that emits one finding per
//! `nullable` would be correct and useless, and this is the only place that
//! difference shows up.
//!
//! Skipped when the global-event-bus checkout is absent — the documents are a
//! `scripts/downloadSchemas.js` artifact and are not tracked, so this cannot be
//! a hard dependency of the suite. Discovery matches `simplify_parity.rs`
//! exactly: an explicit `GLOBAL_EVENT_BUS_PATH`, else the usual checkout. A
//! second convention meant this test silently skipped for everyone whose
//! machine the other one already found.

use pontifex_lib::schema::model::SchemaFile;
use pontifex_lib::schema::validate;
use std::path::PathBuf;

/// Locate the global-event-bus checkout, honouring an env override so CI can
/// point at a different path.
fn schemas_dir() -> Option<PathBuf> {
    let root = match std::env::var("GLOBAL_EVENT_BUS_PATH") {
        Ok(path) => PathBuf::from(path),
        Err(_) => dirs::home_dir()?
            .join("Projects")
            .join("trajector")
            .join("global-event-bus"),
    };
    let schemas = root.join("schemas");
    schemas.is_dir().then_some(schemas)
}

#[test]
fn every_registry_document_produces_a_readable_report() {
    let Some(dir) = schemas_dir() else {
        eprintln!(
            "skipping: global-event-bus checkout not found \
             (set GLOBAL_EVENT_BUS_PATH to run this test)"
        );
        return;
    };

    let mut documents = 0usize;
    let mut worst = 0usize;
    let mut blocked: Vec<String> = Vec::new();

    for entry in std::fs::read_dir(&dir).expect("readable schemas dir") {
        let path = entry.expect("dir entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).expect("readable schema");
        // Read through `SchemaFile` rather than reaching for a `"content"` key
        // by hand: it is the type the app and the repo's own scripts agree on,
        // and `validate` unwraps the JSON-string form of `content` itself.
        let file: SchemaFile = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("{} is not a schema file: {e}", path.display()));

        let report = validate::validate(&file.content, None);
        documents += 1;
        worst = worst.max(report.findings.len());

        if !report.valid {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let errors: Vec<&str> = report
                .findings
                .iter()
                .filter(|f| f.severity == validate::Severity::Error)
                .map(|f| f.message.as_str())
                .collect();
            blocked.push(format!("{name}: {}", errors.join(" | ")));
        }
    }

    assert!(documents > 0, "no documents found in {}", dir.display());

    // The number that matters. Aggregation is what keeps a document using
    // `nullable` 60 times from producing 60 findings; without it this is in the
    // hundreds and the report is unusable.
    assert!(
        worst <= 25,
        "one document produced {worst} findings — the parity pass is aggregating badly"
    );

    // Not an assertion that the registry is clean — it may genuinely contain
    // documents the bus cannot compile, and that is the discovery. Printed so
    // `cargo test -- --nocapture` names them.
    if !blocked.is_empty() {
        eprintln!("{} of {documents} documents would not compile:", blocked.len());
        for line in &blocked {
            eprintln!("  {line}");
        }
    }
}
