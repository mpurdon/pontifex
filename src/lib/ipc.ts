import { invoke as rawInvoke } from '@tauri-apps/api/core'
import type {
  AiRequest,
  AiResponse,
  AuthorizeUrlParts,
  ApplyImportResult,
  BedrockModel,
  ClaudeSettingsImport,
  DeclaredBus,
  DetailProperty,
  DevInfo,
  Environment,
  EventSource,
  FileTicketRequest,
  FileTicketResult,
  ExportResult,
  FieldObservation,
  ImportPlan,
  InferredDraft,
  IpcError,
  Issue,
  JiraProject,
  JiraSite,
  JiraStatus,
  RequiredField,
  LogGroupSummary,
  LogPage,
  LogQuery,
  LoginResult,
  NewSchemaDraft,
  ProfileCheck,
  ProfileStatus,
  RealityCheckRequest,
  RealityCheckResult,
  RegistryReport,
  RegistryReportRequest,
  RegistrySummary,
  Repair,
  RepairSuggestion,
  SchemaDetail,
  SchemaHistoryEntry,
  SchemaIssues,
  SchemaSummary,
  SchemaVersionSummary,
  Settings,
  TimeZone,
  Openapi30Repair,
  SimplifyPreview,
  SsoAccount,
  SsoSessionStatus,
  TicketContext,
  TicketPreview,
  Topology,
  ValidationReport,
  Watch,
  CachedAnalysis,
  CompiledWatch,
  EventOrigin,
  GithubStatus,
  NotifierOutcome,
  WatchHit,
  WatchMark,
  WatchProbe,
  WatchStatus,
  WriteResult,
} from './types'

/** Log levels the backend accepts from the webview. */
export type LogLevel = 'error' | 'warn' | 'info' | 'debug' | 'trace'

/**
 * Write into the application log.
 *
 * Fire-and-forget on purpose: logging must never be able to fail the thing it
 * is describing, and it must never recurse back through the instrumented
 * `invoke` below.
 */
export function log(level: LogLevel, category: string, message: string): void {
  void rawInvoke('ui_log', { level, category, message }).catch(() => {})
}

/**
 * Commands that must not be logged.
 *
 * `ui_log` would recurse. The other two are polled by the Developer tab every
 * few seconds while it is open, so logging them would fill the log with the act
 * of reading the log.
 */
const UNLOGGED = new Set(['ui_log', 'log_categories', 'read_app_logs', 'dev_info', 'watch_status'])

/** A call slower than this is worth seeing without turning on debug. */
const SLOW_CALL_MS = 2_000

/** Monotonic id so a call's start and finish can be paired in the log. */
let callSequence = 0

/** Calls issued but not yet settled, by call id. */
const inFlight = new Map<number, { describe: string; started: number }>()

/**
 * Backend calls issued but not yet returned, oldest first.
 *
 * A log that only records completions cannot show you a hang: the call that
 * never comes back is precisely the one that writes no line. The Developer tab
 * reads this so an in-flight call is visible while it is still stuck.
 */
export function inFlightCalls(): { describe: string; elapsedMs: number }[] {
  const now = performance.now()
  return [...inFlight.values()]
    .map(({ describe, started }) => ({ describe, elapsedMs: Math.round(now - started) }))
    .sort((a, b) => b.elapsedMs - a.elapsedMs)
}

/**
 * Every backend call, timed and recorded.
 *
 * This is the one place all 47 commands pass through, which is what makes
 * complete IPC logging possible without touching each command. Successful
 * calls log at debug (turn the level up to trace the app's every move); slow
 * ones and failures log loudly, because those are what you go looking for.
 *
 * Both the start and the finish are logged. Logging only the finish leaves a
 * call that never returns completely absent from the log, which is the one
 * case you most need it for.
 *
 * Argument *values* are deliberately not logged — they include whole schema
 * documents and event payloads. The names alone say which call this was.
 */
function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (UNLOGGED.has(command)) return rawInvoke<T>(command, args)

  const id = ++callSequence
  const started = performance.now()
  const argNames = args ? Object.keys(args).filter((k) => args[k] !== undefined) : []
  const describe = argNames.length ? `${command}(${argNames.join(', ')})` : `${command}()`

  inFlight.set(id, { describe, started })
  log('debug', 'ipc', `#${id} ${describe} — started`)

  const settle = () => {
    inFlight.delete(id)
    return Math.round(performance.now() - started)
  }

  return rawInvoke<T>(command, args).then(
    (result) => {
      const ms = settle()
      log(ms >= SLOW_CALL_MS ? 'warn' : 'debug', 'ipc', `#${id} ${describe} ok — ${ms}ms`)
      return result
    },
    (error: unknown) => {
      const ms = settle()
      const { kind, message } = asIpcError(error)
      log('error', 'ipc', `#${id} ${describe} failed after ${ms}ms — ${kind}: ${message}`)
      throw error
    },
  )
}

