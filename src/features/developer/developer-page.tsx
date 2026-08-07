import { useMutation, useQuery } from '@tanstack/react-query'
import { useEffect, useMemo, useRef, useState } from 'react'
import {
  Check,
  Copy,
  FolderOpen,
  RefreshCw,
  Trash2,
  Wrench,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { DevInfo, IpcError } from '@/lib/types'
import {
  Badge,
  Button,
  Checkbox,
  ErrorBox,
  Input,
  Note,
  Panel,
  Select,
  Spinner,
  Toolbar,
  cn,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { formatBytes } from '@/lib/format'
import {
  categoryCounts,
  filterLogLines,
  parseLogLines,
  type LogLevelName,
} from '@/lib/log-line'

/** Colour a line by severity; unparsed lines stay muted. */
const LEVEL_CLASS: Record<string, string> = {
  error: 'text-danger',
  warn: 'text-warn',
  info: 'text-ink-muted',
  debug: 'text-ink-faint',
  trace: 'text-ink-faint',
}

/** Each category gets a stable colour so the log is scannable by eye. */
const CATEGORY_CLASS: Record<string, string> = {
  ipc: 'text-accent',
  aws: 'text-warn',
  sso: 'text-warn',
  registry: 'text-info',
  schema: 'text-info',
  events: 'text-ok',
  cache: 'text-ok',
  bedrock: 'text-info',
  topology: 'text-info',
  settings: 'text-ink-muted',
  app: 'text-ink-muted',
  ui: 'text-accent',
}

/** A label/value definition list — the shape every panel on this page uses. */
function Facts({ rows }: { rows: (readonly [string, string | number, string?])[] }) {
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-[11px]">
      {rows.map(([label, value, hint]) => (
        <div key={label} className="contents">
          <dt className="text-ink-faint" title={hint}>
            {label}
          </dt>
          <dd className="font-mono text-ink-muted">{value}</dd>
        </div>
      ))}
    </dl>
  )
}

/**
 * Diagnostics for the app itself — where its files are, how big they have
 * grown, and what it has been logging.
 *
 * Everything here answers "is gebman behaving as expected", which is otherwise
 * only visible from a terminal the app was not started from.
 */
export function DeveloperPage() {
  const { envId } = useSettings()
  const [logLines, setLogLines] = useState(300)
  const [minLevel, setMinLevel] = useState<LogLevelName | null>(null)
  const [selectedCategories, setSelectedCategories] = useState<Set<string>>(new Set())
  const [logFilter, setLogFilter] = useState('')
  const [follow, setFollow] = useState(true)
  const [copied, setCopied] = useState(false)
  const logRef = useRef<HTMLPreElement>(null)

  const info = useQuery<DevInfo>({
    queryKey: ['devInfo'],
    queryFn: ipc.devInfo,
    refetchInterval: 10_000,
    retry: false,
  })

  const logs = useQuery({
    queryKey: ['appLogs', logLines],
    queryFn: () => ipc.readAppLogs(logLines),
    refetchInterval: follow ? 3_000 : false,
    retry: false,
  })

  // The vocabulary comes from the backend so the chips cannot drift from the
  // categories that actually exist; anything unexpected in the buffer is
  // appended so it is still selectable.
  const knownCategories = useQuery({
    queryKey: ['logCategories'],
    queryFn: ipc.logCategories,
    staleTime: Infinity,
    retry: false,
  })

  const openPath = useMutation<void, IpcError, string>({
    mutationFn: ipc.openAppPath,
  })
  const clearLogs = useMutation<void, IpcError>({
    mutationFn: ipc.clearAppLogs,
    onSuccess: () => logs.refetch(),
  })
  const clearCache = useMutation<number, IpcError>({
    mutationFn: () => ipc.clearEventCache(envId, true),
    onSuccess: () => info.refetch(),
  })

  // Polled rather than derived: the point is to see a call that is *still*
  // running, which no completed-call log line can show.
  const [inFlight, setInFlight] = useState<{ describe: string; elapsedMs: number }[]>([])
  useEffect(() => {
    const tick = () => setInFlight(ipc.inFlightCalls())
    tick()
    const timer = setInterval(tick, 1000)
    return () => clearInterval(timer)
  }, [])

  const parsed = useMemo(() => parseLogLines(logs.data ?? []), [logs.data])

  // Counts come from the *unfiltered* set, so a chip still tells you how much
  // it would bring back after you have narrowed things down.
  const counts = useMemo(() => categoryCounts(parsed), [parsed])

  const visibleLines = useMemo(
    () =>
      filterLogLines(parsed, {
        categories: selectedCategories,
        minLevel,
        text: logFilter,
      }),
    [parsed, selectedCategories, minLevel, logFilter],
  )

  const categoryChips = useMemo(() => {
    const known = knownCategories.data ?? []
    const extra = [...counts.keys()].filter((c) => !known.includes(c)).sort()
    return [...known, ...extra]
  }, [knownCategories.data, counts])

  const toggleCategory = (category: string) =>
    setSelectedCategories((prev) => {
      const next = new Set(prev)
      if (next.has(category)) next.delete(category)
      else next.add(category)
      return next
    })

  useEffect(() => {
    if (follow && logRef.current) {
      logRef.current.scrollTop = logRef.current.scrollHeight
    }
  }, [visibleLines, follow])

  /** A single block to paste into a bug report. */
  const copyDiagnostics = async () => {
    if (!info.data) return
    const { runtime, storage, caches, logFile } = info.data
    const text = [
      `gebman ${runtime.appVersion} (${runtime.profile})`,
      `tauri ${runtime.tauriVersion} · ${runtime.os}/${runtime.arch} · log level ${runtime.logLevel}`,
      '',
      'Storage:',
      ...storage.map(
        (s) =>
          `  ${s.label}: ${formatBytes(s.bytes)} in ${s.files} file(s) — ${s.path}${s.exists ? '' : ' (missing)'}`,
      ),
      '',
      `Caches: ${caches.sdkConfigs} SDK config(s), ${caches.eventTypes} event type(s), ${caches.cachedEvents} cached event(s)`,
      `Log file: ${logFile ?? 'none yet'}`,
      '',
      'Recent errors and warnings:',
      ...parsed
        .filter((l) => l.level === 'error' || l.level === 'warn')
        .slice(-20)
        .map((l) => `  ${l.raw}`),
    ].join('\n')

    await navigator.clipboard.writeText(text)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="flex h-full flex-col">
      <Toolbar>
        <Wrench className="size-3.5 text-ink-faint" />
        <span className="text-xs font-semibold text-ink">Developer</span>
        {info.data && (
          <span className="font-mono text-[10px] text-ink-faint">
            v{info.data.runtime.appVersion} · {info.data.runtime.profile}
          </span>
        )}
        <div className="ml-auto flex items-center gap-1.5">
          <Button variant="ghost" size="sm" onClick={copyDiagnostics}>
            {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
            Copy diagnostics
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              info.refetch()
              logs.refetch()
            }}
          >
            <RefreshCw
              className={cn('size-3', info.isFetching && 'animate-spin')}
            />
          </Button>
        </div>
      </Toolbar>

      <div className="min-h-0 flex-1 space-y-3 overflow-auto p-3">
        {info.isError && <ErrorBox error={ipc.asIpcError(info.error)} />}
        {info.isLoading && <Spinner label="Gathering diagnostics…" />}

        {info.data && (
          <>
            <div className="grid grid-cols-2 gap-3">
              <Panel title="Runtime" bodyClassName="p-2">
                <Facts
                  rows={[
                    ['Version', info.data.runtime.appVersion],
                    ['Build', info.data.runtime.profile],
                    ['Tauri', info.data.runtime.tauriVersion],
                    ['Platform', `${info.data.runtime.os}/${info.data.runtime.arch}`],
                    ['Log level', info.data.runtime.logLevel],
                  ]}
                />
              </Panel>

              <Panel
                title="Caches"
                actions={
                  <Button
                    variant="ghost"
                    size="sm"
                    loading={clearCache.isPending}
                    onClick={() => clearCache.mutate()}
                    title="Drop every cached event sample"
                  >
                    <Trash2 className="size-3" />
                    Clear events
                  </Button>
                }
                bodyClassName="p-2"
              >
                <Facts
                  rows={[
                    [
                      'SDK configs',
                      info.data.caches.sdkConfigs,
                      'Resolved AWS SDK configs held in memory',
                    ],
                    ['Event types', info.data.caches.eventTypes],
                    ['Cached events', info.data.caches.cachedEvents.toLocaleString()],
                  ]}
                />
                {clearCache.isSuccess && (
                  <Note className="mt-2">
                    Cleared {clearCache.data} cached event types.
                  </Note>
                )}
              </Panel>
            </div>

            <Panel
              title={
                <span className="flex items-center gap-2">
                  In-flight calls
                  {inFlight.length > 0 && (
                    <Badge tone={inFlight.some((c) => c.elapsedMs > 10_000) ? 'warn' : 'neutral'}>
                      {inFlight.length}
                    </Badge>
                  )}
                </span>
              }
              bodyClassName="p-2"
            >
              {inFlight.length === 0 ? (
                <p className="text-[11px] text-ink-faint">
                  Nothing waiting on the backend.
                </p>
              ) : (
                <ul className="flex flex-col gap-1">
                  {inFlight.map((call) => (
                    <li
                      key={call.describe}
                      className="flex items-center gap-2 rounded-md border border-edge bg-surface-2 px-2 py-1"
                    >
                      <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-ink-muted">
                        {call.describe}
                      </span>
                      <Badge tone={call.elapsedMs > 10_000 ? 'warn' : 'neutral'}>
                        {(call.elapsedMs / 1000).toFixed(1)}s
                      </Badge>
                    </li>
                  ))}
                </ul>
              )}
            </Panel>

            <Panel title="Storage" bodyClassName="p-2">
              <ul className="flex flex-col gap-1">
                {info.data.storage.map((location) => (
                  <li
                    key={location.id}
                    className="flex items-center gap-2 rounded-md border border-edge bg-surface-2 px-2 py-1.5"
                  >
                    <span className="w-16 shrink-0 text-[11px] font-medium text-ink">
                      {location.label}
                    </span>
                    <div className="min-w-0 flex-1">
                      <p
                        className="truncate font-mono text-[10px] text-ink-muted"
                        title={location.path}
                      >
                        {location.path}
                      </p>
                      <p className="truncate text-[10px] text-ink-faint">
                        {location.note}
                      </p>
                    </div>
                    {!location.exists && <Badge tone="neutral">not created</Badge>}
                    <Badge tone="neutral">{formatBytes(location.bytes)}</Badge>
                    <span className="w-16 shrink-0 text-right text-[10px] text-ink-faint">
                      {location.files} file{location.files === 1 ? '' : 's'}
                    </span>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() => openPath.mutate(location.id)}
                      title="Reveal in file manager"
                    >
                      <FolderOpen className="size-3" />
                    </Button>
                  </li>
                ))}
              </ul>
              {openPath.isError && (
                <ErrorBox className="mt-2" error={openPath.error} />
              )}
            </Panel>

            <Panel
              title={
                <span className="flex items-center gap-2">
                  Logs
                  {info.data.logFile ? (
                    <span
                      className="font-mono text-[10px] font-normal text-ink-faint"
                      title={info.data.logFile}
                    >
                      {info.data.logFile.split('/').pop()}
                    </span>
                  ) : (
                    <Badge tone="neutral">no file yet</Badge>
                  )}
                </span>
              }
              actions={
                <div className="flex items-center gap-1.5">
                  <Select
                    value={minLevel ?? ''}
                    onChange={(e) =>
                      setMinLevel((e.target.value || null) as LogLevelName | null)
                    }
                    title="Show this level and everything more severe"
                  >
                    <option value="">all levels</option>
                    <option value="error">error</option>
                    <option value="warn">warn and above</option>
                    <option value="info">info and above</option>
                    <option value="debug">debug and above</option>
                    <option value="trace">trace and above</option>
                  </Select>
                  <Select
                    value={logLines}
                    onChange={(e) => setLogLines(Number(e.target.value))}
                  >
                    {[100, 300, 1000, 5000].map((n) => (
                      <option key={n} value={n}>
                        last {n}
                      </option>
                    ))}
                  </Select>
                  <Input
                    value={logFilter}
                    onChange={(e) => setLogFilter(e.target.value)}
                    placeholder="filter…"
                    className="w-40"
                  />
                  <Checkbox
                    checked={follow}
                    onChange={(e) => setFollow(e.target.checked)}
                    label="follow"
                  />
                  <Button
                    variant="ghost"
                    size="sm"
                    loading={clearLogs.isPending}
                    onClick={() => clearLogs.mutate()}
                  >
                    <Trash2 className="size-3" />
                  </Button>
                </div>
              }
              className="min-h-80"
              bodyClassName="p-0"
            >
              {logs.isError && (
                <div className="p-2">
                  <ErrorBox error={ipc.asIpcError(logs.error)} />
                </div>
              )}

              <div className="flex flex-wrap items-center gap-1 border-b border-edge px-2 py-1.5">
                <span className="mr-1 text-[10px] text-ink-faint">categories</span>
                {categoryChips.map((category) => {
                  const active = selectedCategories.has(category)
                  return (
                    <button
                      key={category}
                      type="button"
                      onClick={() => toggleCategory(category)}
                      title={`${counts.get(category) ?? 0} line(s) in the current buffer`}
                      className={cn(
                        'rounded-full border px-2 py-0.5 font-mono text-[10px] transition-colors',
                        active
                          ? 'border-accent bg-accent/15 text-ink'
                          : 'border-edge text-ink-faint hover:bg-surface-2',
                        !counts.has(category) && !active && 'opacity-40',
                      )}
                    >
                      {category}
                      {counts.has(category) && (
                        <span className="ml-1 text-ink-faint">{counts.get(category)}</span>
                      )}
                    </button>
                  )
                })}
                {selectedCategories.size > 0 && (
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setSelectedCategories(new Set())}
                  >
                    clear
                  </Button>
                )}
                <span className="ml-auto text-[10px] text-ink-faint">
                  {visibleLines.length} of {parsed.length}
                </span>
              </div>

              {visibleLines.length === 0 ? (
                <p className="p-4 text-center text-[11px] text-ink-faint">
                  {parsed.length
                    ? 'Nothing matches these filters.'
                    : 'Nothing logged yet.'}
                </p>
              ) : (
                <pre
                  ref={logRef}
                  className="max-h-96 overflow-auto p-2 font-mono text-[10px] leading-relaxed"
                >
                  {visibleLines.map((line, i) => (
                    <div key={i} className="whitespace-pre-wrap">
                      {line.timestamp && (
                        <span className="text-ink-faint">
                          {line.timestamp.slice(11, 23)}{' '}
                        </span>
                      )}
                      {line.category && (
                        <span
                          className={cn(
                            'font-semibold',
                            CATEGORY_CLASS[line.category] ?? 'text-ink-faint',
                          )}
                        >
                          {line.category.padEnd(8)}
                        </span>
                      )}
                      <span className={LEVEL_CLASS[line.level ?? ''] ?? 'text-ink-faint'}>
                        {line.category ? line.message : line.raw}
                      </span>
                    </div>
                  ))}
                </pre>
              )}
            </Panel>
          </>
        )}
      </div>
    </div>
  )
}
