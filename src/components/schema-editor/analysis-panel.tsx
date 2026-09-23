import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useImperativeHandle, useMemo, useState } from 'react'
import type { Ref } from 'react'
import {
  AlertTriangle,
  Bug,
  CheckCircle2,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Database,
  Eye,
  Info,
  RefreshCw,
  Sparkles,
  Undo2,
  Wand2,
  X,
  XCircle,
} from 'lucide-react'
import { listen } from '@tauri-apps/api/event'
import * as ipc from '@/lib/ipc'
import { useSettings } from '@/app/settings-context'
import { KIND_LABELS, SEVERITY_TONE, describeSeverity } from '@/lib/issues'
import type {
  FieldObservation,
  FiledTicket,
  IpcError,
  Issue,
  IssueSeverity,
  RealityCheckResult,
  EventTicket,
  Repair,
  RepairSuggestion,
  TicketContext,
  IssueExample,
} from '@/lib/types'
import {
  Badge,
  Button,
  CopyButton,
  EmptyState,
  Checkbox,
  ErrorBox,
  Marked,
  Note,
  Select,
  Spinner,
  cn,
} from '@/components/ui'
import { WINDOWS, formatAge, formatDateTime } from '@/lib/format'
import { examplesFor, repairLabel } from '@/lib/repair'
import { FileTicketDialog, FiledChip, TicketChip } from '@/features/jira/file-ticket-dialog'
import { RepoFileLink } from '@/features/origin/origin-panel'

/**
 * A value as evidence, quoted the way JSON would write it.
 *
 * Strings keep their quotes on purpose: `e.g. 14` and `e.g. "14"` are the
 * difference between a number and a string, which for a type mismatch is the
 * entire finding. It also makes an empty string visible — `""` rather than a
 * row that trails off after "e.g." saying nothing at all.
 */
function preview(value: unknown): string {
  if (value === undefined) return ''
  const text = JSON.stringify(value) ?? String(value)
  return text.length > 60 ? `${text.slice(0, 60)}…` : text
}

/** How a row left the working list. */
interface Resolution {
  /** `fixed` edited the draft; `filed` only recorded the problem elsewhere. */
  how: 'fixed' | 'filed'
  path: string
  label: string
}

/** What the editor's chrome can ask of the analysis it is showing. */
export interface AnalysisHandle {
  /**
   * Open one ticket covering every finding worth filing.
   *
   * Lives here rather than in the caller because filing is this panel's
   * business — it owns the findings, the ticket context, and the record of
   * what has already been filed — while the button that starts it sits in the
   * tab strip above.
   */
  fileAll: () => void
}

/**
 * Checks a schema against the events actually on the bus.
 *
 * Validation alone only says whether events would be rejected. The drift
 * sections answer the more useful question — where the schema and real traffic
 * have diverged — which is what you act on.
 */
