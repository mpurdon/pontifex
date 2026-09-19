//! Which fields a consumer reads off an event.
//!
//! Severity from the validator alone says whether events are rejected, not
//! whether anyone downstream would notice a field going wrong. The consumer
//! files the origin lookup finds are the evidence for that: a handler that
//! dereferences `detail.payload.matterId` breaks when `matterId` goes
//! missing, and a field no handler reads can drift without anyone noticing.
//!
//! This reads the accesses out of source text without parsing it. Chains
//! rooted at the envelope's `detail` (`event.detail.a.b`, `detail?.a`,
//! `detail['a']`, `detail.get("a")`, `event["detail"]["a"]`); destructuring
//! of the same (`const { a, b: { c } } = event.detail`, `({ detail: { a } })`);
//! and, because a handler usually unwraps the detail into a local before
//! reading it, aliases: `const d = event.detail.payload` makes `d.x` a read
//! of `payload.x`, and every name a destructuring binds is an alias for the
//! key it was bound from. A bare `payload` is taken to be the detail's
//! `payload` wrapper, which is how this bus's events are shaped.
//!
//! Good enough for TypeScript and Python handlers, which is what subscribes
//! to the bus; anything it misses is a reader not found, never one invented.

use crate::schema::events::join_path;
use std::collections::{BTreeMap, BTreeSet};

/// Properties that are about the container, not fields of the event.
const NOT_FIELDS: [&str; 4] = ["length", "size", "constructor", "prototype"];

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    /// `.`, `?.`, `!.` — followed directly by a name, so that a full stop
    /// in prose (`the payload. These fields…`) is not an access.
    Dot,
    Str(String),
    Open(char),
    Close(char),
    Eq,
    Colon,
    Comma,
    Spread,
    Other,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let is_ident_start = |c: char| c.is_alphabetic() || c == '_' || c == '$';
    let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // Comments: a field named in one is not a field read.
        if c == '/' && chars.get(i + 1) == Some(&'/') || c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            out.push(Tok::Ident(chars[start..i].iter().collect()));
            continue;
        }
        if c == '\'' || c == '"' || c == '`' {
            let quote = c;
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != quote {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            let s: String = chars[start..i.min(chars.len())].iter().collect();
            i += 1;
            out.push(Tok::Str(s));
            continue;
        }
        if c == '.' && chars.get(i + 1) == Some(&'.') && chars.get(i + 2) == Some(&'.') {
            out.push(Tok::Spread);
            i += 3;
            continue;
        }
        // `.`, `?.`, `!.`: an access only when a name follows directly;
        // `?.[` and `?.(` are the plain bracket with a null guard.
        let dot_width = match c {
            '.' => Some(1),
            '?' | '!' if chars.get(i + 1) == Some(&'.') => Some(2),
            _ => None,
        };
        if let Some(width) = dot_width {
            match chars.get(i + width) {
                Some(&next) if is_ident_start(next) => out.push(Tok::Dot),
                Some('[' | '(') if width == 2 => {}
                _ => out.push(Tok::Other),
            }
            i += width;
            continue;
        }
        out.push(match c {
            '(' | '{' | '[' => Tok::Open(c),
            ')' | '}' | ']' => Tok::Close(c),
            '=' if chars.get(i + 1) != Some(&'=') && chars.get(i + 1) != Some(&'>') => Tok::Eq,
            '=' => {
                i += 1;
                Tok::Other
            }
            ':' => Tok::Colon,
            ',' => Tok::Comma,
            _ => Tok::Other,
        });
        i += 1;
    }
    out
}

/// One thing a destructuring pattern binds: the key it reads, and the name
/// it binds it to when a name is bound (a nested pattern binds none).
struct Binding {
    key: String,
    name: Option<String>,
}

impl Binding {
    fn shorthand(key: String) -> Self {
        Binding {
            name: Some(key.clone()),
            key,
        }
    }
}

/// One pass over a file's tokens, carrying the names that stand for some
/// part of the detail.
struct Scanner<'t> {
    toks: &'t [Tok],
    /// Name → the dotted path it stands for, relative to the detail.
    aliases: BTreeMap<String, String>,
    found: BTreeSet<String>,
}

