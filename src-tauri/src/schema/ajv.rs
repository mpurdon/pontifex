//! The global-event-bus validator, reproduced.
//!
//! pontifex exists to manage the schemas that bus enforces, so a verdict here has
//! to be the verdict there. "Roughly the same rules" is worse than no tool at
//! all, because people act on it: a schema pontifex calls fine that the bus
//! rejects turns into a page at 3am, and a schema pontifex calls broken that the
//! bus accepts turns into a change nobody needed to make.
//!
//! What is being matched, from `packages/core/src/schemaValidator.ts`:
//!
//! ```js
//! this.ajv = new Ajv({ allErrors: true, strict: 'log', validateFormats: true });
//! addFormats(this.ajv);
//! ```
//!
//! Three consequences, each of which this module has to reproduce deliberately
//! because the Rust [`jsonschema`] crate defaults the other way:
//!
//! 1. **Draft-07.** Ajv 8's default export carries only the draft-07
//!    meta-schema. The crate defaults to 2020-12 when `$schema` is absent, and
//!    the two disagree about `dependencies`, `$ref` siblings, `items`, and
//!    `exclusiveMinimum`. [`options`] pins draft-07.
//! 2. **Formats are assertions.** `validateFormats: true` plus `ajv-formats`
//!    means `format` rejects, rather than annotating. The crate asserts formats
//!    under draft-07 too, but its *set* of formats is the JSON Schema spec's,
//!    not `ajv-formats`'. [`FORMATS`] closes that gap in both directions.
//! 3. **`strict: 'log'`.** Unknown keywords and unknown formats are logged and
//!    then ignored — they constrain nothing. That includes OpenAPI's
//!    `nullable`, which is why [`crate::schema::validate`] reports it rather
//!    than honouring it.
//!
//! The format checks below are ported from `ajv-formats`' `fullFormats` —
//! `addFormats(ajv)` with no options selects full mode, not fast. They are
//! deliberately transcribed rather than reimplemented; where a check looks odd,
//! it is odd in `ajv-formats` too, and matching it is the point.

use crate::error::{Error, Result};
use fancy_regex::Regex;
use jsonschema::{Draft, Validator};
use serde_json::Value;
use std::sync::LazyLock;

/// The `$schema` Ajv 8's default export can compile.
///
/// Both spellings; draft-07 predates the `https://` move.
const DRAFT_07_IDS: [&str; 2] = [
    "http://json-schema.org/draft-07/schema#",
    "https://json-schema.org/draft-07/schema#",
];

/// Build a validator that grades the way the bus grades.
///
/// Prefer this over [`jsonschema::validator_for`] everywhere in pontifex: the
/// latter picks 2020-12 and the spec's format set, neither of which the bus
/// does.
pub fn validator_for(schema: &Value) -> Result<Validator> {
    // Ajv resolves `$schema` against its own registry, which holds draft-07 and
    // nothing else, so a document declaring any other dialect fails to compile
    // there. The crate would instead happily switch dialects and grade under
    // rules the bus never applies, which is exactly the silent divergence this
    // module exists to prevent.
    if let Some(dialect) = foreign_dialect(schema) {
        return Err(Error::Invalid(foreign_dialect_message(&dialect)));
    }
    // `nullable` is the one OpenAPI keyword Ajv honours and this crate does
    // not; widened here, on a copy, so every grade agrees with the bus.
    compile(&crate::schema::openapi::widen_nullable(schema))
}

/// Compile without re-scanning for a foreign dialect.
///
/// For callers that have already scanned the enclosing document —
/// [`crate::schema::validate`] checks the whole thing once, then compiles each
/// component, and repeating the scan per component walked the document N more
/// times to reach a verdict it already had.
pub fn compile(schema: &Value) -> Result<Validator> {
    AJV_OPTIONS
        .build(schema)
        .map_err(|e| Error::Invalid(e.to_string()))
}

/// Why a non-draft-07 `$schema` is fatal rather than cosmetic.
///
/// One wording, because the editor and the compiler both have to say it and
/// two copies of a 40-word explanation only stay in agreement by luck.
pub fn foreign_dialect_message(dialect: &str) -> String {
    format!(
        "`$schema` is '{dialect}'. The bus compiles with Ajv's draft-07 meta-schema and \
         would throw on load, so no event of this type would ever be graded. Remove \
         `$schema`, or set it to '{}'.",
        DRAFT_07_IDS[0]
    )
}

/// Is this `$schema` one Ajv 8's default export can compile?
///
/// Tolerates the trailing `#`, which is optional in practice and written both
/// ways in the wild.
pub fn is_draft_07(id: &str) -> bool {
    let normalised = id.trim_end_matches('#');
    DRAFT_07_IDS
        .iter()
        .any(|d| d.trim_end_matches('#') == normalised)
}

