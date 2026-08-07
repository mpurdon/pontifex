import { useEffect, useMemo, useState } from 'react'
import { AlertTriangle, ChevronLeft, ChevronRight, History } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { SchemaHistoryEntry } from '@/lib/types'
import { Badge, Button, EmptyState, ErrorBox, Spinner } from '@/components/ui'
import { buildTree, payloadSchemaName, type SchemaNode } from '@/lib/schema-model'
import { countChanges, diffTrees } from '@/lib/schema-diff'
import { recurringFields } from '@/lib/schema-history'
import { formatAge } from '@/lib/format'
import { SchemaTree } from '@/components/schema-editor/tree'
import { useSchemaHistory } from './use-schema-history'

/**
 * A schema's history as something you can scrub through.
 *
 * The History list answers "what changed in v5" one row at a time. This
 * answers the question you actually have when a field keeps moving — *when did
 * this start* — by letting you walk the versions and watch the shape change in
 * place. Each step marks what that version did relative to the one before it,
 * and the marks glow briefly so the change is visible even when it is three
 * levels down a long type.
 */
export function VersionTimeline({
  name,
  envId,
}: {
  name: string
  envId: string | undefined
}) {
  const history = useSchemaHistory(name, envId)
  const entries: SchemaHistoryEntry[] = history.entries

  // Index into `entries`, which is newest-first. The slider runs oldest → newest
  // left to right, so it is presented reversed; this stays in data order.
  const [index, setIndex] = useState(0)

  useEffect(() => {
    // Land on the newest version whenever the schema changes.
    setIndex(0)
  }, [name, envId])

  const ledger = history.ledger
  const recurring = useMemo(() => recurringFields(ledger), [ledger])

  const current = entries[index]
  const previous = entries[index + 1]

  const tree = useMemo(() => {
    if (!current) return null
    const typeName =
      payloadSchemaName(current.content) ??
      (previous ? payloadSchemaName(previous.content) : undefined)
    if (!typeName) return null
    const after = buildTree(current.content, typeName)
    const before = previous ? buildTree(previous.content, typeName) : null
    return diffTrees(after, before)
  }, [current, previous])

  const changes = useMemo(() => countChanges(tree), [tree])

  // Everything visible: the point is to see the change, not to hunt for it.
  const expanded = useMemo(() => {
    const ids = new Set<string>()
    const walk = (node: SchemaNode) => {
      ids.add(node.id)
      node.children.forEach(walk)
    }
    if (tree) walk(tree)
    return ids
  }, [tree])

  if (history.isLoading) return <Spinner label="Loading history…" />
  if (history.isError) {
    return (
      <div className="p-3">
        <ErrorBox error={ipc.asIpcError(history.error)} />
      </div>
    )
  }
  if (entries.length === 0 || !current) {
    return (
      <EmptyState
        icon={<History className="size-8" />}
        title="No version history"
        detail="This schema has only ever been written once."
      />
    )
  }

  const at = current.createdAt ? Date.parse(current.createdAt) : NaN
  const oldest = entries.length - 1
  // Slider value runs oldest→newest; data index is the reverse.
  const sliderValue = oldest - index
  const step = (delta: number) =>
    setIndex((i) => Math.min(oldest, Math.max(0, i - delta)))

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* Scrubber */}
      <div className="shrink-0 border-b border-edge px-3 py-2">
        <div className="flex items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => step(-1)}
            disabled={index >= oldest}
            title="Older version"
          >
            <ChevronLeft className="size-3" />
          </Button>

          <input
            type="range"
            min={0}
            max={oldest}
            step={1}
            value={sliderValue}
            onChange={(e) => setIndex(oldest - Number(e.target.value))}
            className="h-1 min-w-0 flex-1 cursor-pointer accent-accent"
            aria-label="Schema version"
          />

          <Button
            variant="ghost"
            size="sm"
            onClick={() => step(1)}
            disabled={index === 0}
            title="Newer version"
          >
            <ChevronRight className="size-3" />
          </Button>
        </div>

        <div className="mt-1.5 flex items-center gap-2 text-[11px]">
          <span className="font-mono text-ink">v{current.version}</span>
          {index === 0 && <Badge tone="ok">live</Badge>}
          <span className="text-ink-faint">
            {Number.isNaN(at)
              ? 'date unknown'
              : `${formatAge(Date.now() - at)} ago · ${new Date(at).toLocaleString()}`}
          </span>

          <span className="ml-auto flex items-center gap-1">
            {previous ? (
              <>
                {changes.added > 0 && <Badge tone="ok">+{changes.added}</Badge>}
                {changes.removed > 0 && (
                  <Badge tone="danger">−{changes.removed}</Badge>
                )}
                {changes.changed > 0 && <Badge tone="warn">~{changes.changed}</Badge>}
                {changes.added === 0 &&
                  changes.removed === 0 &&
                  changes.changed === 0 && (
                    <span className="text-ink-faint">no payload change</span>
                  )}
                <span className="text-ink-faint">vs v{previous.version}</span>
              </>
            ) : (
              <span className="text-ink-faint">first version — nothing before it</span>
            )}
          </span>
        </div>
      </div>

      {/* Fields that keep moving. Shown above the tree because it is the
          conclusion the timeline exists to produce. */}
      {recurring.length > 0 && (
        <div className="shrink-0 border-b border-edge bg-warn/5 px-3 py-2">
          <p className="flex items-center gap-1.5 text-[11px] font-semibold text-warn">
            <AlertTriangle className="size-3" />
            Changed more than once
          </p>
          <ul className="mt-1 flex flex-wrap gap-1">
            {recurring.map((field) => (
              <li key={field.id}>
                <span
                  className="rounded bg-warn/15 px-1.5 py-0.5 font-mono text-[10px] text-warn"
                  title={field.changes
                    .map(
                      (c) =>
                        `v${c.version}: ${
                          c.kind === 'changed'
                            ? c.detail.join(', ') || 'changed'
                            : c.kind
                        }`,
                    )
                    .join('\n')}
                >
                  {field.name} ×{field.changes.length}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}

      {/* Keyed on the version so React remounts the rows, which restarts the
          glow animation on every step. */}
      <div key={current.version} className="min-h-0 flex-1 overflow-auto">
        {tree ? (
          <SchemaTree
            node={tree}
            selectedId={null}
            expanded={expanded}
            history={ledger.byField}
            glow
            // Read-only: this is a record of what happened, not an editor.
            onSelect={() => {}}
            onToggle={() => {}}
            onAddChild={() => {}}
            onFollowRef={() => {}}
          />
        ) : (
          <div className="p-4 text-center text-[11px] text-ink-faint">
            This version has no payload type to show.
          </div>
        )}
      </div>
    </div>
  )
}
