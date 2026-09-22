import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import {
  Bell,
  BellOff,
  Eye,
  EyeOff,
  Braces,
  ChevronDown,
  ChevronRight,
  FlaskConical,
  Pencil,
  Plus,
  Radar,
  Square,
  Trash2,
  BellRing,
  FilePlus2,
  Gauge,
  RefreshCw,
  X,
  Bug,
  CheckCircle2,
  ShieldCheck,
  AlertTriangle,
  HelpCircle,
  XCircle,
} from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { CompiledWatch, Environment, FiledTicket, IpcError, Issue, NotifierOutcome, HitGrade, Watch, WatchCondition, WatchHit, WatchMark, WatchProbe, WatchStatus } from '@/lib/types'
import { KIND_LABELS, SEVERITY_TONE, describeSeverity } from '@/lib/issues'
import { FileTicketDialog, FiledChip } from '@/features/jira/file-ticket-dialog'
import { formatAge, formatDateTime, formatMoment, formatTime, stringify } from '@/lib/format'
import { TimeZoneToggle } from '@/components/time-zone-toggle'
import {
  Badge,
  Button,
  Checkbox,
  CopyButton,
  EmptyState,
  ErrorBox,
  Marked,
  Field,
  Input,
  Note,
  Select,
  Spinner,
  Textarea,
  Toolbar,
  cn,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'
import { upsertStatus, useNow, useWatchStatuses, watchKeys } from './use-watch-events'
import { OriginPanel } from '@/features/origin/origin-panel'

/**
 * The poll-interval ladder: fine steps at the fast end, coarser toward the
 * fifteen-minute ceiling the backend enforces. Index-based, so each step gets
 * equal travel on the slider however uneven the values.
 */
const POLL_STEPS = [5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 300, 600, 900]

function formatInterval(seconds: number): string {
  return seconds < 60 ? `${seconds}s` : `${seconds / 60}m`
}

/** The ladder index whose value is closest to `value`. */
function nearestIndex(steps: number[], value: number): number {
  let best = 0
  for (let i = 1; i < steps.length; i++) {
    if (Math.abs(steps[i] - value) < Math.abs(steps[best] - value)) best = i
  }
  return best
}

/**
 * Colours a watch can claim for its hits. Chosen to stay distinct from each
 * other and from the status tones (amber accent, green ok, red danger, blue
 * info) on the dark surface.
 */
const WATCH_PALETTE = [
  '#60a5fa', // blue
  '#34d399', // green
  '#f472b6', // pink
  '#a78bfa', // violet
  '#fb923c', // orange
  '#22d3ee', // cyan
  '#facc15', // yellow
  '#f87171', // red
  '#a3e635', // lime
  '#e879f9', // fuchsia
]

/** A small stable hash, so a watch keeps its auto colour when the list reorders. */
function hashId(id: string): number {
  let h = 2166136261
  for (let i = 0; i < id.length; i++) h = Math.imul(h ^ id.charCodeAt(i), 16777619)
  return Math.abs(h)
}

/**
 * The colour a watch's hits are drawn in.
 *
 * Unset picks a palette entry from the watch's id, so every watch is
 * distinct without anyone choosing and keeps its colour when the list
 * changes; `'default'` is the plain badge; anything else is used as given.
 * `index` breaks ties when two auto watches hash to the same entry.
 */
function watchColor(watch: Watch, index: number, taken: Set<string>): string | null {
  if (watch.color === 'default') return null
  if (!watch.color) {
    let pick = hashId(watch.id) % WATCH_PALETTE.length
    for (let tries = 0; tries < WATCH_PALETTE.length && taken.has(WATCH_PALETTE[pick]); tries++) {
      pick = (pick + 1 + index) % WATCH_PALETTE.length
    }
    return WATCH_PALETTE[pick]
  }
  return watch.color
}

/** Badge styling in a watch's colour: tinted background, full-strength text. */
function tint(color: string): React.CSSProperties {
  return { backgroundColor: `color-mix(in oklab, ${color} 18%, transparent)`, color }
}

/** Add or remove `id` in a set — the one shape every toggle here takes. */
function toggled(prev: Set<string>, id: string): Set<string> {
  const next = new Set(prev)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  return next
}

function blankWatch(envId: string): Watch {
  return {
    id: '',
    envId,
    label: '',
    enabled: true,
    logGroup: null,
    source: null,
    detailType: null,
    conditions: [],
    rawPattern: null,
    notify: true,
    color: null,
  }
}

/** A one-line description of what a watch matches, for the list. */
function summarize(watch: Watch): string {
  if (watch.rawPattern?.trim()) return watch.rawPattern.trim()
  const parts: string[] = []
  if (watch.source?.trim()) parts.push(watch.source.trim())
  if (watch.detailType?.trim()) parts.push(watch.detailType.trim())
  for (const c of watch.conditions) {
    if (!c.path.trim()) continue
    parts.push(`${c.path.trim()} ${c.op === 'eq' ? '=' : '≠'} ${c.value.trim()}`)
  }
  return parts.length ? parts.join(' · ') : 'every event'
}

export function WatchPage() {
  const { envId, activeEnvironment, timeZone } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)
  const queryClient = useQueryClient()

  const watches = useQuery({
    queryKey: watchKeys.list(envId),
    queryFn: () => ipc.listWatches(envId),
    enabled: !!envId,
    retry: false,
  })
  const statuses = useWatchStatuses()
  const status = statuses.data?.find((s) => s.envId === envId)
  const hits = useQuery({
    queryKey: watchKeys.hits(envId),
    queryFn: () => ipc.listWatchHits(envId),
    enabled: !!envId,
    retry: false,
  })
  /**
   * Watches whose hits are hidden from the list. A viewing choice, not a
   * change to the watch: it keeps polling and notifying, the rows just stay
   * out of the way while you look at something else.
   */
  const [hidden, setHidden] = useState<Set<string>>(new Set())
  const toggleHidden = (id: string) => setHidden((prev) => toggled(prev, id))
  const marks = useQuery({
    queryKey: watchKeys.marks(envId),
    queryFn: () => ipc.listWatchMarks(envId),
    enabled: !!envId,
    retry: false,
  })

  /**
   * Hits and session marks interleaved by time, newest first, so each
   * watching session reads as a group: a start rule, with the hits it caught
   * above it pushing it down as they arrive. Look-back hits fall naturally
   * below the start that found them.
   *
   * Only starts are drawn — a stop mark just tells its start when the
   * session ended. Counts follow the eye toggles: a session's number is what
   * you can see of it, and a session with nothing to show drops out, unless
   * it is the one running now.
   */
  // Hits whose payload the schema rejects or disagrees with, or that have no
  // schema at all — what you are usually watching for.
  const [problemsOnly, setProblemsOnly] = useState(false)
  const problemCount = useMemo(
    () => (hits.data ?? []).filter((h) => !hidden.has(h.watchId) && isProblem(h)).length,
    [hits.data, hidden],
  )

  const timeline = useMemo<TimelineRow[]>(() => {
    const visible = (hits.data ?? []).filter(
      (h) => !hidden.has(h.watchId) && (!problemsOnly || isProblem(h)),
    )
    const items: Array<{ at: number; order: number; row: TimelineRow }> = visible.map((hit) => ({
      at: hit.timestamp,
      order: 1,
      row: { kind: 'hit', hit },
    }))
    // Ascending, so a session's hits are one slice found by two searches.
    const hitTimes = visible.map((h) => h.timestamp).sort((a, b) => a - b)
    const lowerBound = (t: number) => {
      let lo = 0
      let hi = hitTimes.length
      while (lo < hi) {
        const mid = (lo + hi) >> 1
        if (hitTimes[mid] < t) lo = mid + 1
        else hi = mid
      }
      return lo
    }
    const liveSince = status?.armed ? (status.watchingSince ?? null) : null
    const sorted = [...(marks.data ?? [])].sort((a, b) => a.at - b.at)
    sorted.forEach((mark, i) => {
      if (mark.kind !== 'start') return
      // The session runs to the next mark of any kind — its stop, or the
      // next start when the app was relaunched without one — or to now.
      const next = sorted[i + 1]
      const to = next?.at ?? Number.POSITIVE_INFINITY
      const watched = next?.kind === 'stop' ? next.at - mark.at : null
      const caught = lowerBound(to + 1) - lowerBound(mark.at)
      const live = liveSince === mark.at
      if (caught === 0 && !live) return
      // A start sits below the hits at the same instant.
      items.push({ at: mark.at, order: 0, row: { kind: 'mark', mark, watched, caught, live } })
    })
    items.sort((a, b) => b.at - a.at || b.order - a.order)
    return items.map((i) => i.row)
  }, [hits.data, marks.data, hidden, status?.armed, status?.watchingSince, problemsOnly])

  const arm = useMutation<WatchStatus, IpcError, boolean>({
    mutationFn: (enabled) => ipc.setWatching(enabled, envId),
    onSuccess: (next) => upsertStatus(queryClient, next),
  })
  /** One save for every path that edits a watch: list toggles and the editor. */
  const saveWatch = useMutation<Watch, IpcError, Watch>({
    mutationFn: ipc.saveWatch,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: watchKeys.list(envId) }),
  })
  const setPoll = useMutation<number, IpcError, number>({
    mutationFn: ipc.setWatchPollSeconds,
    onSuccess: () => statuses.refetch(),
  })
  const setIdle = useMutation<number, IpcError, number>({
    mutationFn: ipc.setWatchIdleTimeout,
    onSuccess: () => statuses.refetch(),
  })
  const clear = useMutation<void, IpcError, string | undefined>({
    mutationFn: (watchId) => ipc.clearWatchHits(envId, watchId),
    onSuccess: (_, watchId) => {
      queryClient.setQueryData<WatchHit[]>(watchKeys.hits(envId), (old) =>
        watchId ? (old ?? []).filter((h) => h.watchId !== watchId) : [],
      )
      // Marks whose session no longer holds any hit are pruned backend-side.
      void queryClient.invalidateQueries({ queryKey: watchKeys.marks(envId) })
    },
  })
  const pollNow = useMutation<void, IpcError>({ mutationFn: () => ipc.pollNow(envId) })

  /**
   * Which event types have a registered schema. Shares the Schemas screen's
   * cache key, so the list is fetched once for both. A hit whose type has no
   * schema gets "draft one from traffic" rather than a broken link — the same
   * choice the Health report makes for undocumented types.
   */
  const schemas = useQuery({
    queryKey: ['schemas', envId, 'list'],
    queryFn: () => ipc.listSchemas(envId),
    enabled: !!envId,
    retry: false,
  })
  const schemaByIdentity = useMemo(() => {
    const map = new Map<string, string>()
    for (const schema of schemas.data ?? []) {
      map.set(schema.name, schema.name)
      if (schema.source && schema.detailType) {
        map.set(`${schema.source}@${schema.detailType}`, schema.name)
      }
    }
    return map
  }, [schemas.data])

  /** Hit colour per watch id, from each watch's own choice or its list position. */
  const colors = useMemo(() => {
    const map = new Map<string, string | null>()
    const taken = new Set<string>()
    watches.data?.forEach((w, i) => {
      const color = watchColor(w, i, taken)
      if (color) taken.add(color)
      map.set(w.id, color)
    })
    return map
  }, [watches.data])
  const markSeen = useMutation<void, IpcError>({
    mutationFn: () => ipc.markWatchHitsSeen(envId),
  })

  /**
   * Where "new" starts for this visit. Captured once when the page opens so the
   * highlight on unread rows does not vanish the instant they are marked seen.
   */
  const openedAt = useRef<number | null>(null)
  if (openedAt.current === null && status) openedAt.current = status.seenAt

  // Everything on screen counts as read — both what was here on arrival and
  // what lands while the page stays open.
  const unread = status?.unread ?? 0
  useEffect(() => {
    if (unread > 0 && envId) markSeen.mutate()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [unread, envId])

  const [editing, setEditing] = useState<Watch | null>(null)

  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const toggle = (id: string) => setExpanded((prev) => toggled(prev, id))

  if (!envId || !activeEnvironment) {
    return <EmptyState title="Pick an environment to watch" />
  }

  const armed = status?.armed ?? false
  const enabledWatches = watches.data?.filter((w) => w.enabled).length ?? 0

  return (
    <div className="flex h-full flex-col">
      <Toolbar>
        {/*
          The label names the action, not the state — "Watching" as a button
          read as a status badge, and stopping felt like clicking a light.
          State lives in the status line beside it.
        */}
        {armed ? (
          <Button
            variant="secondary"
            onClick={() => arm.mutate(false)}
            loading={arm.isPending}
            title="Stop polling this environment. Watches and hits are kept."
          >
            <Square className="size-3 fill-current" />
            Stop watching
          </Button>
        ) : (
          <Button
            variant="primary"
            onClick={() => arm.mutate(true)}
            loading={arm.isPending}
            title="Start polling this environment. Stays on across restarts until you stop it."
          >
            <Radar className="size-3.5" />
            Start watching
          </Button>
        )}

        <StatusLine
          status={status}
          enabledWatches={enabledWatches}
          onPollNow={() => pollNow.mutate()}
          polling={pollNow.isPending}
        />

        <div className="ml-auto flex items-center gap-2">
          <PollIntervalSlider
            value={status?.pollSeconds ?? 15}
            onChange={(seconds) => setPoll.mutate(seconds)}
          />
          <label
            className="flex items-center gap-1.5 text-[11px] text-ink-muted"
            title="With nobody touching the app for this long, Pontifex asks whether to keep watching, and stops if nobody answers within five minutes. Hits arriving do not count as activity."
          >
            ask after
            <Select
              value={status?.idleTimeoutMinutes ?? 240}
              onChange={(e) => setIdle.mutate(Number(e.target.value))}
              className="h-6 text-[11px]"
            >
              <option value={30}>30m idle</option>
              <option value={60}>1h idle</option>
              <option value={120}>2h idle</option>
              <option value={240}>4h idle</option>
              <option value={480}>8h idle</option>
              <option value={0}>never</option>
            </Select>
          </label>
          <TimeZoneToggle />
          <NotifierControls />
        </div>
      </Toolbar>

      {arm.isError && (
        <div className="p-3">
          <ErrorBox error={arm.error} {...credentials} />
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <aside className="flex w-[400px] shrink-0 flex-col border-r border-edge bg-surface-1">
          {editing ? (
            <WatchEditor
              key={editing.id || 'new'}
              draft={editing}
              logGroups={activeEnvironment.logGroups}
              envId={envId}
              hitCount={hits.data?.filter((h) => h.watchId === editing.id).length ?? 0}
              onClearHits={() => clear.mutate(editing.id)}
              onSave={(watch) => saveWatch.mutateAsync(watch)}
              onClose={() => setEditing(null)}
            />
          ) : (
            <WatchList
              watches={watches.data ?? []}
              hits={hits.data ?? []}
              status={status}
              colors={colors}
              onClear={(watchId) => clear.mutate(watchId)}
              onSave={(watch) => saveWatch.mutate(watch)}
              hidden={hidden}
              onToggleHidden={toggleHidden}
              isLoading={watches.isLoading}
              error={watches.error ? ipc.asIpcError(watches.error) : null}
              onNew={() => setEditing(blankWatch(envId))}
              onEdit={setEditing}
            />
          )}
        </aside>

        <section className="min-w-0 flex-1 overflow-auto">
          {hits.isError && (
            <div className="p-3">
              <ErrorBox error={ipc.asIpcError(hits.error)} {...credentials} />
            </div>
          )}
          {status?.paused && (
            <div className="p-3">
              <ErrorBox
                error={{ kind: 'auth', message: status.lastError ?? status.paused }}
                {...credentials}
              />
            </div>
          )}
          {status?.lastError && !status.paused && (
            <div className="p-3">
              <Note tone="warn">
                Last poll failed and will be retried: {status.lastError}
              </Note>
            </div>
          )}
          {hits.isLoading && <Spinner label="Loading hits…" />}
          {hits.data && hits.data.length === 0 && (
            <EmptyState
              icon={<Radar className="size-8" />}
              title={
                !armed
                  ? 'Not watching'
                  : enabledWatches === 0
                    ? 'No watches to match'
                    : status?.polls
                      ? 'Nothing in the last hour'
                      : 'Looking back over the last hour…'
              }
              detail={
                !armed
                  ? 'Start watching to poll this environment for events that match your watches. Polling reads only the slice of the log group since the last pass, so it can run all day.'
                  : enabledWatches === 0
                    ? 'Add a watch on the left. Until one is enabled the poller idles.'
                    : status?.polls
                      ? `Nothing matched in the last hour, so these watches are quiet rather than broken. Polling continues every ${status?.pollSeconds ?? 15}s; use Probe in the editor to see how often a watch fires over a day.`
                      : 'The first pass fills in the last hour so you can see what these watches would have caught.'
              }
            />
          )}
          {hits.data && hits.data.length > 0 && (
            <div className="flex items-center gap-3 border-b border-edge bg-surface-1 px-3 py-1.5 text-[10px] text-ink-faint">
              <Checkbox
                checked={problemsOnly}
                onChange={(e) => setProblemsOnly(e.target.checked)}
                label={`problems only${problemCount > 0 ? ` (${problemCount})` : ''}`}
                title="Show only hits the schema rejects, disagrees with, or has no schema for"
              />
              {problemsOnly && problemCount === 0 && <span>nothing is broken</span>}
            </div>
          )}
          {hits.data && hits.data.length > 0 && (
            <table className="w-full border-collapse text-[11px]">
              <thead className="sticky top-0 bg-surface-1">
                <tr className="border-b border-edge text-left text-ink-faint">
                  <th className="w-6" />
                  <th className="whitespace-nowrap px-2 py-1 font-medium">
                    Time <span className="font-normal text-ink-faint">{timeZone === 'utc' ? 'UTC' : 'local'}</span>
                  </th>
                  <th className="w-6" title="The payload against its schema, graded as the hit was caught" />
                  <th className="whitespace-nowrap px-2 py-1 font-medium">Watch</th>
                  <th className="whitespace-nowrap px-2 py-1 font-medium">Source</th>
                  {/* The slack column: shows the whole name whenever there is
                      room, and is the one that truncates when there is not. */}
                  <th className="w-full px-2 py-1 font-medium">Detail type</th>
                  <th className="w-20" />
                </tr>
              </thead>
              <tbody>
                {timeline.map((row, index) =>
                  row.kind === 'mark' ? (
                    <MarkRow
                      key={row.mark.id}
                      mark={row.mark}
                      watched={row.watched}
                      caught={row.caught}
                      live={row.live}
                      // Nearer-the-top markers paint over the ones below when
                      // several are stuck to the bottom edge at once, so the
                      // one you would reach next is the one you see.
                      stack={1000 - index}
                      />
                  ) : (
                    <HitRow
                      key={row.hit.id}
                      hit={row.hit}
                      color={colors.get(row.hit.watchId) ?? null}
                      schemaName={
                        row.hit.source && row.hit.detailType
                          ? (schemaByIdentity.get(`${row.hit.source}@${row.hit.detailType}`) ?? null)
                          : null
                      }
                      schemasKnown={schemas.isSuccess}
                      environment={activeEnvironment}
                      isNew={row.hit.receivedAt > (openedAt.current ?? 0)}
                      expanded={expanded.has(row.hit.id)}
                      onToggle={() => toggle(row.hit.id)}
                    />
                  ),
                )}
              </tbody>
            </table>
          )}
        </section>
      </div>
    </div>
  )
}

/** What the poller is doing right now, in one line. */
function StatusLine({
  status,
  enabledWatches,
  onPollNow,
  polling,
}: {
  status: WatchStatus | undefined
  enabledWatches: number
  onPollNow: () => void
  polling: boolean
}) {
  // "last poll 12s ago" has to keep moving.
  const now = useNow(1000)

  if (!status?.armed) {
    return (
      <span className="text-[11px] text-ink-faint">
        {enabledWatches} enabled watch{enabledWatches === 1 ? '' : 'es'}
      </span>
    )
  }

  const nextIn = status.nextPollAt && status.nextPollAt > now ? status.nextPollAt - now : 0
  const counts = `${status.polls} poll${status.polls === 1 ? '' : 's'} · ${status.hits} hit${status.hits === 1 ? '' : 's'}`
  const countsTitle = [
    `${status.fetchedTotal} event${status.fetchedTotal === 1 ? '' : 's'} fetched in total`,
    `${status.lastFetched} on the last poll`,
    `${status.callsPerPass} CloudWatch call${status.callsPerPass === 1 ? '' : 's'} per poll`,
    `last poll took ${status.lastPassMs} ms`,
  ].join('\n')

  return (
    <span className="flex items-center gap-2 text-[11px] text-ink-faint">
      {status.paused ? (
        <Badge tone="warn">paused · {status.paused}</Badge>
      ) : status.lastError ? (
        <Badge tone="danger" title={status.lastError}>
          retrying
        </Badge>
      ) : status.running ? (
        <Badge tone="ok">
          <Radar className="size-3 animate-pulse" />
          watching
        </Badge>
      ) : (
        <Badge tone="neutral">starting</Badge>
      )}
      {status.lastPollAt && <span>last poll {formatAge(now - status.lastPollAt)} ago</span>}
      {/*
        The countdown is also the "poll now" control: hovering swaps the label,
        clicking cuts the wait short and the interval restarts from that poll.
      */}
      <button
        type="button"
        onClick={onPollNow}
        disabled={polling}
        title="Poll now, then restart the interval"
        className={cn(
          'group inline-flex h-5 items-center gap-1 rounded px-1.5 font-medium transition-colors',
          'bg-orange/15 text-orange hover:bg-orange hover:text-surface-0 disabled:opacity-60',
        )}
      >
        <RefreshCw className={cn('size-3', polling && 'animate-spin')} />
        <span className="group-hover:hidden">
          {polling ? 'polling…' : nextIn > 0 ? `next in ${formatAge(nextIn)}` : 'polling…'}
        </span>
        <span className="hidden group-hover:inline">Poll now</span>
      </button>
      <span title={countsTitle} className="cursor-help decoration-dotted underline-offset-2 hover:underline">
        {counts}
      </span>
      {status.note && <span className="text-ink-muted">· {status.note}</span>}
    </span>
  )
}

/**
 * How hard the poller is working for this environment, Sumo-style.
 *
 * Effort is not one number. What costs money is events fetched that never
 * become hits (scanned and thrown away), and what costs time is calls per
 * poll and how long each pass takes. An unfiltered watch is the worst of
 * both: every event on the group, downloaded, every poll.
 */
function effortLevel(status: WatchStatus | undefined): {
  level: 0 | 1 | 2 | 3
  label: string
  detail: string
} {
  if (!status?.armed || !status.polls) {
    return { level: 0, label: 'idle', detail: 'Not polling.' }
  }
  const wasted = status.fetchedTotal > 0 ? 1 - status.hits / status.fetchedTotal : 0
  const heavyFetch = status.lastFetched >= 200
  let level: 1 | 2 | 3 = 1
  if (status.unfiltered > 0 || status.callsPerPass > 4 || status.lastPassMs > 4000 || (heavyFetch && wasted > 0.9)) {
    level = 3
  } else if (status.callsPerPass > 2 || status.lastPassMs > 1500 || heavyFetch) {
    level = 2
  }
  const labels = { 1: 'light', 2: 'moderate', 3: 'heavy' } as const
  const detail = [
    `${status.callsPerPass} CloudWatch call${status.callsPerPass === 1 ? '' : 's'} per poll, every ${status.pollSeconds}s`,
    `last poll took ${status.lastPassMs} ms and fetched ${status.lastFetched} event${status.lastFetched === 1 ? '' : 's'}`,
    `${status.fetchedTotal} fetched in total, ${status.hits} kept as hits`,
    status.unfiltered > 0
      ? `${status.unfiltered} watch${status.unfiltered === 1 ? '' : 'es'} match every event — no server-side filter, so the whole group is downloaded each poll`
      : 'every call is filtered server-side',
  ].join('\n')
  return { level, label: labels[level], detail }
}

function EffortGauge({ status }: { status: WatchStatus | undefined }) {
  const { level, label, detail } = effortLevel(status)
  const tone = level === 3 ? 'bg-danger' : level === 2 ? 'bg-warn' : level === 1 ? 'bg-ok' : 'bg-ink-faint'
  return (
    <span
      className="flex cursor-help items-center gap-1.5 text-[10px] text-ink-faint"
      title={`Effort: ${label}\n${detail}`}
    >
      <Gauge className="size-3" />
      <span className="flex items-end gap-px">
        {[1, 2, 3].map((bar) => (
          <span
            key={bar}
            className={cn('w-1 rounded-sm', bar <= level ? tone : 'bg-surface-3')}
            style={{ height: `${4 + bar * 3}px` }}
          />
        ))}
      </span>
      {label}
    </span>
  )
}

/**
 * The poll interval as a ladder slider, the way Cora's GitHub settings do it.
 *
 * WKWebView paints the native range thumb at a position that does not track
 * the standard geometry, so tick labels cannot be aligned to it. The native
 * thumb is hidden and a thumb and one label per step are drawn from the same
 * formula — thumb left edge at frac·(track − thumb), label centre at that
 * plus half a thumb — so they line up by construction. The filled portion of
 * the track comes from the same fraction.
 */
function PollIntervalSlider({
  value,
  onChange,
}: {
  value: number
  onChange: (seconds: number) => void
}) {
  const index = nearestIndex(POLL_STEPS, value)
  const fracOf = (i: number) => i / (POLL_STEPS.length - 1)
  const style = {
    '--frac': String(fracOf(index)),
    '--fill': `${fracOf(index) * 100}%`,
  } as React.CSSProperties
  // Every step is a tick, but only some get a label or the scale is a blur.
  const labelled = new Set([0, 4, 6, 8, 10, POLL_STEPS.length - 1])
  return (
    <label
      className="flex items-center gap-2 text-[11px] text-ink-muted"
      title="How often each environment's poller asks CloudWatch for new events. Each pass reads only what arrived since the last one."
    >
      <span className="whitespace-nowrap">
        every <span className="font-mono text-ink">{formatInterval(POLL_STEPS[index])}</span>
      </span>
      <span className="ladder-slider" style={style}>
        <input
          type="range"
          min={0}
          max={POLL_STEPS.length - 1}
          step={1}
          value={index}
          onChange={(e) => onChange(POLL_STEPS[Number(e.target.value)])}
          aria-label="Poll interval"
        />
        <span className="ladder-slider-thumb" aria-hidden="true" />
        <span className="ladder-slider-scale">
          {POLL_STEPS.map((v, i) => (
            <span
              key={v}
              style={{ left: `calc(${fracOf(i)} * (100% - var(--thumb-w)) + var(--thumb-w) / 2)` }}
            >
              {labelled.has(i) ? formatInterval(v) : '·'}
            </span>
          ))}
        </span>
      </span>
    </label>
  )
}

// --- watch list -----------------------------------------------------------

function WatchList({
  watches,
  hits,
  status,
  colors,
  onClear,
  onSave,
  hidden,
  onToggleHidden,
  isLoading,
  error,
  onNew,
  onEdit,
}: {
  watches: Watch[]
  hits: WatchHit[]
  status: WatchStatus | undefined
  colors: Map<string, string | null>
  /** Forget hits: one watch's, or every watch's when no id is given. */
  onClear: (watchId: string | undefined) => void
  onSave: (watch: Watch) => void
  /** Watches whose hits are hidden from the list. */
  hidden: Set<string>
  onToggleHidden: (watchId: string) => void
  isLoading: boolean
  error: IpcError | null
  onNew: () => void
  onEdit: (watch: Watch) => void
}) {

  // One clock for every row's "last 3m ago", rather than a timer per row.
  const now = useNow(30_000)
  // Per watch: how many hits are on screen and when the newest landed. This is
  // what says "quiet" versus "never fires" without opening anything.
  const activity = useMemo(() => {
    const map = new Map<string, { count: number; latest: number }>()
    for (const hit of hits) {
      const entry = map.get(hit.watchId) ?? { count: 0, latest: 0 }
      entry.count += 1
      entry.latest = Math.max(entry.latest, hit.timestamp)
      map.set(hit.watchId, entry)
    }
    return map
  }, [hits])

  return (
    <>
      <header className="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-edge px-3">
        <h2 className="text-xs font-semibold text-ink">Watches</h2>
        <EffortGauge status={status} />
        <span className="flex items-center gap-1">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => onClear(undefined)}
            disabled={hits.length === 0}
            title={`Forget every recorded hit (${hits.length})`}
          >
            <Trash2 className="size-3" />
            Clear all
          </Button>
          <Button variant="ghost" size="sm" onClick={onNew}>
            <Plus className="size-3" />
            New
          </Button>
        </span>
      </header>
      <div className="min-h-0 flex-1 overflow-auto">
        {isLoading && <Spinner />}
        {error && (
          <div className="p-2">
            <ErrorBox error={error} />
          </div>
        )}
        {!isLoading && watches.length === 0 && (
          <EmptyState
            title="No watches yet"
            detail="A watch names a source, a detail type, a field in the payload — or any mix — and the poller says when a matching event goes past."
            action={
              <Button variant="primary" onClick={onNew}>
                <Plus className="size-3.5" />
                New watch
              </Button>
            }
          />
        )}
        {watches.map((watch) => (
          <div
            key={watch.id}
            className={cn(
              'group flex items-start gap-2 border-b border-edge/40 px-3 py-2 hover:bg-surface-2',
              !watch.enabled && 'opacity-60',
              hidden.has(watch.id) && 'bg-surface-0/40',
            )}
          >
            <span className="flex shrink-0 items-center gap-1">
              <Checkbox
                label=""
                checked={watch.enabled}
                onChange={(e) => onSave({ ...watch, enabled: e.target.checked })}
                title={watch.enabled ? 'Enabled — disable to keep it without polling for it' : 'Disabled'}
                className="mt-0.5"
              />
              <button
                type="button"
                onClick={() => onToggleHidden(watch.id)}
                title={
                  hidden.has(watch.id)
                    ? "Hidden from the list — click to show this watch's hits again"
                    : "Shown — click to hide this watch's hits from the list. It keeps polling and notifying."
                }
                className={cn(
                  'mt-0.5 rounded p-0.5 hover:bg-surface-3',
                  hidden.has(watch.id) ? 'text-ink-faint' : 'text-ink-muted',
                )}
              >
                {hidden.has(watch.id) ? <EyeOff className="size-3.5" /> : <Eye className="size-3.5" />}
              </button>
            </span>
            <button
              type="button"
              onClick={() => onEdit(watch)}
              className="min-w-0 flex-1 text-left"
            >
              <div className="flex items-center gap-1.5">
                {colors.get(watch.id) && (
                  <span
                    className="size-2 shrink-0 rounded-full"
                    style={{ backgroundColor: colors.get(watch.id) ?? undefined }}
                  />
                )}
                <span className="truncate text-xs text-ink">{watch.label}</span>
                {watch.notify ? (
                  <Bell className="size-3 shrink-0 text-ink-faint" aria-label="notifies" />
                ) : (
                  <BellOff className="size-3 shrink-0 text-ink-faint" aria-label="silent" />
                )}
              </div>
              <div className="truncate font-mono text-[10px] text-ink-faint" title={summarize(watch)}>
                {summarize(watch)}
              </div>
              {watch.logGroup && (
                <div className="truncate font-mono text-[10px] text-ink-faint/70">{watch.logGroup}</div>
              )}
              <WatchActivity activity={activity.get(watch.id)} now={now} />
            </button>
            <span className="flex shrink-0 items-center opacity-0 group-hover:opacity-100">
              <Button
                variant="ghost"
                size="sm"
                onClick={() => onClear(watch.id)}
                disabled={!activity.get(watch.id)}
                title={
                  activity.get(watch.id)
                    ? `Forget this watch's ${activity.get(watch.id)?.count} hit${activity.get(watch.id)?.count === 1 ? '' : 's'}`
                    : 'No hits to clear'
                }
              >
                <Trash2 className="size-3" />
              </Button>
              <Button variant="ghost" size="sm" onClick={() => onEdit(watch)} title="Edit">
                <Pencil className="size-3" />
              </Button>
            </span>
          </div>
        ))}
      </div>
    </>
  )
}

/** "12 hits · last 3m ago", or that there have been none. */
function WatchActivity({
  activity,
  now,
}: {
  activity: { count: number; latest: number } | undefined
  now: number
}) {
  if (!activity) {
    return <div className="mt-0.5 text-[10px] text-ink-faint">no hits yet</div>
  }
  return (
    <div className="mt-0.5 text-[10px] text-ink-muted">
      {activity.count} hit{activity.count === 1 ? '' : 's'} · last{' '}
      {formatAge(now - activity.latest)} ago
    </div>
  )
}

/**
 * Fire a notification on demand and say what became of it.
 *
 * A real hit should not be the first test of whether this machine shows them
 * — by then the thing you were waiting for has already gone past. The helper
 * reports back, so "sent" here means macOS accepted it, and a refusal names
 * the setting to change.
 */
function NotifierControls() {
  // One mutation for both actions, so `.data` is simply the latest verdict.
  const test = useMutation<NotifierOutcome, IpcError, 'test' | 'askAgain'>({
    mutationFn: (action) =>
      action === 'test' ? ipc.testNotification() : ipc.askNotificationPermissionAgain(),
  })
  const outcome = test.data
  const tone =
    outcome?.status === 'delivered'
      ? 'text-ok'
      : outcome?.status === 'pending'
        ? 'text-warn'
        : outcome
          ? 'text-danger'
          : ''
  const label = !outcome
    ? 'Test notification'
    : outcome.status === 'delivered'
      ? 'Delivered'
      : outcome.status === 'pending'
        ? 'Waiting for permission…'
        : outcome.status === 'denied'
          ? 'Turned off in System Settings'
          : 'Failed'
  return (
    <span className="flex items-center gap-1">
    <NotificationSettingsButton outcome={outcome} onAskAgain={() => test.mutate('askAgain')} />
    <Button
      variant="ghost"
      size="sm"
      onClick={() => test.mutate('test')}
      loading={test.isPending}
      title={
        test.isError
          ? test.error.message
          : outcome
            ? `${outcome.message}\nHelper: ${outcome.helper}`
            : 'Send a test notification. The first time, macOS asks whether Pontifex may notify you — click Allow.'
      }
    >
      <BellRing className={cn('size-3', test.isError ? 'text-danger' : tone)} />
      {test.isError ? 'Failed' : label}
    </Button>
    </span>
  )
}

/**
 * Shown beside the test button once macOS has said no.
 *
 * The proper fix is System Settings. But macOS withdraws an unanswered prompt
 * after a few minutes and records a refusal, and does not always list the
 * refused app — so there is also "ask again", which reinstalls the helper
 * under a fresh identity and raises the prompt afresh.
 */
function NotificationSettingsButton({
  outcome,
  onAskAgain,
}: {
  outcome: NotifierOutcome | undefined
  onAskAgain: () => void
}) {
  if (outcome?.status !== 'denied' && outcome?.status !== 'error') return null
  return (
    <>
      <Button
        variant="secondary"
        size="sm"
        onClick={() => void ipc.openNotificationSettings()}
        title="Open System Settings → Notifications. Find Pontifex and allow it, then test again."
      >
        Notification Settings
      </Button>
      <Button
        variant="secondary"
        size="sm"
        onClick={onAskAgain}
        title="If Pontifex is not listed in System Settings, ask macOS again under a fresh identity. Answer the prompt within a couple of minutes."
      >
        Ask again
      </Button>
    </>
  )
}

// --- editor ---------------------------------------------------------------

function WatchEditor({
  draft: initial,
  logGroups,
  envId,
  hitCount,
  onClearHits,
  onSave,
  onClose,
}: {
  draft: Watch
  logGroups: string[]
  envId: string
  hitCount: number
  onClearHits: () => void
  onSave: (watch: Watch) => Promise<Watch>
  onClose: () => void
}) {
  const queryClient = useQueryClient()
  const [draft, setDraft] = useState<Watch>(initial)
  const [useRaw, setUseRaw] = useState(!!initial.rawPattern?.trim())
  const isNew = !initial.id

  const update = (changes: Partial<Watch>) => setDraft((d) => ({ ...d, ...changes }))
  const setCondition = (index: number, changes: Partial<WatchCondition>) =>
    setDraft((d) => ({
      ...d,
      conditions: d.conditions.map((c, i) => (i === index ? { ...c, ...changes } : c)),
    }))

  // What is actually sent depends on the mode; the other half is dropped so a
  // stale raw pattern cannot silently override the fields you can see.
  const effective = useMemo<Watch>(
    () => (useRaw ? { ...draft, source: null, detailType: null, conditions: [] } : { ...draft, rawPattern: null }),
    [draft, useRaw],
  )

  // Keyed on the fields that shape the pattern, so typing a label does not
  // round-trip to the backend on every keystroke.
  const { label: _label, notify: _notify, color: _color, ...shape } = effective
  const compiled = useQuery<CompiledWatch, IpcError>({
    queryKey: ['watch', 'compile', JSON.stringify(shape)],
    queryFn: () => ipc.compileWatchPattern(effective),
    retry: false,
    staleTime: Infinity,
  })

  const probe = useMutation<WatchProbe, IpcError, number>({
    mutationFn: (hours) => ipc.probeWatch(effective, hours),
  })

  const save = useMutation<Watch, IpcError, Watch>({
    mutationFn: onSave,
    onSuccess: onClose,
  })
  const remove = useMutation<void, IpcError, string>({
    mutationFn: ipc.deleteWatch,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: watchKeys.list(envId) })
      queryClient.invalidateQueries({ queryKey: watchKeys.hits(envId) })
      onClose()
    },
  })

  const matchesEverything = compiled.isSuccess && compiled.data.pattern === null

  return (
    <>
      <header className="flex h-9 shrink-0 items-center justify-between gap-2 border-b border-edge px-3">
        <h2 className="text-xs font-semibold text-ink">{isNew ? 'New watch' : 'Edit watch'}</h2>
        {!isNew && (
          // Takes effect immediately, independent of Save: pausing a watch is
          // a decision about now, not about the draft.
          <Checkbox
            checked={draft.enabled}
            onChange={(e) => {
              const enabled = e.target.checked
              update({ enabled })
              void onSave({ ...initial, enabled })
            }}
            label={draft.enabled ? 'Enabled' : 'Paused'}
            className="ml-auto"
            title="Pause or resume this watch without closing the editor"
          />
        )}
        <Button variant="ghost" size="sm" onClick={onClose} title="Close without saving">
          <X className="size-3" />
        </Button>
      </header>

      <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3">
        <Field label="Label" hint="Shown in the hit list and as the notification title. Left blank, it is derived from the filters.">
          <Input
            value={draft.label}
            onChange={(e) => update({ label: e.target.value })}
            placeholder={compiled.data?.summary ?? summarize(effective)}
          />
        </Field>

        <Field label="Log group">
          <Select
            value={draft.logGroup ?? ''}
            onChange={(e) => update({ logGroup: e.target.value || null })}
          >
            <option value="">{defaultGroup(logGroups)} (default)</option>
            {logGroups
              .filter((g) => g !== defaultGroup(logGroups))
              .map((g) => (
                <option key={g} value={g}>
                  {g}
                </option>
              ))}
          </Select>
        </Field>

        <Checkbox
          checked={useRaw}
          onChange={(e) => setUseRaw(e.target.checked)}
          label="Write the CloudWatch filter pattern myself"
        />

        {useRaw ? (
          <Field
            label="Filter pattern"
            hint="A complete CloudWatch Logs filter pattern. Polled in its own call, so it cannot be combined with other watches on the same group."
          >
            <Textarea
              rows={3}
              value={draft.rawPattern ?? ''}
              onChange={(e) => update({ rawPattern: e.target.value })}
              placeholder={'{ $.source = "orders*" && $.detail.clientId = "abc-123" }'}
              className="font-mono"
              spellCheck={false}
            />
          </Field>
        ) : (
          <>
            <div className="grid grid-cols-2 gap-2">
              <Field label="Source">
                <Input
                  value={draft.source ?? ''}
                  onChange={(e) => update({ source: e.target.value || null })}
                  placeholder="orders* or exact"
                  className="font-mono"
                  spellCheck={false}
                />
              </Field>
              <Field label="Detail type">
                <Input
                  value={draft.detailType ?? ''}
                  onChange={(e) => update({ detailType: e.target.value || null })}
                  placeholder="*assigned"
                  className="font-mono"
                  spellCheck={false}
                />
              </Field>
            </div>

            <div className="flex flex-col gap-1">
              <div className="flex items-center justify-between">
                <span className="text-[11px] font-medium text-ink-muted">Payload conditions</span>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() =>
                    update({
                      conditions: [...draft.conditions, { path: 'detail.', op: 'eq', value: '' }],
                    })
                  }
                >
                  <Plus className="size-3" />
                  Condition
                </Button>
              </div>
              {draft.conditions.map((condition, index) => (
                <div key={index} className="flex items-center gap-1">
                  <Input
                    value={condition.path}
                    onChange={(e) => setCondition(index, { path: e.target.value })}
                    placeholder="detail.clientId"
                    className="flex-1 font-mono"
                    spellCheck={false}
                  />
                  <Select
                    value={condition.op}
                    onChange={(e) => setCondition(index, { op: e.target.value as 'eq' | 'ne' })}
                    className="w-14"
                  >
                    <option value="eq">=</option>
                    <option value="ne">≠</option>
                  </Select>
                  <Input
                    value={condition.value}
                    onChange={(e) => setCondition(index, { value: e.target.value })}
                    placeholder="abc-123"
                    className="flex-1 font-mono"
                    spellCheck={false}
                  />
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      update({ conditions: draft.conditions.filter((_, i) => i !== index) })
                    }
                    title="Remove condition"
                  >
                    <X className="size-3" />
                  </Button>
                </div>
              ))}
              <span className="text-[10px] text-ink-faint">
                Paths start at the envelope: <span className="font-mono">detail.clientId</span>,{' '}
                <span className="font-mono">detail.items[0].id</span>. Unquoted numbers compare
                numerically; <span className="font-mono">*</span> is a wildcard in strings;
                wrap a value in quotes to force a string match.
              </span>
            </div>
          </>
        )}

        <Field label="Compiled pattern" hint="What CloudWatch is actually asked. Matching happens server-side, so only events that pass this are ever downloaded.">
          {compiled.isError ? (
            <ErrorBox error={compiled.error} />
          ) : (
            <pre className="whitespace-pre-wrap break-all rounded-md border border-edge bg-surface-0 px-2 py-1.5 font-mono text-[10px] text-ink-muted">
              {compiled.data?.pattern ?? (compiled.isSuccess ? '(no filter — every event)' : '…')}
            </pre>
          )}
        </Field>

        {matchesEverything && (
          <Note tone="warn">
            This watch matches every event on the bus. It works, but on a busy group the hit list
            fills fast and notifications become noise — consider turning notifications off.
          </Note>
        )}

        <Checkbox
          checked={draft.notify}
          onChange={(e) => update({ notify: e.target.checked })}
          label="Desktop notification on a hit"
        />

        <Field label="Colour" hint="How this watch's hits are marked in the list. Auto gives each watch its own colour without choosing.">
          <div className="flex flex-wrap items-center gap-1.5">
            <ColorChoice
              selected={!draft.color}
              onClick={() => update({ color: null })}
              title="Auto: a distinct colour, kept for the life of the watch"
              className="bg-gradient-to-br from-[#60a5fa] via-[#34d399] to-[#f472b6]"
            />
            <ColorChoice
              selected={draft.color === 'default'}
              onClick={() => update({ color: 'default' })}
              title="The plain badge, no colour"
              className="bg-accent/40"
            />
            {WATCH_PALETTE.map((c) => (
              <ColorChoice
                key={c}
                selected={draft.color === c}
                onClick={() => update({ color: c })}
                title={c}
                style={{ backgroundColor: c }}
              />
            ))}
            <input
              type="color"
              value={draft.color && draft.color !== 'default' ? draft.color : '#60a5fa'}
              onChange={(e) => update({ color: e.target.value })}
              title="Any colour"
              className="size-5 cursor-pointer rounded border border-edge bg-transparent p-0"
            />
          </div>
        </Field>

        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2">
            <Button
              variant="secondary"
              size="sm"
              onClick={() => probe.mutate(24)}
              loading={probe.isPending}
              disabled={compiled.isError}
              title="Sample the last 24 hours of the log group with this pattern, spread across the whole day, and count what it would have matched"
            >
              <FlaskConical className="size-3" />
              Probe the last 24h
            </Button>
            {probe.isPending && (
              <span className="text-[10px] text-ink-faint">scanning, up to ~12s…</span>
            )}
          </div>
          {probe.data && <ProbeResult probe={probe.data} />}
          {probe.isError && <ErrorBox error={probe.error} />}
        </div>
        {save.isError && <ErrorBox error={save.error} />}
        {remove.isError && <ErrorBox error={remove.error} />}
      </div>

      <footer className="flex shrink-0 items-center gap-2 border-t border-edge p-3">
        <Button
          variant="primary"
          onClick={() => save.mutate(effective)}
          loading={save.isPending}
          disabled={compiled.isError}
        >
          {isNew ? 'Add watch' : 'Save'}
        </Button>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        {!isNew && (
          <span className="ml-auto flex items-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              onClick={onClearHits}
              disabled={hitCount === 0}
              title="Forget this watch's recorded hits; the watch stays"
            >
              <Trash2 className="size-3" />
              Clear {hitCount} hit{hitCount === 1 ? '' : 's'}
            </Button>
            <Button
              variant="danger"
              size="sm"
              onClick={() => remove.mutate(draft.id)}
              loading={remove.isPending}
              title="Delete this watch and its hits"
            >
              <Trash2 className="size-3" />
              Delete
            </Button>
          </span>
        )}
      </footer>
    </>
  )
}

