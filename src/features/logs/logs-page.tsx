import { useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { Check, ChevronDown, ChevronRight, Copy, RefreshCw, ScrollText } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { LogEvent } from '@/lib/types'
import {
  Badge,
  Button,
  Checkbox,
  EmptyState,
  ErrorBox,
  Input,
  Select,
  Spinner,
  Toolbar,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'

const RANGES = [
  { label: 'Last 15m', minutes: 15 },
  { label: 'Last 1h', minutes: 60 },
  { label: 'Last 6h', minutes: 360 },
  { label: 'Last 24h', minutes: 1440 },
  { label: 'Last 7d', minutes: 10080 },
]

function formatTime(ms: number | null): string {
  if (ms === null) return '—'
  const date = new Date(ms)
  return `${date.toLocaleTimeString([], { hour12: false })}.${String(date.getMilliseconds()).padStart(3, '0')}`
}

export function LogsPage() {
  const { envId, activeEnvironment } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)

  const [logGroup, setLogGroup] = useState('')
  const [minutes, setMinutes] = useState(60)
  const [source, setSource] = useState('')
  const [detailType, setDetailType] = useState('')
  const [rawPattern, setRawPattern] = useState('')
  const [useRaw, setUseRaw] = useState(false)
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

  const query = useMemo(
    () => ({
      logGroup,
      startTime: Date.now() - minutes * 60_000,
      source: useRaw ? undefined : source || undefined,
      detailType: useRaw ? undefined : detailType || undefined,
      filterPattern: useRaw ? rawPattern || undefined : undefined,
      limit: 300,
    }),
    // `runToken` is intentionally a dependency: it is what makes Search
    // re-evaluate the (otherwise stable) filter inputs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [logGroup, minutes, runToken],
  )

  const logs = useQuery({
    queryKey: ['logs', envId, 'events', query],
    queryFn: () => ipc.queryLogs(query, envId),
    enabled: !!envId && !!logGroup,
    retry: false,
    refetchInterval: autoRefresh ? 10_000 : false,
  })

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

        <Button variant="primary" onClick={() => setRunToken((t) => t + 1)}>
          Search
        </Button>

        <div className="ml-auto flex items-center gap-2">
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
              {logs.data.events.length} event
              {logs.data.events.length === 1 ? '' : 's'}
            </span>
          )}
        </div>
      </Toolbar>

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

        {logs.data && logs.data.events.length === 0 && (
          <EmptyState
            icon={<ScrollText className="size-8" />}
            title="No events in this window"
            detail="Widen the time range, or check the source and detail-type filters."
          />
        )}

        {logs.data && logs.data.events.length > 0 && (
          <table className="w-full border-collapse text-[11px]">
            <thead className="sticky top-0 bg-surface-1">
              <tr className="border-b border-edge text-left text-ink-faint">
                <th className="w-6" />
                <th className="w-24 px-2 py-1 font-medium">Time</th>
                <th className="w-48 px-2 py-1 font-medium">Source</th>
                <th className="w-56 px-2 py-1 font-medium">Detail type</th>
                <th className="px-2 py-1 font-medium">Event id</th>
                <th className="w-8" />
              </tr>
            </thead>
            <tbody>
              {logs.data.events.map((event, index) => (
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

function CopyEventButton({ event }: { event: LogEvent }) {
  const [copied, setCopied] = useState(false)
  return (
    <Button
      variant="ghost"
      size="sm"
      title="Copy event JSON"
      onClick={(e) => {
        // The row toggles on click; copying should not expand it.
        e.stopPropagation()
        void navigator.clipboard.writeText(eventText(event)).then(() => {
          setCopied(true)
          setTimeout(() => setCopied(false), 1500)
        })
      }}
    >
      {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
    </Button>
  )
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
        <td className="px-2 py-1 font-mono text-ink-faint">
          {formatTime(event.timestamp)}
        </td>
        <td className="px-2 py-1">
          {event.source ? (
            <Badge tone="info">{event.source}</Badge>
          ) : (
            <span className="text-ink-faint">—</span>
          )}
        </td>
        <td className="truncate px-2 py-1 font-mono text-ink-muted">
          {event.detailType ?? '—'}
        </td>
        <td className="truncate px-2 py-1 font-mono text-ink-faint">
          {event.eventId ?? '—'}
        </td>
        <td className="pr-1 text-right">
          <CopyEventButton event={event} />
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
