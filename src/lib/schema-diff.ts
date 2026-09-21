import type { SchemaNode } from './schema-model'

/**
 * Marking a draft's pending changes on the tree.
 *
 * Before saving, the only way to see what a set of edits actually amounts to
 * was the JSON diff view — which means leaving the structure editor, reading
 * two columns of serialized JSON, and coming back. Colouring the tree answers
 * "what am I about to change" in the place the changes are being made.
 *
 * Removed fields are the awkward half: they are gone from the draft, so the
 * tree cannot show them by construction. They are re-inserted as inert ghost
 * rows, in their original position, marked removed.
 */

export type ChangeStatus = 'added' | 'removed' | 'changed'

/** A tree node carrying how it differs from the baseline. */
export interface DiffNode extends SchemaNode {
  change?: ChangeStatus
  /** For a `changed` node, what specifically differs. */
  changeDetail?: string[]
  /** True for a node that exists only in the baseline — nothing to edit. */
  ghost?: boolean
  children: DiffNode[]
}

/**
 * What specifically changed about a field, in words.
 *
 * "Changed" on its own is not actionable — a retype and a requiredness flip
 * are different problems with different causes. Each entry is phrased
 * `before → after` so the direction is never in question.
 */
/** `minLength 2, pattern ^A`, or `none` when there are no bounds. */
function describeConstraints(constraints: Record<string, unknown>): string {
  const entries = Object.entries(constraints)
  if (entries.length === 0) return 'none'
  return entries.map(([key, value]) => `${key} ${asText(value)}`).join(', ')
}

/** `oneOf×2`, or empty when the node composes nothing. */
function describeComposition(node: SchemaNode): string {
  return node.composition.map(({ keyword, count }) => `${keyword}×${count}`).join(', ')
}

function asText(value: unknown): string | undefined {
  if (value === undefined) return undefined
  return typeof value === 'string' ? value : JSON.stringify(value)
}

export function describeChange(draft: SchemaNode, baseline: SchemaNode): string[] {
  const notes: string[] = []
  const or = (v: string | undefined) => (v === undefined || v === '' ? 'none' : v)

  if (draft.type !== baseline.type) {
    notes.push(`type ${baseline.type} → ${draft.type}`)
  }
  if (draft.required !== baseline.required) {
    notes.push(draft.required ? 'became required' : 'became optional')
  }
  // Null *acceptance* is the fact worth reporting, whichever spelling
  // carries it: `nullable: true` and a `null` type both count, as they do
  // for the bus's validator.
  if (draft.acceptsNull !== baseline.acceptsNull) {
    notes.push(draft.acceptsNull ? 'now accepts null' : 'no longer accepts null')
  }
  if (draft.refTarget !== baseline.refTarget) {
    notes.push(`type ref ${or(baseline.refTarget)} → ${or(draft.refTarget)}`)
  }
  if (draft.format !== baseline.format) {
    notes.push(`format ${or(baseline.format)} → ${or(draft.format)}`)
  }
  // Constraints and composition decide whether an event is rejected, so a
  // tightened `pattern` or a new `oneOf` branch is exactly the kind of change
  // a version diff exists to surface — they were computed per node and read
  // nowhere, which made the tree mark such a version unchanged.
  if (JSON.stringify(draft.constraints) !== JSON.stringify(baseline.constraints)) {
    notes.push(
      `constraints ${describeConstraints(baseline.constraints)} → ${describeConstraints(draft.constraints)}`,
    )
  }
  if (describeComposition(draft) !== describeComposition(baseline)) {
    notes.push(
      `composition ${or(describeComposition(baseline))} → ${or(describeComposition(draft))}`,
    )
  }
  if (JSON.stringify(draft.constValue) !== JSON.stringify(baseline.constValue)) {
    notes.push(
      `fixed value ${or(asText(baseline.constValue))} → ${or(asText(draft.constValue))}`,
    )
  }
  if (draft.additionalProperties !== baseline.additionalProperties) {
    notes.push(
      draft.additionalProperties
        ? 'now allows extra fields'
        : 'no longer allows extra fields',
    )
  }
  if (
    JSON.stringify(draft.enumValues ?? null) !==
    JSON.stringify(baseline.enumValues ?? null)
  ) {
    notes.push('allowed values changed')
  }
  return notes
}

/** Index a tree by node id so the walk below is a lookup rather than a search. */
function indexById(node: SchemaNode | null, into = new Map<string, SchemaNode>()) {
  if (!node) return into
  into.set(node.id, node)
  for (const child of node.children) indexById(child, into)
  return into
}

/** A baseline node turned into an inert row: present for reading, not editing. */
function toGhost(node: SchemaNode): DiffNode {
  return {
    ...node,
    change: 'removed',
    ghost: true,
    // Descendants of a removed field are removed with it, and expanding into
    // them would suggest they still exist.
    children: node.children.map(toGhost),
  }
}

/**
 * Merge a baseline tree into a draft tree, marking the differences.
 *
 * Returns the draft unchanged when there is no baseline — a brand-new schema
 * has nothing to compare against, and colouring every field green would be
 * noise rather than information.
 */
export function diffTrees(
  draft: SchemaNode | null,
  baseline: SchemaNode | null,
): DiffNode | null {
  if (!draft) return null
  if (!baseline) return draft as DiffNode

  const baselineById = indexById(baseline)

  const walk = (node: SchemaNode): DiffNode => {
    const before = baselineById.get(node.id)
    const children: DiffNode[] = node.children.map(walk)

    // Re-insert this node's removed children, keeping their original order
    // among the survivors so a deletion reads in place rather than at the end.
    if (before) {
      const present = new Set(node.children.map((c) => c.id))
      const removed = before.children.filter((c) => !present.has(c.id))
      for (const gone of removed) {
        const originalIndex = before.children.findIndex((c) => c.id === gone.id)
        const insertAt = Math.min(originalIndex, children.length)
        children.splice(insertAt, 0, toGhost(gone))
      }
    }

    let change: ChangeStatus | undefined
    let changeDetail: string[] | undefined
    if (!before) {
      change = 'added'
    } else {
      const notes = describeChange(node, before)
      if (notes.length > 0) {
        change = 'changed'
        changeDetail = notes
      }
    }

    return { ...node, change, changeDetail, children }
  }

  return walk(draft)
}

/** Totals for a summary line, counting ghosts as removals. */
export function countChanges(node: DiffNode | null): {
  added: number
  removed: number
  changed: number
} {
  const totals = { added: 0, removed: 0, changed: 0 }
  const visit = (n: DiffNode) => {
    if (n.change) totals[n.change] += 1
    for (const child of n.children) visit(child)
  }
  if (node) visit(node)
  return totals
}