/** One swatch in the colour picker. */
function ColorChoice({
  selected,
  onClick,
  title,
  className,
  style,
}: {
  selected: boolean
  onClick: () => void
  title: string
  className?: string
  style?: React.CSSProperties
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      style={style}
      className={cn(
        'size-5 rounded-full border-2 transition-transform hover:scale-110',
        selected ? 'border-ink' : 'border-transparent',
        className,
      )}
    />
  )
}

/** What a probe found, as a verdict first and the breakdown second. */
function ProbeResult({ probe }: { probe: WatchProbe }) {
  const now = Date.now()
  if (probe.count === 0) {
    return (
      <Note tone="warn">
        No matches in the last {probe.hours}h. Either this is genuinely rare, or a filter is
        wrong — check the source and detail type spelling against the Logs screen.
      </Note>
    )
  }
  const count = probe.truncated ? `${probe.count}+` : String(probe.count)
  const perHour = probe.count / probe.hours
  const rate =
    perHour >= 1
      ? `~${Math.round(perHour)}/hour`
      : `~${Math.max(1, Math.round(probe.count / (probe.hours / 24)))}/day`
  return (
    <div className="flex flex-col gap-1 rounded-md border border-edge bg-surface-0 px-2 py-1.5 text-[10px]">
      <div className="text-ink">
        <span className="font-medium">{count} matches</span> in the last {probe.hours}h
        {probe.truncated ? ' (sample capped)' : ''} · {rate}
        {probe.lastAt && <> · last {formatAge(now - probe.lastAt)} ago</>}
      </div>
      {probe.byType.slice(0, 6).map(([type, n]) => (
        <div key={type} className="flex justify-between gap-2 font-mono text-ink-muted">
          <span className="truncate">{type}</span>
          <span className="shrink-0">{n}</span>
        </div>
      ))}
      {probe.byType.length > 6 && (
        <div className="text-ink-faint">and {probe.byType.length - 6} more types</div>
      )}
    </div>
  )
}

