import { useMutation, useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle,
  Bug,
  CheckCircle2,
  Database,
  Info,
  Plus,
  RefreshCw,
  Sparkles,
  XCircle,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  FieldObservation,
  FiledTicket,
  IpcError,
  Issue,
  IssueKind,
  IssueSeverity,
  RealityCheckResult,
  TicketContext,
} from '@/lib/types'
import {
  Badge,
  Button,
  Checkbox,
  ErrorBox,
  Marked,
  Note,
  Select,
  Spinner,
  cn,
} from '@/components/ui'
import { WINDOWS, formatAge } from '@/lib/format'
import { FileTicketDialog, FiledChip } from '@/features/jira/file-ticket-dialog'

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
  const [minutes, setMinutes] = useState(1440)
  const [result, setResult] = useState<RealityCheckResult | null>(null)
  const [live, setLive] = useState(true)

  /** Both paths land here: a result is a result, however it was fetched. */
  const receive = (next: RealityCheckResult) => {
    setResult(next)
    onCoverage(next.coverage, next.sampled)
  }

  const check = useMutation<RealityCheckResult, IpcError, boolean | void>({
    mutationFn: (refresh) =>
      ipc.checkAgainstEvents(
        { name: schemaName, content: document, minutes, limit: 300, refresh: !!refresh },
        envId,
      ),
    onSuccess: receive,
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
   * Declaring one field at a time, including nested ones — the backend walks
   * the ref graph to find whichever type actually owns the path.
   */
  const addOne = useMutation<unknown, IpcError, FieldObservation>({
    mutationFn: (field) =>
      ipc.addObservedField(document, result!.typeName, field),
    onSuccess: onApplySuggestions,
  })

  // Memoised on the result: `document` changes on every keystroke while the
  // panel sits mounted behind the other tabs, and none of this depends on it.
  const { topLevelUndeclared, undeclaredByPath, actionable } = useMemo(() => {
    const undeclared = result?.drift.undeclared ?? []
    return {
      // The bulk apply is top-level only — `suggest_additions` skips nested
      // paths, so offering them here would silently drop them. The per-row
      // declare below has no such limit.
      topLevelUndeclared: undeclared.filter(
        (f) => !f.path.includes('.') && !f.path.includes('[]'),
      ),
      /** The observation behind an `undeclared` issue, for one-click declaring. */
      undeclaredByPath: new Map(undeclared.map((f) => [f.path, f])),
      /** Info-level entries are FYI; "clean" means nothing above them. */
      actionable: result?.issues.filter((i) => i.severity !== 'info') ?? [],
    }
  }, [result])

  // Is filing even possible? Read once rather than per row — it is a keychain
  // lookup, not a network call, but it is the same answer for every issue.
  const jira = useQuery({
    queryKey: ['jira', 'status'],
    queryFn: ipc.jiraStatus,
    staleTime: 30_000,
    retry: false,
  })
  const [filing, setFiling] = useState<Issue | null>(null)
  /** Tickets filed in this session, so the row can show where it went. */
  const [filed, setFiled] = useState<Record<string, FiledTicket>>({})

  /** The observation to declare for an issue, when there is one. */
  const declarable = (issue: Issue) =>
    issue.path.includes('[]') ? undefined : undeclaredByPath.get(issue.path)

  const ticketContext: TicketContext | null = result
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
    : null

  return (
    <div className="flex h-full min-h-0 flex-col bg-surface-0">
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
          >
            Run
          </Button>

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
            {result.fromCache && (
              <Badge
                tone="info"
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
            {result.sampled > 0 &&
              (result.failed === 0 ? (
                <Badge tone="ok">
                  <CheckCircle2 className="size-2.5" />
                  all pass
                </Badge>
              ) : (
                <Badge tone="danger">
                  <XCircle className="size-2.5" />
                  {result.failed} fail
                </Badge>
              ))}
            {recheck.isPending && (
              <span className="text-[10px] text-ink-faint">re-checking…</span>
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

            <ul className="flex flex-col gap-2">
              {result.issues.map((issue) => (
                <IssueRow
                  key={issue.key}
                  issue={issue}
                  // Nested paths included: the backend walks the ref graph to
                  // find whichever type owns `metadata.trackingId`. Only a bare
                  // array element has no property to add.
                  onDeclare={declarable(issue) ? () => addOne.mutate(declarable(issue)!) : undefined}
                  declaring={addOne.isPending}
                  filed={filed[issue.key]}
                  onFile={
                    // Info-level rows are notes, not bugs — nobody wants a
                    // ticket saying a field was quiet this week.
                    issue.severity !== 'info' && ticketContext
                      ? () => setFiling(issue)
                      : undefined
                  }
                  canFile={jira.data?.connected ?? false}
                />
              ))}
            </ul>

            {actionable.length === 0 && (
              <Note tone="ok">
                This schema matches every sampled event, with no undeclared fields.
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
      {addOne.isError && (
        <div className="p-2">
          <ErrorBox error={addOne.error} />
        </div>
      )}

      {ticketContext && (
        <FileTicketDialog
          open={!!filing}
          issue={filing}
          context={ticketContext}
          onClose={() => setFiling(null)}
          onFiled={(issueKey, ticket) =>
            setFiled((prev) => ({ ...prev, [issueKey]: ticket }))
          }
        />
      )}
    </div>
  )
}

/** What each kind of issue is called, in the reader's terms. */
const KIND_LABELS: Record<IssueKind, string> = {
  wrongType: 'wrong type',
  outsideEnum: 'value not allowed',
  missingRequired: 'required field missing',
  undeclared: 'undeclared field',
  neverSeen: 'never seen',
  rejected: 'rejected',
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
 * One problem: what disagrees, how much traffic it affects, what to do.
 *
 * Deliberately not grouped by category. The categories overlapped — a wrong
 * type is also a rejection — so the same problem appeared twice under two
 * headings with two different frequencies, and neither said what to do.
 */
function IssueRow({
  issue,
  onDeclare,
  declaring,
  onFile,
  canFile,
  filed,
}: {
  issue: Issue
  /** Present only for an undeclared top-level field, which can be added here. */
  onDeclare?: () => void
  declaring: boolean
  /** Opens the ticket preview. Absent for rows not worth filing. */
  onFile?: () => void
  /** Whether Jira is connected — the button explains itself when it is not. */
  canFile: boolean
  filed?: FiledTicket
}) {
  const { border, text, icon: Icon } = SEVERITY_STYLES[issue.severity]
  // Never round a real occurrence down to 0%: one event in 224 is 0.4%, and
  // "0%" beside a rejection reads as nothing happening.
  const percent =
    issue.sampled > 0 && issue.affected > 0
      ? Math.max(1, Math.round((issue.affected / issue.sampled) * 100))
      : 0

  return (
    <li className={cn('rounded-md border bg-surface-1/40 px-2 py-1.5', border)}>
      <div className="flex items-start gap-2">
        <Icon className={cn('mt-px size-3 shrink-0', text)} />
        <p className="min-w-0 flex-1 text-[11px] leading-snug text-ink-muted">
          <Marked text={issue.summary} />
        </p>
        {onDeclare && (
          <button
            type="button"
            onClick={onDeclare}
            disabled={declaring}
            title={`Declare ${issue.path} in the draft`}
            className="shrink-0 rounded p-0.5 text-ink-faint hover:bg-surface-3 hover:text-accent disabled:opacity-40"
          >
            <Plus className="size-3" />
          </button>
        )}
      </div>

      <div className="mt-1 flex flex-wrap items-center gap-1.5 pl-5">
        <Badge tone={issue.severity === 'error' ? 'danger' : 'neutral'}>
          {KIND_LABELS[issue.kind]}
        </Badge>
        {/* The distinction that decides urgency: drifted, or actively losing
            events to validation. */}
        {issue.rejects && <Badge tone="danger">rejected today</Badge>}
        {issue.kind !== 'neverSeen' && (
          <span
            className="whitespace-nowrap text-[10px] text-ink-faint"
            title={`${issue.affected} of ${issue.sampled} sampled events`}
          >
            {issue.affected}/{issue.sampled} · {percent}%
          </span>
        )}
        {issue.example !== undefined && issue.example !== null && (
          <span
            className="min-w-0 max-w-full truncate font-mono text-[10px] text-ink-faint"
            title={preview(issue.example)}
          >
            e.g. {preview(issue.example)}
          </span>
        )}
        {filed ? (
          <FiledChip ticket={filed} />
        ) : (
          onFile && (
            <button
              type="button"
              onClick={onFile}
              disabled={!canFile}
              title={
                canFile
                  ? 'File this with the team that owns the producer'
                  : 'Connect Jira in Settings → Jira to file this'
              }
              className="ml-auto inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] text-ink-faint hover:bg-surface-3 hover:text-accent disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-ink-faint"
            >
              <Bug className="size-2.5" />
              File
            </button>
          )
        )}
      </div>

      {/* The validator's own words. For a rejection nothing else explains,
          this is the only line that says what is actually wrong — and it was
          being carried all the way from Rust and then dropped here. */}
      {issue.message && (
        <p
          className="mt-1 truncate pl-5 font-mono text-[10px] text-ink-muted"
          title={issue.message}
        >
          {issue.message}
        </p>
      )}

      <p className="mt-1 pl-5 text-[10px] leading-snug text-ink-faint">
        <Marked text={issue.action} />
      </p>
    </li>
  )
}