/// The `$schema` value found anywhere in `schema`, when it is not draft-07.
///
/// Nested rather than root-only: a component schema carrying its own `$schema`
/// is just as unloadable, and these documents are assembled from parts.
///
/// Blind rather than structural, unlike [`crate::schema::openapi::walk_schemas`]:
/// this is the guard on [`validator_for`], where refusing to compile something
/// questionable is the safe direction. The editor's report uses the structural
/// walk instead, so it does not flag a `$schema` sitting in an `example`.
pub fn foreign_dialect(schema: &Value) -> Option<String> {
    match schema {
        Value::Object(map) => {
            if let Some(Value::String(id)) = map.get("$schema") {
                if !is_draft_07(id) {
                    return Some(id.clone());
                }
            }
            map.values().find_map(foreign_dialect)
        }
        Value::Array(items) => items.iter().find_map(foreign_dialect),
        _ => None,
    }
}

/// Compilation options equivalent to the bus's `Ajv` instance.
///
/// Built once. `with_format` allocates a `String` key and an `Arc` per entry,
/// and a validator is compiled per component schema — 61 of them in the
/// largest registry document — on every keystroke, so rebuilding this map each
/// time was thousands of allocations per edit for a table that never changes.
static AJV_OPTIONS: LazyLock<jsonschema::ValidationOptions> = LazyLock::new(|| {
    let mut opts = jsonschema::options()
        .with_draft(Draft::Draft7)
        // Belt and braces: draft-07 asserts formats in this crate anyway, but
        // saying so means a future draft change cannot quietly turn `format`
        // back into an annotation while the bus still rejects on it.
        .should_validate_formats(true)
        // `strict: 'log'` logs an unknown format and moves on. Reporting it is
        // `validate::check_ajv_parity`'s job, not the validator's.
        .should_ignore_unknown_formats(true);

    for entry in FORMATS {
        opts = opts.with_format(entry.name, entry.check());
    }
    opts
});

/// How the bus treats one `format`, and therefore how pontifex must.
///
/// The distinctions used to live in prose beside the table while
/// `is_known_format` and the numeric-format list re-derived them from
/// hardcoded name lists — three encodings of one fact, which had already
/// drifted. Naming the provenance makes each row state its own consequences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provenance {
    /// `ajv-formats` asserts it against strings. Ported below.
    Asserted,
    /// `ajv-formats` defines it as `true`: present, never failing.
    AlwaysTrue,
    /// Declared `{type: "number"}` in `ajv-formats`, with a real check. Ajv
    /// applies it to numbers and never to strings; this crate only ever hands
    /// `format` a string, so the check is structurally unreachable here.
    NumericOnly,
    /// Declared `{type: "number"}`, but the check is `validateNumber`, which
    /// always returns true. Nothing is lost by not reproducing it — which is
    /// why `float` and `double` are not reported alongside `int32`/`int64`.
    NumericNoop,
    /// Not in `ajv-formats` at all. Ajv logs "unknown format" and ignores it,
    /// while this crate implements it and would reject values the bus waves
    /// through — so it has to be neutralised explicitly.
    NotInAjv,
}

type FormatCheck = fn(&str) -> bool;

/// One `format` name, and everything that follows from where it comes from.
struct Format {
    name: &'static str,
    provenance: Provenance,
    /// The ported check. Only consulted when [`Provenance::Asserted`].
    assert: FormatCheck,
}

impl Format {
    /// What to register with the compiler.
    ///
    /// Everything that is not asserted against strings must pass every string,
    /// which is exactly what Ajv does with it.
    fn check(&self) -> FormatCheck {
        match self.provenance {
            Provenance::Asserted => self.assert,
            _ => accept_all,
        }
    }
}

const fn asserted(name: &'static str, assert: FormatCheck) -> Format {
    Format {
        name,
        provenance: Provenance::Asserted,
        assert,
    }
}

const fn other(name: &'static str, provenance: Provenance) -> Format {
    Format {
        name,
        provenance,
        assert: accept_all,
    }
}

