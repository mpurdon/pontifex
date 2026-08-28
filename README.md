# Pontifex

> **Pontifex** (Latin, *pons* "bridge" + *facere* "to make"), *m.* — a
> bridge-builder; in Rome, one of the college of priests who kept the rites
> and the calendar. Their chief was the *Pontifex maximus*.
>
> *Ego a ponte arbitror: nam ab his Sublicius est factus primum ut restitutus
> saepe.* — "I think it comes from *pons*: for the Sublician bridge was first
> built by them, and often restored." Varro, *De Lingua Latina* V.83,
> weighing in on where the word came from.

A desktop manager for an EventBridge-based event bus — browse, edit, generate
and register schemas, tail event logs, and inspect the bus topology, without
hand-editing JSON or running one-off scripts. The bridge-keeper for the bridge.

Built with [Tauri 2](https://tauri.app) (Rust backend, React + TypeScript
frontend). Every AWS call happens in Rust; the webview never sees credentials.

Companion to `~/Projects/trajector/global-event-bus`.

## What it does

| Screen       | What it gives you                                                                                                                                                                  |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Schemas**  | Every schema in the registry, grouped by event source and fuzzy-filterable. Structured tree editor plus raw JSON, live validation, version history, diff-vs-live and diff-vs-version, register/delete. |
| **Generate** | Describe an event in plain English and get a conforming schema back from Bedrock. Also refactors and explains existing schemas. Output is always a validated draft you must save.    |
| **Logs**     | CloudWatch view of the `/aws/events/*` groups, filtered by source and detail-type (or a raw filter pattern), with expandable event payloads.                                         |
| **Health**   | Every schema in the registry graded against real traffic in one pass — failing, drifting, healthy, or unseen — plus event types on the bus with no schema at all.                     |
| **Topology** | What `stacks/busConfiguration.ts` declares versus what is actually deployed, with rules, targets and drift in both directions.                                                       |
| **Settings** | AWS profiles with live status and SSO sign-in, per-stage environments, registry discovery, Bedrock model catalog.                                                                    |
| **Developer**| Optional. The application log, where Pontifex's files live and how large they are, and cache state — for checking the app itself, not the bus.                                          |

The registry is the source of truth. Local `schemas/` directories are an
import/export path, not the workflow.

## Requirements

- Rust 1.82+ and Node 20+ (pnpm 10)
- On macOS: Xcode command line tools
- AWS CLI v2 is **optional** — only used as a sign-in fallback

## Getting started

```bash
pnpm install
pnpm tauri dev          # run in development
pnpm tauri build        # produce a distributable bundle
```

The dev server runs on port **1428** rather than Tauri's default 1420, so it
does not collide with other Tauri projects.

### First run

Pontifex bootstraps itself from your machine:

- **Environments** — one per stage (`sandbox`, `dev`, `stg`, `prd`) using the
  naming from the SST stack: registry `<stage>-global-registry` in `us-east-2`,
  log groups `/aws/events/<stage>-global-events` and `-external-events`.
- **AWS profile** — the first SSO profile in `~/.aws/config`, deliberately
  skipping the Claude Code Bedrock profile (that role cannot read the schema
  registry).
- **Bedrock** — models and the LLM profile are imported from
  `~/.claude/trajector-settings.json`, falling back to the `.superseded`
  variant and then to `~/.claude/settings.json`.
- **Repo path** — `~/Projects/trajector/global-event-bus`, if it exists.

Then open **Settings** and point each environment at the right profile.

## Configuring AWS access

Each Global Event Bus stage lives in its own AWS account. There are two ways to
reach them.

### In-app SSO (no profiles needed)

The shortest path, and the one that avoids an external credential manager
entirely:

1. **Settings → SSO sessions** lists the `[sso-session]` blocks in
   `~/.aws/config`. Sign in to one — the device-code flow opens your browser and
   writes the token to `~/.aws/sso/cache`, shared with the AWS CLI.
2. For each environment, set **Credentials** to *SSO account* and pick the
   account and role. The account list is filtered to Global Event Bus accounts
   by default; toggle **All** to see everything the session grants.

Pontifex then redeems the session token for that role's credentials on demand.
Nothing is written to `~/.aws/config`, and no profile has to exist for an
account to be reachable — one sign-in covers every account and role.

Only an `[sso-session]` block is required:

```ini
[sso-session trajector]
sso_start_url = https://YOUR-ORG.awsapps.com/start
sso_region = us-east-2
sso_registration_scopes = sso:account:access
```

If you also want the same account/role from the terminal, use **Export as AWS
profile** on the environment. That is the only thing that ever writes to
`~/.aws/config`, it only ever appends, and it refuses to overwrite an existing
profile.

Exported profiles default to a `gm-` prefix and carry a provenance comment:

```ini
# written by Pontifex — 111111111111 (Global Event Bus Development) as AdministratorAccess
[profile gm-dev]
sso_session = trajector
sso_account_id = 111111111111
sso_role_name = AdministratorAccess
region = us-east-2
```

The prefix makes Pontifex's profiles easy to spot in a picker, but the comment is
what makes provenance reliable — names get edited, and a prefix tells you
nothing about which account or role a profile actually grants.

### Named profiles

Set **Credentials** to *AWS profile* to use an existing profile instead. This
covers profiles written by an external manager (Leapp, aws-vault, …), which
appear as `external` in the profile list — Pontifex cannot refresh those, so it
points you back to the owning tool rather than offering a sign-in that would do
nothing.

**Profiles written by a credential manager can vanish.** Leapp and aws-vault
delete their profile when a session ends, which leaves an environment pointing
at a name that no longer exists. Pontifex detects that and routes you to the fix
rather than offering a sign-in that would fail — switching the environment to
an SSO account avoids the problem entirely, since Pontifex re-redeems the session
token itself.

To declare profiles yourself, one per stage:

```ini
[sso-session trajector]
sso_start_url = https://YOUR-ORG.awsapps.com/start
sso_region = us-east-2
sso_registration_scopes = sso:account:access

[profile geb-dev]
sso_session = trajector
sso_account_id = 111111111111        # Global Event Bus Development
sso_role_name = AdministratorAccess  # whichever role you hold
region = us-east-2

[profile geb-stg]
sso_session = trajector
sso_account_id = 222222222222        # Global Event Bus Staging
sso_role_name = ReadOnlyAccess
region = us-east-2

[profile geb-prd]
sso_session = trajector
sso_account_id = 333333333333        # Global Event Bus Production
sso_role_name = ReadOnlyAccess
region = us-east-2

[profile geb-sandbox]
sso_session = trajector
sso_account_id = 444444444444        # Global Event Bus Sandbox
sso_role_name = SandboxPowerUserAccess
region = us-east-2
```

**Sandbox registries are per-pull-request** (`PR-248-global-registry`), not
`sandbox-global-registry`. Use **Discover** next to the registry field in
Settings to list what actually exists in the account.

### Signing in

Pontifex runs the OIDC device-code flow itself, opens your browser, and writes
the token to `~/.aws/sso/cache` using the same SHA-1 filename scheme the AWS CLI
uses — so a sign-in here also works in your terminal, and vice versa. Signing in
to a *session* covers every account it grants; signing in to a *profile* is the
narrower equivalent and falls back to `aws sso login --profile <name>` if the
native flow cannot run.

### The three credential slots

Matching how the app is meant to be used, three things are configured
independently:

1. **Per-environment credentials** — an SSO account+role or a named profile,
   used for that stage's schema registry, logs and buses.
2. **Bedrock model catalog** — the inference-profile ARNs, imported from your
   Claude Code settings or discovered live from Bedrock.
3. **LLM profile** — a *separate* AWS profile used only for Bedrock calls, so
   schema work and model calls can authenticate as different principals.

## Safety

Production changes are allowed — they just should not be one keystroke away:

- **Writing to a protected environment shows the diff first.** The confirmation
  renders the live-vs-draft diff, and requires both an acknowledgement and
  typing the environment name. Seeing the actual change is what makes the
  confirmation meaningful; a plain "are you sure?" is not.
- Deleting a schema requires typing its exact name.
- Environments marked **protected** (any registry whose name contains `prd`)
  require a second, explicit acknowledgement.
- Both gates are re-checked in Rust, so a UI bug cannot delete by accident.
- Writes are validated before they are sent. AWS accepts documents that violate
  the EventBridge envelope contract and they then fail silently at
  event-validation time, so Pontifex blocks them up front.

## Vocabulary

"Schema" would otherwise mean two different things one click apart, which makes
it impossible to say which one you mean — to a colleague or to an AI. So:

| Term       | What it means                                                                                  |
| ---------- | ---------------------------------------------------------------------------------------------- |
| **Schema** | The registry entry, e.g. `clientProfile-v1@client-profile-v1-sync`. Versioned. AWS's own word.  |
| **Type**   | A named object defined inside it (`components.schemas.*`), e.g. `Payload`, `Metadata`.          |
| **Field**  | A property on a type, e.g. `payload.clientId`.                                                  |

The UI uses these consistently: the registry list holds **schemas**, the editor's
left rail holds **types**, and the tree holds **fields**.

## The structure editor

Raw JSON is fine for reading a small schema and painful for authoring a nested
one. The registry's real documents compose through `$ref` rather than deep
inline nesting — `clientProfile-v1@client-profile-v1-sync` has **61 component
schemas** and there are 874 refs across the set — so editing one as text means
jumping between definitions constantly.

**Structure** is the default view and is built around that fact:

- **Refs resolve inline.** Expanding a `$ref` field shows the target's fields in
  place, so the whole effective event shape reads as one tree. Each ref row
  carries a chip naming its target; clicking it jumps to that definition.
- **Edits land where the data lives.** A field reached through a ref points at
  the shared definition, and the inspector says so — including how many places
  depend on it — before you change something used elsewhere.
- **Nothing is silently dropped.** Edits merge into the existing node, so
  keywords the UI does not model (`oneOf`, `pattern`, `minimum`, …) survive
  untouched. Any node that has them shows a `+n` badge, and the inspector names
  them and points you at the JSON view.
- **Shape is scannable.** Colour-coded type badges, a required dot next to the
  name, a `null` chip for nullable fields, and inline enum previews.
- **The left rail lists every type** with how many fields use it, so unused ones
  show up as `unused` and the envelope is marked as the generated boilerplate it
  is. Hovering a type reveals a remove button.
- **Cycles are handled.** A self-referencing type stops with a `cycle` marker
  rather than expanding forever.

### Extra fields