impl Scanner<'_> {
    fn scan(&mut self) {
        let mut i = 0;
        while i < self.toks.len() {
            if let Some(value) = self.assignment_at(i) {
                // `const d = event.detail.payload`: from here on `d` is that
                // part of the detail, and the right-hand side is a read of it.
                if let (Tok::Ident(name), Some((prefix, rest))) =
                    (&self.toks[i], self.chain_from(value))
                {
                    let path = join_path(&prefix, &rest);
                    self.aliases.insert(name.clone(), path.clone());
                    if !path.is_empty() {
                        self.found.insert(path);
                    }
                }
                i = value;
            } else if let Some((prefix, rest)) = self.chain_from(i) {
                // `messageBody.detail.payload.x`, from wherever it starts.
                if !rest.is_empty() {
                    self.found.insert(join_path(&prefix, &rest));
                }
                i += 1;
            } else if let Some((bindings, prefix)) = self.destructuring_at(i) {
                self.bind(&bindings, &prefix);
                i += 1;
            } else {
                i += 1;
            }
        }
    }

    /// Where the value starts when the name at `at` is being assigned:
    /// `d = …`, or a declaration with a type, `const d: Dto = …`. A
    /// property (`o.d = …`) or an object key (`d: …`) is neither.
    fn assignment_at(&self, at: usize) -> Option<usize> {
        if !matches!(self.toks[at], Tok::Ident(_)) {
            return None;
        }
        let previous = at.checked_sub(1).map(|p| &self.toks[p]);
        if matches!(previous, Some(Tok::Dot | Tok::Colon)) {
            return None;
        }
        match self.toks.get(at + 1) {
            Some(Tok::Eq) => Some(at + 2),
            Some(Tok::Colon) if matches!(previous, Some(Tok::Ident(kw)) if ["const", "let", "var"].contains(&kw.as_str())) =>
            {
                // Skip the type: a handful of tokens, none of which opens a
                // block or ends the statement.
                (at + 2..(at + 14).min(self.toks.len()))
                    .take_while(|&j| {
                        !matches!(self.toks[j], Tok::Open('{') | Tok::Close(_) | Tok::Comma)
                    })
                    .find(|&j| self.toks[j] == Tok::Eq)
                    .map(|j| j + 1)
            }
            _ => None,
        }
    }

    /// `{ a, b: { c } } = event.detail` with its `{` at `open`: what it
    /// binds, and the path it is bound from.
    fn destructuring_at(&mut self, open: usize) -> Option<(Vec<Binding>, String)> {
        if self.toks[open] != Tok::Open('{') {
            return None;
        }
        let (bindings, next) = self.pattern(open)?;
        if self.toks.get(next) != Some(&Tok::Eq) {
            return None;
        }
        let (prefix, rest) = self.chain_from(next + 1)?;
        Some((bindings, join_path(&prefix, &rest)))
    }

    /// Record a pattern's bindings as reads of, and aliases for, the keys
    /// under `prefix`.
    fn bind(&mut self, bindings: &[Binding], prefix: &str) {
        for binding in bindings {
            let path = join_path(prefix, &binding.key);
            self.found.insert(path.clone());
            if let Some(name) = &binding.name {
                self.aliases.insert(name.clone(), path);
            }
        }
    }

    /// The part of the detail a chain expression at `at` stands for, as the
    /// path of the name it is rooted in and the path read beneath it —
    /// `event.detail`, `e?.detail.payload`, `d.x`, `evt["detail"]` — or
    /// `None` when it is not rooted in the detail at all. `messageBody.
    /// detail.payload` is rooted at `detail` whatever `messageBody` is, and
    /// the chain may head a longer expression (`… || {}`, `… as Dto`).
    fn chain_from(&self, at: usize) -> Option<(String, String)> {
        let mut head = at;
        loop {
            let (name, width) = match self.toks.get(head)? {
                Tok::Ident(name) => (name, 1),
                Tok::Open('[') => match self.toks.get(head + 1..head + 3) {
                    Some([Tok::Str(name), Tok::Close(']')]) => (name, 3),
                    _ => return None,
                },
                _ => return None,
            };
            if let Some(prefix) = self.aliases.get(name) {
                return Some((prefix.clone(), chain_after(self.toks, head + width).0));
            }
            head = match (self.toks.get(head + width), self.toks.get(head + width + 1)) {
                (Some(Tok::Dot), Some(Tok::Ident(_))) => head + width + 1,
                (Some(Tok::Open('[')), Some(Tok::Str(_))) => head + width,
                _ => return None,
            };
        }
    }

    /// Parse a destructuring pattern whose `{` is at `open`. Returns what it
    /// binds and the position after its `}`, or `None` when the braces hold
    /// something that is not a pattern. A key that already stands for part
    /// of the detail — `{ detail: { a } }` in a handler's parameter — has
    /// its nested bindings recorded outright, whatever the pattern is bound
    /// to.
    fn pattern(&mut self, open: usize) -> Option<(Vec<Binding>, usize)> {
        let mut bindings: Vec<Binding> = Vec::new();
        let mut at = open + 1;
        loop {
            match self.toks.get(at)? {
                Tok::Close('}') => return Some((bindings, at + 1)),
                Tok::Comma => at += 1,
                Tok::Spread => {
                    at += 1;
                    // `...rest` binds the remainder under another name,
                    // which says nothing about which fields are read.
                    if let Some(Tok::Ident(_)) = self.toks.get(at) {
                        at += 1;
                    }
                }
                Tok::Ident(key) | Tok::Str(key) => {
                    let key = key.clone();
                    at += 1;
                    match self.toks.get(at) {
                        Some(Tok::Colon) => {
                            at += 1;
                            match self.toks.get(at)? {
                                Tok::Open('{') => {
                                    let (nested, next) = self.pattern(at)?;
                                    match self.aliases.get(&key).cloned() {
                                        Some(prefix) => self.bind(&nested, &prefix),
                                        None => {
                                            bindings.push(Binding {
                                                key: key.clone(),
                                                name: None,
                                            });
                                            bindings.extend(nested.into_iter().map(|b| Binding {
                                                key: join_path(&key, &b.key),
                                                name: b.name,
                                            }));
                                        }
                                    }
                                    at = next;
                                }
                                // `a: [first]` binds elements; the field is `a`.
                                Tok::Open('[') => {
                                    bindings.push(Binding { key, name: None });
                                    at = skip_call(self.toks, at);
                                }
                                Tok::Ident(name) => {
                                    bindings.push(Binding {
                                        key,
                                        name: Some(name.clone()),
                                    });
                                    at += 1;
                                }
                                // `a: 1` is an object literal, not a pattern.
                                _ => return None,
                            }
                        }
                        Some(Tok::Eq | Tok::Comma | Tok::Close('}')) => {
                            bindings.push(Binding::shorthand(key));
                        }
                        // `a(`, `a.b`: a call or an expression, not a binding.
                        _ => return None,
                    }
                    // A default, after any of the forms above.
                    if self.toks.get(at) == Some(&Tok::Eq) {
                        at = skip_default(self.toks, at + 1);
                    }
                }
                _ => return None,
            }
        }
    }
}