/**
 * Narrow an unknown rejection to our `IpcError`. Rust commands always reject
 * with `{ kind, message }`, but a panic or a serialization failure can still
 * surface as something else, so callers get a usable message either way.
 */
export function asIpcError(error: unknown): IpcError {
  if (
    typeof error === 'object' &&
    error !== null &&
    'kind' in error &&
    'message' in error
  ) {
    return error as IpcError
  }
  return { kind: 'internal', message: String(error) }
}

/**
 * Reject if a call has not returned in time.
 *
 * A Tauri command that never resolves — a credential provider stuck on a
 * timeout, say — would otherwise leave the UI spinning with nothing to show.
 * The underlying work may still finish; this only stops the wait.
 */
export function withTimeout<T>(
  promise: Promise<T>,
  ms: number,
  what: string,
): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) =>
      setTimeout(
        () =>
          reject({
            kind: 'internal',
            message: `${what} did not respond within ${Math.round(ms / 1000)}s. Credentials may be unresolvable — check the badge in the header.`,
          } satisfies IpcError),
        ms,
      ),
    ),
  ])
}

/** True when signing in would plausibly fix the error. */
export function needsLogin(error: unknown): boolean {
  return asIpcError(error).kind === 'auth'
}

// --- settings -------------------------------------------------------------

export const getSettings = () => invoke<Settings>('get_settings')

export const saveSettings = (settings: Settings) =>
  invoke<Settings>('save_settings', { settings })

/** Panel geometry only — see `save_panel_sizes` for why it is not a full save. */
export const savePanelSizes = (id: string, sizes: number[]) =>
  invoke<Settings>('save_panel_sizes', { id, sizes })

/** Persist the display zone without invalidating anything fetched from AWS. */
export const setTimeZone = (zone: TimeZone) =>
  invoke<Settings>('set_time_zone', { zone })

export const setActiveEnvironment = (envId: string) =>
  invoke<Settings>('set_active_environment', { envId })

export const defaultEnvironmentForStage = (stage: string, profile: string) =>
  invoke<Environment>('default_environment_for_stage', { stage, profile })

export const listStages = () => invoke<string[]>('stages')

export const importClaudeSettings = (path?: string) =>
  invoke<ClaudeSettingsImport>('import_claude_settings', { path })

// --- auth -----------------------------------------------------------------

export const listProfiles = () => invoke<ProfileStatus[]>('list_profiles')

export const checkProfile = (profile: string, region?: string) =>
  invoke<ProfileCheck>('check_profile', { profile, region })

/** Checks whichever credential source the environment uses. */
export const checkEnvironment = (envId: string) =>
  invoke<ProfileCheck>('check_environment', { envId })

export const ssoLogin = (profile: string, forceCli = false) =>
  invoke<LoginResult>('sso_login', { profile, forceCli })

export const refreshCredentials = () => invoke<void>('refresh_credentials')

// --- SSO sessions ---------------------------------------------------------

export const listSsoSessions = () =>
  invoke<SsoSessionStatus[]>('list_sso_sessions')

export const ssoSessionLogin = (session: string) =>
  invoke<LoginResult>('sso_session_login', { session })

export const listSsoAccounts = (session: string) =>
  invoke<SsoAccount[]>('list_sso_accounts', { session })

export const listSsoAccountRoles = (session: string, accountId: string) =>
  invoke<string[]>('list_sso_account_roles', { session, accountId })

/** Opt-in: writes an [sso-session]-backed profile block to ~/.aws/config. */
export const exportSsoProfile = (envId: string, profileName: string) =>
  invoke<string>('export_sso_profile', { envId, profileName })

// --- schemas --------------------------------------------------------------

export const listRegistries = (envId?: string) =>
  invoke<RegistrySummary[]>('list_registries', { envId })

export const listSchemas = (envId?: string) =>
  invoke<SchemaSummary[]>('list_schemas', { envId })

export const describeSchema = (
  name: string,
  version?: string,
  envId?: string,
) => invoke<SchemaDetail>('describe_schema', { name, version, envId })