export function AnalysisPanel({
  schemaName,
  document,
  onApplySuggestions,
  onCoverage,
  onFindingCount,
  ref,
  active = true,
  envId,
  environmentLabel,
  registryName,
}: {
  schemaName: string
  document: unknown
  onApplySuggestions: (next: unknown) => void
  /** Feeds observed frequencies back so the tree can annotate each field. */
  onCoverage: (counts: Record<string, number>, sampled: number) => void
  /**
   * How many findings a roll-up would cover, so the chrome above can offer it.
   * Zero — no analysis, or nothing left to file — and the bug button goes back
   * to raising a concern.
   */
  onFindingCount?: (count: number) => void
  ref?: Ref<AnalysisHandle>
  /** Named on every ticket: the same drift in dev and prd are different bugs. */
  environmentLabel?: string
  registryName?: string
  /**
   * Whether this tab is the one on screen.
   *
   * The panel stays mounted behind the other tabs so a sample survives a look
   * at the inspector, but a hidden panel should not be re-checking on every
   * keystroke.
   */
  active?: boolean
  envId?: string
}) {
  const queryClient = useQueryClient()
  const [minutes, setMinutes] = useState(1440)
  const [result, setResult] = useState<RealityCheckResult | null>(null)
  /** When the result on screen was run, so its age can be shown. */
  const [analysedAt, setAnalysedAt] = useState<number | null>(null)

  /**
   * The last analysis run for this schema, from disk. Coming back to a schema
   * shows what was found last time and when, and Run re-scans when a fresher
   * answer matters.
   */
  const cached = useQuery({
    queryKey: ['analysis', envId, schemaName],
    queryFn: () => ipc.cachedAnalysis(schemaName, envId),
    enabled: !!envId,
    staleTime: Infinity,
    retry: false,
  })
  useEffect(() => {
    if (result || !cached.data) return
    setResult(cached.data.result)
    setAnalysedAt(cached.data.analysedAt)
    onCoverage(cached.data.result.coverage, cached.data.result.sampled)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cached.data])
  const [live, setLive] = useState(true)
  /** What has been dealt with this session, keyed by issue. */
  const [resolved, setResolved] = useState<Record<string, Resolution>>({})

  /** Both paths land here: a result is a result, however it was fetched. */
  const receive = (next: RealityCheckResult) => {
    setResult(next)
    onCoverage(next.coverage, next.sampled)

    // A repair that did not actually clear its issue brings the row back.
    //
    // Hiding on click is what makes the list workable, but it is a claim about
    // the document, and the next check is what can confirm it. Widening a type
    // on a field that is also rejected by a pattern leaves the row reported —
    // and a still-failing field that has been struck off the list is the one
    // outcome this panel must not produce. Filed rows are not a claim about
    // the document, so they stay put.
    setResolved((prev) => {
      const stillReported = new Set(next.issues.map((i) => i.key))
      const kept = Object.entries(prev).filter(
        ([key, entry]) => entry.how === 'filed' || !stillReported.has(key),
      )
      return kept.length === Object.keys(prev).length
        ? prev
        : Object.fromEntries(kept)
    })
  }

  const check = useMutation<RealityCheckResult, IpcError, boolean | void>({
    mutationFn: (refresh) =>
      ipc.checkAgainstEvents(
        {
          name: schemaName,
          content: document,
          minutes,
          limit: 300,
          refresh: !!refresh,
          persist: true,
        },
        envId,
      ),
    onSuccess: (next) => {
      receive(next)
      setAnalysedAt(Date.now())
    },
  })

  /**
   * Re-check the draft against *cached* events as it changes.
   *
   * This is what the cache buys: editing a schema shows its effect on real
   * traffic immediately, with no CloudWatch call and no cost. Only runs once a
   * sample exists, and never fetches.
   */
  const recheck = useMutation<RealityCheckResult, IpcError>({
    mutationFn: () =>
      ipc.checkAgainstEvents(
        { name: schemaName, content: document, minutes, cachedOnly: true },
        envId,
      ),
    onSuccess: receive,
  })

  const hasSample = (result?.sampled ?? 0) > 0

  // `mutate` is a stable reference across renders in React Query v5, so it can
  // sit in the dependency list without re-arming the timer every render.
  const recheckNow = recheck.mutate

  /**
   * A lookup landing for this type re-grades the list: the backend grades
   * from its own cache of the origin, so a re-check against the cached
   * sample picks the new grade up. The search itself is the Origin tab's to
   * start — a GitHub code search per schema opened would not do.
   */
  const source = result?.source
  const detailType = result?.detailType
  useEffect(() => {
    if (!hasSample) return
    const unlisten = listen<{ source: string; detailType: string }>(
      'origin://updated',
      (event) => {
        if (event.payload.source === source && event.payload.detailType === detailType) {
          recheckNow()
        }
      },
    )
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [hasSample, source, detailType, recheckNow])

  useEffect(() => {
    if (!live || !hasSample || !active) return
    const timer = setTimeout(() => recheckNow(), 400)
    return () => clearTimeout(timer)
    // Deliberately keyed on the document: an edit is what should re-check.
  }, [document, live, hasSample, minutes, active, recheckNow])

  const applySuggestions = useMutation<unknown, IpcError, FieldObservation[]>({
    mutationFn: (fields) =>
      ipc.applyFieldSuggestions(document, result!.typeName, fields),
    onSuccess: onApplySuggestions,
  })

  /**
   * Apply the repair the row is offering.
   *
   * Every repairable kind goes through here — widening a type, extending an
   * enum, dropping a `required`, declaring a field. The repair travels back
   * exactly as it arrived on the issue, so what is applied is what was offered.
   * The result lands in the draft, where it is reviewed as a diff like any
   * other edit; nothing is written to AWS.
   */
  const applyRepair = useMutation<
    unknown,
    IpcError,
    { issue: Issue; repair: Repair }
  >({
    mutationFn: ({ issue, repair }) =>
      ipc.applyIssueRepair(document, result!.typeName, issue.path, repair),
    onSuccess: (next, { issue, repair }) => {
      onApplySuggestions(next)
      setResolved((prev) => ({
        ...prev,
        [issue.key]: { how: 'fixed', path: issue.path, label: repairLabel(repair) },
      }))
    },
  })

  /**
   * Ask a model what to do about a row the local planner could not decide.
   *
   * The four mechanical repairs are computed from the sample and cost nothing,
   * so this is only offered where there is no `fix`: a rejected pattern, a
   * bound, a shape the sample does not settle. What comes back is the same
   * `Repair` the button already applies, so an AI-chosen edit gets the same
   * diff review as a computed one.
   */
  const suggest = useMutation<
    { issue: Issue; suggestion: RepairSuggestion },
    IpcError,
    Issue
  >({
    mutationFn: async (issue) => ({
      issue,
      suggestion: await ipc.aiSuggestRepair({
        content: document,
        typeName: result!.typeName,
        issue,
        examples: examplesFor(result, issue),
      }),
    }),
    onSuccess: ({ issue, suggestion }) =>
      setSuggestions((prev) => ({ ...prev, [issue.key]: suggestion })),
  })

  const [suggestions, setSuggestions] = useState<Record<string, RepairSuggestion>>({})

  // Memoised on the result: `document` changes on every keystroke while the
  // panel sits mounted behind the other tabs, and none of this depends on it.
  const { topLevelUndeclared, actionable, handlers } = useMemo(() => {
    const undeclared = result?.drift.undeclared ?? []
    return {
      /** How many consumer files the grade rests on; none means validation only. */
      handlers: result?.issues.find((i) => i.impact)?.impact?.handlers ?? 0,
      // The bulk apply is top-level only — `suggest_additions` skips nested
      // paths, so offering them here would silently drop them. The per-row
      // repair below has no such limit.
      topLevelUndeclared: undeclared.filter(
        (f) => !f.path.includes('.') && !f.path.includes('[]'),
      ),
      /** Info-level entries are FYI; "clean" means nothing above them. */
      actionable: result?.issues.filter((i) => i.severity !== 'info') ?? [],
    }
  }, [result])

  /** The working list: what is left to deal with. */
  const outstanding = useMemo(
    () => result?.issues.filter((i) => !resolved[i.key]) ?? [],
    [result, resolved],
  )
  /** Open issues worth a warning, for the verdict badge. */
  const drifting = outstanding.filter((i) => i.severity !== 'info').length
  // Everything dealt with this session, including repairs the re-check can no
  // longer see — those are exactly the ones that worked, and a record that
  // drops them the moment they succeed is no record at all.
  const resolvedEntries = useMemo(() => Object.entries(resolved), [resolved])

  // Is filing even possible? Read once rather than per row — it is a keychain
  // lookup, not a network call, but it is the same answer for every issue.
  const jira = useQuery({
    queryKey: ['jira', 'status'],
    queryFn: ipc.jiraStatus,
    staleTime: 30_000,
    retry: false,
  })
  /** The findings whose ticket is open: one row's, or a roll-up of all. */
  const [filing, setFiling] = useState<Issue[]>([])
  /** Tickets filed in this session, so the row can show where it went. */
  const [filed, setFiled] = useState<Record<string, FiledTicket>>({})
  /** The issue whose events are open in the drawer. */
  const [eventsFor, setEventsFor] = useState<Issue | null>(null)

  // Memoised: the roll-up handle and the count reported upwards both depend on
  // it, and a fresh object on every keystroke would fire them on every keystroke.
  const ticketContext: TicketContext | null = useMemo(
    () =>
      result
        ? {
            schemaName,
            environment: environmentLabel ?? envId ?? 'unknown',
            registry: registryName ?? null,
            source: result.source,
            detailType: result.detailType,
            logGroup: result.logGroup,
            minutes: result.minutes,
            typeName: result.typeName,
          }
        : null,
    [result, schemaName, environmentLabel, envId, registryName],
  )

  /**
   * What a roll-up ticket would cover: exactly the rows that offer a File
   * button of their own. `outstanding` has already dropped whatever was fixed
   * or filed this session, and an info-level row is a note nobody should
   * receive a ticket for.
   */
  const fileable = useMemo(
    () => outstanding.filter((issue) => issue.severity !== 'info'),
    [outstanding],
  )
  useEffect(() => {
    onFindingCount?.(ticketContext ? fileable.length : 0)
  }, [fileable.length, ticketContext, onFindingCount])
  useImperativeHandle(ref, () => ({ fileAll: () => setFiling(fileable) }), [fileable])

  /** The fields on screen, so each ticket can say which of them it covers. */
  const paths = useMemo(
    () => [...new Set((result?.issues ?? []).map((i) => i.path).filter(Boolean))].sort(),
    [result],
  )

  /**
   * What has already been filed about this event type, and where it stands.
   *
   * Asked of Jira rather than remembered here: a ticket key kept in this
   * panel's state lasted until the window was closed, and said nothing about
   * what happened to it afterwards. The labels on a ticket name the bus, the
   * producer and the event type, so the search is the record.
   */
  const tickets = useQuery({
    queryKey: ['jira', 'eventTickets', envId, schemaName, paths.join('|')],
    queryFn: () => ipc.jiraEventTickets(ticketContext!, paths),
    enabled: !!ticketContext && !!jira.data?.connected,
    staleTime: 60_000,
    retry: false,
  })

  /**
   * The ticket to show against one finding: an open one in preference to a
   * closed one, and the newest where there are several — a roll-up and the
   * single ticket it was filed alongside both cover the same field.
   */
  const ticketByPath = useMemo(() => {
    const byPath = new Map<string, EventTicket>()
    for (const ticket of tickets.data ?? []) {
      for (const path of ticket.covers) {
        const held = byPath.get(path)
        if (!held || (held.done && !ticket.done)) byPath.set(path, ticket)
      }
    }
    return byPath
  }, [tickets.data])

  /** Everything a row needs, so a grouped row and a lone one get the same. */
  const rowProps = (issue: Issue): RowProps => ({
    issue,
    // Nested paths included: the backend walks the ref graph to find
    // whichever type owns `metadata.trackingId`. Only a bare array element
    // has no property to repair, and the backend withholds the repair for
    // those.
    onFix: issue.fix ? () => applyRepair.mutate({ issue, repair: issue.fix! }) : undefined,
    fixing: applyRepair.isPending,
    suggestion: suggestions[issue.key],
    // Only where nothing mechanical applies: the computed repairs are free
    // and immediate, and asking a model to re-derive one would be slower
    // and no better.
    onSuggest:
      !issue.fix && issue.severity !== 'info' ? () => suggest.mutate(issue) : undefined,
    suggesting: suggest.isPending && suggest.variables?.key === issue.key,
    onApplySuggestion: (repair) => applyRepair.mutate({ issue, repair }),
    filed: filed[issue.key],
    tracked: ticketByPath.get(issue.path),
    // Info-level rows are notes, not bugs — nobody wants a ticket saying a
    // field was quiet this week.
    onFile: issue.severity !== 'info' && ticketContext ? () => setFiling([issue]) : undefined,
    canFile: jira.data?.connected ?? false,
    // The events behind the finding. Only where events were graded: an
    // unregistered type or a reviewer's concern has no per-event evidence.
    onExamples:
      result && issue.kind !== 'unregistered' && issue.kind !== 'concern'
        ? () => setEventsFor(issue)
        : undefined,
  })

  return (
    <div className="relative flex h-full min-h-0 flex-col overflow-hidden bg-surface-0">
      {eventsFor && result && (
        <ExampleDrawer
          key={eventsFor.key}
          issue={eventsFor}
          schemaName={schemaName}
          content={document}
          typeName={result.typeName}
          minutes={result.minutes}
          envId={envId}
          onClose={() => setEventsFor(null)}
        />
      )}
      {/*
        Controls wrap rather than compress: this now lives in a side column, so
        a fixed single row would either overflow or squeeze the window picker
        down to nothing.
      */}
      <div className="shrink-0 border-b border-edge px-2 py-1.5">
        <div className="flex flex-wrap items-center gap-1.5">
          <Select
            value={minutes}
            onChange={(e) => setMinutes(Number(e.target.value))}
          >
            {WINDOWS.map((w) => (
              <option key={w.minutes} value={w.minutes}>
                {w.label}
              </option>
            ))}
          </Select>

          <Button
            variant="primary"
            size="sm"
            loading={check.isPending}
            onClick={() => check.mutate()}
            title={
              analysedAt
                ? `Last run ${formatAge(Date.now() - analysedAt)} ago — run again for a fresh answer`
                : 'Check this schema against real events'
            }
          >
            Run
          </Button>
          {analysedAt && (
            <span
              className="text-[10px] text-ink-faint"
              title={`Analysed ${new Date(analysedAt).toLocaleString()}. Kept for 30 days.`}
            >
              analysed {formatAge(Date.now() - analysedAt)} ago
            </span>
          )}

          {result && (
            <Button
              variant="ghost"
              size="sm"
              loading={check.isPending}
              onClick={() => check.mutate(true)}
              title="Discard the cached sample and fetch fresh events"
            >
              <RefreshCw className="size-3" />
              Refresh
            </Button>
          )}

          <Checkbox
            checked={live}
            onChange={(e) => setLive(e.target.checked)}
            label="live"
            title="Re-check against cached events as you edit"
            className="ml-auto"
          />
        </div>

        {result && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
            <Badge tone="neutral">{result.sampled} sampled</Badge>
            {/* Two verdicts, not one: whether the schema rejects the events,
                and whether it should. "All pass" over a list of drift read as
                a contradiction, so passing with open issues says both. */}
            {result.sampled > 0 &&
              (result.failed > 0 ? (
                <Badge tone="danger">
                  <XCircle className="size-2.5" />
                  {result.failed} fail
                </Badge>
              ) : drifting > 0 ? (
                <Badge
                  tone="warn"
                  title="Every event validates, but the schema and the traffic disagree — see below"
                >
                  <AlertTriangle className="size-2.5" />
                  all pass · {drifting} {drifting === 1 ? 'issue' : 'issues'}
                </Badge>
              ) : (
                <Badge tone="ok">
                  <CheckCircle2 className="size-2.5" />
                  all pass
                </Badge>
              ))}
            {/* Provenance, not a verdict: kept apart from the counts on the right. */}
            {result.fromCache && (
              <Badge
                tone="info"
                className="ml-auto"
                title={
                  result.cacheAgeMs != null
                    ? `Cached ${formatAge(result.cacheAgeMs)} ago — no AWS call`
                    : 'From cache'
                }
              >
                <Database className="size-2.5" />
                cached
                {result.cacheAgeMs != null && ` ${formatAge(result.cacheAgeMs)}`}
              </Badge>
            )}

            {recheck.isPending && (
              <span className="text-[10px] text-ink-faint">re-checking…</span>
            )}
          </div>
        )}

        {/* What has been filed about this event type, whenever it was filed
            and whoever filed it — the answer to "did we raise this, and did
            anything happen". */}
        {(tickets.data?.length || tickets.isError) && (
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            <span className="text-[10px] text-ink-faint">filed</span>
            {tickets.isError ? (
              <span
                className="text-[10px] text-ink-faint"
                title={ipc.asIpcError(tickets.error).message}
              >
                — could not ask Jira
              </span>
            ) : (
              tickets.data?.map((ticket) => <TicketChip key={ticket.key} ticket={ticket} />)
            )}
          </div>
        )}
      </div>

      <div className="min-h-0 flex-1 space-y-3 overflow-auto p-2">
        {check.isPending && <Spinner label="Sampling events…" />}
        {check.isError && <ErrorBox error={check.error} />}

        {result?.note && <Note>{result.note}</Note>}

        {result && result.sampled > 0 && (
          <>
            <p className="px-1 text-[10px] text-ink-faint">
              {result.sampled} events matching{' '}
              <span className="font-mono">{result.source}</span> /{' '}
              <span className="font-mono">{result.detailType}</span> from{' '}
              <span className="font-mono">{result.logGroup}</span>, validated
              against <span className="font-mono">{result.typeName}</span>.
            </p>

            {actionable.length > 0 && topLevelUndeclared.length > 0 && (
              <div className="flex justify-end px-1">
                <Button
                  variant="secondary"
                  size="sm"
                  loading={applySuggestions.isPending}
                  onClick={() => applySuggestions.mutate(topLevelUndeclared)}
                  title="Adds them to the draft — nothing is saved to AWS"
                >
                  <Sparkles className="size-3" />
                  Declare {topLevelUndeclared.length} top-level
                </Button>
              </div>
            )}

            {resolvedEntries.length > 0 && (
              <ResolvedSummary
                entries={resolvedEntries}
                onRestore={(key) =>
                  setResolved((prev) => {
                    const next = { ...prev }
                    delete next[key]
                    return next
                  })
                }
              />
            )}

            {/* What the severities mean. Without a consumer found they are
                the validator's alone — rejected or not — and saying so is
                the difference between a grade and a guess. */}
            {outstanding.length > 0 && (
              <p className="px-1 text-[10px] text-ink-faint">
                {handlers > 0
                  ? `Graded by who reads each field: ${handlers} consumer file${handlers === 1 ? '' : 's'} found.`
                  : 'Graded by validation only. Open Origin to find who consumes this event and grade by who reads each field.'}
              </p>
            )}

            <ul className="flex flex-col gap-2">
              {groupIssues(outstanding).map((group) =>
                group.length === 1 ? (
                  <IssueRow key={group[0].key} {...rowProps(group[0])} />
                ) : (
                  <IssueGroup key={group[0].key} issues={group} rowProps={rowProps} />
                ),
              )}
            </ul>

            {actionable.length === 0 && (
              <Note tone="ok">
                This schema matches every sampled event, with no undeclared fields.
              </Note>
            )}

            {actionable.length > 0 && outstanding.length === 0 && (
              <Note tone="ok">
                Everything reported has been dealt with. Save the draft to
                publish a new version.
              </Note>
            )}
          </>
        )}

        {!result && !check.isPending && !check.isError && (
          <p className="p-3 text-center text-[11px] text-ink-faint">
            Samples recent events from CloudWatch and reports where the schema
            and real traffic disagree.
          </p>
        )}
      </div>

      {applySuggestions.isError && (
        <div className="p-2">
          <ErrorBox error={applySuggestions.error} />
        </div>
      )}
      {applyRepair.isError && (
        <div className="p-2">
          <ErrorBox error={applyRepair.error} />
        </div>
      )}
      {suggest.isError && (
        <div className="p-2">
          <ErrorBox error={suggest.error} />
        </div>
      )}

      {ticketContext && (
        <FileTicketDialog
          open={filing.length > 0}
          findings={filing}
          context={ticketContext}
          onClose={() => setFiling([])}
          onFiled={(issueKeys, ticket) => {
            setFiled((prev) => ({
              ...prev,
              ...Object.fromEntries(issueKeys.map((key) => [key, ticket])),
            }))
            // Filed is dealt with, even though the document did not change:
            // the problem now belongs to whoever owns the producer, and
            // leaving it in the working list means meeting it again on every
            // pass down the same list. A roll-up settles every row it covered.
            // Ask Jira again, so the ticket joins the strip above with a real
            // status. It may not be in the search index for a few seconds yet;
            // the chip on the row is what covers that gap.
            queryClient.invalidateQueries({ queryKey: ['jira', 'eventTickets'] })
            setResolved((prev) => ({
              ...prev,
              ...Object.fromEntries(
                issueKeys.map((key) => [
                  key,
                  {
                    how: 'filed' as const,
                    path: result?.issues.find((i) => i.key === key)?.path ?? key,
                    label: ticket.key,
                  },
                ]),
              ),
            }))
          }}
        />
      )}
    </div>
  )
}


