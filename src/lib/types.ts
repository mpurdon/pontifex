/**
 * Mirrors the serde types in `src-tauri/src`. Every Rust struct that crosses
 * the IPC boundary uses `#[serde(rename_all = "camelCase")]`, so these are a
 * direct transliteration.
 */

// --- errors ---------------------------------------------------------------

export type ErrorKind =
  | 'auth'
  | 'forbidden'
  | 'notFound'
  | 'invalid'
  | 'aws'
  | 'internal'

/** Every rejected `invoke` resolves to this shape. See `error.rs`. */
export interface IpcError {
  kind: ErrorKind
  message: string
}

// --- settings -------------------------------------------------------------

/**
 * An account + role reachable through a signed-in SSO session. When set on an
 * environment, pontifex resolves credentials itself and no AWS profile is needed.
 */
export interface SsoTarget {
  session: string
  accountId: string
  roleName: string
  accountName?: string | null
}

export interface Environment {
  id: string
  label: string
  /** Used when `sso` is unset. */
  awsProfile: string
  /** Takes precedence over `awsProfile` when present. */
  sso?: SsoTarget | null
  region: string
  registryName: string
  logGroups: string[]
  protected: boolean
}

export interface SsoSession {
  name: string
  startUrl: string
  ssoRegion: string
  /** Profiles in ~/.aws/config that reference this session. */
  profiles: string[]
}

/** `SsoSession` flattened together with its sign-in state. */
export interface SsoSessionStatus extends SsoSession {
  sso: SsoStatus
}

export interface SsoAccount {
  accountId: string
  accountName: string | null
  email: string | null
  /** True when the account name looks like a Global Event Bus account. */
  matchesEventBus: boolean
}

export interface BedrockModel {
  id: string
  label: string
  modelId: string
}

export interface LlmSettings {
  awsProfile: string | null
  region: string
  models: BedrockModel[]
  selectedModelId: string | null
  claudeSettingsPath: string | null
}

/** Limits on how much work a scan does, and how much of it is kept. */
export interface ScanSettings {
  /** Wall-clock ceiling on a scan, in seconds. */
  maxSeconds: number
  /** Events collected per scan, divided across the time slices. */
  maxEvents: number
  /** Disk budget for the sampled-event cache, in megabytes. */
  cacheMb: number
}

/** One routing rule: which Jira project owns an event source. */
export interface SourceRoute {
  id: string
  /** Glob over the event source; `*` is the only metacharacter. */
  pattern: string
  projectKey: string
  issueType: string | null
  labels: string[]
  assigneeAccountId: string | null
}

/**
 * A routing rule matched against who owns the producer rather than what it is
 * called: the CODEOWNERS team (`@org/team`) or repository (`org/repo`) the
 * origin lookup found. Same shape as a source rule.
 */
export type OwnerRoute = SourceRoute

/**
 * Everything about Jira that is not a secret.
 *
 * The client secret and OAuth tokens are absent by design — they live in the
 * OS keychain, since this object is persisted as plain JSON.
 */
/**
 * An event source that exists, and the spelling a routing rule must match.
 *
 * These differ when a source holds a character a schema name cannot: events
 * from `Atomic Forms` are registered as `Atomic-Forms`, and a ticket about
 * them carries the registered spelling.
 */
export interface EventSource {
  /** As the events carry it. */
  source: string
  /** As a routing rule must spell it. */
  pattern: string
  /** Seen in sampled traffic, as opposed to only being registered. */
  inTraffic: boolean
}

/** A value a Jira field will accept. */
export interface AllowedValue {
  id: string
  label: string
}

/** A field a project makes mandatory and pontifex does not already set. */
export interface RequiredField {
  fieldId: string
  name: string
  /** `schema.type`: `string`, `option`, `array`, `number`, `user`, … */
  fieldType: string
  /** Element type, when `fieldType` is `array`. */
  items: string | null
  /**
   * Jira claims it supplies this one.
   *
   * Shown but never blocking — the claim is not always true of a REST create.
   */
  hasDefault: boolean
  allowedValues: AllowedValue[]
}