/** Mirrors the backend's choice: the bus's own events group, else the first. */
function defaultGroup(groups: string[]): string {
  return groups.find((g) => g.includes('global-events')) ?? groups[0] ?? ''
}

// --- hits -----------------------------------------------------------------

type TimelineRow =
  | { kind: 'hit'; hit: WatchHit }
  | { kind: 'mark'; mark: WatchMark; watched: number | null; caught: number; live: boolean }

/**
 * A rule across the table where a watching session started.
 *
 * Sessions read as groups: the hits caught live sit above their start rule
 * and push it down as more arrive. The rule says how long the session ran
 * and what it caught; the running one is green. Hits below a start rule that
 * have no session of their own were found by the look-back and were never
 * notified.
 */
function MarkRow({
  mark,
  watched,
  caught,
  live,
  stack,
}: {
  mark: WatchMark
  watched: number | null
  caught: number
  live: boolean
  stack: number
}) {
  const now = useNow(30_000)
  const { timeZone } = useSettings()
  const when = formatMoment(mark.at, timeZone)
  return (
    <tr>
      {/*
        Sticky to the bottom edge: while the rule's own place in the timeline
        is below the visible area it stays pinned at the bottom, so the
        boundary of the session you are looking at is never out of sight; scroll
        down and it settles into place at the first hit of that session, and
        the next rule below takes over the bottom edge.
      */}
      <td
        colSpan={6}
        className="sticky bottom-0 bg-surface-0 px-2 py-1"
        style={{ zIndex: stack }}
      >
        {/*
          Left-anchored, with a fixed lead-in, so every rule's text starts at
          the same column: scanning down a list of sessions is reading the
          same words at the same place, not hunting for them across centred
          rules of different widths.
        */}
        <div className="flex items-center gap-2 text-[10px] text-ink-muted">
          <span className="h-px w-16 shrink-0 bg-ok/60" />
          <Radar className={cn('size-3 shrink-0 text-ok', live && 'animate-pulse')} />
          <span className="whitespace-nowrap">
            started watching <span className="font-mono">{when}</span>
            {live ? (
              <> · watching for {formatAge(now - mark.at)}</>
            ) : watched != null ? (
              <> · watched for {formatAge(watched)}</>
            ) : null}
            {' · '}
            <span className="text-ink">{caught}</span> hit{caught === 1 ? '' : 's'}
            {live ? ' so far' : ''}
          </span>
          <span className="h-px flex-1 bg-ok/60" />
        </div>
      </td>
    </tr>
  )
}

