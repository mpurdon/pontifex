import { useMemo } from 'react'
import { History } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { SchemaHistoryEntry } from '@/lib/types'
import { Badge, EmptyState, ErrorBox, Spinner, cn } from '@/components/ui'
import { buildTree, payloadSchemaName } from '@/lib/schema-model'
import { countChanges, diffTrees } from '@/lib/schema-diff'
import { formatAge } from '@/lib/format'
import { useSchemaHistory } from './use-schema-history'

/**
 * What changed between one version and the one before it.
 *
 * Counted on the payload type — the type people actually edit — because the
 * envelope is generated boilerplate and a diff of it says nothing.
 */
function changesBetween(newer: SchemaHistoryEntry, older: SchemaHistoryEntry) {
  const typeName = payloadSchemaName(newer.content) ?? payloadSchemaName(older.content)
  if (!typeName) return null
  const after = buildTree(newer.content, typeName)
  const before = buildTree(older.content, typeName)
  return countChanges(diffTrees(after, before))
}

/** Absolute date, since "6 days ago" is ambiguous once you are comparing. */
function formatWhen(iso: string | null, now: number): string {
  if (!iso) return 'date unknown'
  const at = Date.parse(iso)
  if (Number.isNaN(at)) return 'date unknown'
  return `${formatAge(now - at)} ago · ${new Date(at).toLocaleDateString()}`
}

/**
 * A schema's version history.
 *
 * The registry versions every write, but the version *number* alone only says
 * that something changed. This turns the numbers into a record of what: each
 * entry carries what it added, removed and retyped relative to its
 * predecessor, so a field that keeps being changed is visible as a pattern
 * rather than something you have to remember noticing.
 */
export function HistoryPanel({
  name,
  envId,
  currentVersion,
  selectedVersion,
  onSelectVersion,
}: {
  name: string
  envId: string | undefined
  currentVersion: string
  /** The version currently being compared against, if any. */
  selectedVersion: string | null
  onSelectVersion: (version: string) => void
}) {
  const history = useSchemaHistory(name, envId)

  const now = Date.now()

  const rows = useMemo(() => {
    const entries = history.entries
    return entries.map((entry, i) => {
      const older = entries[i + 1]
      return {
        entry,
        changes: older ? changesBetween(entry, older) : null,
        isOldest: !older,
      }
    })
  }, [history.entries])

  if (history.isLoading) return <Spinner label="Loading history…" />
  if (history.isError) {
    return (
      <div className="p-3">
        <ErrorBox error={ipc.asIpcError(history.error)} />
      </div>
    )
  }
  if (rows.length === 0) {
    return (
      <EmptyState
        icon={<History className="size-8" />}
        title="No version history"
        detail="This schema has only ever been written once."
      />
    )
  }

  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <p className="border-b border-edge px-3 py-2 text-[11px] text-ink-faint">
        Every write creates a version. Counts are against the version below —
        select one to diff it against what is live.
      </p>

      <ul className="divide-y divide-edge">
        {rows.map(({ entry, changes, isOldest }) => {
          const isCurrent = entry.version === currentVersion
          const isSelected = entry.version === selectedVersion
          return (
            <li key={entry.version}>
              <button
                type="button"
                onClick={() => onSelectVersion(entry.version)}
                disabled={isCurrent}
                className={cn(
                  'flex w-full items-center gap-2 px-3 py-2 text-left text-xs',
                  isCurrent ? 'cursor-default' : 'cursor-pointer hover:bg-surface-2',
                  isSelected && 'bg-accent/10',
                )}
                title={
                  isCurrent
                    ? 'The version currently live'
                    : `Compare v${entry.version} against live`
                }
              >
                <span className="w-12 shrink-0 font-mono text-ink">v{entry.version}</span>

                {isCurrent && <Badge tone="ok">live</Badge>}

                <span className="min-w-0 flex-1 truncate text-ink-faint">
                  {formatWhen(entry.createdAt, now)}
                </span>

                {isOldest ? (
                  <span className="shrink-0 text-[10px] text-ink-faint">first version</span>
                ) : changes ? (
                  <span className="flex shrink-0 items-center gap-1">
                    {changes.added > 0 && <Badge tone="ok">+{changes.added}</Badge>}
                    {changes.removed > 0 && (
                      <Badge tone="danger">−{changes.removed}</Badge>
                    )}
                    {changes.changed > 0 && <Badge tone="warn">~{changes.changed}</Badge>}
                    {changes.added === 0 &&
                      changes.removed === 0 &&
                      changes.changed === 0 && (
                        <span
                          className="text-[10px] text-ink-faint"
                          title="The payload type is unchanged — the edit was elsewhere in the document"
                        >
                          no payload change
                        </span>
                      )}
                  </span>
                ) : null}
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