export interface JiraSettings {
  clientId: string
  /** The callback URL registered in the developer console, used verbatim. */
  callbackUrl: string
  defaultProject: string | null
  issueType: string
  labels: string[]
  routes: SourceRoute[]
  ownerRoutes: OwnerRoute[]
  /**
   * Values for fields a project makes mandatory, keyed by project then field id.
   *
   * The demand belongs to the project rather than to any one rule, so several
   * rules pointing at one project share these.
   */
  fieldDefaults: Record<string, Record<string, unknown>>
}

/** Where producer code lives, for the origin lookup. The token is in the keychain or borrowed from the GitHub CLI. */
export interface GithubSettings {
  /** Organisation to search; unset reads it from the bus checkout's remote. */
  org: string | null
  /** Path globs the origin lookup never examines. CODEOWNERS pattern rules. */
  ignore: string[]
}

export interface Settings {
  environments: Environment[]
  activeEnvironmentId: string | null
  llm: LlmSettings
  scan: ScanSettings
  jira: JiraSettings
  github: GithubSettings
  eventBusRepoPath: string | null
  /** Sources kept at the top of the schema list. */
  pinnedSources: string[]
  /** Persisted panel sizes, keyed by layout id. */
  panelSizes: Record<string, number[]>
  /** Reveals the Developer tab. */
  developerMode: boolean
  /** `off` | `error` | `warn` | `info` | `debug` | `trace`. */
  logLevel: string
}

// --- developer tools ------------------------------------------------------

export interface StorageLocation {
  id: string
  label: string
  path: string
  exists: boolean
  bytes: number
  files: number
  note: string
}

export interface RuntimeInfo {
  appVersion: string
  tauriVersion: string
  os: string
  arch: string
  profile: 'debug' | 'release'
  logLevel: string
}

export interface CacheInfo {
  sdkConfigs: number
  eventTypes: number
  cachedEvents: number
}

export interface DevInfo {
  runtime: RuntimeInfo
  storage: StorageLocation[]
  caches: CacheInfo
  logFile: string | null
}

export interface ClaudeSettingsImport {
  sourcePath: string
  awsProfile: string | null
  region: string | null
  models: BedrockModel[]
}

// --- auth -----------------------------------------------------------------

export type ProfileKind = 'ssoSession' | 'ssoLegacy' | 'assumeRole' | 'static'

export interface SsoStatus {
  applicable: boolean
  hasToken: boolean
  /** Past use and not silently renewable; a lapsed token with a live refresh token is not expired. */
  expired: boolean
  expiresAt: string | null
  /** The access token renews on its own until the SSO session itself ends. */
  refreshable: boolean
}

/** `AwsProfile` flattened together with its `SsoStatus`. */
export interface ProfileStatus {
  name: string
  kind: ProfileKind
  region: string | null
  ssoStartUrl: string | null
  ssoRegion: string | null
  ssoAccountId: string | null
  ssoRoleName: string | null
  ssoSession: string | null
  sourceProfile: string | null
  /**
   * Credentials live in ~/.aws/credentials, written by an external manager
   * (Leapp, aws-vault, …). pontifex cannot refresh them.
   */
  externallyManaged: boolean
  sso: SsoStatus
}

export interface CallerIdentity {
  account: string | null
  arn: string | null
  userId: string | null
}

export interface ProfileCheck {
  profile: string
  region: string
  ok: boolean
  identity: CallerIdentity | null
  error: IpcError | null
  needsLogin: boolean
}

export interface DeviceAuthorization {
  verificationUri: string
  verificationUriComplete: string | null
  userCode: string
  expiresIn: number
  interval: number
}

export interface LoginResult {
  method: 'native' | 'cli' | 'cliFallback'
  message: string
  expiresAt: string | null
}

// --- schemas --------------------------------------------------------------

export interface RegistrySummary {
  name: string
  arn: string | null
  description: string | null
  tags: Record<string, string>
}

export interface SchemaSummary {
  name: string
  source: string | null
  detailType: string | null
  version: string | null
  lastModified: string | null
  arn: string | null
}

export type Severity = 'error' | 'warning'

/**
 * A mechanical whole-document repair that clears a finding.
 *
 * Sent by the validator rather than inferred from the message, so rewording a
 * sentence cannot silently remove the button that acts on it.
 */
