import { useMemo, type ReactNode } from 'react'
import { ChevronDown, ChevronUp } from 'lucide-react'
import type { RegistryReport } from '@/lib/types'
import { cn } from '@/components/ui'
import { STATUS, STATUS_ORDER, type FilterStatus } from './status'

/**
 * The report at a glance.
 *
 * A 300-row table answers "what is wrong with this schema" but never "how is
 * the bus doing" — proportion is not something you can see by scrolling. Three
 * questions are worth a picture: how the registry splits across statuses,
 * which event types carry the most traffic and whether those are the healthy
 * ones, and where the drift is concentrated.
 *
 * Drawn with plain elements rather than a charting library. Each is a bar
 * chart, and the app has no network access for a CDN anyway.
 */

/** A titled panel, so each chart reads as its own instrument. */
function Panel({
  title,
  subtitle,
  right,
  children,
}: {
  title: string
  subtitle?: string
  right?: ReactNode
  children: ReactNode
}) {
  return (
    <section className="flex min-w-0 flex-col rounded-md border border-edge bg-surface-1">
      <header className="flex items-baseline gap-2 border-b border-edge px-3 py-1.5">
        <h3 className="text-[11px] font-semibold text-ink">{title}</h3>
        {subtitle && <span className="text-[10px] text-ink-faint">{subtitle}</span>}
        {right && <span className="ml-auto text-[10px] text-ink-faint">{right}</span>}
      </header>
      <div className="min-w-0 flex-1 p-3">{children}</div>
    </section>
  )
}

/** Right-aligned monospace figure, so columns of numbers line up. */
function Value({ children }: { children: ReactNode }) {
  return (
    <span className="w-16 shrink-0 text-right font-mono text-[10px] tabular-nums text-ink-muted">
      {children}
    </span>
  )
}

/**
 * Every event type in the registry, split by status.
 *
 * The segments are the filter: clicking one is the same as clicking its chip,
 * and anything filtered out dims, so the bar always agrees with the table
 * beneath it.
 */