const SEVERITY_STYLES: Record<
  IssueSeverity,
  { border: string; text: string; icon: typeof XCircle }
> = {
  error: { border: 'border-danger/40', text: 'text-danger', icon: XCircle },
  warning: { border: 'border-warn/40', text: 'text-warn', icon: AlertTriangle },
  info: { border: 'border-edge', text: 'text-ink-muted', icon: Info },
}

/**
 * A field's path, wrapping at the dots rather than clipping.
 *
 * The parent is dim and the field itself is not: in a column this narrow a
 * six-segment path is two lines, and the reader's eye wants the last word.
 */
function PathLabel({ path, leaf = false }: { path: string; leaf?: boolean }) {
  const at = path.lastIndexOf('.')
  const parent = at >= 0 ? path.slice(0, at + 1) : ''
  const name = at >= 0 ? path.slice(at + 1) : path
  return (
    <span className="font-mono text-[11px] [overflow-wrap:anywhere]" title={path}>
      {!leaf &&
        parent.split(/(?<=\.)/).map((segment, i) => (
          <span key={i} className="text-ink-faint">
            {segment}
            <wbr />
          </span>
        ))}
      <span className="text-ink">{name}</span>
    </span>
  )
}

/**
 * A sentence with the field's own path replaced by "this field", for beside
 * a path that is already on screen: a card that names the field once is one
 * you can scan.
 */