export type Fix = 'widenNullable'

export interface Finding {
  severity: Severity
  /** JSON Pointer into the document. */
  path: string
  message: string
  fix?: Fix
}

export interface ValidationReport {
  valid: boolean
  findings: Finding[]
  identity: { source: string; detailType: string } | null
}

export interface SchemaDetail {
  name: string
  version: string
  schemaType: string
  description: string | null
  lastModified: string | null
  arn: string | null
  content: unknown
  validation: ValidationReport
}

export interface SchemaVersionSummary {
  version: string
  schemaType: string | null
}

/** One version in a schema's history, with the document as it stood then. */
export interface SchemaHistoryEntry {
  version: string
  createdAt: string | null
  content: unknown
}

export interface WriteResult {
  name: string
  version: string
  created: boolean
  validation: ValidationReport
}

export interface DetailProperty {
  name: string
  type: string
}

export interface NewSchemaDraft {
  name: string
  content: unknown
  fileName: string
  validation: ValidationReport
}

export interface SimplifyChange {
  schema: string
  requiredBefore: number
  requiredAfter: number
}

export interface SimplifyPreview {
  content: unknown
  changes: SimplifyChange[]
}

/** Result of rewriting `nullable: true` into the spelling Ajv honours. */
export interface NullableRepair {
  content: unknown
  /** JSON Pointers to the fields that were widened. */
  changed: string[]
}

export type ImportAction = 'create' | 'update' | 'unchanged' | 'error'

export interface ImportPlanEntry {
  file: string
  name: string
  action: ImportAction
  message: string | null
  content: unknown
  validation: ValidationReport | null
  simplifyChanges: SimplifyChange[]
}

export interface ImportPlan {
  directory: string
  entries: ImportPlanEntry[]
}

export interface ApplyImportResult {
  applied: string[]
  /** `[schemaName, errorMessage]` pairs. */
  failed: [string, string][]
}

export interface ExportResult {
  directory: string
  written: string[]
  failed: [string, string][]
}

// --- reality check --------------------------------------------------------

export interface FailureGroup {
  /** JSON Pointer into the event payload. */
  pointer: string
  message: string
  count: number
  example: unknown | null
}

export interface FieldObservation {
  /** Dotted path within the payload, e.g. `metadata.trackingId`. */
  path: string
  types: string[]
  seenIn: number
  example: unknown | null
}

export interface UnusedField {
  path: string
  required: boolean
}

export interface TypeMismatch {
  path: string
  declared: string
  observed: string[]
  /** Events containing the field at all. */
  seenIn: number
  /** Events that carried one of the `observed` types — the number that matters. */
  mismatchedIn: number
  example: unknown | null
  /**
   * The type set that would accept every event in the sample.
   *
   * Not `observed`, which holds only the types the declaration forbids: for a
   * field declared `string` that is null in 7% of events, `observed` is
   * `["null"]` alone, and redeclaring it as that would fix the 7% by rejecting
   * the other 93%.
   */
  suggested: string[]
}

export interface EnumDrift {
  path: string
  declared: unknown[]
  unexpected: unknown[]
  seenIn: number
  /** Events that carried a value outside the enum. */
  unexpectedIn: number
}

export type IssueKind =
  | 'wrongType'
  | 'outsideEnum'
  | 'missingRequired'
  | 'undeclared'
  | 'neverSeen'
  | 'rejected'
  | 'unregistered'
  | 'concern'

export type IssueSeverity = 'error' | 'warning' | 'info'

/** A consumer file, as much of it as an issue needs to name it. */
export type Reader = Pick<ProducerOrigin, 'repo' | 'path' | 'url' | 'owners'>

/**
 * What the consumers found say about an issue. Present only once the origin
 * lookup has found a file reading fields off the event; absent, the severity
 * is the validator's alone.
 */
export interface Impact {
  /** Consumer files found reading fields off this event at all. */
  handlers: number
  /** The ones that read this field, or something inside it. */
  readers: Reader[]
  /**
   * The ones that take an ancestor of the field along whole, so may read it
   * where the scan cannot see. Withholds "nobody reads it".
   */
  indirect: Reader[]
}