/// Every format the bus's validator has an opinion about.
///
/// Registered wholesale rather than only where the crate disagrees, so there is
/// one place to read what a format means here. `with_format` takes precedence
/// over the crate's built-ins, so this table *is* the behaviour.
static FORMATS: &[Format] = &[
    asserted("date", is_date),
    asserted("time", is_time_strict_tz),
    asserted("date-time", is_date_time_strict_tz),
    asserted("iso-time", is_iso_time),
    asserted("iso-date-time", is_iso_date_time),
    asserted("duration", is_duration),
    asserted("uri", is_uri),
    asserted("uri-reference", is_uri_reference),
    asserted("uri-template", is_uri_template),
    asserted("url", is_url),
    asserted("email", is_email),
    asserted("hostname", is_hostname),
    asserted("ipv4", is_ipv4),
    asserted("ipv6", is_ipv6),
    asserted("regex", is_regex),
    asserted("uuid", is_uuid),
    asserted("json-pointer", is_json_pointer),
    asserted("json-pointer-uri-fragment", is_json_pointer_uri_fragment),
    asserted("relative-json-pointer", is_relative_json_pointer),
    asserted("byte", is_byte),
    other("int32", Provenance::NumericOnly),
    other("int64", Provenance::NumericOnly),
    other("float", Provenance::NumericNoop),
    other("double", Provenance::NumericNoop),
    other("password", Provenance::AlwaysTrue),
    other("binary", Provenance::AlwaysTrue),
    other("iri", Provenance::NotInAjv),
    other("iri-reference", Provenance::NotInAjv),
    other("idn-email", Provenance::NotInAjv),
    other("idn-hostname", Provenance::NotInAjv),
];

fn provenance(name: &str) -> Option<Provenance> {
    FORMATS
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.provenance)
}

/// Does `ajv-formats` define this name at all?
///
/// Anything outside this set is a format the bus logs and ignores, so declaring
/// it constrains nothing at runtime — worth saying in the editor.
pub fn is_known_format(name: &str) -> bool {
    !matches!(provenance(name), None | Some(Provenance::NotInAjv))
}

/// Every format name the bus asserts, rather than logs and ignores.
///
/// Exposed so the editor's format picker can be checked against it — see
/// `tests/format_picker_parity.rs`.
pub fn asserted_format_names() -> impl Iterator<Item = &'static str> {
    FORMATS
        .iter()
        .filter(|f| f.provenance != Provenance::NotInAjv)
        .map(|f| f.name)
}

/// Does the bus assert this format on numbers, where pontifex cannot follow?
///
/// Surfaced by the editor so the gap is stated rather than discovered.
pub fn is_unreproducible_format(name: &str) -> bool {
    provenance(name) == Some(Provenance::NumericOnly)
}

fn accept_all(_: &str) -> bool {
    true
}

// ---------------------------------------------------------------------------
// Ported checks
//
// Regexes are transcribed from ajv-formats' `fullFormats` with two mechanical
// substitutions: `/i` becomes a leading `(?i)`, and `\d` becomes `[0-9]`.
// The second is not cosmetic — JavaScript's `\d` is ASCII-only while Rust's is
// every Unicode decimal digit, so leaving it alone would accept Arabic-Indic
// digits in an IP address that the bus rejects.
// ---------------------------------------------------------------------------

/// Compile once; these run on every keystroke through the reality check.
///
/// Takes anything string-like so a `format!`-assembled pattern reaches it
/// without a second binding — three of the ports share sub-patterns.
fn compiled(pattern: impl AsRef<str>) -> Regex {
    Regex::new(pattern.as_ref()).expect("ported ajv-formats pattern must compile")
}

