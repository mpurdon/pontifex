import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { listen } from '@tauri-apps/api/event'
import { memo, useCallback, useEffect, useMemo, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import {
  Bug,
  CheckCircle2,
  ClipboardList,
  HelpCircle,
  Wand2,
  X,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  IpcError,
  RegistryReport,
  ReportRow,
} from '@/lib/types'
import {
  Button,
  Checkbox,
  EmptyState,
  ErrorBox,
  Input,
  Note,
  Select,
  Toolbar,
  cn,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import {
  STATUS,
  STATUS_ORDER,
  type FilterStatus,
  type StatusStyle,
} from './status'
import { DriftBreakdown, StatusBar, TrafficChart } from './health-charts'
import { WINDOWS, formatAge, formatWindow } from '@/lib/format'
import { useLoginForEnvironment } from '@/app/login-dialog'
import { useWorkbench } from '@/features/schemas/workbench-context'
import { BulkFileDialog } from '@/features/jira/bulk-file-dialog'

/**
 * Statuses worth filing a ticket about.
 *
 * `ok` and `noTraffic` have nothing for a producer team to fix. `missing`
 * is fileable too: a type on the bus with no schema is the producer's
 * contract left unwritten, and the ticket asks for it — Pontifex can draft
 * one from traffic, but the owner has to own it.
 */
const FILEABLE = new Set<FilterStatus>(['failing', 'drifting', 'error', 'missing'])

/**
 * A table row: either a graded schema, or an event type with no schema.
 *
 * The two are shown together, so they need one shape. A missing row has no
 * grading figures because there was nothing to grade it against.
 */
interface DisplayRow extends Omit<ReportRow, 'status'> {
  status: FilterStatus
  /** True when no schema exists — the row opens the draft flow, not the editor. */
  missing: boolean
}

/** Selected-chip fill per tone, so an active filter reads as pressed. */
const TONE_ACTIVE: Record<StatusStyle['tone'], string> = {
  danger: 'border-danger/50 bg-danger/15 text-danger',
  violet: 'border-violet/50 bg-violet/15 text-violet',
  orange: 'border-orange/50 bg-orange/15 text-orange',
  warn: 'border-warn/50 bg-warn/15 text-warn',
  ok: 'border-ok/50 bg-ok/15 text-ok',
  neutral: 'border-edge-strong bg-surface-3 text-ink-muted',
}

/**
 * Registry-wide health: every schema graded against real traffic.
 *
 * Built for triage rather than browsing — worst first, filterable, and one
 * click through to the schema that needs fixing.
 */
export function ReportPage() {
  const { envId, activeEnvironment } = useSettings()
  const {
    chartsOpen,
    setChartsOpen,
    healthMinutes: minutes,
    setHealthMinutes: setMinutes,
    healthFilter: filter,
    setHealthFilter: setFilter,
    healthStatuses: active,
    setHealthStatuses: setActive,
  } = useWorkbench()
  const navigate = useNavigate()
  const credentials = useLoginForEnvironment(activeEnvironment)

  const queryClient = useQueryClient()
  const [progress, setProgress] = useState<string | null>(null)
  /** Cap warnings are informational and repetitive; dismissed per report. */
  const [dismissedNote, setDismissedNote] = useState<number | null>(null)

  useEffect(() => {
    const unlisten = listen<{
      scanned?: number
      types?: number
      graded?: number
      total?: number
    }>('report://progress', (event) => {
      const p = event.payload
      setProgress(
        p.graded !== undefined
          ? `grading ${p.graded}/${p.total} schemas…`
          : `scanned ${p.scanned?.toLocaleString()} events · ${p.types} event types…`,
      )
    })
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [])

  /**
   * The report lives in the query cache, not component state, so switching
   * tabs does not throw away a scan that took seconds and cost money.
   * `enabled: false` means it is only ever produced by an explicit run.
   */
  const { data: report } = useQuery<RegistryReport | null>({
    queryKey: ['registryReport', envId],
    queryFn: () => null,
    enabled: false,
    staleTime: Infinity,
    gcTime: Infinity,
    initialData: null,
  })

  const run = useMutation<RegistryReport, IpcError>({
    mutationFn: () => {
      setProgress('starting…')
      // Limits come from Settings → Scanning; omitting them lets the backend
      // apply the configured values rather than this screen guessing.
      return ipc.registryReport({ minutes }, envId)
    },
    onSettled: () => setProgress(null),
    onSuccess: (next) =>
      queryClient.setQueryData(['registryReport', envId], next),
  })

  // Age of the report relative to the window it claims to cover.
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 30_000)
    return () => clearInterval(timer)
  }, [])

  const ageMs = report ? now - report.generatedAt : 0
  const windowMs = report ? report.minutes * 60_000 : 0
  const stale = report != null && ageMs > windowMs

  const counts = useMemo(() => {
    const out = Object.fromEntries(
      STATUS_ORDER.map((s) => [s, 0]),
    ) as Record<FilterStatus, number>
    for (const row of report?.rows ?? []) out[row.status] += 1
    out.missing = report?.unregistered.length ?? 0
    return out
  }, [report])

  const total = useMemo(
    () => STATUS_ORDER.reduce((sum, s) => sum + counts[s], 0),
    [counts],
  )

  const toggle = (status: FilterStatus) =>
    setActive((prev) => {
      const next = new Set(prev)
      if (next.has(status)) next.delete(status)
      else next.add(status)
      return next
    })

  /** No chip selected means no filter, rather than an empty screen. */
  const showAll = active.size === 0

  /**
   * Graded schemas and undocumented event types in one list.
   *
   * They used to be separate sections, which meant the worst finding on the
   * screen sat in its own box above the table and out of the sort order.
   * Merged, `missing` is just another status: it filters with the same chip
   * and sorts to the top like any other severity.
   */
  const rows: DisplayRow[] = useMemo(() => {
    if (!report) return []

    const graded: DisplayRow[] = report.rows.map((row) => ({ ...row, missing: false }))

    const undocumented: DisplayRow[] = report.unregistered.map((entry) => ({
      name: `${entry.source}@${entry.detailType}`,
      source: entry.source,
      detailType: entry.detailType,
      // Nothing was graded, because there is nothing to grade against.
      sampled: 0,
      observed: entry.count,
      passed: 0,
      failed: 0,
      undeclared: 0,
      typeMismatches: 0,
      enumDrift: 0,
      missingRequired: 0,
      status: 'missing',
      headline: 'Published but undocumented — open to draft a schema from its own traffic',
      missing: true,
    }))

    // Missing first; the backend already ordered the rest worst-first.
    return [...undocumented, ...graded]
  }, [report])

  /**
   * Schemas picked for bulk filing.
   *
   * Kept as names rather than rows so a re-run of the report — which replaces
   * every row object — does not silently drop the selection.
   */
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [filing, setFiling] = useState(false)

  const visible = useMemo(() => {
    const needle = filter.trim().toLowerCase()
    return rows.filter((row) => {
      if (!showAll && !active.has(row.status)) return false
      if (!needle) return true
      return row.name.toLowerCase().includes(needle)
    })
  }, [rows, filter, active, showAll])

  const toggleSelected = useCallback((name: string, next: boolean) => {
    setSelected((prev) => {
      const updated = new Set(prev)
      if (next) updated.add(name)
      else updated.delete(name)
      return updated
    })
  }, [])

  const reportMinutes = report?.minutes
  const openRow = useCallback(
    (row: DisplayRow) =>
      navigate(
        row.missing
          ? // No schema to open — go straight to drafting one from this event
            // type's own traffic.
            `/schemas?draft=${encodeURIComponent(row.name)}&minutes=${reportMinutes}`
          : `/schemas?select=${encodeURIComponent(row.name)}`,
      ),
    [navigate, reportMinutes],
  )

  /**
   * Types whose events carry an identity the registry cannot spell.
   *
   * Not a grading problem — these are matched and graded fine here, because
   * pontifex tries the sanitized spelling too. It is a warning about everything
   * downstream that does not.
   */
  const mismatched = useMemo(
    () => rows.filter((row) => !!row.wireIdentity),
    [rows],
  )

  /** Rows with something a producer team could act on. */
  const fileable = useMemo(
    () => visible.filter((row) => FILEABLE.has(row.status)),
    [visible],
  )

  return (
    <div className="flex h-full flex-col">
      <Toolbar>
        <Select value={minutes} onChange={(e) => setMinutes(Number(e.target.value))}>
          {WINDOWS.map((w) => (
            <option key={w.minutes} value={w.minutes}>
              {w.label}
            </option>
          ))}
        </Select>

        <Button variant="primary" loading={run.isPending} onClick={() => run.mutate()}>
          Run report
        </Button>

        {progress && (
          <span className="text-[11px] text-ink-faint">{progress}</span>
        )}

        {selected.size > 0 && (
          <>
            <Button variant="secondary" onClick={() => setFiling(true)}>
              <Bug className="size-3" />
              File {selected.size} schema{selected.size === 1 ? '' : 's'}
            </Button>
            <Button variant="ghost" onClick={() => setSelected(new Set())}>
              Clear
            </Button>
          </>
        )}

        <div className="ml-auto flex items-center gap-2">
          <Input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="filter…"
            className="w-52"
          />
        </div>
      </Toolbar>

      <div className="min-h-0 flex-1 space-y-3 overflow-auto p-3">
        {run.isError && <ErrorBox error={run.error} {...credentials} />}

        {!report && !run.isPending && !run.isError && (
          <EmptyState
            icon={<ClipboardList className="size-8" />}
            title="No report yet"
            detail={`Scans ${activeEnvironment?.logGroups?.[0] ?? 'the event log'} once, buckets events by type, and grades every schema in ${activeEnvironment?.registryName} against what it finds.`}
            action={
              <Button variant="primary" onClick={() => run.mutate()}>
                Run report
              </Button>
            }
          />
        )}

        {report && (
          <>
            <p className="text-[10px] text-ink-faint">
              {report.scannedEvents.toLocaleString()} events from{' '}
              {report.logGroups.map((group, i) => (
                <span key={group}>
                  {i > 0 && ' and '}
                  <span className="font-mono">{group}</span>
                </span>
              ))}{' '}
              over the last{' '}
              {formatWindow(report.minutes)}, graded against {report.rows.length} schemas in{' '}
              <span className="font-mono">{report.registry}</span>. Generated{' '}
              {ageMs < 60_000 ? 'just now' : `${formatAge(ageMs)} ago`}.
            </p>

            {mismatched.length > 0 && (
              <Note tone="warn">
                <span>
                  {mismatched.length} event type
                  {mismatched.length === 1 ? ' is' : 's are'} published under a name the
                  registry cannot hold, so {mismatched.length === 1 ? 'it is' : 'they are'}{' '}
                  registered under a different spelling:{' '}
                  {mismatched.slice(0, 3).map((row, i) => (
                    <span key={row.name}>
                      {i > 0 && ', '}
                      <span className="font-mono text-ink">{row.wireIdentity}</span> as{' '}
                      <span className="font-mono text-ink">{row.name}</span>
                    </span>
                  ))}
                  {mismatched.length > 3 && ` and ${mismatched.length - 3} more`}. Anything
                  that derives the schema name from an event's own source — the bus
                  validator does — will look for a name that does not exist and report the
                  schema as missing.
                </span>
              </Note>
            )}

            {stale && <Note tone="warn"><span>
                  This report is {formatAge(ageMs)} old but only covers a{' '}
                  {formatWindow(report.minutes)} window — everything it sampled
                  has now fallen outside that window. Re-run it before acting on
                  “no traffic”.
                </span></Note>}

            {report.truncated && dismissedNote !== report.generatedAt && (
              <div className="relative">
                <Note>
                  {report.scanNote ??
                    'The scan stopped early, so “no traffic” here means “not in this sample” rather than “never published”.'}{' '}
                  The window was sampled in {report.stripes} slices scanned in
                  parallel, so the coverage is spread across the whole period
                  rather than concentrated at one end.
                </Note>
                <button
                  type="button"
                  onClick={() => setDismissedNote(report.generatedAt)}
                  title="Dismiss until the next report"
                  className="absolute right-1 top-1 rounded p-0.5 text-ink-faint hover:bg-surface-3 hover:text-ink"
                >
                  <X className="size-3" />
                </button>
              </div>
            )}

            {/* The health bar is always here; only the detail charts fold. */}
            <StatusBar
              counts={counts}
              total={total}
              active={active}
              onToggle={toggle}
              chartsOpen={chartsOpen}
              onToggleCharts={() => setChartsOpen(!chartsOpen)}
            />

            {chartsOpen && (
              <div className="grid gap-3 lg:grid-cols-2">
                <TrafficChart
                  report={report}
                  onSelect={(name) =>
                    navigate(`/schemas?select=${encodeURIComponent(name)}`)
                  }
                />
                <DriftBreakdown report={report} />
              </div>
            )}

            {/* The filter belongs with the rows it filters, not up in the
                toolbar beside the run controls. */}
            <div className="flex flex-wrap items-center gap-1 border-b border-edge pb-2">
              <span className="mr-1 text-[10px] font-semibold uppercase tracking-wide text-ink-faint">
                Show
              </span>
              {STATUS_ORDER.map((status) => {
                const count = counts[status]
                const isActive = active.has(status)
                return (
                  <button
                    key={status}
                    type="button"
                    disabled={count === 0}
                    onClick={() => toggle(status)}
                    title={`${STATUS[status].hint}. Click to ${isActive ? 'stop showing' : 'show'} these.`}
                    className={cn(
                      'flex items-center gap-1.5 rounded border px-2 py-0.5 text-[10px] font-medium transition-colors',
                      count === 0
                        ? 'cursor-default border-transparent text-ink-faint/40'
                        : 'cursor-pointer',
                      count > 0 && isActive && TONE_ACTIVE[STATUS[status].tone],
                      count > 0 &&
                        !isActive &&
                        'border-edge text-ink-faint hover:bg-surface-2 hover:text-ink-muted',
                    )}
                  >
                    <span className={cn('size-2 shrink-0 rounded-sm', STATUS[status].dot)} />
                    {STATUS[status].label}
                    <span className="font-mono tabular-nums">{count}</span>
                  </button>
                )
              })}
              {active.size > 0 && (
                <button
                  type="button"
                  onClick={() => setActive(() => new Set())}
                  title="Clear the filter and show everything"
                  className="rounded px-1.5 py-0.5 text-[10px] text-ink-faint hover:bg-surface-2 hover:text-ink-muted"
                >
                  clear
                </button>
              )}
              <span className="ml-auto text-[10px] text-ink-faint">
                {visible.length.toLocaleString()} shown
              </span>
            </div>

            {visible.length === 0 ? (
              <EmptyState
                icon={<CheckCircle2 className="size-8" />}
                title={
                  filter
                    ? `Nothing matches “${filter}”`
                    : active.size > 0
                      ? 'Nothing in the selected statuses'
                      : 'No schemas graded'
                }
                detail={
                  active.size > 0 && !filter
                    ? `Showing ${[...active].map((s) => STATUS[s].label).join(', ')}. Click another chip above to widen the filter.`
                    : undefined
                }
              />
            ) : (
              /* Fixed layout: with auto layout the drift cell wrapped into
                 four lines and every row grew to match, so 30 rows filled the
                 screen. Fixed widths plus nowrap keep a row one line tall. */
              <table className="w-full table-fixed border-collapse text-[11px]">
                <thead className="sticky top-0 bg-surface-0">
                  <tr className="border-b border-edge text-left text-ink-faint">
                    {/* Selecting rows is for filing them; a row with nothing
                        wrong has nothing to file. */}
                    <th className="w-[28px] px-2 py-1 font-medium">
                      <Checkbox
                        checked={
                          fileable.length > 0 &&
                          fileable.every((row) => selected.has(row.name))
                        }
                        onChange={(e) =>
                          setSelected(
                            e.target.checked
                              ? new Set(fileable.map((row) => row.name))
                              : new Set(),
                          )
                        }
                        label=""
                        title="Select every schema with something to file"
                      />
                    </th>
                    <th className="w-[92px] px-2 py-1 font-medium">Status</th>
                    {/* Sized to the content rather than left to absorb all the
                        slack — an auto-width name column pushed Pass/Fail to
                        the far side of the window. */}
                    <th className="w-[340px] px-2 py-1 font-medium">Schema</th>
                    <th className="w-[72px] px-2 py-1 text-right font-medium">
                      Pass/Fail
                    </th>
                    <th className="w-[180px] px-2 py-1 font-medium">Drift</th>
                    <th className="px-2 py-1 font-medium">Detail</th>
                  </tr>
                </thead>
                <tbody>
                  {visible.map((row) => (
                    <Row
                      key={row.name}
                      row={row}
                      selectable={FILEABLE.has(row.status)}
                      selected={selected.has(row.name)}
                      onSelect={toggleSelected}
                      onOpen={openRow}
                    />
                  ))}
                </tbody>
              </table>
            )}
          </>
        )}
      </div>

      <BulkFileDialog
        open={filing}
        schemaNames={[...selected]}
        minutes={report?.minutes ?? minutes}
        envId={envId}
        onClose={() => setFiling(false)}
      />
    </div>
  )
}