function withoutPath(text: string, path: string): string {
  if (!path) return text
  const stripped = text.replace(`\`${path}\``, 'this field').trim()
  return stripped.charAt(0).toUpperCase() + stripped.slice(1)
}

/**
 * Undeclared siblings together: an object the schema does not describe
 * shows up as one issue per field it carries, and five cards that each say
 * "the schema does not describe this" about `subscriptions[].something` are
 * one finding wearing five hats. Everything else stays one to a card.
 */
function groupIssues(issues: Issue[]): Issue[][] {
  const groups: Issue[][] = []
  const byParent = new Map<string, Issue[]>()
  for (const issue of issues) {
    const at = issue.kind === 'undeclared' ? issue.path.lastIndexOf('.') : -1
    if (at < 0) {
      groups.push([issue])
      continue
    }
    const parent = issue.path.slice(0, at)
    const existing = byParent.get(parent)
    if (existing) existing.push(issue)
    else {
      const group = [issue]
      byParent.set(parent, group)
      groups.push(group)
    }
  }
  return groups
}

interface RowProps {
  issue: Issue
  /** Applies the repair the issue carries. Absent when it carries none. */
  onFix?: () => void
  fixing: boolean
  /** A model's proposal, once asked for. */
  suggestion?: RepairSuggestion
  /** Offered only where no mechanical repair applies. */
  onSuggest?: () => void
  suggesting: boolean
  onApplySuggestion: (repair: Repair) => void
  /** Opens the ticket preview. Absent for rows not worth filing. */
  onFile?: () => void
  /** Whether Jira is connected — the button explains itself when it is not. */
  canFile: boolean
  filed?: FiledTicket
  /** A ticket in Jira that already covers this field, filed whenever. */
  tracked?: EventTicket
  /** Opens the events that exhibit this issue. Absent where none were graded. */
  onExamples?: () => void
}