export function StatusBar({
  counts,
  done,
  total,
  active,
  onToggle,
  chartsOpen,
  onToggleCharts,
}: {
  counts: Record<FilterStatus, number>
  /** Of each count, how many have been dealt with since the report. */
  done: Record<FilterStatus, number>
  total: number
  active: Set<FilterStatus>
  onToggle: (status: FilterStatus) => void
  /** Whether the detail charts below this bar are expanded. */
  chartsOpen: boolean
  onToggleCharts: () => void
}) {
  const present = STATUS_ORDER.filter((s) => counts[s] > 0)
  if (total === 0) return null

  return (
    <Panel
      title="Registry health"
      subtitle={`${total.toLocaleString()} event types`}
      right={
        <span className="flex items-center gap-3">
          <span className="hidden sm:inline">click a band to filter</span>
          {/* The bar stays; only the charts below it fold away, so the screen
              can be mostly table when you are working through it. */}
          <button
            type="button"
            onClick={onToggleCharts}
            title={chartsOpen ? 'Hide the charts below' : 'Show the charts below'}
            className="flex items-center gap-1 rounded px-1 py-0.5 text-ink-faint hover:bg-surface-2 hover:text-ink-muted"
          >
            {chartsOpen ? (
              <ChevronUp className="size-3" />
            ) : (
              <ChevronDown className="size-3" />
            )}
            charts
          </button>
        </span>
      }
    >
      <div className="flex h-7 w-full overflow-hidden rounded border border-edge/60">
        {present.map((status) => {
          const share = (counts[status] / total) * 100
          const doneShare = (done[status] / counts[status]) * 100
          const isActive = active.has(status)
          return (
            <button
              key={status}
              type="button"
              onClick={() => onToggle(status)}
              style={{ width: `${share}%` }}
              title={`${counts[status]} ${STATUS[status].label} — ${share.toFixed(1)}%${
                done[status] > 0 ? `, ${done[status]} dealt with since the report` : ''
              }. ${STATUS[status].hint}`}
              className={cn(
                'relative flex h-full min-w-[2px] items-center justify-center transition-opacity',
                STATUS[status].bar,
                active.size > 0 && !isActive ? 'opacity-20' : 'opacity-100',
                'hover:opacity-75',
              )}
            >
              {/* The dealt-with share is hatched out from the right, so the
                  band still shows what the report found while the solid part
                  is what is left to do. */}
              {doneShare > 0 && (
                <span
                  aria-hidden
                  className="pointer-events-none absolute inset-y-0 right-0"
                  style={{
                    width: `${doneShare}%`,
                    backgroundImage:
                      'repeating-linear-gradient(135deg, transparent 0 3px, rgba(0,0,0,0.55) 3px 6px)',
                  }}
                />
              )}
              {/* Only label a band wide enough to hold the number without
                  clipping; the legend below carries the rest. */}
              {share >= 6 && (
                <span className="relative font-mono text-[10px] font-semibold text-surface-0">
                  {counts[status] - done[status] > 0 && done[status] > 0
                    ? `${counts[status] - done[status]}/${counts[status]}`
                    : counts[status]}
                </span>
              )}
            </button>
          )
        })}
      </div>

      {/* Legend: the bar alone cannot say which colour is which, and a thin
          band has nowhere to put its own label. */}
      <ul className="mt-2 flex flex-wrap gap-x-4 gap-y-1">
        {STATUS_ORDER.map((status) => {
          const count = counts[status]
          const isActive = active.has(status)
          return (
            <li key={status}>
              <button
                type="button"
                disabled={count === 0}
                onClick={() => onToggle(status)}
                title={STATUS[status].hint}
                className={cn(
                  'flex items-center gap-1.5 rounded px-1 py-0.5 text-[10px]',
                  count === 0
                    ? 'cursor-default opacity-40'
                    : 'cursor-pointer hover:bg-surface-2',
                  active.size > 0 && !isActive && count > 0 && 'opacity-50',
                )}
              >
                <span className={cn('size-2 shrink-0 rounded-sm', STATUS[status].dot)} />
                <span className={isActive ? 'text-ink' : 'text-ink-muted'}>
                  {STATUS[status].label}
                </span>
                <span className="font-mono tabular-nums text-ink-faint">
                  {count.toLocaleString()}
                </span>
                <span className="font-mono tabular-nums text-ink-faint/60">
                  {total > 0 ? `${((count / total) * 100).toFixed(0)}%` : '0%'}
                </span>
                {done[status] > 0 && (
                  <span
                    className="font-mono tabular-nums text-ok/80"
                    title={`${done[status]} dealt with since the report`}
                  >
                    ✓{done[status]}
                  </span>
                )}
              </button>
            </li>
          )
        })}
      </ul>
    </Panel>
  )
}

/**
 * The busiest event types, coloured by health.
 *
 * Ranked by *observed* volume rather than sampled: the grading bucket caps at
 * 50 events per type, so `sampled` says nothing about which types dominate the
 * bus. This is the axis the table cannot show — a purple bar at the top of
 * this list is a much larger problem than the same bar at the bottom.
 */