/**
 * One graded schema.
 *
 * Memoised with stable handlers: a filter keystroke or the staleness tick
 * re-renders the page, and without this every one of ~300 rows reconciled to
 * show the same thing.
 */
const Row = memo(function Row({
  row,
  onOpen,
  selectable,
  selected,
  onSelect,
}: {
  row: DisplayRow
  onOpen: (row: DisplayRow) => void
  selectable: boolean
  selected: boolean
  onSelect: (name: string, next: boolean) => void
}) {
  const status = STATUS[row.status]
  const Icon = status.icon
  const drift =
    row.undeclared + row.typeMismatches + row.enumDrift + row.missingRequired

  return (
    <tr
      onClick={() => onOpen(row)}
      className="group h-6 cursor-pointer border-b border-edge/40 hover:bg-surface-2"
      title={
        row.missing
          ? `Draft a schema for ${row.name} from its own traffic`
          : 'Open this schema'
      }
    >
      {/* Stops the click reaching the row, which navigates away. */}
      <td className="px-2" onClick={(e) => e.stopPropagation()}>
        {selectable && (
          <Checkbox
            checked={selected}
            onChange={(e) => onSelect(row.name, e.target.checked)}
            label=""
            title={`Include ${row.name} when filing tickets`}
          />
        )}
      </td>

      <td className="px-2">
        <span className={cn('inline-flex items-center gap-1 whitespace-nowrap', status.text)}>
          <Icon className="size-3 shrink-0" />
          {status.label}
        </span>
      </td>

      <td className="px-2 font-mono text-ink-muted">
        <span className="block truncate pr-2" title={row.name}>
          {row.name}
          {row.wireIdentity && (
            <span
              className="ml-1.5 text-warn"
              title={`Events carry ${row.wireIdentity}, which cannot be a registry name — anything deriving the name from the event's own source will miss`}
            >
              ≠ {row.wireIdentity}
            </span>
          )}
        </span>
      </td>

      <td className="whitespace-nowrap px-2 text-right font-mono text-ink-faint">
        {row.missing ? (
          <span title="Nothing to grade against — no schema exists">—</span>
        ) : row.sampled === 0 ? (
          <span className="inline-flex items-center gap-1" title="Not seen in this sample">
            <HelpCircle className="size-3" />—
          </span>
        ) : (
          <>
            <span className="text-ok">{row.passed}</span>
            <span className="text-ink-faint">/</span>
            <span className={row.failed > 0 ? 'text-danger' : 'text-ink-faint'}>
              {row.failed}
            </span>
          </>
        )}
      </td>

      {/* One line, always: the counts are abbreviated and never wrap, because
          a wrapping cell used to set the height of every row in the table. */}
      <td className="overflow-hidden whitespace-nowrap px-2 text-ink-faint">
        {drift === 0 ? (
          '—'
        ) : (
          <span className="inline-flex items-center gap-1.5">
            {row.undeclared > 0 && (
              <span className="text-warn" title={`${row.undeclared} field(s) in events but not in the schema`}>
                +{row.undeclared}&nbsp;new
              </span>
            )}
            {row.typeMismatches > 0 && (
              <span className="text-danger" title={`${row.typeMismatches} type mismatch(es)`}>
                {row.typeMismatches}&nbsp;type
              </span>
            )}
            {row.enumDrift > 0 && (
              <span className="text-warn" title={`${row.enumDrift} value(s) outside the declared enum`}>
                {row.enumDrift}&nbsp;enum
              </span>
            )}
            {row.missingRequired > 0 && (
              <span className="text-warn" title={`${row.missingRequired} required field(s) not always present`}>
                {row.missingRequired}&nbsp;req
              </span>
            )}
          </span>
        )}
      </td>

      <td className="px-2 text-ink-faint">
        <span className="flex items-center gap-1.5">
          <span className="min-w-0 truncate" title={row.headline ?? ''}>
            {row.headline ?? ''}
          </span>
          {row.missing && (
            <span className="ml-auto flex shrink-0 items-center gap-0.5 whitespace-nowrap text-[10px] text-accent opacity-0 group-hover:opacity-100">
              <Wand2 className="size-2.5" />
              draft schema
            </span>
          )}
        </span>
      </td>
    </tr>
  )
})
