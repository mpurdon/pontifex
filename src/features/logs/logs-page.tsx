import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, RefreshCw, ScrollText, SlidersHorizontal } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { IpcError, LogEvent } from '@/lib/types'
import { ConditionEditor } from '@/features/watch/conditions'
import { useSchemaIndex } from '@/features/schemas/schema-links'
import { SchemaRowActions } from '@/features/schemas/schema-row-actions'
import { useWorkbench, type LogFilters } from '@/features/schemas/workbench-context'
import {
  Badge,
  Button,
  Checkbox,
  CopyButton,
  EmptyState,
  ErrorBox,
  Input,
  Select,
  Spinner,
  Toolbar,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'
import { formatDateTime, formatTime } from '@/lib/format'
import { TimeZoneToggle } from '@/components/time-zone-toggle'
import { cn } from '@/components/ui'

const RANGES = [
  { label: 'Last 15m', minutes: 15 },
  { label: 'Last 1h', minutes: 60 },
  { label: 'Last 6h', minutes: 360 },
  { label: 'Last 24h', minutes: 1440 },
  { label: 'Last 7d', minutes: 10080 },
]

export function LogsPage() {
  const { envId, activeEnvironment, timeZone } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)

  /**
   * The filter bar and the search it has run live above the router: a search
   * is work, and opening a schema or a watch to check something used to throw
   * it away. Only Clear — or quitting the app — puts the bar back to defaults.
   */
  const {
    logFilters: filters,
    setLogFilters,
    logRun,
    startLogRun,
    appendLogPage,
    clearLogSearch,
  } = useWorkbench()
  const {
    logGroup,
    minutes,
    source,
    detailType,
    rawPattern,
    useRaw,
    // Payload conditions, the same grammar a watch uses, compiled into the
    // pattern with the source and detail type when the search runs.
    advanced,
    conditions,
  } = filters
  const set = <K extends keyof LogFilters>(key: K, value: LogFilters[K]) =>
    setLogFilters((prev) => ({ ...prev, [key]: value }))

  const [compileError, setCompileError] = useState<IpcError | null>(null)
  const [autoRefresh, setAutoRefresh] = useState(false)
  const [expanded, setExpanded] = useState<Set<number>>(new Set())

  const groups = useQuery({
    queryKey: ['logs', envId, 'groups'],
    queryFn: () => ipc.listLogGroups(envId),
    enabled: !!envId,
    retry: false,
  })

  // Whether the chosen group is one this environment has. The filters outlive
  // an environment switch, so the group left from the last one — a
  // `dev-global-events` against the prd account — would otherwise be searched.
  // A list that failed to load cannot say, so the typed group is trusted then.
  const groupKnown =
    !!logGroup && (groups.isError || !!groups.data?.some((g) => g.name === logGroup))

  // Default to the environment's own global-events group.
  useEffect(() => {
    if (!groupKnown && groups.data?.length) {
      const preferred =
        groups.data.find((g) => g.configured && g.name.includes('global-events')) ??
        groups.data.find((g) => g.configured) ??
        groups.data[0]
      set('logGroup', preferred.name)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groups.data, groupKnown])

  const activeConditions = conditions.filter((c) => c.path.trim())
  const structured = !useRaw && advanced && activeConditions.length > 0

  /**
   * Search. With conditions, the pattern is compiled first by the same code
   * that compiles a watch, so the two screens cannot disagree about the
   * grammar, and the query only runs once the pattern is in hand.
   *
   * The query is built here rather than derived from the filters, so editing a
   * field does not re-fire the search on every keystroke — and so the query
   * that is running keeps its identity, and its cached results, while you are
   * off on another screen.
   */
  const search = async () => {
    if (!envId || !groupKnown) return
    setCompileError(null)
    let pattern = useRaw ? rawPattern || undefined : undefined
    if (structured) {
      try {
        const compiled = await ipc.compileWatchPattern({
          id: '',
          envId,
          label: '',
          enabled: true,
          logGroup,
          source: source || null,
          detailType: detailType || null,
          conditions: activeConditions,
          rawPattern: null,
          notify: false,
          color: null,
        })
        set('compiledPattern', compiled.pattern)
        pattern = compiled.pattern ?? undefined
      } catch (e) {
        setCompileError(ipc.asIpcError(e))
        return
      }
    }
    startLogRun(envId, {
      logGroup,
      startTime: Date.now() - minutes * 60_000,
      // A compiled pattern already carries the source and detail type.
      source: useRaw || structured ? undefined : source || undefined,
      detailType: useRaw || structured ? undefined : detailType || undefined,
      filterPattern: pattern,
      limit: 300,
    })
  }

  // A run belongs to the environment it was made in; after a switch the
  // effect below starts a fresh one rather than showing another account's
  // events under this account's name.
  const run = logRun?.envId === envId ? logRun : null

  /**
   * Nothing to show and nothing running: search once the log group is known,
   * so arriving on the screen — or switching environment — opens on events
   * instead of an empty panel waiting for a click.
   */
  useEffect(() => {
    if (run || !envId || !groupKnown) return
    void search()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [run, envId, groupKnown])

  const query = run?.query
  const logs = useQuery({
    queryKey: ['logs', envId, 'events', query],
    queryFn: () => ipc.queryLogs(query!, envId),
    enabled: !!envId && !!query,
    retry: false,
    refetchInterval: autoRefresh ? 10_000 : false,
  })

  // Older pages, fetched on demand from where the last scan stopped. They
  // belong to the run, so a new search drops them with it.
  const older = run?.older ?? []
  const [continuing, setContinuing] = useState<IpcError | 'busy' | null>(null)
  useEffect(() => {
    setContinuing(null)
  }, [query])
  const last = older.at(-1) ?? logs.data
  const events = useMemo(
    () => [...(logs.data?.events ?? []), ...older.flatMap((p) => p.events)],
    [logs.data, older],
  )
  const searchMore = async () => {
    if (!last || !query) return
    setContinuing('busy')
    try {
      const page = await ipc.queryLogs({ ...query, endTime: last.searchedFrom }, envId)
      appendLogPage(page)
      setContinuing(null)
    } catch (e) {
      setContinuing(ipc.asIpcError(e))
    }
  }

  /** So a row can offer the schema for its event type, or offer to draft one. */
  const schemaIndex = useSchemaIndex(envId)

  const toggle = (index: number) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(index)) next.delete(index)
      else next.add(index)
      return next
    })
  }

  return (
    <div className="flex h-full flex-col">
      {/* One line. Wrapped, the filters and the Search button they belong to
          ended up on different rows, which reads as two unrelated bars — and
          auto-refresh and the event count moved with them.

          The fields give way rather than the bar: every control keeps its
          size except the three text fields, which shrink to a floor wide
          enough to still read a source name. Narrower than that and the bar
          scrolls. */}
      <Toolbar className="flex-nowrap overflow-x-auto">
        <Select
          value={logGroup}
          onChange={(e) => set('logGroup', e.target.value)}
          className="w-72 min-w-32"
        >
          {groups.data?.map((group) => (
            <option key={group.name} value={group.name}>
              {group.configured ? '★ ' : ''}
              {group.name}
            </option>
          ))}
        </Select>

        <Select
          value={minutes}
          onChange={(e) => set('minutes', Number(e.target.value))}
          className="shrink-0"
        >
          {RANGES.map((range) => (
            <option key={range.minutes} value={range.minutes}>
              {range.label}
            </option>
          ))}
        </Select>

        {useRaw ? (
          <Input
            value={rawPattern}
            onChange={(e) => set('rawPattern', e.target.value)}
            placeholder={'{ $.detail.veteranId = "123" }'}
            className="w-96 min-w-40 font-mono"
            spellCheck={false}
          />
        ) : (
          <>
            <Input
              value={source}
              onChange={(e) => set('source', e.target.value)}
              placeholder="source (orders*)"
              title="Exact match, or use * as a leading/trailing wildcard"
              className="w-44 min-w-28"
              spellCheck={false}
            />
            <Input
              value={detailType}
              onChange={(e) => set('detailType', e.target.value)}
              placeholder="detail-type (*assigned)"
              title="Exact match, or use * as a leading/trailing wildcard"
              className="w-52 min-w-28"
              spellCheck={false}
            />
          </>
        )}

        <Checkbox
          checked={useRaw}
          onChange={(e) => set('useRaw', e.target.checked)}
          label="raw pattern"
          className="shrink-0"
        />

        {!useRaw && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => set('advanced', !advanced)}
            title="Add conditions on the event payload"
            className={cn('shrink-0', advanced && 'bg-surface-3 text-ink')}
          >
            <SlidersHorizontal className="size-3" />
            Advanced
            {activeConditions.length > 0 && ` (${activeConditions.length})`}
          </Button>
        )}

        <Button variant="primary" onClick={() => void search()} className="shrink-0">
          Search
        </Button>

        {/* The one thing that throws a search away, since nothing else does
            any more. */}
        <Button
          variant="ghost"
          size="sm"
          onClick={clearLogSearch}
          title="Put the filters back to their defaults"
          className="shrink-0"
        >
          Clear
        </Button>

        <div className="ml-auto flex shrink-0 items-center gap-2">
          <TimeZoneToggle />
          <Checkbox
            checked={autoRefresh}
            onChange={(e) => setAutoRefresh(e.target.checked)}
            label="auto-refresh"
          />
          <Button
            variant="ghost"
            size="sm"
            onClick={() => logs.refetch()}
            title="Refresh now"
          >
            <RefreshCw
              className={logs.isFetching ? 'size-3 animate-spin' : 'size-3'}
            />
          </Button>
          {logs.data && (
            <span className="text-[11px] text-ink-faint">
              {events.length} event
              {events.length === 1 ? '' : 's'}
            </span>
          )}
        </div>
      </Toolbar>

      {advanced && !useRaw && (
        <div className="flex flex-col gap-2 border-b border-edge bg-surface-1 px-3 py-2">
          <div className="max-w-2xl">
            <ConditionEditor
              conditions={conditions}
              onChange={(next) => set('conditions', next)}
              envId={envId}
              source={source}
              detailType={detailType}
            />
          </div>
          {compileError && <ErrorBox error={compileError} className="max-w-2xl" />}
        </div>
      )}

      {logs.data?.filterPattern && (
        <div className="border-b border-edge bg-surface-1 px-3 py-1 font-mono text-[10px] text-ink-faint">
          {logs.data.filterPattern}
        </div>
      )}

      <div className="min-h-0 flex-1 overflow-auto">
        {groups.isError && (
          <div className="p-3">
            <ErrorBox error={ipc.asIpcError(groups.error)} {...credentials} />
          </div>
        )}
        {logs.isError && (
          <div className="p-3">
            <ErrorBox error={ipc.asIpcError(logs.error)} {...credentials} />
          </div>
        )}
        {logs.isLoading && <Spinner label="Querying CloudWatch…" />}

        {/* How far back the scan reached is the fact that turns "no events"
            into an answer: nothing between here and now, or nothing looked at
            yet. Continue picks up where it stopped. */}
        {last && !logs.isLoading && (
          <div className="flex flex-wrap items-center gap-2 border-b border-edge/60 px-3 py-1.5 text-[10px] text-ink-faint">
            <span>
              Searched back to{' '}
              <span className="font-mono text-ink-muted">
                {formatDateTime(last.searchedFrom, timeZone)}
              </span>
              {last.complete ? ' — the whole window' : ''}
            </span>
            {last.scanNote && <span>· {last.scanNote}</span>}
            {!last.complete && (
              <Button
                size="sm"
                loading={continuing === 'busy'}
                onClick={searchMore}
                title="Search the next stretch of the window, further back in time"
              >
                Continue further back
              </Button>
            )}
            {continuing && continuing !== 'busy' && (
              <ErrorBox error={continuing} className="w-full" />
            )}
          </div>
        )}

        {logs.data && events.length === 0 && (
          <EmptyState
            icon={<ScrollText className="size-8" />}
            title={last?.complete ? 'No events in this window' : 'Nothing yet in the stretch searched'}
            detail={
              last?.complete
                ? 'Widen the time range, or check the source and detail-type filters.'
                : 'The scan stopped before reaching the start of the window. Continue further back, or narrow the filters.'
            }
          />
        )}

        {logs.data && events.length > 0 && (
          <table className="w-full border-collapse text-[11px]">
            <thead className="sticky top-0 bg-surface-1">
              <tr className="border-b border-edge text-left text-ink-faint">
                <th className="w-6" />
                <th className="whitespace-nowrap px-2 py-1 font-medium">
                  Time <span className="font-normal text-ink-faint">{timeZone === 'utc' ? 'UTC' : 'local'}</span>
                </th>
                <th className="whitespace-nowrap px-2 py-1 font-medium">Source</th>
                <th className="whitespace-nowrap px-2 py-1 font-medium">Detail type</th>
                {/* The slack column: sized to whatever the name columns leave. */}
                <th className="w-full px-2 py-1 font-medium">Event id</th>
                <th className="w-8" />
              </tr>
            </thead>
            <tbody>
              {events.map((event, index) => (
                <LogRow
                  key={`${event.timestamp}-${index}`}
                  event={event}
                  // The group the rows were searched in, not the one the bar
                  // has been changed to since: a draft scans it for this event.
                  logGroup={query?.logGroup ?? logGroup}
                  schemaName={schemaIndex.nameFor(event.source, event.detailType)}
                  schemasKnown={schemaIndex.known}
                  expanded={expanded.has(index)}
                  onToggle={() => toggle(index)}
                />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  )
}

/** The text a row shows when expanded — and what its copy button copies. */
function eventText(event: LogEvent): string {
  return event.event ? JSON.stringify(event.event, null, 2) : event.message
}

function LogRow({
  event,
  logGroup,
  schemaName,
  schemasKnown,
  expanded,
  onToggle,
}: {
  event: LogEvent
  /** The group this row was read from, for drafting from the same place. */
  logGroup: string
  /** The registered schema for this event type, when one exists. */
  schemaName: string | null
  /** False while the registry list is loading, so neither action is offered. */
  schemasKnown: boolean
  expanded: boolean
  onToggle: () => void
}) {
  const { timeZone } = useSettings()
  return (
    <>
      <tr
        onClick={onToggle}
        className="cursor-pointer border-b border-edge/40 hover:bg-surface-2"
      >
        <td className="pl-2 text-ink-faint">
          {expanded ? (
            <ChevronDown className="size-3" />
          ) : (
            <ChevronRight className="size-3" />
          )}
        </td>
        <td className="whitespace-nowrap px-2 py-1 font-mono text-ink-faint">
          {formatTime(event.timestamp, timeZone)}
        </td>
        <td className="whitespace-nowrap px-2 py-1">
          {event.source ? (
            <Badge tone="info">{event.source}</Badge>
          ) : (
            <span className="text-ink-faint">—</span>
          )}
        </td>
        <td className="whitespace-nowrap px-2 py-1 font-mono text-ink-muted">
          {event.detailType ?? '—'}
        </td>
        <td className="w-full max-w-0 truncate px-2 py-1 font-mono text-ink-faint">
          {event.eventId ?? '—'}
        </td>
        <td className="whitespace-nowrap pr-1 text-right">
          <SchemaRowActions
            source={event.source}
            detailType={event.detailType}
            logGroup={logGroup}
            timestamp={event.timestamp}
            schemaName={schemaName}
            known={schemasKnown}
          />
          <CopyButton text={eventText(event)} title="Copy event JSON" />
        </td>
      </tr>
      {expanded && (
        <tr className="border-b border-edge/40 bg-surface-0">
          <td colSpan={6} className="p-0">
            <pre className="max-h-80 overflow-auto p-3 font-mono text-[11px] leading-relaxed text-ink-muted">
              {eventText(event)}
            </pre>
          </td>
        </tr>
      )}
    </>
  )
}