/** Several undeclared fields under one parent, as one card. */
function IssueGroup({
  issues,
  rowProps,
}: {
  issues: Issue[]
  rowProps: (issue: Issue) => RowProps
}) {
  const first = issues[0]
  const parent = first.path.slice(0, first.path.lastIndexOf('.'))
  const worst = issues.some((i) => i.severity === 'error') ? 'error' : first.severity
  const { border, text, icon: Icon } = SEVERITY_STYLES[worst]
  return (
    <li className={cn('rounded-md border bg-surface-1/40 px-2 py-1.5', border)}>
      <div className="flex items-start gap-2">
        <Icon className={cn('mt-0.5 size-3 shrink-0', text)} />
        <div className="min-w-0 flex-1">
          <PathLabel path={parent} />
          <p className="text-[11px] leading-snug text-ink-muted">
            carries {issues.length} fields the schema does not describe
          </p>
        </div>
        <Badge tone="neutral">{KIND_LABELS.undeclared}</Badge>
      </div>
      <ul className="mt-1.5 flex flex-col divide-y divide-edge/60 border-t border-edge/60 pl-5">
        {issues.map((issue) => (
          <IssueRow key={issue.key} {...rowProps(issue)} nested />
        ))}
      </ul>
    </li>
  )
}

/**
 * One problem: what disagrees, how much traffic it affects, what to do.
 *
 * Deliberately not grouped by category. The categories overlapped — a wrong
 * type is also a rejection — so the same problem appeared twice under two
 * headings with two different frequencies, and neither said what to do.
 *
 * The path leads and the sentences do not repeat it: the path is the part
 * that was being clipped, and the part the reader is looking for.
 */