/** A hit the schema rejects or disagrees with, or that has no schema at all. */
function isProblem(hit: WatchHit): boolean {
  return hit.grade === 'failing' || hit.grade === 'drifting' || hit.grade === 'missing'
}

/** One glyph per grade, in the Health screen's colours; the headline is the tooltip. */
const GRADE_MARKS: Record<
  HitGrade,
  { icon: typeof CheckCircle2; className: string; label: string }
> = {
  ok: { icon: CheckCircle2, className: 'text-ok/70', label: 'Matches the schema' },
  failing: { icon: XCircle, className: 'text-danger', label: 'Rejected by the schema' },
  drifting: {
    icon: AlertTriangle,
    className: 'text-warn',
    label: 'Validates, but drifts from the schema',
  },
  missing: {
    icon: HelpCircle,
    className: 'text-danger',
    label: 'No schema is registered for this event type',
  },
  unknown: { icon: HelpCircle, className: 'text-ink-faint', label: 'Could not be graded' },
}

function GradeMark({ grade, headline }: { grade: HitGrade | null; headline: string | null }) {
  // Hits stored before grading existed have nothing to say.
  if (!grade) return <span className="block size-3" />
  const { icon: Icon, className, label } = GRADE_MARKS[grade]
  return (
    <span className="block size-3" title={headline ? `${label}: ${headline}` : label}>
      <Icon className={cn('size-3', className)} aria-label={label} />
    </span>
  )
}