export function TrafficChart({
  report,
  limit = 12,
  onSelect,
}: {
  report: RegistryReport
  limit?: number
  onSelect: (name: string) => void
}) {
  const bars = useMemo(() => {
    const graded = report.rows
      .filter((r) => r.observed > 0)
      .map((r) => ({
        key: r.name,
        label: r.name,
        count: r.observed,
        status: r.status as FilterStatus,
        schemaName: r.name as string | null,
      }))

    const missing = report.unregistered.map((u) => ({
      key: `${u.source}@${u.detailType}`,
      label: `${u.source}@${u.detailType}`,
      count: u.count,
      status: 'missing' as FilterStatus,
      schemaName: null,
    }))

    return [...graded, ...missing].sort((a, b) => b.count - a.count).slice(0, limit)
  }, [report, limit])

  if (bars.length === 0) return null

  const max = bars[0].count
  const shown = bars.reduce((sum, b) => sum + b.count, 0)

  return (
    <Panel
      title="Busiest event types"
      subtitle={`top ${bars.length}`}
      right={`${((shown / Math.max(report.scannedEvents, 1)) * 100).toFixed(0)}% of sampled events`}
    >
      <ul className="space-y-0.5">
        {bars.map((bar) => (
          <li key={bar.key}>
            <button
              type="button"
              disabled={!bar.schemaName}
              onClick={() => bar.schemaName && onSelect(bar.schemaName)}
              title={`${bar.label} — ${bar.count.toLocaleString()} events, ${STATUS[bar.status].label}`}
              className={cn(
                'flex w-full items-center gap-2 rounded px-1 py-px text-left',
                bar.schemaName ? 'cursor-pointer hover:bg-surface-2' : 'cursor-default',
              )}
            >
              <span className="w-56 shrink-0 truncate font-mono text-[10px] text-ink-muted">
                {bar.label}
              </span>
              <span className="relative h-3.5 min-w-0 flex-1 overflow-hidden rounded-sm bg-surface-0">
                <span
                  className={cn('absolute inset-y-0 left-0', STATUS[bar.status].bar)}
                  style={{ width: `${Math.max((bar.count / max) * 100, 0.8)}%` }}
                />
              </span>
              <Value>{bar.count.toLocaleString()}</Value>
            </button>
          </li>
        ))}
      </ul>
    </Panel>
  )
}

/**
 * Where the drift is concentrated.
 *
 * Totals each kind across the registry, so "we have a type-mismatch problem"
 * is distinguishable from "we have an undeclared-fields problem" — different
 * causes, different fixes. Counts also carry how many schemas are affected,
 * because 4,000 undeclared fields across two schemas is a very different
 * morning from the same number spread over two hundred.
 */
export function DriftBreakdown({ report }: { report: RegistryReport }) {
  const kinds = useMemo(() => {
    const spec = [
      {
        key: 'undeclared' as const,
        label: 'undeclared fields',
        bar: 'bg-warn',
        hint: 'Present in events, absent from the schema',
      },
      {
        key: 'typeMismatches' as const,
        label: 'type mismatches',
        bar: 'bg-violet',
        hint: 'The declared type is not what events carry',
      },
      {
        key: 'enumDrift' as const,
        label: 'values outside enum',
        bar: 'bg-orange',
        hint: 'Values the declared enum does not allow',
      },
      {
        key: 'missingRequired' as const,
        label: 'required but absent',
        bar: 'bg-danger',
        hint: 'Declared required, yet missing from some events',
      },
    ]

    return spec.map((k) => ({
      ...k,
      total: report.rows.reduce((sum, row) => sum + row[k.key], 0),
      schemas: report.rows.filter((row) => row[k.key] > 0).length,
    }))
  }, [report])

  const max = Math.max(...kinds.map((k) => k.total), 1)
  if (kinds.every((k) => k.total === 0)) return null

  return (
    <Panel title="Drift by kind" subtitle="across every graded schema">
      <ul className="space-y-1">
        {kinds.map((kind) => (
          <li key={kind.key} title={kind.hint}>
            <div className="flex items-center gap-2">
              <span className="w-36 shrink-0 text-[10px] text-ink-muted">
                {kind.label}
              </span>
              <span className="relative h-3.5 min-w-0 flex-1 overflow-hidden rounded-sm bg-surface-0">
                <span
                  className={cn('absolute inset-y-0 left-0', kind.bar)}
                  style={{ width: `${kind.total === 0 ? 0 : Math.max((kind.total / max) * 100, 0.8)}%` }}
                />
              </span>
              <Value>{kind.total.toLocaleString()}</Value>
            </div>
            <p className="pl-[9.5rem] text-[9px] text-ink-faint/70">
              {kind.schemas === 0
                ? 'none'
                : `across ${kind.schemas} schema${kind.schemas === 1 ? '' : 's'}`}
            </p>
          </li>
        ))}
      </ul>
    </Panel>
  )
}