/** Every version with its document and creation date, newest first. */
export const schemaHistory = (name: string, envId?: string) =>
  invoke<SchemaHistoryEntry[]>('schema_history', { name, envId })

export const listSchemaVersions = (name: string, envId?: string) =>
  invoke<SchemaVersionSummary[]>('list_schema_versions', { name, envId })

export const searchSchemas = (keywords: string, envId?: string) =>
  invoke<SchemaSummary[]>('search_schemas', { keywords, envId })

export const validateSchema = (content: unknown, name?: string) =>
  invoke<ValidationReport>('validate_schema', { content, name })

export const putSchema = (
  name: string,
  content: unknown,
  description?: string,
  envId?: string,
) => invoke<WriteResult>('put_schema', { name, content, description, envId })

export const deleteSchema = (args: {
  name: string
  confirmName: string
  version?: string
  confirmProtected?: boolean
  envId?: string
}) => invoke<void>('delete_schema', args)

export const newSchemaDraft = (
  source: string,
  detailType: string,
  properties: DetailProperty[],
) => invoke<NewSchemaDraft>('new_schema_draft', { source, detailType, properties })

export const simplifySchema = (content: unknown) =>
  invoke<SimplifyPreview>('simplify_schema', { content })

/**
 * Rewrite `nullable: true` as `type: [T, "null"]`.
 *
 * The repair for the validator's `nullable` finding: Ajv has no `nullable`, so
 * those fields reject `null` on the bus today.
 */
export const openapi30Schema = (content: unknown) =>
  invoke<Openapi30Repair>('openapi_30_schema', { content })

export const planImport = (
  directory: string,
  simplifyOnImport: boolean,
  envId?: string,
) => invoke<ImportPlan>('plan_import', { directory, simplifyOnImport, envId })

export const applyImport = (
  entries: { name: string; content: unknown }[],
  envId?: string,
) => invoke<ApplyImportResult>('apply_import', { entries, envId })

export const exportSchemas = (
  directory: string,
  names: string[],
  simplifyOnExport: boolean,
  envId?: string,
) =>
  invoke<ExportResult>('export_schemas', {
    directory,
    names,
    simplifyOnExport,
    envId,
  })

// --- developer tools ------------------------------------------------------

export const devInfo = () => invoke<DevInfo>('dev_info')

export const readAppLogs = (lines?: number) =>
  invoke<string[]>('read_app_logs', { lines })

/** Reveal one of pontifex's own directories in the file manager. */
export const openAppPath = (id: string) => invoke<void>('open_app_path', { id })

export const clearAppLogs = () => invoke<void>('clear_app_logs')

// --- reality check --------------------------------------------------------

/** Sample recent events for a schema and report how well it describes them. */
export const checkAgainstEvents = (
  request: RealityCheckRequest,
  envId?: string,
) => invoke<RealityCheckResult>('check_against_events', { request, envId })

/** The schema's last persisted analysis, if one was run in the last month. */
export const cachedAnalysis = (name: string, envId?: string) =>
  invoke<CachedAnalysis | null>('cached_analysis', { name, envId })

/**
 * Event sources known from sampled traffic, with the spelling a routing rule
 * has to match. Reads the cache, so it costs no AWS call.
 */
export const eventSources = (envId?: string) =>
  invoke<EventSource[]>('event_sources', { envId })

/** `[eventTypes, totalEvents]` currently cached. */
export const eventCacheStats = () => invoke<[number, number]>('event_cache_stats')

export const clearEventCache = (envId?: string, all?: boolean) =>
  invoke<number>('clear_event_cache', { envId, all })

/** Grade every schema in the registry against recent traffic. */
export const registryReport = (
  request: RegistryReportRequest,
  envId?: string,
) => invoke<RegistryReport>('registry_report', { request, envId })

/**
 * Draft a schema for an event type that has none, inferred from its traffic.
 * Uses cached events when available, so this is usually instant.
 *
 * `logGroup` narrows the sample to one group and `aroundMs` to two minutes
 * around one instant. Without them every group is scanned over a day, which
 * is right when nothing is known about where or when the type arrives and
 * wasteful when the caller has just watched it land.
 */
export const draftFromEvents = (
  source: string,
  detailType: string,
  envId?: string,
  minutes?: number,
  logGroup?: string,
  /** Centre a two-minute window on this instant instead of sampling `minutes` back from now. */
  aroundMs?: number,
) =>
  withTimeout(
    invoke<InferredDraft>('draft_from_events', {
      source,
      detailType,
      envId,
      minutes,
      logGroup,
      aroundMs,
    }),
    60_000,
    'Inferring the schema',
  )

