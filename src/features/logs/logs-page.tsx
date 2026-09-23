import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, RefreshCw, ScrollText, SlidersHorizontal } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { IpcError, LogEvent, LogPage, WatchCondition } from '@/lib/types'
import { ConditionEditor } from '@/features/watch/conditions'
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

  const [logGroup, setLogGroup] = useState('')
  const [minutes, setMinutes] = useState(60)
  const [source, setSource] = useState('')
  const [detailType, setDetailType] = useState('')
  const [rawPattern, setRawPattern] = useState('')
  const [useRaw, setUseRaw] = useState(false)
  // Payload conditions, the same grammar a watch uses, compiled into the
  // pattern with the source and detail type when the search runs.
  const [advanced, setAdvanced] = useState(false)
  const [conditions, setConditions] = useState<WatchCondition[]>([])
  const [compiledPattern, setCompiledPattern] = useState<string | null>(null)
  const [compileError, setCompileError] = useState<IpcError | null>(null)
  const [autoRefresh, setAutoRefresh] = useState(false)
  const [expanded, setExpanded] = useState<Set<number>>(new Set())
  /**
   * Bumped on Search to re-run the query with the current filters. Without
   * this the query would re-fire on every keystroke in the filter inputs.
   */
  const [runToken, setRunToken] = useState(0)

  const groups = useQuery({
    queryKey: ['logs', envId, 'groups'],
    queryFn: () => ipc.listLogGroups(envId),
    enabled: !!envId,
    retry: false,
  })

  // Default to the environment's own global-events group.
  useEffect(() => {
    if (!logGroup && groups.data?.length) {
      const preferred =
        groups.data.find((g) => g.configured && g.name.includes('global-events')) ??
        groups.data.find((g) => g.configured) ??
        groups.data[0]
      setLogGroup(preferred.name)
    }
  }, [groups.data, logGroup])

  const activeConditions = conditions.filter((c) => c.path.trim())
  const structured = !useRaw && advanced && activeConditions.length > 0

  const query = useMemo(
    () => ({
      logGroup,
      startTime: Date.now() - minutes * 60_000,
      // A compiled pattern already carries the source and detail type.
      source: useRaw || structured ? undefined : source || undefined,
      detailType: useRaw || structured ? undefined : detailType || undefined,
      filterPattern: useRaw ? rawPattern || undefined : structured ? (compiledPattern ?? undefined) : undefined,
      limit: 300,
    }),
    // `runToken` is intentionally a dependency: it is what makes Search
    // re-evaluate the (otherwise stable) filter inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [logGroup, minutes, runToken],
  )

  /**
   * Search. With conditions, the pattern is compiled first by the same code
   * that compiles a watch, so the two screens cannot disagree about the
   * grammar, and the query only runs once the pattern is in hand.
   */
  const search = async () => {
    setCompileError(null)
    if (structured) {
      try {
        const compiled = await ipc.compileWatchPattern({
          id: '',
          envId: envId ?? '',
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
        setCompiledPattern(compiled.pattern)
      } catch (e) {
        setCompileError(ipc.asIpcError(e))
        return
      }
    }
    setRunToken((t) => t + 1)
  }

  const logs = useQuery({
    queryKey: ['logs', envId, 'events', query],
    queryFn: () => ipc.queryLogs(query, envId),
    enabled: !!envId && !!logGroup,
    retry: false,
    refetchInterval: autoRefresh ? 10_000 : false,
  })

  // Older pages, fetched on demand from where the last scan stopped. Reset
  // whenever the search itself changes.
  const [older, setOlder] = useState<LogPage[]>([])
  const [continuing, setContinuing] = useState<IpcError | 'busy' | null>(null)
  useEffect(() => {
    setOlder([])
    setContinuing(null)
  }, [query])
  const last = older.at(-1) ?? logs.data
  const events = useMemo(
    () => [...(logs.data?.events ?? []), ...older.flatMap((p) => p.events)],
    [logs.data, older],
  )
  const searchMore = async () => {
    if (!last) return
    setContinuing('busy')
    try {
      const page = await ipc.queryLogs({ ...query, endTime: last.searchedFrom }, envId)
      setOlder((prev) => [...prev, page])
      setContinuing(null)
    } catch (e) {
      setContinuing(ipc.asIpcError(e))
    }
  }

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
      <Toolbar>
        <Select
          value={logGroup}
          onChange={(e) => setLogGroup(e.target.value)}
          className="w-72"
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
          onChange={(e) => setMinutes(Number(e.target.value))}
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
            onChange={(e) => setRawPattern(e.target.value)}
            placeholder={'{ $.detail.veteranId = "123" }'}
            className="w-96 font-mono"
            spellCheck={false}
          />
        ) : (
          <>
            <Input
              value={source}
              onChange={(e) => setSource(e.target.value)}
              placeholder="source (orders*)"
              title="Exact match, or use * as a leading/trailing wildcard"
              className="w-44"
              spellCheck={false}
            />
            <Input
              value={detailType}
              onChange={(e) => setDetailType(e.target.value)}
              placeholder="detail-type (*assigned)"
              title="Exact match, or use * as a leading/trailing wildcard"
              className="w-52"
              spellCheck={false}
            />
          </>
        )}

        <Checkbox
          checked={useRaw}
          onChange={(e) => setUseRaw(e.target.checked)}
          label="raw pattern"
        />

        {!useRaw && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setAdvanced((a) => !a)}
            title="Add conditions on the event payload"
            className={cn(advanced && 'bg-surface-3 text-ink')}
          >
            <SlidersHorizontal className="size-3" />
            Advanced
            {activeConditions.length > 0 && ` (${activeConditions.length})`}
          </Button>
        )}

        <Button variant="primary" onClick={search}>
          Search
        </Button>

        <div className="ml-auto flex items-center gap-2">
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
              onChange={setConditions}
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
  expanded,
  onToggle,
}: {
  event: LogEvent
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
        <td className="pr-1 text-right">
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