/// The dotted paths a file reads off the event detail, deduplicated and
/// sorted. Array elements collapse onto the array's path with `[]`, the way
/// the drift report names them.
pub fn field_reads(content: &str) -> Vec<String> {
    let toks = tokenize(content);
    let mut scanner = Scanner {
        toks: &toks,
        aliases: BTreeMap::from([
            ("detail".to_string(), String::new()),
            ("payload".to_string(), "payload".to_string()),
        ]),
        found: BTreeSet::new(),
    };
    scanner.scan();
    scanner.found.into_iter().collect()
}

/// The property chain starting at `at`, as a dotted path, and where it ends.
fn chain_after(toks: &[Tok], mut at: usize) -> (String, usize) {
    let mut segments: Vec<String> = Vec::new();
    loop {
        match (toks.get(at), toks.get(at + 1)) {
            (Some(Tok::Dot), Some(Tok::Ident(name))) => {
                // `.get("a")` reads `a`; any other call is a method, and the
                // chain of fields ends before it.
                if toks.get(at + 2) == Some(&Tok::Open('(')) {
                    if name == "get" {
                        if let Some(Tok::Str(key)) = toks.get(at + 3) {
                            segments.push(key.clone());
                            at = skip_call(toks, at + 2);
                            continue;
                        }
                    }
                    break;
                }
                if NOT_FIELDS.contains(&name.as_str()) {
                    break;
                }
                segments.push(name.clone());
                at += 2;
            }
            // `['name']` is a field; `[0]`, `[i]`, `[i + 1]` an element.
            (Some(Tok::Open('[')), Some(inner)) => {
                let end = skip_call(toks, at);
                match (inner, end - at) {
                    (Tok::Str(key), 3) => segments.push(key.clone()),
                    (Tok::Close(']'), _) => break,
                    _ => match segments.last_mut() {
                        Some(last) => last.push_str("[]"),
                        None => break,
                    },
                }
                at = end;
            }
            _ => break,
        }
    }
    (segments.join("."), at)
}

