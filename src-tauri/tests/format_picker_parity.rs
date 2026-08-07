//! The format picker must offer exactly what the bus asserts.
//!
//! `STRING_FORMATS` in the inspector is a hand-maintained TypeScript copy of
//! `schema::ajv::FORMATS`, and the two had already drifted once: the picker was
//! missing `json-pointer-uri-fragment`, so a field declaring a format the bus
//! does enforce rendered as "none" and silently lost it on the next edit.
//!
//! Deriving the list over IPC would remove the duplication outright, but an
//! async round trip for a constant that changes when `ajv-formats` does is a
//! poor trade. This asserts the contract instead: cheap, and it fails loudly on
//! the next edit to either side.

use std::path::Path;

/// Formats `ajv-formats` scopes to numbers. Ajv never applies them to a string,
/// so the picker — which is only shown for `type: string` — rightly omits them.
const NUMERIC_ONLY: [&str; 4] = ["int32", "int64", "float", "double"];

fn picker_formats() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../src/components/schema-editor/inspector.tsx");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {e}", path.display()));

    let body = source
        .split_once("const STRING_FORMATS = [")
        .expect("inspector.tsx should declare STRING_FORMATS")
        .1
        .split_once(']')
        .expect("STRING_FORMATS should be a closed array")
        .0;

    body.split(',')
        .map(|entry| entry.trim().trim_matches('\'').to_string())
        .filter(|entry| !entry.is_empty())
        .collect()
}

#[test]
fn the_picker_offers_every_string_format_the_bus_asserts() {
    let offered = picker_formats();

    for name in gebman_lib::schema::ajv::asserted_format_names() {
        if NUMERIC_ONLY.contains(&name) {
            continue;
        }
        assert!(
            offered.iter().any(|o| o == name),
            "`{name}` is asserted by the bus but absent from the inspector's STRING_FORMATS, \
             so it cannot be picked and a field already declaring it renders as \"none\""
        );
    }
}

#[test]
fn the_picker_offers_nothing_the_bus_ignores() {
    for offered in picker_formats() {
        assert!(
            gebman_lib::schema::ajv::is_known_format(&offered),
            "the inspector offers `{offered}`, which the bus does not know — picking it would \
             write a constraint that enforces nothing, and the report would then flag it"
        );
        assert!(
            !NUMERIC_ONLY.contains(&offered.as_str()),
            "`{offered}` applies to numbers only; offering it on a string promises a check \
             that never runs"
        );
    }
}