/** Declare one observed field on whichever type owns its path. */
export const addObservedField = (
  content: unknown,
  typeName: string,
  field: FieldObservation,
) => invoke<unknown>('add_observed_field', { content, typeName, field })

/** Returns a patched document; never writes to AWS. */
export const applyFieldSuggestions = (
  content: unknown,
  typeName: string,
  fields: FieldObservation[],
) => invoke<unknown>('apply_field_suggestions', { content, typeName, fields })

/**
 * Apply one issue's repair to the draft.
 *
 * The repair is sent back exactly as it arrived on the issue, so what is
 * applied is what the row offered. Returns a patched document for review;
 * nothing reaches AWS until it is saved like any other edit.
 */
export const applyIssueRepair = (
  content: unknown,
  typeName: string,
  path: string,
  repair: Repair,
) => invoke<unknown>('apply_issue_repair', { content, typeName, path, repair })

// --- bedrock --------------------------------------------------------------

export const aiGenerate = (request: AiRequest) =>
  invoke<AiResponse>('ai_generate', { request })

/** Asks a small model what to do about an issue with no mechanical repair. */
export const aiSuggestRepair = (args: {
  content: unknown
  typeName: string
  issue: Issue
  examples: unknown[]
  modelId?: string
}) => invoke<RepairSuggestion>('ai_suggest_repair', args)

/** Describes what a draft changed, for the version's description field. */
export const aiSummarizeChanges = (
  before: unknown | null,
  after: unknown,
  modelId?: string,
) => invoke<string>('ai_summarize_changes', { before, after, modelId })

export const listBedrockModels = () =>
  invoke<BedrockModel[]>('list_bedrock_models')

// --- logs -----------------------------------------------------------------

export const listLogGroups = (envId?: string) =>
  invoke<LogGroupSummary[]>('list_log_groups', { envId })

export const queryLogs = (query: LogQuery, envId?: string) =>
  invoke<LogPage>('query_logs', { query, envId })

// --- topology -------------------------------------------------------------

export const getTopology = (envId?: string) =>
  invoke<Topology>('get_topology', { envId })

export const previewBusConfiguration = (path: string) =>
  invoke<DeclaredBus[]>('preview_bus_configuration', { path })

/** The backend's category vocabulary, for the Developer tab's filter. */
export const logCategories = () => rawInvoke<string[]>('log_categories')

// --- jira -----------------------------------------------------------------

export const jiraStatus = () => invoke<JiraStatus>('jira_status')

/** Store the app registration. The secret goes to the keychain, not to disk. */
export const setJiraApp = (
  clientId: string,
  clientSecret?: string,
  callbackUrl?: string,
) => invoke<JiraStatus>('set_jira_app', { clientId, clientSecret, callbackUrl })

/**
 * Read the client ID, callback and scopes out of the authorization URL the
 * Atlassian console generates, instead of transcribing them by hand.
 */
export const parseJiraAuthorizeUrl = (url: string) =>
  invoke<AuthorizeUrlParts>('parse_jira_authorize_url', { url })

/**
 * Sign in: opens the browser and resolves once the redirect comes back.
 *
 * Long-running by nature — the user has to consent in another application —
 * so callers should expect this to sit pending for as long as that takes.
 */
export const jiraConnect = () => invoke<JiraSite[]>('jira_connect')

export const jiraSites = () => invoke<JiraSite[]>('jira_sites')

export const selectJiraSite = (cloudId: string) =>
  invoke<JiraStatus>('select_jira_site', { cloudId })

export const jiraDisconnect = () => invoke<JiraStatus>('jira_disconnect')

export const jiraProjects = () => invoke<JiraProject[]>('jira_projects')

export const jiraIssueTypes = (projectKey: string) =>
  invoke<string[]>('jira_issue_types', { projectKey })

/** What a project demands before it will accept a ticket. */
export const jiraRequiredFields = (projectKey: string, issueType: string) =>
  invoke<RequiredField[]>('jira_required_fields', { projectKey, issueType })

/** Remember what to send for a project's mandatory fields. */
export const setJiraFieldDefaults = (
  projectKey: string,
  fields: Record<string, unknown>,
) => invoke<void>('set_jira_field_defaults', { projectKey, fields })