function HitRow({
  hit,
  color,
  schemaName,
  schemasKnown,
  environment,
  isNew,
  expanded,
  onToggle,
}: {
  hit: WatchHit
  color: string | null
  /** The registered schema for this event type, when one exists. */
  schemaName: string | null
  /** False while the registry list is still loading, so neither action is offered yet. */
  schemasKnown: boolean
  environment: Environment
  isNew: boolean
  expanded: boolean
  onToggle: () => void
}) {
  const navigate = useNavigate()
  const { timeZone } = useSettings()
  const identity = hit.source && hit.detailType ? `${hit.source}@${hit.detailType}` : null

  return (
    <>
      <tr
        onClick={onToggle}
        className={cn(
          'cursor-pointer border-b border-edge/40 hover:bg-surface-2',
          isNew && !hit.backfill && 'bg-accent/5',
        )}
        style={color ? { boxShadow: `inset 3px 0 0 ${color}` } : undefined}
      >
        <td className="pl-2 text-ink-faint">
          {expanded ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />}
        </td>
        <td
          className="whitespace-nowrap px-2 py-1 font-mono text-ink-faint"
          title={formatDateTime(hit.timestamp, timeZone)}
        >
          {formatTime(hit.timestamp, timeZone)}
        </td>
        <td className="px-1 py-1">
          <GradeMark grade={hit.grade} headline={hit.headline} />
        </td>
        <td className="whitespace-nowrap px-2 py-1">
          {color ? (
            <span
              className="inline-flex shrink-0 items-center whitespace-nowrap rounded px-1.5 py-0.5 text-[10px] font-medium"
              style={tint(color)}
              title={hit.watchLabel}
            >
              {hit.watchLabel}
            </span>
          ) : (
            <Badge tone="accent" title={hit.watchLabel}>
              {hit.watchLabel}
            </Badge>
          )}
        </td>
        <td className="whitespace-nowrap px-2 py-1">
          {hit.source ? <Badge tone="info">{hit.source}</Badge> : <span className="text-ink-faint">—</span>}
        </td>
        <td className="w-full max-w-0 truncate px-2 py-1 font-mono text-ink-muted" title={hit.detailType ?? undefined}>
          {hit.detailType ?? '—'}
        </td>
        <td className="pr-1 text-right whitespace-nowrap">
          {identity && schemasKnown && schemaName && (
            <Button
              variant="ghost"
              size="sm"
              title={`Open schema ${schemaName}`}
              onClick={(e) => {
                e.stopPropagation()
                navigate(`/schemas?select=${encodeURIComponent(schemaName)}`)
              }}
            >
              <Braces className="size-3" />
            </Button>
          )}
          {identity && schemasKnown && !schemaName && (
            <Button
              variant="ghost"
              size="sm"
              className="text-warn"
              title={`No schema is registered for ${identity}. Draft one from the events within a minute of this hit. Health can do the wider analysis later.`}
              onClick={(e) => {
                e.stopPropagation()
                // The hit's group and time make this a two-minute scan of one
                // group rather than a day of every group: near-instant, and
                // enough to draft from. The Health report is the wide view.
                navigate(
                  `/schemas?draft=${encodeURIComponent(identity)}&group=${encodeURIComponent(hit.logGroup)}&around=${hit.timestamp}`,
                )
              }}
            >
              <FilePlus2 className="size-3" />
            </Button>
          )}
          <CopyButton text={stringify(hit.event)} title="Copy event JSON" />
        </td>
      </tr>
      {expanded && (
        <tr className="border-b border-edge/40 bg-surface-0">
          <td colSpan={7} className="p-0">
            <div className="flex items-center gap-3 px-3 pt-2 font-mono text-[10px] text-ink-faint">
              <span>{hit.eventId ?? 'no id'}</span>
              <span>{hit.logGroup}</span>
              <span>seen {formatTime(hit.receivedAt, timeZone)}</span>
            </div>
            <div className="flex min-w-0 flex-col gap-2 p-2 md:flex-row">
              <div className="flex min-w-0 flex-1 flex-col gap-2">
                <pre className="max-h-80 min-w-0 overflow-auto rounded-md border border-edge/60 p-3 font-mono text-[11px] leading-relaxed text-ink-muted">
                  {stringify(hit.event)}
                </pre>
                {identity && schemasKnown && (
                  <HitCheck hit={hit} identity={identity} environment={environment} />
                )}
              </div>
              {identity && (
                <div className="w-full shrink-0 rounded-md border border-edge/60 md:w-[420px]">
                  <OriginPanel schemaName={identity} compact />
                </div>
              )}
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

/**
 * This one event against its schema, with a ticket one click away.
 *
 * A hit is a discrepancy caught live, and the Health report is a day away.
 * Checking here answers "is this payload what the schema promised?" for the
 * event in front of you, and files the answer with whoever owns the producer
 * — no schema at all being as fileable as a wrong field.
 */
function HitCheck({
  hit,
  identity,
  environment,
}: {
  hit: WatchHit
  identity: string
  environment: Environment
}) {
  const [issues, setIssues] = useState<Issue[] | null>(null)
  const [error, setError] = useState<IpcError | null>(null)
  const [checking, setChecking] = useState(false)
  const [filing, setFiling] = useState<Issue | null>(null)
  const [filed, setFiled] = useState<Record<string, FiledTicket>>({})

  // A plain promise rather than useMutation: the rows mount and unmount as the
  // list scrolls, and a mutation observer orphaned by StrictMode never
  // reports back (see the schemas page).
  const check = async () => {
    setChecking(true)
    setError(null)
    try {
      setIssues(await ipc.validateEvent(identity, hit.event, hit.envId))
    } catch (e) {
      setError(ipc.asIpcError(e))
    } finally {
      setChecking(false)
    }
  }

  const context = {
    schemaName: identity,
    environment: environment.label,
    registry: environment.registryName,
    source: hit.source ?? '',
    detailType: hit.detailType ?? '',
    logGroup: hit.logGroup,
    minutes: null,
    typeName: null,
  }

  return (
    <div className="rounded-md border border-edge/60 p-2 text-[11px]">
      <div className="flex items-center gap-2">
        <Button variant="ghost" size="sm" onClick={check} disabled={checking}>
          {checking ? <Spinner /> : <ShieldCheck className="size-3" />}
          {issues ? 'Check again' : 'Check against schema'}
        </Button>
        {issues && issues.length === 0 && (
          <span className="flex items-center gap-1 text-ok">
            <CheckCircle2 className="size-3" />
            matches the schema
          </span>
        )}
        {issues && issues.length > 0 && (
          <span className="text-ink-faint">
            {issues.length} problem{issues.length === 1 ? '' : 's'} — file one to whoever owns
            the producer
          </span>
        )}
      </div>
      {error && (
        <div className="mt-2">
          <ErrorBox error={error} />
        </div>
      )}
      {issues && issues.length > 0 && (
        <ul className="mt-2 flex flex-col gap-1">
          {issues.map((issue) => (
            <li
              key={issue.key}
              className="flex items-start gap-2 rounded border border-edge/60 px-2 py-1"
            >
              <Badge tone={SEVERITY_TONE[issue.severity]} title={describeSeverity(issue).title}>
                {describeSeverity(issue).label}
              </Badge>
              <Badge tone="neutral">{KIND_LABELS[issue.kind]}</Badge>
              <span className="min-w-0 flex-1 text-ink-muted">
                {issue.path && <span className="mr-1 font-mono text-ink">{issue.path}</span>}
                <Marked text={issue.summary} />
              </span>
              {filed[issue.key] ? (
                <FiledChip ticket={filed[issue.key]} />
              ) : (
                <Button
                  variant="ghost"
                  size="sm"
                  title="File a Jira ticket for this"
                  onClick={() => setFiling(issue)}
                >
                  <Bug className="size-3" />
                  File
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
      <FileTicketDialog
        open={!!filing}
        issue={filing}
        context={context}
        onClose={() => setFiling(null)}
        onFiled={(issueKey, ticket) => setFiled((prev) => ({ ...prev, [issueKey]: ticket }))}
      />
    </div>
  )
}