/**
 * One disagreement between a schema and its traffic.
 *
 * Merges what used to be two overlapping lists — validation failures and drift
 * — so a single problem is a single row. `rejects` is the difference between
 * "events are being thrown away" and "the schema is merely out of date".
 */
export interface Issue {
  /** Stable across runs, so the same problem is recognisable in a later one. */
  key: string
  kind: IssueKind
  severity: IssueSeverity
  /** Dotted path; empty for a problem with the payload as a whole. */
  path: string
  /** What disagrees. Paths are wrapped in backticks. */
  summary: string
  /** The choice this puts in front of you. */
  action: string
  declared: string | null
  observed: string | null
  affected: number
  sampled: number
  /** Whether validation rejects these events today. */
  rejects: boolean
  example: unknown | null
  /** The validator's own message, when one applies. */
  message: string | null
  /**
   * The edit that would clear this issue, where one can be computed.
   *
   * Carried on the issue rather than worked out in the panel, so a button
   * cannot drift from the row it sits on the way one driven by matching the
   * summary text would.
   */
  fix?: Repair
  /**
   * Who reads this field, and what `severity` was graded by above the
   * validator's floor. Absent until the origin lookup has found a consumer
   * that reads fields off the event.
   */
  impact?: Impact
}

/**
 * A mechanical edit to a schema that clears one drift issue.
 *
 * Applied to the draft and reviewed as a diff — never written straight to AWS.
 * The operands travel with it so the repair that is applied is the one that was
 * offered, rather than something re-derived from a path and a kind.
 */
export type Repair =
  /** Redeclare a field as the types real events carry. */
  | { kind: 'widenType'; types: string[] }
  /** Admit values the producers already send. */
  | { kind: 'extendEnum'; values: unknown[] }
  /** Stop requiring a field that events omit. */
  | { kind: 'dropRequired' }
  /** Declare a field that events send and the schema does not describe. */
  | { kind: 'declareField'; types: string[]; example?: unknown }

/** What a model proposes for an issue the local planner cannot decide. */
export interface RepairSuggestion {
  /** Null when the model declined, which is a verdict rather than a failure. */
  repair: Repair | null
  rationale: string
  modelId: string
}

export interface DriftReport {
  undeclared: FieldObservation[]
  unused: UnusedField[]
  missingRequired: FieldObservation[]
  typeMismatches: TypeMismatch[]
  enumDrift: EnumDrift[]
}

export interface RealityCheckResult {
  sampled: number
  passed: number
  failed: number
  /** Every disagreement, deduplicated and ranked. This is what the panel shows. */
  issues: Issue[]
  failures: FailureGroup[]
  drift: DriftReport
  /** Declared dotted path → how many sampled events contained it. */
  coverage: Record<string, number>
  typeName: string
  logGroup: string
  source: string
  detailType: string
  minutes: number
  /** Explains an empty sample rather than showing a misleadingly clean report. */
  note: string | null
  /** True when served from cache rather than a fresh fetch. */
  fromCache: boolean
  cacheAgeMs: number | null
}

export type ReportStatus = 'failing' | 'error' | 'drifting' | 'ok' | 'noTraffic'

export interface ReportRow {
  name: string
  source: string
  detailType: string
  /** Events actually graded — capped per type, so not a measure of volume. */
  sampled: number
  /** Every matching event seen in the window. This is the volume figure. */
  observed: number
  passed: number
  failed: number
  undeclared: number
  typeMismatches: number
  enumDrift: number
  missingRequired: number
  status: ReportStatus
  headline: string | null
  /**
   * The identity the events carry, when it differs from the registered name.
   *
   * A source may hold characters a registry name cannot, so `Atomic Forms` is
   * registered as `Atomic-Forms`. Anything deriving the schema name from an
   * event's own `source` is then looking for a name that does not exist.
   */
  wireIdentity?: string
  /** The schema version this row graded; `null` for a row never described. */
  version: string | null
  /**
   * When that version was written, in the same form `SchemaSummary` carries.
   *
   * A schema whose listed timestamp no longer matches has been saved since
   * the report — which is how the Health screen tells a row is dealt with
   * without re-running the scan.
   */
  lastModified: string | null
}