/// The position after the bracket pair opening at `open`.
fn skip_call(toks: &[Tok], open: usize) -> usize {
    let mut depth = 0usize;
    let mut at = open;
    while at < toks.len() {
        match &toks[at] {
            Tok::Open(_) => depth += 1,
            Tok::Close(_) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
        at += 1;
    }
    at
}

/// The position of the `,` or `}` that ends a default value starting at `at`.
fn skip_default(toks: &[Tok], mut at: usize) -> usize {
    let mut depth = 0usize;
    while at < toks.len() {
        match &toks[at] {
            Tok::Open(_) => depth += 1,
            Tok::Close(_) if depth > 0 => depth -= 1,
            Tok::Close(_) | Tok::Comma if depth == 0 => return at,
            _ => {}
        }
        at += 1;
    }
    at
}

/// A path as the segments the grader compares: `items[].sku` and
/// `items.sku` name the same field.
pub fn segments(path: &str) -> Vec<String> {
    path.split('.')
        .map(|seg| seg.replace("[]", ""))
        .filter(|seg| !seg.is_empty())
        .collect()
}

/// How a read relates to the field an issue is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The read is of the field, or of something inside it — the read
    /// that breaks when the field goes wrong. A handler reading
    /// `metadata.trackingId` reads `metadata`.
    Reads,
    /// The read takes an ancestor of the field along, whole: `const p =
    /// detail.payload` handed to another module uses some of `payload`'s
    /// fields, and this cannot see which. Enough to withhold "nobody reads
    /// it", not enough to say who does.
    PassesAlong,
    Unrelated,
}

