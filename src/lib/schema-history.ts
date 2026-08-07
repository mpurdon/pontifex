import type { SchemaHistoryEntry } from './types'
import { buildTree, payloadSchemaName } from './schema-model'
import { diffTrees, type ChangeStatus, type DiffNode } from './schema-diff'

/**
 * A schema's history as a per-field record.
 *
 * The version list answers "what changed in v5". The question that actually
 * catches problems is the transpose: "how many times has *this field* been
 * changed, and when". A field edited once was a fix; a field edited in three
 * of the last five versions is a producer nobody has pinned down, and that is
 * only visible if the history is indexed by field rather than by version.
 */

/** One change to one field, at one version. */
export interface FieldChange {
  version: string
  createdAt: string | null
  kind: ChangeStatus
  /** For a `changed` entry, what specifically differed. */
  detail: string[]
}

export interface FieldHistory {
  /** Node id — the tree path, stable across versions. */
  id: string
  /** Field name, for display. */
  name: string
  /** Newest first. */
  changes: FieldChange[]
}

export interface Ledger {
  /** Node id → its change history, newest first. */
  byField: Map<string, FieldHistory>
  /** Versions actually compared, newest first. Excludes the oldest, which has
   *  no predecessor to differ from. */
  comparedVersions: string[]
}

/** Every node in a diffed tree that carries a change, ghosts included. */
function changedNodes(node: DiffNode, into: DiffNode[] = []): DiffNode[] {
  if (node.change) into.push(node)
  for (const child of node.children) changedNodes(child, into)
  return into
}

/**
 * Index a version history by field.
 *
 * Entries must be newest-first, as `schema_history` returns them. Each
 * consecutive pair is diffed, so a history of N versions yields N-1
 * comparisons.
 *
 * Diffs the payload type, matching the History tab and the editor: the
 * `AWSEvent` envelope is generated boilerplate, so changes there are noise.
 */
export function buildLedger(entries: SchemaHistoryEntry[]): Ledger {
  const byField = new Map<string, FieldHistory>()
  const comparedVersions: string[] = []

  for (let i = 0; i < entries.length - 1; i++) {
    const newer = entries[i]
    const older = entries[i + 1]
    const typeName =
      payloadSchemaName(newer.content) ?? payloadSchemaName(older.content)
    if (!typeName) continue

    const diff = diffTrees(
      buildTree(newer.content, typeName),
      buildTree(older.content, typeName),
    )
    if (!diff) continue

    comparedVersions.push(newer.version)

    for (const node of changedNodes(diff)) {
      // The root carries the type itself, not a field; skip it so a type-level
      // edit does not read as "every field changed".
      if (node.depth === 0) continue

      const existing = byField.get(node.id)
      const change: FieldChange = {
        version: newer.version,
        createdAt: newer.createdAt,
        kind: node.change!,
        detail: node.changeDetail ?? [],
      }
      if (existing) existing.changes.push(change)
      else byField.set(node.id, { id: node.id, name: node.name, changes: [change] })
    }
  }

  return { byField, comparedVersions }
}

/**
 * Fields changed in more than one version — the ones worth a second look.
 *
 * A field that changes repeatedly is the signal the user described: it was
 * fixed, and then it needed fixing again. Sorted by how often, so the worst
 * offender reads first.
 */
export function recurringFields(ledger: Ledger): FieldHistory[] {
  return [...ledger.byField.values()]
    .filter((f) => f.changes.length > 1)
    .sort((a, b) => b.changes.length - a.changes.length)
}

/** One-line summary of a field's history, for a tooltip. */
export function summarizeHistory(history: FieldHistory): string {
  const lines = history.changes.map((c) => {
    const what =
      c.kind === 'added'
        ? 'added'
        : c.kind === 'removed'
          ? 'removed'
          : c.detail.join(', ') || 'changed'
    return `v${c.version}: ${what}`
  })
  return lines.join('\n')
}