/** An event type on the bus with no schema registered for it. */
export interface UnregisteredEvent {
  source: string
  detailType: string
  count: number
}

export interface RegistryReport {
  rows: ReportRow[]
  unregistered: UnregisteredEvent[]
  scannedEvents: number
  minutes: number
  /**
   * Every log group this report read.
   *
   * More than one for an environment that takes events over the bridge as
   * well as directly — reading only the first made "0 unregistered" a claim
   * about half the traffic.
   */
  logGroups: string[]
  registry: string
  /** Why the scan stopped early, when it did. */
  scanNote: string | null
  /** Time slices the window was split into. */
  stripes: number
  scanMs: number
  /** True when the scan hit its cap, so "no traffic" is not proof. */
  truncated: boolean
  /** When the scan finished, epoch millis. */
  generatedAt: number
}

/** A schema drafted from observed traffic. */
export interface InferredDraft {
  name: string
  content: unknown
  fileName: string
  validation: ValidationReport
  /** How many events the shape was inferred from. */
  sampled: number
  fromCache: boolean
}

export interface RegistryReportRequest {
  minutes?: number
  logGroup?: string
  maxEvents?: number
  /** Wall-clock ceiling on the CloudWatch scan, in seconds. */
  maxSeconds?: number
}

export interface RealityCheckRequest {
  name: string
  content: unknown
  typeName?: string
  minutes?: number
  limit?: number
  logGroup?: string
  /** Force a fetch even when the cache covers the window. */
  refresh?: boolean
  /** Never touch AWS — validate against cached events only. */
  cachedOnly?: boolean
  /** Keep the result as the schema's last analysis, shown when it is opened again. */
  persist?: boolean
  /** Wall-clock ceiling on the CloudWatch scan, in seconds. */
  maxSeconds?: number
}

// --- bedrock --------------------------------------------------------------

export type AiAction = 'generate' | 'refactor' | 'explain'

export interface AiRequest {
  action: AiAction
  prompt: string
  schema?: unknown
  source?: string
  detailType?: string
  modelId?: string
  examples?: unknown[]
}

export interface AiResponse {
  text: string
  content: unknown | null
  validation: ValidationReport | null
  modelId: string
}

// --- logs -----------------------------------------------------------------

export interface LogGroupSummary {
  name: string
  storedBytes: number | null
  retentionDays: number | null
  configured: boolean
}

export interface LogQuery {
  logGroup: string
  /** Epoch milliseconds. */
  startTime: number
  endTime?: number
  source?: string
  detailType?: string
  filterPattern?: string
  limit?: number
  nextToken?: string
}

export interface LogEvent {
  timestamp: number | null
  ingestionTime: number | null
  logStream: string | null
  message: string
  event: Record<string, unknown> | null
  source: string | null
  detailType: string | null
  eventId: string | null
}

export interface LogPage {
  events: LogEvent[]
  nextToken: string | null
  filterPattern: string | null
}

// --- topology -------------------------------------------------------------

export interface DeclaredBus {
  sourceBusName: string
  sourceArn: string
  category: 'trajector' | 'thirdParty'
  stage: string
  accountId: string | null
  region: string | null
}

export interface RuleTarget {
  id: string
  arn: string
  service: string | null
}

export interface LiveRule {
  name: string
  state: string | null
  description: string | null
  eventPattern: string | null
  targets: RuleTarget[]
}

export interface LiveBus {
  name: string
  arn: string | null
  rules: LiveRule[]
}

export interface TopologyDrift {
  declaredOnly: DeclaredBus[]
  liveOnly: string[]
}

export interface Topology {
  stage: string
  region: string
  accountId: string | null
  declared: DeclaredBus[]
  live: LiveBus[]
  drift: TopologyDrift
  configWarning: string | null
  configPath: string | null
}

// --- jira -----------------------------------------------------------------

export interface JiraSite {
  id: string
  name: string
  url: string
  scopes: string[]
  avatarUrl: string | null
}

export interface JiraStatus {
  /** An app is registered — a client ID and secret exist. */
  configured: boolean
  /** There is a live grant. */
  connected: boolean
  site: { cloudId: string; name: string | null; url: string | null } | null
  /** Why it is not usable, when it is not. */
  detail: string | null
}