Select an object field (or a type's root) and set **Extra fields** in the
inspector:

| Setting                  | Meaning                                                                     |
| ------------------------ | --------------------------------------------------------------------------- |
| `Allowed — not specified` | The keyword is absent. JSON Schema treats this as permissive.               |
| `Allowed`                | Explicit `additionalProperties: true`. What the registry's simplify step sets. |
| `Not allowed`            | `additionalProperties: false`. An event with an undeclared field fails validation. |

Closed objects get a `strict` badge in the tree; open ones get nothing, since
open is the convention here and badging every row would be noise. When the
keyword holds a sub-schema rather than a boolean, the control says so and defers
to the JSON view.

### Check against real events

A schema can be structurally valid and still wrong — too strict for real
traffic, or missing fields producers started sending. **Reality** samples recent
events for this schema from CloudWatch and reports where the schema and the bus
disagree:

- **Validation failures** — events that would be rejected, grouped by which
  check failed rather than by message text, so one recurring problem is one row
  with a count.
- **Fields in events but not in the schema** — with how often each appears and
  an example value. Add them one at a time with the **+** on each row, or all
  the top-level ones at once. A nested path is resolved through the ref graph
  to whichever type actually owns it, so `metadata.extra` lands on `Metadata`.
- **Type mismatches** — the schema says string, traffic carries a number.
- **Required but not always present** — passes validation only because the
  field happens to be there; a candidate for making optional.
- **Values outside the declared enum.**
- **Declared but never seen** — possibly dead, possibly just rare.

Running a check also annotates every field in the tree with its observed
frequency, so `95%` and `3%` are visible at a glance. A field declared
`required` but present in less than 100% of traffic is flagged in red.

#### Cached samples

Fetched events are cached on disk, keyed by environment, log group and event
type. That matters for three reasons:

- **`FilterLogEvents` bills per GB scanned** and takes the better part of a
  second over a busy log group. Re-checking the same window should not pay that
  twice.
- **Re-checks become free.** Measured against production: a fetch took 782 ms,
  the equivalent cached read 15 µs. So the panel re-validates your draft against
  real traffic **as you type** (the *live* toggle) — edit a field, immediately
  see whether real events still pass.
- **The health report warms everything.** Its single scan caches every event
  type it saw, so afterwards any schema's check is instant.

Overlapping fetches dedupe by CloudWatch event id, and widening the window only
fetches the new slice. **Refresh** forces a fresh fetch; a truncated sample is
never treated as covering a window, since it stopped early and absence proves
nothing. Settings → Cached events shows the size and can clear it.

Only the event `detail` is validated — the envelope is AWS's, not yours.
`nullable` is translated to JSON Schema's `["type", "null"]` first, or every
legitimately-null field would read as a violation.

This is a read-only diagnostic. Suggested additions land in the draft; nothing
is written to AWS until you save.

### Registry-wide health report

**Health** grades the whole registry at once. It scans the log group **once**
and buckets events by `source@detail-type`, rather than querying per schema —
300+ schemas would otherwise mean 300+ CloudWatch calls, most finding nothing.
In practice that is a few seconds for a few thousand events.

Rows are sorted worst-first and filterable to problems only; clicking one opens
that schema. The single pass also surfaces something per-schema checks
structurally cannot: **event types flowing on the bus with no schema
registered**.

A capped scan says so — with truncation, "no traffic" means "not in this
sample", not "never published".

The report survives navigation: it is held in the query cache rather than the
view, so switching tabs does not discard a scan that took seconds and cost
money. It also states its own age, and warns once the report is older than the
window it covers — at that point everything it sampled has fallen outside that
window, so "no traffic" says nothing useful until you re-run it.

#### Drafting a schema for an undocumented event

The **on the bus with no schema** rows are clickable. Clicking one drafts a
schema whose payload shape is **inferred from that event type's own traffic**,
so the draft opens describing what is actually being published rather than as a
blank form. It uses cached events when available, so this is usually instant.

The inference follows the conventions the registry already uses: only ID-like
fields are marked required (and only when every sampled event carried them),
`additionalProperties` stays open, and `date-time`/`uuid`/`email` formats are
claimed only when *every* observed value matches — a format that holds for most
values would make the schema reject real traffic. Genuinely mixed types are
left open rather than guessed at, and nested objects and arrays are inferred
recursively.

The draft is unsaved. Review it in the structure editor and register it like
any other new schema — including the production diff confirmation.

### Renaming a type

Hover a type in the left rail and click the pencil. The dialog shows how many
references will be rewritten — a rename is not a local edit, since every `$ref`
pointing at the type has to move with it.

### Removing a type

Hover a type in the left rail and click the trash icon. The confirmation names
every type that still references it — removing one that is in use leaves
dangling `$ref`s, which is legal JSON that fails the moment anything reads the
schema. You can still proceed, but not by accident. The `AWSEvent` envelope has
no remove button, since EventBridge requires it.

To delete a whole **schema** from the registry, use the trash button in the
toolbar above instead — that one asks you to type the schema name, and refuses
outright in a protected environment unless you acknowledge it.

The view opens on the payload schema, found by following
`AWSEvent.properties.detail.$ref` rather than guessing from key order.

**Sample** generates a complete example event from the schema — useful for
seeing what an actual event looks like, and available as a ready-to-paste
`aws events put-events` command with the detail correctly escaped.

Structure and JSON edit the same draft; switch freely. Structure needs parseable
JSON, so while the text is mid-edit and broken it points you back to the JSON
view.

Panels are resizable, and the sizes are saved per layout in settings. In the
schema list, hovering a source reveals a **pin** — pinned sources sort to the
top so whatever you are working on stays reachable in a list of hundreds.

## Schema validation

Every document is checked against the `AWSEvent` envelope contract before it can
be saved:

- `type: "object"` and the nine required envelope fields
- `x-amazon-events-source` and `x-amazon-events-detail-type` present, and
  consistent with the name the schema is being saved under
- `properties.detail.$ref` resolving to a schema that exists in the document
- every component schema compiling as valid JSON Schema

Findings are rendered inline in the editor gutter and listed below it.

One deliberate exception: EventBridge's own schema discovery registers schemas
as `<source>@<PascalCaseDetailType>` (`ai-services.c-file@LabelsGenerated` for
detail-type `labels-generated`). That mismatch is AWS's convention, so it is
reported as a warning rather than blocking a save.

## Import / export

- **Export** writes the registry to a directory in the repo's file format
  (`{schemaName, schemaVersion, type, lastModified, content}`) using the
  `<source>_<DetailTitle>.json` naming convention.
- **Import** builds a pre-flight plan first — create / update / unchanged /
  error per file, with validation — and only registers the entries you tick.
- **Simplify** is a Rust port of the repo's `scripts/simplifySchemas.js`: leave
  the `AWSEvent` envelope untouched, set `additionalProperties: true`, and keep
  only ID-like fields (`id`, `ids`, `uuid`, `arn`) in `required`. The registry
  stores the simplified form, so enable it when importing from the repo's full
  `schemas/` directory and disable it when importing from `schemas-simplified/`.

## Testing

```bash
pnpm test               # frontend unit tests (vitest)
pnpm build              # typecheck + frontend build
cd src-tauri && cargo test
```

Two suites worth knowing about:

- A **parity test** runs the Rust `simplify` port over the repo's `schemas/`
  directory and asserts it reproduces the committed `schemas-simplified/`
  output — 292 real schemas. Skipped when the repo is not checked out; point it
  elsewhere with `GLOBAL_EVENT_BUS_PATH`.
- The **structure editor's edit operations** are covered in
  `src/lib/schema-model.test.ts`. These rewrite the user's document, so the
  tests pin the things that would corrupt a schema silently: unmodelled
  keywords surviving, renames keeping field order and updating `required`, and
  every operation leaving its input untouched.
- `tests/reality_live.rs` runs the reality check end-to-end against live AWS —
  read a schema, pull matching events, analyse them. It is `#[ignore]`d by
  default since it needs credentials:

  ```bash
  cd src-tauri && cargo test --test reality_live -- --ignored --nocapture
  ```

  It exists because a unit test asserting a CloudWatch filter pattern *string*
  cannot tell you the API rejects it — which is exactly the bug it caught
  (`$."detail-type"` is invalid; the hyphenated key must be unquoted).
- `tests/report_live.rs` does the same for the registry-wide report, proving the
  single-scan design actually grades a real registry.
- `tests/infer_live.rs` infers schemas from real unregistered traffic and
  asserts each one both passes our validator *and* accepts the very events it
  was inferred from — a draft that rejects its own source events would be wrong
  on arrival.
- `tests/cache_live.rs` proves the cache earns its place — it measures the fetch
  against the cached read, and checks that an overlapping refetch dedupes and
  that the cache survives a restart.

## Architecture

```
src/                          React + TypeScript
  lib/ipc.ts                  typed wrappers over invoke()
  lib/types.ts                mirrors the Rust serde types
  lib/schema-model.ts         tree over the document + immutable edits
  lib/sample-event.ts         example event generation
  components/schema-editor/   tree, inspector, sample panel
  components/                 UI primitives, Monaco setup
  app/                        shell, routing, settings + login context
  features/{schemas,ai,logs,topology,settings}/

src-tauri/src/                Rust
  aws/{profiles,sso,clients}  config parsing, device-code login, SDK cache
  schema/{model,simplify,validate}
  commands/{auth,schemas,bedrock,logs,topology,settings}
  settings.rs, state.rs, error.rs
```

Notes on a few choices:

- **Rust owns AWS.** The webview gets IPC, URL opening and folder pickers, and
  nothing else — no shell access, no credentials.
- **Errors are typed.** Commands reject with `{ kind, message }`, so the UI can
  offer a **Sign in** button on `auth` failures instead of showing a wall of
  text.
- **Monaco is trimmed** to the core editor plus JSON; the default entry point
  bundles every language Monaco ships (~9MB of unused workers).
- **Settings live in Rust** and are written atomically to the platform config
  directory (`~/Library/Application Support/dev.codenaked.pontifex` on
  macOS). Only profile *names* are stored — never credentials.
- **`.gitignore` anchors `/logs`.** A bare `logs` entry also matches
  `src/features/logs/`, which silently excludes that feature from git *and*
  from Tailwind's class scanning, since Tailwind v4 honours `.gitignore` when
  detecting sources.

## Developer mode

**Settings → Developer → Developer mode** adds a Developer tab. It answers "is
Pontifex behaving as expected", which is otherwise only visible from a terminal
the app was not started from:

- **Runtime** — version, debug/release build, Tauri version, platform, log level.
- **Storage** — the config, cache and log directories with their size and file
  count, each openable in the file manager. This is how you find out the event
  cache has grown to 12 MB.
- **Caches** — resolved SDK configs held in memory, cached event types and
  events, with a clear button.
- **Logs** — the application log, filterable by **category**, level and text,
  with follow and a line-count selector. Pontifex logs to a file in *all* builds,
  not only debug, so there is a record after the fact.
- **Copy diagnostics** — versions, paths, sizes and recent errors as one block
  to paste into a bug report.

### Scanning

`FilterLogEvents` paginates from the start time, so a capped linear scan reads a
*contiguous, recent* slice of the window. Measured against `prd-global-events`
over 24 hours with a 5,000-event cap, that slice was **15 minutes** — every
event type that only fires overnight read as "no traffic".

Scans are striped instead: the window is cut into 8 equal slices scanned
concurrently, each with its own share of the event budget, so no single busy
period can spend the whole allowance. The same measurement, striped:

| | events | types | window covered | wall clock |
| --- | --- | --- | --- | --- |
| linear | 5,000 | 145 | 15 min of 1,440 | 6,603 ms |
| striped | 5,000 | 144 | 1,022 min of 1,440 | 3,667 ms |

Same breadth of event types, 68× the temporal coverage, and faster — the slices
are independent requests. `tests/scan_stripes_live.rs` reproduces this against a
real log group.

**Scan up to** on the health report bounds the wall clock (15s / 30s / 60s /
90s / 120s). It bounds how long you *wait*, not how much of the window is
covered: a smaller budget thins every slice evenly rather than truncating one
end. When a scan does stop early, the report says whether it ran out of time or
out of event budget, because those call for different remedies.

One caveat: with a very low event cap the per-slice share gets thin, and a
shallow slice sees fewer distinct types than a deep one. At 1,500 events the
striped scan found 64 types to the linear scan's 115. The default of 5,000 is
well clear of that.

### Logging

Every line is tagged with a category, and the Developer tab filters on it —
that is what makes verbose logging usable rather than a wall of text. Lines are
`[rfc3339][LEVEL][category] message`:

| Category | What it covers |
| --- | --- |
| `app` | Startup, settings load — the app's own lifecycle |
| `ipc` | Every backend call, with arguments named and duration measured |
| `ui` | Anything the webview logs, forwarded into the same file |
| `aws` | Credential resolution, STS identity, SDK config caching |
| `sso` | Device-code flow, token cache, session discovery |
| `registry` | EventBridge Schemas API — notably every create, update and delete |
| `schema` | Parsing, validating, simplifying and inferring documents |
| `events` | CloudWatch scans and reality checks |
| `cache` | The on-disk event sample cache, including evictions |
| `bedrock` | Bedrock model calls |
| `topology` | Bus and rule topology |
| `settings` | Reading and writing settings |

**Settings → Developer → Log level** controls volume, applied on next launch:

- `info` (default) — normal operations, failures, and any call slower than 2s.
- `debug` — every backend call and its duration. This is the one to use when
  something is behaving oddly.
- `trace` — everything.

Complete IPC coverage comes from one wrapper around `invoke` in `src/lib/ipc.ts`
rather than 47 hand-edited commands. Argument *values* are deliberately never
logged: they include whole schema documents and event payloads. The AWS SDK's
own loggers are pinned to `warn`, since at debug they bury the app's lines at
exactly the level you turned on to see them.

Registry writes and deletes log at `info` regardless of level — "who changed prd
and when" has to stay answerable.

The event cache is capped at 32 MB as well as by count, evicting
least-recently-used event types. The count caps alone were not enough: payloads
vary hugely in size, and 500 types × 200 events of real traffic could run to
hundreds of megabytes.

## Packaging

`pnpm tauri build` produces a `.dmg` and `.app` on macOS, `.msi`/NSIS on
Windows, and `.deb`/AppImage on Linux. Bundles are unsigned; add signing
identities to `src-tauri/tauri.conf.json` before distributing.

**macOS `.dmg` needs Finder automation permission.** Tauri's DMG step drives
Finder over AppleScript to arrange the window, and without permission it fails
with `Finder got an error: AppleEvent timed out (-1712)` *after* the `.app` has
already been built successfully. Grant your terminal Automation → Finder access
in System Settings → Privacy & Security, or ship the `.app` directly:

```bash
# The .app is complete even when the .dmg step fails.
open src-tauri/target/release/bundle/macos/
```