function IssueRow({
  issue,
  onFix,
  fixing,
  suggestion,
  onSuggest,
  suggesting,
  onApplySuggestion,
  onFile,
  canFile,
  filed,
  tracked,
  onExamples,
  nested = false,
}: RowProps & {
  /** Inside a group: no border, the parent path already shown above. */
  nested?: boolean
}) {
  const { border, text, icon: Icon } = SEVERITY_STYLES[issue.severity]
  // Never round a real occurrence down to 0%: one event in 224 is 0.4%, and
  // "0%" beside a rejection reads as nothing happening.
  const percent =
    issue.sampled > 0 && issue.affected > 0
      ? Math.max(1, Math.round((issue.affected / issue.sampled) * 100))
      : 0
  const summary = withoutPath(issue.summary, issue.path)
  const action = withoutPath(issue.action, issue.path)
  const severity = describeSeverity(issue)

  const actions = (
    <div className="flex flex-wrap items-center justify-end gap-1">
      {onExamples && (
        <button
          type="button"
          onClick={onExamples}
          title="Show the sampled events that have this problem"
          className="inline-flex items-center gap-1 whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] text-ink-muted hover:bg-surface-3 hover:text-accent"
        >
          <Eye className="size-2.5" />
          Events
        </button>
      )}
      {/* The repair sits beside the reason for it, and says what it will
          do rather than "Fix" — `Redeclare as string | null` is a claim
          you can disagree with before clicking, which "Fix" is not. */}
      {onFix && issue.fix && (
        <button
          type="button"
          onClick={onFix}
          disabled={fixing}
          title={`${repairLabel(issue.fix)} in the draft — reviewed as a diff, nothing is saved to AWS`}
          className="inline-flex items-center gap-1 whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] text-accent hover:bg-surface-3 disabled:opacity-40"
        >
          <Wand2 className="size-2.5" />
          {repairLabel(issue.fix)}
        </button>
      )}

      {onSuggest && !suggestion && (
        <button
          type="button"
          onClick={onSuggest}
          disabled={suggesting}
          title="Ask a model what to do about this, from the values real events carry"
          className="inline-flex items-center gap-1 whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] text-ink-muted hover:bg-surface-3 hover:text-accent disabled:opacity-40"
        >
          <Sparkles className="size-2.5" />
          {suggesting ? 'Asking…' : 'Suggest'}
        </button>
      )}

      {/* An open ticket is the answer to "file this", so it stands in for the
          button — both at once read as a choice where there is none.

          A closed one is history: the problem is back, and it can be raised
          again. The old ticket stays beside the button as the link to what
          was done about it last time.

          The session's own record wins over Jira's either way, because a
          ticket filed a moment ago is not in the search index yet. */}
      {filed ? (
        <FiledChip ticket={filed} />
      ) : tracked && !tracked.done ? (
        <TicketChip ticket={tracked} />
      ) : (
        <>
          {tracked?.done && <TicketChip ticket={tracked} />}
          {onFile && (
            <button
              type="button"
              onClick={onFile}
              disabled={!canFile}
              title={
                canFile
                  ? 'File this with the team that owns the producer'
                  : 'Connect Jira in Settings → Jira to file this'
              }
              // Muted, not faint: faint is what a disabled control looks
              // like, and an action that can be taken should not.
              className="inline-flex items-center gap-1 whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] text-ink-muted hover:bg-surface-3 hover:text-accent disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-ink-muted"
            >
              <Bug className="size-2.5" />
              File
            </button>
          )}
        </>
      )}
    </div>
  )

  return (
    <li
      className={cn(
        nested ? 'py-1.5' : cn('rounded-md border bg-surface-1/40 px-2 py-1.5', border),
      )}
    >
      <div className="flex items-start gap-2">
        {!nested && <Icon className={cn('mt-0.5 size-3 shrink-0', text)} />}
        <div className="min-w-0 flex-1">
          {issue.path ? (
            <PathLabel path={issue.path} leaf={nested} />
          ) : (
            <span className="text-[11px] text-ink">{KIND_LABELS[issue.kind]}</span>
          )}
          <p className="text-[11px] leading-snug text-ink-muted">
            <Marked text={summary} />
          </p>
        </div>
      </div>

      {/* Everything below the header hangs under the icon. */}
      <div className={cn(!nested && 'pl-5')}>
        <div className="mt-1 flex flex-wrap items-center gap-x-1.5 gap-y-1">
          {/* The badge says what the severity rests on — rejected, read by
              consumers, drifting unread — not a level. "Error" beside a
              missing field claimed an impact nothing here had measured. */}
          <Badge tone={SEVERITY_TONE[issue.severity]} title={severity.title}>
            {severity.label}
          </Badge>
          {!nested && <Badge tone="neutral">{KIND_LABELS[issue.kind]}</Badge>}
          {issue.kind !== 'neverSeen' && (
            <span
              className="whitespace-nowrap text-[10px] text-ink-faint"
              title={`${issue.affected} of ${issue.sampled} sampled events`}
            >
              {issue.affected}/{issue.sampled} · {percent}%
            </span>
          )}
          {issue.observed && nested && (
            <span className="font-mono text-[10px] text-ink-faint">{issue.observed}</span>
          )}
          {issue.example !== undefined && issue.example !== null && (
            <span
              className="min-w-0 max-w-full truncate font-mono text-[10px] text-ink-faint"
              title={preview(issue.example)}
            >
              e.g. {preview(issue.example)}
            </span>
          )}
        </div>

        {/* Who breaks. The files are the evidence for the badge above, and
            the owners are who to talk to before changing the field. A file
            that passes the parent along whole is shown as that. */}
        {issue.impact && (issue.impact.readers.length > 0 || issue.impact.indirect.length > 0) && (
          <ul className="mt-1 flex flex-col gap-0.5">
            {issue.impact.readers.map((file) => (
              <li key={`${file.repo}/${file.path}`}>
                <RepoFileLink repo={file.repo} path={file.path} url={file.url}>
                  {file.owners.join(' ')}
                </RepoFileLink>
              </li>
            ))}
            {issue.impact.indirect.map((file) => (
              <li key={`${file.repo}/${file.path}`}>
                <RepoFileLink repo={file.repo} path={file.path} url={file.url}>
                  passes the parent along
                </RepoFileLink>
              </li>
            ))}
          </ul>
        )}

        {/* The validator's own words. For a rejection nothing else explains,
            this is the only line that says what is actually wrong — and it was
            being carried all the way from Rust and then dropped here. */}
        {issue.message && (
          <p className="mt-1 truncate font-mono text-[10px] text-ink-muted" title={issue.message}>
            {issue.message}
          </p>
        )}

        {/* What to do and the buttons that do it, on one line while they fit:
            the sentence explains the button beside it. The sentence keeps a
            floor and the buttons drop below it rather than crushing it —
            "Fix the producer, or redeclare this field as number." set five
            words wide is not a sentence anyone reads. */}
        <div className="mt-1 flex flex-wrap items-start gap-x-2 gap-y-1">
          <p className="min-w-[12rem] flex-1 text-[10px] leading-snug text-ink-faint">
            <Marked text={action} />
          </p>
          <div className="ml-auto shrink-0">{actions}</div>
        </div>

        {/* A model's proposal, shown with its reasoning and not applied until
            asked. The rationale is the part worth reading: it says what the edit
            gives up, which is the half of the decision the row cannot show. */}
        {suggestion && (
          <div className="mt-1.5 rounded border border-edge bg-surface-2/50 px-2 py-1.5">
            <p className="text-[10px] leading-snug text-ink-muted">
              {suggestion.rationale}
            </p>
            {suggestion.repair ? (
              <button
                type="button"
                onClick={() => onApplySuggestion(suggestion.repair!)}
                disabled={fixing}
                className="mt-1 inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-accent hover:bg-surface-3 disabled:opacity-40"
              >
                <Wand2 className="size-2.5" />
                {repairLabel(suggestion.repair)}
              </button>
            ) : (
              <p className="mt-1 text-[10px] text-ink-faint">
                No mechanical edit proposed — this one wants a decision.
              </p>
            )}
          </div>
        )}
      </div>
    </li>
  )
}