/** What the console's authorization URL generator tells us. */
export interface AuthorizeUrlParts {
  clientId: string
  callbackUrl: string
  scopes: string[]
  /** Scopes pontifex needs that the app does not have. */
  missingScopes: string[]
}

export interface JiraProject {
  key: string
  name: string
}

/** A ticket that exists in Jira. */
export interface FiledTicket {
  key: string
  url: string
  status?: string
  summary?: string
}

/** Where an issue was found — everything a reader needs to reproduce it. */
export interface TicketContext {
  schemaName: string
  environment: string
  registry?: string | null
  source: string
  detailType: string
  logGroup?: string | null
  minutes?: number | null
  typeName?: string | null
  /** Who publishes it, when the origin lookup has run. Filled in by the backend. */
  origin?: TicketOrigin | null
}

/** The publisher, as the origin lookup found it, for the ticket body and routing. */
export interface TicketOrigin {
  repo: string
  path: string
  url: string
  owners: string[]
  introducedBy: string | null
  pullUrl: string | null
  publisher: boolean
}

/** A ticket, fully rendered, before anyone has agreed to create it. */
export interface TicketDraft {
  projectKey: string
  issueType: string
  summary: string
  description: string
  labels: string[]
  /** Values for fields the target project makes mandatory. */
  fields: Record<string, unknown>
  /** Identifies this exact problem across runs, as a label. */
  fingerprint: string
  assigneeAccountId: string | null
  /** Why this project — so routing is auditable. */
  routedBy: string
}

export interface TicketPreview extends TicketDraft {
  /** An open ticket already filed for this exact problem. */
  existing: FiledTicket | null
}

export type FileOutcome = 'created' | 'commented' | 'skipped' | 'failed'

export interface FileTicketResult {
  issueKey: string
  schemaName: string
  outcome: FileOutcome
  ticket: FiledTicket | null
  error: IpcError | null
}

export interface FileTicketRequest {
  issue: Issue
  context: TicketContext
  /** Edits made in the preview. */
  summary?: string
  description?: string
  projectKey?: string
  issueType?: string
  /** Values for the project's mandatory fields, as answered in the preview. */
  fields?: Record<string, unknown>
  /** Comment on this existing ticket instead of creating a second one. */
  commentOn?: string
}

/** Issues re-derived for one schema, for filing several at once. */
export interface SchemaIssues {
  schemaName: string
  context: TicketContext
  issues: Issue[]
  /** Why this schema produced nothing, when it produced nothing. */
  note: string | null
  error: IpcError | null
}

// --- watch mode -----------------------------------------------------------

export type WatchOp = 'eq' | 'ne'

/** One `field = value` test on the event envelope. */
export interface WatchCondition {
  /** Dotted path from the envelope root, e.g. `detail.clientId`. */
  path: string
  op: WatchOp
  /** Unquoted numbers compare numerically; `*` is a wildcard; quotes force a string. */
  value: string
}

/** Something the passive poller should say when it goes past. */
export interface Watch {
  id: string
  envId: string
  label: string
  enabled: boolean
  /** Which of the environment's log groups to poll. `null` = its bus events group. */
  logGroup: string | null
  source: string | null
  detailType: string | null
  conditions: WatchCondition[]
  /** A complete CloudWatch filter pattern; overrides the structured fields. */
  rawPattern: string | null
  /** Raise a desktop notification on a hit. */
  notify: boolean
  /** A CSS colour for this watch's hits; `null` picks a stable one from the palette; `'default'` is the plain badge. */
  color: string | null
}

/** What the editor shows about a watch as it is typed. */
export interface CompiledWatch {
  /** The CloudWatch filter pattern; `null` matches everything. */
  pattern: string | null
  /** The one-line description the backend uses as the label fallback. */
  summary: string
}

export type NotifierStatus = 'delivered' | 'denied' | 'error' | 'timeout' | 'pending'

/** What the notification helper reported after a test. */
export interface NotifierOutcome {
  /** `pending` while a permission prompt waits. */
  status: NotifierStatus
  message: string
  /** Where the helper app is installed. */
  helper: string
}