/// A format that is exactly "does this match the transcribed pattern".
///
/// Thirteen of the ports were character-for-character identical but for the
/// pattern, and each hand-written `.unwrap_or(false)` was a chance to type
/// `true` and silently accept everything for one format. The pattern stays
/// verbatim, which is the point of the module; only the wrapper is shared.
macro_rules! regex_format {
    ($(#[$meta:meta])* $name:ident, $pattern:expr) => {
        $(#[$meta])*
        fn $name(s: &str) -> bool {
            static RE: LazyLock<Regex> = LazyLock::new(|| compiled($pattern));
            RE.is_match(s).unwrap_or(false)
        }
    };
}

fn is_leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const DAYS: [u32; 13] = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

/// RFC 3339 `full-date`, calendar-aware — `2026-02-30` is not a date.
fn is_date(s: &str) -> bool {
    static DATE: LazyLock<Regex> =
        LazyLock::new(|| compiled(r"^([0-9][0-9][0-9][0-9])-([0-9][0-9])-([0-9][0-9])$"));
    let Ok(Some(caps)) = DATE.captures(s) else {
        return false;
    };
    let part = |i: usize| {
        caps.get(i)
            .map_or(0, |m| m.as_str().parse::<u32>().unwrap_or(0))
    };
    let (year, month, day) = (part(1), part(2), part(3));
    (1..=12).contains(&month)
        && day >= 1
        && day
            <= if month == 2 && is_leap_year(year) {
                29
            } else {
                DAYS[month as usize]
            }
}

/// `ajv-formats`' `TIME`, which also yields the offset so leap seconds can be
/// checked against UTC rather than local time.
static TIME: LazyLock<Regex> = LazyLock::new(|| {
    compiled(
        r"(?i)^([0-9][0-9]):([0-9][0-9]):([0-9][0-9](?:\.[0-9]+)?)(z|([+-])([0-9][0-9])(?::?([0-9][0-9]))?)?$",
    )
});

/// RFC 3339 `full-time`. `strict_time_zone` is what separates `time` (offset
/// required) from `iso-time` (offset optional).
fn check_time(s: &str, strict_time_zone: bool) -> bool {
    let Ok(Some(caps)) = TIME.captures(s) else {
        return false;
    };
    let num = |i: usize| {
        caps.get(i)
            .map_or(0.0, |m| m.as_str().parse::<f64>().unwrap_or(0.0))
    };
    let hr = num(1);
    let min = num(2);
    let sec = num(3);
    let tz = caps.get(4);
    let tz_sign = if caps.get(5).map(|m| m.as_str()) == Some("-") {
        -1.0
    } else {
        1.0
    };
    let tz_h = num(6);
    let tz_m = num(7);

    if tz_h > 23.0 || tz_m > 59.0 || (strict_time_zone && tz.is_none()) {
        return false;
    }
    if hr <= 23.0 && min <= 59.0 && sec < 60.0 {
        return true;
    }
    // A leap second is only legal at 23:59:60 UTC, so the offset has to come
    // off before the hour and minute mean anything.
    let utc_min = min - tz_m * tz_sign;
    let utc_hr = hr - tz_h * tz_sign - if utc_min < 0.0 { 1.0 } else { 0.0 };
    (utc_hr == 23.0 || utc_hr == -1.0) && (utc_min == 59.0 || utc_min == -1.0) && sec < 61.0
}

fn is_time_strict_tz(s: &str) -> bool {
    check_time(s, true)
}

fn is_iso_time(s: &str) -> bool {
    check_time(s, false)
}

/// Split on `t`, `T`, or whitespace, the way `String.prototype.split` does with
/// `ajv-formats`' `/t|\s/i` — every occurrence, so two separators means three
/// parts and a rejection.
fn split_date_time(s: &str) -> Vec<&str> {
    s.split(|c: char| c == 't' || c == 'T' || c.is_whitespace() || c == '\u{feff}')
        .collect()
}

fn check_date_time(s: &str, strict_time_zone: bool) -> bool {
    let parts = split_date_time(s);
    parts.len() == 2 && is_date(parts[0]) && check_time(parts[1], strict_time_zone)
}

fn is_date_time_strict_tz(s: &str) -> bool {
    check_date_time(s, true)
}

fn is_iso_date_time(s: &str) -> bool {
    check_date_time(s, false)
}

regex_format!(
    is_duration,
    r"^P(?!$)(([0-9]+Y)?([0-9]+M)?([0-9]+D)?(T(?=[0-9])([0-9]+H)?([0-9]+M)?([0-9]+S)?)?|([0-9]+W)?)$"
);

/// The body of `ajv-formats`' `URI`, shared with `uri-reference`.
///
/// One string rather than two literals because the two patterns differ only in
/// whether the scheme is required and which characters the authority allows;
/// transcribing 2kB of alternation twice is how they drift apart.
const IPV6_IN_URI: &str = r"(?:(?:(?:(?:[0-9a-f]{1,4}:){6}|::(?:[0-9a-f]{1,4}:){5}|(?:[0-9a-f]{1,4})?::(?:[0-9a-f]{1,4}:){4}|(?:(?:[0-9a-f]{1,4}:){0,1}[0-9a-f]{1,4})?::(?:[0-9a-f]{1,4}:){3}|(?:(?:[0-9a-f]{1,4}:){0,2}[0-9a-f]{1,4})?::(?:[0-9a-f]{1,4}:){2}|(?:(?:[0-9a-f]{1,4}:){0,3}[0-9a-f]{1,4})?::[0-9a-f]{1,4}:|(?:(?:[0-9a-f]{1,4}:){0,4}[0-9a-f]{1,4})?::)(?:[0-9a-f]{1,4}:[0-9a-f]{1,4}|(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?))|(?:(?:[0-9a-f]{1,4}:){0,5}[0-9a-f]{1,4})?::[0-9a-f]{1,4}|(?:(?:[0-9a-f]{1,4}:){0,6}[0-9a-f]{1,4})?::)|[Vv][0-9a-f]+\.[a-z0-9\-._~!$&'()*+,;=:]+)";

const IPV4_IN_URI: &str =
    r"(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)";

fn is_uri(s: &str) -> bool {
    static NOT_URI_FRAGMENT: LazyLock<Regex> = LazyLock::new(|| compiled(r"/|:"));
    static URI: LazyLock<Regex> = LazyLock::new(|| {
        compiled(format!(
            r"(?i)^(?:[a-z][a-z0-9+\-.]*:)(?:/?/(?:(?:[a-z0-9\-._~!$&'()*+,;=:]|%[0-9a-f]{{2}})*@)?(?:\[{ipv6}\]|{ipv4}|(?:[a-z0-9\-._~!$&'()*+,;=]|%[0-9a-f]{{2}})*)(?::[0-9]*)?(?:/(?:[a-z0-9\-._~!$&'()*+,;=:@]|%[0-9a-f]{{2}})*)*|/(?:(?:[a-z0-9\-._~!$&'()*+,;=:@]|%[0-9a-f]{{2}})+(?:/(?:[a-z0-9\-._~!$&'()*+,;=:@]|%[0-9a-f]{{2}})*)*)?|(?:[a-z0-9\-._~!$&'()*+,;=:@]|%[0-9a-f]{{2}})+(?:/(?:[a-z0-9\-._~!$&'()*+,;=:@]|%[0-9a-f]{{2}})*)*)(?:\?(?:[a-z0-9\-._~!$&'()*+,;=:@/?]|%[0-9a-f]{{2}})*)?(?:\#(?:[a-z0-9\-._~!$&'()*+,;=:@/?]|%[0-9a-f]{{2}})*)?$",
            ipv6 = IPV6_IN_URI,
            ipv4 = IPV4_IN_URI,
        ))
    });
    NOT_URI_FRAGMENT.is_match(s).unwrap_or(false) && URI.is_match(s).unwrap_or(false)
}

regex_format!(
    /// Note the `"` inside several classes: it is in `ajv-formats`' pattern and
    /// not in its `uri` twin, so a `"` is legal in a reference and not in a URI.
    is_uri_reference,
    format!(
        r#"(?i)^(?:[a-z][a-z0-9+\-.]*:)?(?://(?:(?:[a-z0-9\-._~!$&'()*+,;=:]|%[0-9a-f]{{2}})*@)?(?:\[{ipv6}\]|{ipv4}|(?:[a-z0-9\-._~!$&'"()*+,;=]|%[0-9a-f]{{2}})*)(?::[0-9]*)?(?:/(?:[a-z0-9\-._~!$&'"()*+,;=:@]|%[0-9a-f]{{2}})*)*|/(?:(?:[a-z0-9\-._~!$&'"()*+,;=:@]|%[0-9a-f]{{2}})+(?:/(?:[a-z0-9\-._~!$&'"()*+,;=:@]|%[0-9a-f]{{2}})*)*)?|(?:[a-z0-9\-._~!$&'"()*+,;=:@]|%[0-9a-f]{{2}})+(?:/(?:[a-z0-9\-._~!$&'"()*+,;=:@]|%[0-9a-f]{{2}})*)*)?(?:\?(?:[a-z0-9\-._~!$&'"()*+,;=:@/?]|%[0-9a-f]{{2}})*)?(?:\#(?:[a-z0-9\-._~!$&'"()*+,;=:@/?]|%[0-9a-f]{{2}})*)?$"#,
        ipv6 = IPV6_IN_URI,
        ipv4 = IPV4_IN_URI,
    )
);

regex_format!(
    is_uri_template,
    r##"(?i)^(?:(?:[^\x00-\x20"'<>%\\^`{|}]|%[0-9a-f]{2})|\{[+#./;?&=,!@|]?(?:[a-z0-9_]|%[0-9a-f]{2})+(?::[1-9][0-9]{0,3}|\*)?(?:,(?:[a-z0-9_]|%[0-9a-f]{2})+(?::[1-9][0-9]{0,3}|\*)?)*\})*$"##
);

regex_format!(
    is_url,
    r"(?i)^(?:https?|ftp)://(?:\S+(?::\S*)?@)?(?:(?!(?:10|127)(?:\.[0-9]{1,3}){3})(?!(?:169\.254|192\.168)(?:\.[0-9]{1,3}){2})(?!172\.(?:1[6-9]|2[0-9]|3[0-1])(?:\.[0-9]{1,3}){2})(?:[1-9][0-9]?|1[0-9][0-9]|2[01][0-9]|22[0-3])(?:\.(?:1?[0-9]{1,2}|2[0-4][0-9]|25[0-5])){2}(?:\.(?:[1-9][0-9]?|1[0-9][0-9]|2[0-4][0-9]|25[0-4]))|(?:(?:[a-z0-9\u{00a1}-\u{ffff}]+-)*[a-z0-9\u{00a1}-\u{ffff}]+)(?:\.(?:[a-z0-9\u{00a1}-\u{ffff}]+-)*[a-z0-9\u{00a1}-\u{ffff}]+)*(?:\.(?:[a-z\u{00a1}-\u{ffff}]{2,})))(?::[0-9]{2,5})?(?:/[^\s]*)?$"
);

regex_format!(
    is_email,
    r"(?i)^[a-z0-9!#$%&'*+/=?^_`{|}~-]+(?:\.[a-z0-9!#$%&'*+/=?^_`{|}~-]+)*@(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$"
);

regex_format!(
    is_hostname,
    r"(?i)^(?=.{1,253}\.?$)[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?(?:\.[a-z0-9](?:[-0-9a-z]{0,61}[0-9a-z])?)*\.?$"
);

regex_format!(
    is_ipv4,
    r"^(?:(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])\.){3}(?:25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])$"
);

/// `ajv-formats` writes the dotted-quad tail out eight times; keeping the shape
/// identical is what makes this checkable against the original.
const IPV4_IN_IPV6: &str = r"((25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])(\.(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])){3})";

regex_format!(
    is_ipv6,
    format!(
        r"(?i)^((([0-9a-f]{{1,4}}:){{7}}([0-9a-f]{{1,4}}|:))|(([0-9a-f]{{1,4}}:){{6}}(:[0-9a-f]{{1,4}}|{v4}|:))|(([0-9a-f]{{1,4}}:){{5}}(((:[0-9a-f]{{1,4}}){{1,2}})|:{v4}|:))|(([0-9a-f]{{1,4}}:){{4}}(((:[0-9a-f]{{1,4}}){{1,3}})|((:[0-9a-f]{{1,4}})?:{v4})|:))|(([0-9a-f]{{1,4}}:){{3}}(((:[0-9a-f]{{1,4}}){{1,4}})|((:[0-9a-f]{{1,4}}){{0,2}}:{v4})|:))|(([0-9a-f]{{1,4}}:){{2}}(((:[0-9a-f]{{1,4}}){{1,5}})|((:[0-9a-f]{{1,4}}){{0,3}}:{v4})|:))|(([0-9a-f]{{1,4}}:){{1}}(((:[0-9a-f]{{1,4}}){{1,6}})|((:[0-9a-f]{{1,4}}){{0,4}}:{v4})|:))|(:(((:[0-9a-f]{{1,4}}){{1,7}})|((:[0-9a-f]{{1,4}}){{0,5}}:{v4})|:)))$",
        v4 = IPV4_IN_IPV6,
    )
);

/// Does the string compile as a regular expression?
///
/// `ajv-formats` rejects `\Z` first — Perl's end-of-string anchor, which JS
/// silently reads as a literal `Z` — then asks `new RegExp`. `fancy_regex` is
/// the closest thing available: like JS and unlike the plain `regex` crate it
/// supports lookaround and backreferences, so patterns the bus accepts are not
/// rejected here for being expressible.
fn is_regex(s: &str) -> bool {
    static Z_ANCHOR: LazyLock<Regex> = LazyLock::new(|| compiled(r"[^\\]\\Z"));
    if Z_ANCHOR.is_match(s).unwrap_or(false) {
        return false;
    }
    Regex::new(s).is_ok()
}

regex_format!(
    is_uuid,
    r"(?i)^(?:urn:uuid:)?[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$"
);

regex_format!(is_json_pointer, r"^(?:/(?:[^~/]|~0|~1)*)*$");

regex_format!(
    is_json_pointer_uri_fragment,
    r"(?i)^\#(?:/(?:[a-z0-9_\-.!$&'()*+,;:=@]|%[0-9a-f]{2}|~0|~1)*)*$"
);

regex_format!(
    is_relative_json_pointer,
    r"^(?:0|[1-9][0-9]*)(?:\#|(?:/(?:[^~/]|~0|~1)*)*)$"
);

regex_format!(
    /// Base64, `ajv-formats`-style — including the multiline flag it carries.
    ///
    /// `/m` makes the anchors line anchors, and the pattern matches the empty
    /// string, so any input containing a blank line passes. That is a quirk of
    /// `ajv-formats`, not of this port; the bus accepts `"###\n"` as a `byte`
    /// and so does this.
    is_byte,
    r"(?m)^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$"
);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid(schema: Value, instance: Value) -> bool {
        validator_for(&schema).unwrap().is_valid(&instance)
    }

    #[test]
    fn compiles_under_draft_07_not_2020_12() {
        // `dependencies` is draft-07's spelling and was split into
        // `dependentRequired`/`dependentSchemas` in 2019-09, so a validator that
        // honours it is on draft-07 and one that ignores it is not.
        let schema = json!({
            "type": "object",
            "properties": { "card": { "type": "string" }, "cvv": { "type": "string" } },
            "dependencies": { "card": ["cvv"] }
        });
        assert!(!valid(schema.clone(), json!({ "card": "4111" })));
        assert!(valid(schema, json!({ "card": "4111", "cvv": "123" })));
    }

    #[test]
    fn a_ref_ignores_its_siblings() {
        // Draft-07 behaviour, and the reason a `nullable` beside a `$ref` never
        // did anything at the bus.
        let schema = json!({
            "definitions": { "Name": { "type": "string" } },
            "properties": { "who": { "$ref": "#/definitions/Name", "maxLength": 2 } }
        });
        assert!(valid(schema, json!({ "who": "much longer than two" })));
    }

    #[test]
    fn formats_reject_rather_than_annotate() {
        assert!(!valid(json!({ "format": "date" }), json!("2026-02-30")));
        assert!(valid(json!({ "format": "date" }), json!("2026-02-28")));
    }

    #[test]
    fn asserts_the_formats_ajv_adds_beyond_the_spec() {
        // The crate gates `uuid` and `duration` to 2019-09+, so under draft-07
        // both would be unasserted without the ported table.
        assert!(!valid(json!({ "format": "uuid" }), json!("not-a-uuid")));
        assert!(valid(
            json!({ "format": "uuid" }),
            json!("123e4567-e89b-12d3-a456-426614174000")
        ));
        assert!(!valid(json!({ "format": "duration" }), json!("P")));
        assert!(valid(
            json!({ "format": "duration" }),
            json!("P3Y6M4DT12H30M5S")
        ));
    }

    #[test]
    fn ignores_the_formats_ajv_does_not_implement() {
        // The crate implements `idn-hostname` and would reject this; Ajv logs
        // "unknown format" and lets it through, so pontifex must too.
        assert!(valid(
            json!({ "format": "idn-hostname" }),
            json!("-not a hostname-")
        ));
        assert!(valid(json!({ "format": "iri" }), json!("not an iri")));
    }

    #[test]
    fn an_unknown_format_constrains_nothing() {
        assert!(valid(json!({ "format": "phone-number" }), json!("banana")));
        assert!(!is_known_format("phone-number"));
    }

    #[test]
    fn numeric_formats_do_not_reject_strings() {
        // Ajv scopes `int32` to numbers, so a string carrying it is unchecked.
        assert!(valid(json!({ "format": "int32" }), json!("not a number")));
    }

    #[test]
    fn only_the_numeric_formats_that_actually_check_are_reported_as_gaps() {
        // `int32`/`int64` run a real check on numbers that this crate cannot
        // reach. `float`/`double` are `validateNumber`, which always returns
        // true — nothing is lost, so reporting them would be a false alarm.
        assert!(is_unreproducible_format("int32"));
        assert!(is_unreproducible_format("int64"));
        assert!(!is_unreproducible_format("float"));
        assert!(!is_unreproducible_format("double"));
        assert!(!is_unreproducible_format("date"));
    }

    #[test]
    fn every_format_the_crate_implements_for_draft_07_is_in_the_table() {
        // The crate's own draft-07 format set, transcribed from
        // `jsonschema/src/keywords/format.rs`. Any of these left out of
        // `FORMATS` would fall through to the crate's implementation, which is
        // the spec's rather than `ajv-formats`' — the exact divergence this
        // module exists to prevent, and invisible without an assertion.
        //
        // This guards our side of the drift. It cannot catch the crate *adding*
        // a format in a future release: the set is not introspectable, so a
        // `jsonschema` bump still needs this list re-read against the source.
        const CRATE_DRAFT_07_FORMATS: [&str; 17] = [
            "date",
            "date-time",
            "email",
            "hostname",
            "idn-email",
            "idn-hostname",
            "ipv4",
            "ipv6",
            "iri",
            "iri-reference",
            "json-pointer",
            "regex",
            "relative-json-pointer",
            "time",
            "uri",
            "uri-reference",
            "uri-template",
        ];

        for name in CRATE_DRAFT_07_FORMATS {
            assert!(
                FORMATS.iter().any(|f| f.name == name),
                "`{name}` is implemented by the crate but absent from FORMATS, so it would \
                 be asserted with the spec's semantics instead of ajv-formats'"
            );
        }
    }

    #[test]
    fn a_foreign_dialect_is_a_compile_error_the_way_ajv_would_fail() {
        let schema = json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "string"
        });
        let error = validator_for(&schema).unwrap_err().to_string();
        assert!(error.contains("2020-12"), "{error}");
        assert!(error.contains("draft-07"), "{error}");
    }

    #[test]
    fn draft_07_is_accepted_under_either_spelling() {
        for id in DRAFT_07_IDS {
            let schema = json!({ "$schema": id, "type": "string" });
            assert!(validator_for(&schema).is_ok(), "{id}");
        }
    }

    #[test]
    fn a_nested_foreign_dialect_is_caught_too() {
        let schema = json!({
            "components": { "schemas": { "Thing": { "$schema": "http://json-schema.org/draft-04/schema#" } } }
        });
        assert_eq!(
            foreign_dialect(&schema).as_deref(),
            Some("http://json-schema.org/draft-04/schema#")
        );
    }

    // -- ported format checks, against ajv-formats' own expectations ---------

    #[test]
    fn date_is_calendar_aware() {
        assert!(is_date("2024-02-29"));
        assert!(!is_date("2023-02-29"));
        assert!(!is_date("2026-13-01"));
        assert!(!is_date("2026-1-01"));
    }

    #[test]
    fn time_requires_an_offset_and_iso_time_does_not() {
        assert!(is_time_strict_tz("12:00:00Z"));
        assert!(!is_time_strict_tz("12:00:00"));
        assert!(is_iso_time("12:00:00"));
        assert!(!is_iso_time("24:00:00"));
    }

    #[test]
    fn a_leap_second_is_only_legal_at_utc_midnight() {
        assert!(is_time_strict_tz("23:59:60Z"));
        assert!(!is_time_strict_tz("12:00:60Z"));
    }

    #[test]
    fn date_time_needs_exactly_one_separator() {
        assert!(is_date_time_strict_tz("2026-08-06T12:00:00Z"));
        assert!(!is_date_time_strict_tz("2026-08-06T12:00:00Z T"));
        // `iso-date-time` accepts a space and a missing offset; `date-time` does
        // not accept the missing offset.
        assert!(is_iso_date_time("2026-08-06 12:00:00"));
        assert!(!is_date_time_strict_tz("2026-08-06 12:00:00"));
    }

    #[test]
    fn digits_are_ascii_only() {
        // Rust's `\d` matches these; JavaScript's does not, and the bus is
        // JavaScript. Written as an assertion because getting this wrong is
        // invisible until an event arrives.
        assert!(!is_ipv4("١٢٧.0.0.1"));
        assert!(is_ipv4("127.0.0.1"));
        assert!(!is_date("٢٠٢٦-08-06"));
    }

    #[test]
    fn uri_needs_a_scheme_and_a_slash_or_colon() {
        assert!(is_uri("https://example.com/a?b=c#d"));
        assert!(is_uri("urn:uuid:123"));
        assert!(!is_uri("example.com"));
        // A bare fragment is a reference, not a URI.
        assert!(!is_uri("#/components/schemas/Thing"));
        assert!(is_uri_reference("#/components/schemas/Thing"));
        assert!(is_uri_reference("/a/b"));
    }

    #[test]
    fn hostname_bounds_the_whole_name() {
        assert!(is_hostname("events.example.com"));
        assert!(!is_hostname("-leading-hyphen.com"));
        assert!(!is_hostname(&format!("{}.com", "a".repeat(300))));
    }

    #[test]
    fn ipv6_accepts_the_compressed_and_mapped_forms() {
        assert!(is_ipv6("::1"));
        assert!(is_ipv6("2001:db8::8a2e:370:7334"));
        assert!(is_ipv6("::ffff:192.0.2.1"));
        assert!(!is_ipv6("2001:db8:::1"));
    }

    #[test]
    fn url_rejects_private_ranges_the_way_ajv_does() {
        assert!(is_url("https://example.com"));
        assert!(!is_url("https://10.0.0.1"));
        assert!(!is_url("example.com"));
    }

    #[test]
    fn regex_rejects_a_perl_anchor_javascript_would_misread() {
        assert!(is_regex(r"^[a-z]+$"));
        assert!(!is_regex(r"^abc\Z"));
        // Lookahead is legal in JavaScript, so it must be legal here.
        assert!(is_regex(r"^(?=.*\d)[a-z\d]+$"));
    }

    #[test]
    fn email_and_uuid_match_ajvs_spelling() {
        assert!(is_email("someone@example.com"));
        assert!(!is_email("no-at-sign"));
        assert!(!is_email("trailing@dot."));
        assert!(is_uuid("urn:uuid:123E4567-E89B-12D3-A456-426614174000"));
    }

    #[test]
    fn json_pointers_match_ajvs_spelling() {
        assert!(is_json_pointer(""));
        assert!(is_json_pointer("/a/b~0c"));
        assert!(!is_json_pointer("a/b"));
        assert!(is_relative_json_pointer("0#"));
        assert!(is_relative_json_pointer("2/a"));
        assert!(!is_relative_json_pointer("01"));
        assert!(is_json_pointer_uri_fragment("#/a/b"));
        assert!(!is_json_pointer_uri_fragment("/a/b"));
    }

    #[test]
    fn byte_is_base64_with_ajvs_multiline_quirk() {
        assert!(is_byte("aGVsbG8="));
        assert!(!is_byte("###"));
        // Documented above: the `/m` flag makes a blank line enough.
        assert!(is_byte("###\n"));
    }
}