/**
 * What has been dealt with, rolled up out of the way.
 *
 * The working list is only workable if a row leaves it once it has been
 * handled — but a row that vanishes with no trace makes it impossible to say
 * afterwards what a session actually changed. This is that trace, collapsed by
 * default because the answer to "what is left" is the question the panel is
 * open for.
 */
function ResolvedSummary({
  entries,
  onRestore,
}: {
  entries: [string, Resolution][]
  onRestore: (key: string) => void
}) {
  const [open, setOpen] = useState(false)
  const fixed = entries.filter(([, e]) => e.how === 'fixed').length
  const filed = entries.length - fixed

  return (
    <div className="rounded-md border border-edge bg-surface-1/30 px-2 py-1">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="flex w-full items-center gap-1.5 text-[10px] text-ink-faint hover:text-ink-muted"
      >
        {open ? (
          <ChevronDown className="size-3" />
        ) : (
          <ChevronRight className="size-3" />
        )}
        <span>
          {entries.length} resolved
          {fixed > 0 && filed > 0 && ` · ${fixed} fixed, ${filed} filed`}
          {fixed > 0 && filed === 0 && ' · fixed in the draft'}
          {fixed === 0 && filed > 0 && ' · filed'}
        </span>
      </button>

      {open && (
        <ul className="mt-1 flex flex-col gap-0.5 pl-4">
          {entries.map(([key, entry]) => (
            <li key={key} className="flex items-center gap-1.5 text-[10px]">
              {entry.how === 'fixed' ? (
                <Wand2 className="size-2.5 shrink-0 text-accent" />
              ) : (
                <Bug className="size-2.5 shrink-0 text-ink-faint" />
              )}
              <span className="min-w-0 truncate font-mono text-ink-muted">
                {entry.path}
              </span>
              <span className="shrink-0 text-ink-faint">{entry.label}</span>
              {/* Only a filed row can be put back. A repair is already in the
                  draft, so "undo" here would restore the row while leaving the
                  edit in place — the editor's own undo is what reverses it. */}
              {entry.how === 'filed' && (
                <button
                  type="button"
                  onClick={() => onRestore(key)}
                  title="Put this back in the list"
                  className="ml-auto shrink-0 rounded p-0.5 text-ink-faint hover:bg-surface-3 hover:text-accent"
                >
                  <Undo2 className="size-2.5" />
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

/**
 * The events behind one finding, one at a time.
 *
 * A finding says "blank in 67% of events"; deciding what to do about it
 * means looking at a few of those events — is it one producer, one path, a
 * field that was renamed. Slides in over the issue list, inside the panel,
 * so the tree and the schema stay where they are.
 */
function ExampleDrawer({
  issue,
  schemaName,
  content,
  typeName,
  minutes,
  envId,
  onClose,
}: {
  issue: Issue
  schemaName: string
  content: unknown
  typeName: string | null
  minutes: number
  envId?: string
  onClose: () => void
}) {
  const { timeZone } = useSettings()
  const [index, setIndex] = useState(0)
  // Mounted off-screen, then slid in: the transition needs a frame to see the
  // starting position.
  const [shown, setShown] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => setShown(true))
    return () => cancelAnimationFrame(frame)
  }, [])

  const examples = useQuery({
    queryKey: ['analysis', envId, schemaName, 'examples', issue.key, minutes],
    queryFn: () =>
      ipc.eventsForIssue(
        {
          name: schemaName,
          content,
          typeName: typeName ?? undefined,
          issueKey: issue.key,
          minutes,
        },
        envId,
      ),
    enabled: !!envId,
    staleTime: 60_000,
  })

  const list: IssueExample[] = examples.data ?? []
  const current = list[index]
  const prev = () => setIndex((i) => Math.max(0, i - 1))
  const next = () => setIndex((i) => Math.min(list.length - 1, i + 1))

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
      else if (e.key === 'ArrowLeft') prev()
      else if (e.key === 'ArrowRight') next()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [list.length, onClose])

  // The offending field, for picking its lines out of the payload. The last
  // segment is enough: `client.dob` is found by `"dob"`.
  const leaf = issue.path.replace(/\[\d*\]$/, '').split('.').pop() ?? ''
  const text = current ? JSON.stringify(current.detail, null, 2) : ''

  return (
    <div
      role="dialog"
      aria-label="Events with this issue"
      className={cn(
        'absolute inset-y-0 right-0 z-10 flex w-full flex-col border-l border-edge bg-surface-1 shadow-2xl transition-transform duration-200 ease-out',
        shown ? 'translate-x-0' : 'translate-x-full',
      )}
    >
      <div className="chrome-panel flex shrink-0 items-start gap-2 border-b border-edge px-3 py-2">
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="truncate font-mono text-xs text-ink">{issue.path || schemaName}</span>
            <Badge tone="neutral">{KIND_LABELS[issue.kind]}</Badge>
          </div>
          <p className="truncate text-[11px] text-ink-muted" title={issue.summary}>
            {issue.summary}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <span className="w-16 text-center font-mono text-[10px] tabular-nums text-ink-faint">
            {list.length > 0 ? `${index + 1} of ${list.length}` : examples.isLoading ? '…' : '0'}
          </span>
          <Button variant="ghost" size="sm" onClick={prev} disabled={index === 0} title="Previous event (←)">
            <ChevronLeft className="size-3" />
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={next}
            disabled={index >= list.length - 1}
            title="Next event (→)"
          >
            <ChevronRight className="size-3" />
          </Button>
          <Button variant="ghost" size="sm" onClick={onClose} title="Close (Esc)">
            <X className="size-3" />
          </Button>
        </div>
      </div>

      {examples.isError && (
        <div className="p-3">
          <ErrorBox error={ipc.asIpcError(examples.error)} />
        </div>
      )}
      {examples.isLoading && <Spinner label="Grading cached events…" />}
      {examples.isSuccess && list.length === 0 && (
        <EmptyState
          title="No cached event shows this"
          detail="The sample this analysis graded is no longer in the cache. Run the analysis again to fetch a fresh one."
        />
      )}

      {current && (
        <>
          <div className="flex shrink-0 items-center gap-3 border-b border-edge/60 px-3 py-1.5 font-mono text-[10px] text-ink-faint">
            <span className="truncate" title={current.id}>
              {current.id}
            </span>
            <span className="ml-auto whitespace-nowrap">
              {formatDateTime(current.timestamp, timeZone)}
            </span>
            <CopyButton text={text} title="Copy event detail" />
          </div>
          <pre className="min-h-0 flex-1 overflow-auto px-3 py-2 font-mono text-[11px] leading-snug text-ink-muted">
            {text.split('\n').map((line, i) => (
              <div
                key={i}
                className={cn(
                  leaf && line.includes(`"${leaf}"`) && '-mx-3 bg-warn/15 px-3 text-ink',
                )}
              >
                {line}
              </div>
            ))}
          </pre>
        </>
      )}
    </div>
  )
}