/** A moment watching started or stopped, so the hit list can show sessions as groups. */
export interface WatchMark {
  id: string
  envId: string
  kind: 'start' | 'stop'
  /** Epoch ms. */
  at: number
}

/** One event that satisfied a watch. */
export interface WatchHit {
  id: string
  watchId: string
  watchLabel: string
  envId: string
  logGroup: string
  /** The EventBridge event id, or the CloudWatch one when the envelope had none. */
  eventId: string
  /** The log event's timestamp, epoch ms. */
  timestamp: number
  /** When the poller saw it, epoch ms. */
  receivedAt: number
  source: string | null
  detailType: string | null
  /** The full envelope as it landed in CloudWatch. */
  event: unknown
  /** From the look-back pass that runs on arming: context, not news. Never notified. */
  backfill: boolean
}

/** How often a watch would have fired over recent history. */
export interface WatchProbe {
  hours: number
  /** Matches found; a floor when `truncated`. */
  count: number
  truncated: boolean
  lastAt: number | null
  /** `source@detail-type` → count, most frequent first. */
  byType: [string, number][]
}

/** What one environment's poller is doing. */
export interface WatchStatus {
  envId: string
  /** Persisted intent: the poller should be running. */
  armed: boolean
  /** Live fact: the task exists. */
  running: boolean
  pollSeconds: number
  /** Minutes without anyone touching the app before it asks whether to keep watching; 0 never asks. */
  idleTimeoutMinutes: number
  lastPollAt: number | null
  nextPollAt: number | null
  lastError: string | null
  /** Set while parked on an auth failure. */
  paused: string | null
  note: string | null
  polls: number
  lastFetched: number
  /** Events fetched since this poller started. */
  fetchedTotal: number
  /** CloudWatch calls each pass makes. */
  callsPerPass: number
  /** Wall-clock time of the last pass, ms. */
  lastPassMs: number
  /** Watches with no server-side filter at all. */
  unfiltered: number
  hits: number
  unread: number
  seenAt: number
  /** When this poller started, epoch ms. Older hits are look-back; newer were caught live. */
  watchingSince: number | null
}

// --- origin ---------------------------------------------------------------

/** A person and a moment, from a commit. */
export interface Authorship {
  author: string
  email: string | null
  login: string | null
  /** RFC 3339. */
  date: string
  sha: string
  subject: string
  commitUrl: string | null
  pullNumber: number | null
  pullTitle: string | null
  pullUrl: string | null
  /** The pull request's author, when it differs from the commit's. */
  pullAuthor: string | null
}

/** One place in the organisation's code that carries the event type. */
export interface ProducerOrigin {
  repo: string
  path: string
  url: string
  introduced: Authorship | null
  owners: string[]
  /** A test, fixture or document rather than the producer itself. */
  incidental: boolean
  /** What the file does with the event type. */
  role: 'publisher' | 'consumer' | 'mention'
  /** Fields it reads off the event detail, as dotted paths. */
  reads: string[]
}

export interface GithubOutcome {
  status: 'ok' | 'unconfigured' | 'error'
  message: string | null
  org: string | null
  /** Matches the search reported, before the ignore list. */
  totalMatches: number
  /** Matches the ignore list kept from being examined. */
  skipped: number
  /** Which ones, and by which pattern. */
  skippedFiles: SkippedMatch[]
}

export interface SkippedMatch {
  repo: string
  path: string
  url: string
  /** The ignore-list entry that matched. */
  pattern: string
}

export interface EventOrigin {
  source: string
  detailType: string
  /** The commit that wired the type into the bus repo, from the local checkout. */
  wiring: Authorship | null
  wiringRepoUrl: string | null
  producers: ProducerOrigin[]
  github: GithubOutcome
  cachedAt: number
}

export interface GithubStatus {
  org: string | null
  orgFromRemote: string | null
  tokenSource: 'keychain' | 'ghCli' | 'none'
  /** The ignore list when left alone, for the placeholder and the reset button. */
  defaultIgnore: string[]
}

/** A schema's last persisted reality check, kept for a month. */
export interface CachedAnalysis {
  result: RealityCheckResult
  /** When it was run, epoch ms. */
  analysedAt: number
}