pub fn relation(read: &[String], path: &[String]) -> Relation {
    let shorter = read.len().min(path.len());
    if path.is_empty() || read.is_empty() || read[..shorter] != path[..shorter] {
        Relation::Unrelated
    } else if read.len() >= path.len() {
        Relation::Reads
    } else {
        Relation::PassesAlong
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_reads(src: &str, expected: &[&str]) {
        let got = field_reads(src);
        let want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn reads_property_chains_in_typescript() {
        assert_reads(
            r#"
            export const handler = async (event: EventBridgeEvent<'x', Detail>) => {
              const id = event.detail.matterId;
              const tracking = event.detail.metadata?.trackingId ?? null;
              const first = event.detail.items[0].sku;
              const legacy = event.detail['client-id'];
              if (event.detail.items.length > 0) log(event['detail-type']);
              event.detail.items.map((i) => i.id);
              const nth = event.detail.items[i + 1]?.qty;
              const maybe = event.detail?.['flags'];
            };
            "#,
            &[
                "client-id",
                "flags",
                "items",
                "items[].qty",
                "items[].sku",
                "matterId",
                "metadata.trackingId",
            ],
        );
    }

    #[test]
    fn reads_destructuring_of_the_detail() {
        let got = field_reads(
            r#"
            const { matterId, clientId: client, metadata: { trackingId }, tags = [] } = event.detail;
            const { detail: { caseId } } = event;
            export const handler = async ({ detail: { leadId, status = 'new' } }: Ev) => {};
            const { a, ...rest } = detail;
            "#,
        );
        for expected in [
            "matterId",
            "clientId",
            "metadata",
            "metadata.trackingId",
            "tags",
            "caseId",
            "leadId",
            "status",
            "a",
        ] {
            assert!(
                got.contains(&expected.to_string()),
                "{expected} missing from {got:?}"
            );
        }
        assert!(!got.contains(&"rest".to_string()));
    }

    #[test]
    fn follows_the_detail_through_locals_and_bindings() {
        // The shape the bus's handlers actually take: unwrap the detail's
        // payload into a local, destructure it, and read the pieces.
        assert_reads(
            r#"
            for (const record of event.Records) {
              const messageBody = JSON.parse(record.body);
              const eventDetail: ClientCaseDto = messageBody.detail?.payload || {};
              const { demographics, clientId } = eventDetail;
              const { email } = demographics || {};
              if (!email) throw new Error('Missing required field: email in demographics');
              logger.info('Updating', { email, clientId });
              await updateGlobalClientId(body?.detail?.payload);
            }
            "#,
            &[
                "payload",
                "payload.clientId",
                "payload.demographics",
                "payload.demographics.email",
            ],
        );
    }

    #[test]
    fn a_bare_payload_is_the_details_payload() {
        assert_reads(
            "const send = (payload: Detail['payload']) => notify(payload.matterId, payload.client.id);",
            &["payload.client.id", "payload.matterId"],
        );
    }

    #[test]
    fn an_event_pattern_filtering_on_a_field_reads_it() {
        assert_reads(
            "rule: { pattern: { detailType: ['x'], detail: { payload: { updated: ['demographics.email'] } } } }",
            &["payload.updated"],
        );
    }

    #[test]
    fn an_array_literal_naming_an_alias_is_not_a_read() {
        // `['payload']` used to tokenize as a subscript, which made `keys`
        // an alias for the payload — the scanner's one way of inventing a
        // reader.
        assert_reads(
            "const keys = ['payload', 'detail']; const one = ['detail']; keys.forEach(log);",
            &[],
        );
    }

    #[test]
    fn reads_python_subscripts_and_get() {
        assert_reads(
            r#"
            def handler(event, context):
                detail = event["detail"]
                payload = detail["payload"]
                matter_id = payload["matterId"]
                tracking = detail.get("metadata", {}).get("trackingId")
                for item in payload.get("items", []):
                    print(item["sku"], event["detail"]["payload"]["ref"])
                # detail["commented"] is not a read
            "#,
            &[
                "metadata.trackingId",
                "payload",
                "payload.items",
                "payload.matterId",
                "payload.ref",
            ],
        );
    }

    #[test]
    fn prose_calls_and_literals_are_not_reads() {
        let src = r#"
            /** Finds the innermost payload in nested detail.payload structures */
            const help = 'Fields are extracted from the event payload. These fields are used later.';
            const entry = { Detail: JSON.stringify(detail), DetailType: 'x' };
            await client.send(new PutEventsCommand({ Entries: [entry] }));
            const keys = Object.keys(detail);
            <p>Paste the JSON payload. The system extracts <code>detail.payload</code> fields.</p>
        "#;
        // The one thing left is the `detail.payload` in the code sample,
        // which is a read of `payload` by any reading.
        assert_reads(src, &["payload"]);
    }

    /// Not a test: `READS_FILE=path cargo test --lib reads_of_a_file -- --ignored --nocapture`
    /// prints what the scanner makes of a real file, for checking it
    /// against the code by eye.
    #[test]
    #[ignore = "reads a file named by READS_FILE"]
    fn reads_of_a_file() {
        let path = std::env::var("READS_FILE").expect("READS_FILE");
        let text = std::fs::read_to_string(&path).expect("readable");
        for read in field_reads(&text) {
            eprintln!("{read}");
        }
    }

    #[test]
    fn reading_and_passing_along_are_told_apart() {
        let rel = |read: &str, path: &str| relation(&segments(read), &segments(path));
        assert_eq!(rel("payload.matterId", "payload.matterId"), Relation::Reads);
        assert_eq!(
            rel("payload.metadata.trackingId", "payload.metadata"),
            Relation::Reads
        );
        assert_eq!(
            rel("payload.items[].sku", "payload.items[]"),
            Relation::Reads
        );
        assert_eq!(rel("payload", "payload.matterId"), Relation::PassesAlong);
        assert_eq!(
            rel("payload.items", "payload.items[].sku"),
            Relation::PassesAlong
        );
        assert_eq!(
            rel("payload.matter", "payload.matterId"),
            Relation::Unrelated
        );
        assert_eq!(
            rel("payload.other", "payload.matterId"),
            Relation::Unrelated
        );
        assert_eq!(rel("payload.matterId", ""), Relation::Unrelated);
    }
}