/** Render a ticket without filing it, and report any ticket already open for it. */
export const previewJiraTicket = (issue: Issue, context: TicketContext) =>
  invoke<TicketPreview>('preview_jira_ticket', { request: { issue, context } })

export const fileJiraTicket = (request: FileTicketRequest) =>
  invoke<FileTicketResult>('file_jira_ticket', { request })

export const fileJiraTickets = (requests: FileTicketRequest[]) =>
  invoke<FileTicketResult[]>('file_jira_tickets', { requests })

/**
 * Re-derive the issues for several schemas, for filing them in one go.
 *
 * Reads the events the report already cached rather than re-scanning, so this
 * costs one describe per schema and no CloudWatch time.
 */
export const issuesForSchemas = (
  names: string[],
  minutes?: number,
  logGroup?: string,
  envId?: string,
) =>
  invoke<SchemaIssues[]>('issues_for_schemas', { names, minutes, logGroup, envId })

/**
 * Check one caught event against the schema registered for its type.
 *
 * A type with no schema comes back as a single `unregistered` issue rather
 * than an error: that absence is the finding.
 */
export const validateEvent = (name: string, event: unknown, envId?: string) =>
  invoke<Issue[]>('validate_event', { name, event, envId })

// --- watch mode -----------------------------------------------------------

export const listWatches = (envId?: string) =>
  invoke<Watch[]>('list_watches', { envId })

/** Create or update. An empty `id` gets one assigned. */
export const saveWatch = (watch: Watch) => invoke<Watch>('save_watch', { watch })

export const deleteWatch = (id: string) => invoke<void>('delete_watch', { id })

/** The filter pattern a watch compiles to, and its one-line summary. */
export const compileWatchPattern = (watch: Watch) =>
  invoke<CompiledWatch>('compile_watch_pattern', { watch })

/** Arm or disarm an environment's poller. Persisted across restarts. */
export const setWatching = (enabled: boolean, envId?: string) =>
  invoke<WatchStatus>('set_watching', { envId, enabled })

export const watchStatus = () => invoke<WatchStatus[]>('watch_status')

export const setWatchPollSeconds = (seconds: number) =>
  invoke<number>('set_watch_poll_seconds', { seconds })

export const setWatchIdleTimeout = (minutes: number) =>
  invoke<number>('set_watch_idle_timeout', { minutes })

/** Show and focus the main window, e.g. to put a question in front of someone. */
export const showMainWindow = () => invoke<void>('show_main_window')

export const listWatchHits = (envId?: string, limit?: number) =>
  invoke<WatchHit[]>('list_watch_hits', { envId, limit })

/** When watching started and stopped, newest first. */
export const listWatchMarks = (envId?: string) =>
  invoke<WatchMark[]>('list_watch_marks', { envId })

/** Forget an environment's hits — all of them, or one watch's. */
export const clearWatchHits = (envId?: string, watchId?: string) =>
  invoke<void>('clear_watch_hits', { envId, watchId })

export const markWatchHitsSeen = (envId?: string) =>
  invoke<void>('mark_watch_hits_seen', { envId })

/** Show a notification now and report what macOS did with it. */
export const testNotification = () => invoke<NotifierOutcome>('test_notification')

/** Ask macOS for permission again under a fresh identity, then test. */
export const askNotificationPermissionAgain = () =>
  invoke<NotifierOutcome>('ask_notification_permission_again')

/** System Settings → Notifications, for turning Pontifex back on. */
export const openNotificationSettings = () => invoke<void>('open_notification_settings')

/** Poll immediately; the interval restarts from that poll. */
export const pollNow = (envId?: string) => invoke<void>('poll_now', { envId })

/** Run a watch's pattern over the last `hours`, sampled across the whole window. */
export const probeWatch = (watch: Watch, hours?: number) =>
  invoke<WatchProbe>('probe_watch', { watch, hours })

// --- origin ---------------------------------------------------------------

/** Where an event type came from: who wired it in and who publishes it. Cached a week unless `refresh`. */
export const eventOrigin = (name: string, refresh?: boolean, limit?: number) =>
  withTimeout(
    invoke<EventOrigin>('event_origin', { name, refresh, limit }),
    180_000,
    'Looking up the origin',
  )

export const githubStatus = () => invoke<GithubStatus>('github_status')

/** Store a personal access token in the keychain; empty clears it. */
export const setGithubToken = (token: string) => invoke<void>('set_github_token', { token })

/** Who the current token authenticates as. */
export const githubCheck = () => invoke<string>('github_check')
