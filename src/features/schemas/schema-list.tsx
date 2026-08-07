import { useVirtualizer } from '@tanstack/react-virtual'
import { useEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, ChevronRight, Pin, PinOff, Search } from 'lucide-react'
import type { SchemaSummary } from '@/lib/types'
import { Input, cn } from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import {
  ALPHA_KEYS,
  availableAlphaKeys,
  firstRowForAlphaKey,
  matchRank,
  matchesQuery,
} from '@/lib/schema-search'

/** A flattened tree row: either a source header or a schema under it. */
type Row =
  | { kind: 'group'; source: string; count: number; pinned: boolean }
  | { kind: 'schema'; schema: SchemaSummary }

export function SchemaList({
  schemas,
  selected,
  onSelect,
}: {
  schemas: SchemaSummary[]
  selected: string | null
  onSelect: (name: string) => void
}) {
  const [filter, setFilter] = useState('')
  /**
   * Single-open accordion: opening a source closes the previous one.
   *
   * There are over a hundred sources in a production registry, so keeping
   * several expanded buries the one you are actually looking at.
   */
  const [openSource, setOpenSource] = useState<string | null>(null)
  const scrollRef = useRef<HTMLDivElement>(null)
  const { settings, saveSettings } = useSettings()
  const pinned = useMemo(
    () => new Set(settings?.pinnedSources ?? []),
    [settings?.pinnedSources],
  )

  const togglePin = (source: string) => {
    if (!settings) return
    const next = pinned.has(source)
      ? (settings.pinnedSources ?? []).filter((s) => s !== source)
      : [...(settings.pinnedSources ?? []), source]
    void saveSettings({ ...settings, pinnedSources: next }).catch(() => {
      // A failed pin is not worth interrupting the user for.
    })
  }

  const rows = useMemo<Row[]>(() => {
    const matched = schemas.filter((s) => matchesQuery(s, filter))

    // Group by event source. Schemas whose names do not follow the
    // `source@detail-type` convention land under "(ungrouped)" rather than
    // disappearing.
    const groups = new Map<string, SchemaSummary[]>()
    for (const schema of matched) {
      const key = schema.source ?? '(ungrouped)'
      const bucket = groups.get(key)
      if (bucket) bucket.push(schema)
      else groups.set(key, [schema])
    }

    // Best rank any schema in the source achieved, so searching `billing`
    // floats the `billing-*` sources above ones that merely mention it in a
    // detail type.
    const bestRank = new Map<string, number>()
    for (const [source, items] of groups) {
      bestRank.set(source, Math.min(...items.map((s) => matchRank(s, filter))))
    }

    // Pinned sources first, so whatever you are working on stays reachable in
    // a list of hundreds; then relevance while searching; alphabetical within
    // each band.
    const ordered = [...groups.keys()].sort((a, b) => {
      const pinDelta = Number(pinned.has(b)) - Number(pinned.has(a))
      if (pinDelta !== 0) return pinDelta
      const rankDelta = (bestRank.get(a) ?? 0) - (bestRank.get(b) ?? 0)
      if (rankDelta !== 0) return rankDelta
      return a.localeCompare(b)
    })

    const out: Row[] = []
    for (const source of ordered) {
      const items = groups.get(source)!
      out.push({
        kind: 'group',
        source,
        count: items.length,
        pinned: pinned.has(source),
      })
      // Filtering implies intent to find something, so every match expands
      // regardless of which source is open.
      if (filter || openSource === source) {
        for (const schema of items) out.push({ kind: 'schema', schema })
      }
    }
    return out
  }, [schemas, filter, openSource, pinned])

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => 26,
    overscan: 12,
  })

  const alphaAvailable = useMemo(
    () =>
      availableAlphaKeys(
        rows.flatMap((row) => (row.kind === 'group' ? [row.source] : [])),
      ),
    [rows],
  )

  /**
   * Jump to the first source under a letter.
   *
   * Indexes into the row list rather than the sources, so pinning and search
   * ranking — which both reorder the list — cannot send it to the wrong place.
   */
  const jumpToLetter = (key: string) => {
    const index = firstRowForAlphaKey(rows, key)
    if (index >= 0) virtualizer.scrollToIndex(index, { align: 'start' })
  }

  // Toggling the open source shut leaves nothing open, which is the right
  // way to collapse the list back down.
  const toggle = (source: string) =>
    setOpenSource((current) => (current === source ? null : source))

  // Keep the selected schema's source open — after a reload or a jump from
  // elsewhere in the app, the selection would otherwise be hidden.
  useEffect(() => {
    if (!selected) return
    const source = schemas.find((s) => s.name === selected)?.source
    if (source) setOpenSource(source)
  }, [selected, schemas])

  return (
    <div className="flex h-full flex-col">
      <div className="relative shrink-0 border-b border-edge p-2">
        <Search className="pointer-events-none absolute left-4 top-1/2 size-3 -translate-y-1/2 text-ink-faint" />
        <Input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder={`Filter ${schemas.length} schemas…`}
          className="pl-7"
        />
      </div>

      <div className="flex min-h-0 flex-1">
        {/*
          A–Z rail. Jumping to a letter is the fastest way through a hundred-plus
          sources when you know roughly what it is called but not exactly, which
          is the case search does not serve well.

          On the left, in its own gutter: against the tree it read as another
          column of content, and a rail you scan rather than read wants to be
          visibly furniture.
        */}
        <div
          // `min-h-0` + scroll so a short window compresses the rail instead of
          // letting 27 letters overflow the panel.
          className="flex min-h-0 shrink-0 select-none flex-col items-stretch justify-start overflow-y-auto border-r border-edge bg-surface-2 px-1 py-1"
          aria-label="Jump to source by letter"
        >
          {ALPHA_KEYS.map((key) => {
            const available = alphaAvailable.has(key)
            return (
              <button
                key={key}
                type="button"
                disabled={!available}
                onClick={() => jumpToLetter(key)}
                title={available ? `Jump to ${key}` : `No sources under ${key}`}
                className={cn(
                  'rounded px-1 text-center font-mono text-[18px] leading-[1.15]',
                  available
                    ? 'text-ink-muted hover:bg-surface-3 hover:text-accent'
                    : 'text-ink-faint/25',
                )}
              >
                {key}
              </button>
            )
          })}
        </div>

        <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto">
        {rows.length === 0 ? (
          <p className="p-4 text-center text-xs text-ink-faint">
            Nothing matches “{filter}”
          </p>
        ) : (
          <div
            style={{ height: virtualizer.getTotalSize(), position: 'relative' }}
          >
            {virtualizer.getVirtualItems().map((item) => {
              const row = rows[item.index]
              return (
                <div
                  key={item.key}
                  style={{
                    position: 'absolute',
                    top: 0,
                    left: 0,
                    width: '100%',
                    height: item.size,
                    transform: `translateY(${item.start}px)`,
                  }}
                >
                  {row.kind === 'group' ? (
                    <div
                      className={cn(
                        'group/src flex h-full w-full items-center text-[11px] font-semibold',
                        row.pinned ? 'text-accent' : 'text-ink-muted',
                      )}
                    >
                      <button
                        type="button"
                        onClick={() => toggle(row.source)}
                        className="flex h-full min-w-0 flex-1 items-center gap-1 px-2 text-left hover:bg-surface-2"
                      >
                        {openSource === row.source || filter ? (
                          <ChevronDown className="size-3 shrink-0" />
                        ) : (
                          <ChevronRight className="size-3 shrink-0" />
                        )}
                        <span className="truncate">{row.source}</span>
                        <span className="ml-auto shrink-0 pl-1 text-ink-faint">
                          {row.count}
                        </span>
                      </button>
                      <button
                        type="button"
                        onClick={() => togglePin(row.source)}
                        title={row.pinned ? 'Unpin' : 'Pin to top'}
                        className={cn(
                          'shrink-0 rounded p-1 hover:bg-surface-3',
                          row.pinned
                            ? 'text-accent'
                            : 'text-ink-faint opacity-0 group-hover/src:opacity-100 focus:opacity-100',
                        )}
                      >
                        {row.pinned ? (
                          <PinOff className="size-3" />
                        ) : (
                          <Pin className="size-3" />
                        )}
                      </button>
                    </div>
                  ) : (
                    <button
                      type="button"
                      onClick={() => onSelect(row.schema.name)}
                      title={row.schema.name}
                      className={cn(
                        'flex h-full w-full items-center gap-2 pl-6 pr-2 text-left text-xs',
                        selected === row.schema.name
                          ? 'bg-accent/15 text-accent'
                          : 'text-ink-muted hover:bg-surface-2 hover:text-ink',
                      )}
                    >
                      <span className="truncate">
                        {row.schema.detailType ?? row.schema.name}
                      </span>
                      {row.schema.version && (
                        <span className="ml-auto shrink-0 font-mono text-[10px] text-ink-faint">
                          v{row.schema.version}
                        </span>
                      )}
                    </button>
                  )}
                </div>
              )
            })}
          </div>
        )}
        </div>
      </div>
    </div>
  )
}
