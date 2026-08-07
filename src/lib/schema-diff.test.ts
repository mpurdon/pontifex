import { describe, expect, it } from 'vitest'
import { buildTree } from './schema-model'
import { countChanges, diffTrees, type DiffNode } from './schema-diff'

function doc(payload: Record<string, unknown>, required: string[] = []) {
  return {
    openapi: '3.0.0',
    components: {
      schemas: {
        Payload: {
          type: 'object',
          required,
          properties: payload,
          additionalProperties: true,
        },
      },
    },
  }
}

function tree(document: unknown) {
  return buildTree(document, 'Payload')
}

/** Flatten to `name: change` for readable assertions. */
function marks(node: DiffNode | null): Record<string, string> {
  const out: Record<string, string> = {}
  const visit = (n: DiffNode) => {
    if (n.change) out[n.name] = n.change
    n.children.forEach(visit)
  }
  if (node) visit(node)
  return out
}

describe('diffTrees', () => {
  it('marks a new field as added', () => {
    const before = tree(doc({ id: { type: 'string' } }))
    const after = tree(doc({ id: { type: 'string' }, email: { type: 'string' } }))
    expect(marks(diffTrees(after, before))).toEqual({ email: 'added' })
  })

  it('marks a deleted field as removed and keeps it visible', () => {
    // The field is gone from the draft, so without a ghost row there would be
    // nothing on screen to colour — the deletion would be invisible.
    const before = tree(doc({ id: { type: 'string' }, email: { type: 'string' } }))
    const after = tree(doc({ id: { type: 'string' } }))
    const diff = diffTrees(after, before)

    expect(marks(diff)).toEqual({ email: 'removed' })
    const email = diff!.children.find((c) => c.name === 'email')
    expect(email?.ghost).toBe(true)
  })

  it('keeps a removed field in its original position', () => {
    // Appending it would make a deletion in the middle of a long object read
    // as though the last field went.
    const before = tree(
      doc({ a: { type: 'string' }, b: { type: 'string' }, c: { type: 'string' } }),
    )
    const after = tree(doc({ a: { type: 'string' }, c: { type: 'string' } }))
    const diff = diffTrees(after, before)
    expect(diff!.children.map((c) => c.name)).toEqual(['a', 'b', 'c'])
  })

  it('marks a retyped field as changed', () => {
    const before = tree(doc({ count: { type: 'string' } }))
    const after = tree(doc({ count: { type: 'integer' } }))
    expect(marks(diffTrees(after, before))).toEqual({ count: 'changed' })
  })

  it('marks a requiredness change', () => {
    const before = tree(doc({ id: { type: 'string' } }))
    const after = tree(doc({ id: { type: 'string' } }, ['id']))
    expect(marks(diffTrees(after, before))).toEqual({ id: 'changed' })
  })

  it('marks nothing when the documents match', () => {
    const before = tree(doc({ id: { type: 'string' } }, ['id']))
    const after = tree(doc({ id: { type: 'string' } }, ['id']))
    expect(marks(diffTrees(after, before))).toEqual({})
  })

  it('marks a nested addition without marking its parent', () => {
    const before = tree(doc({ meta: { type: 'object', properties: {} } }))
    const after = tree(
      doc({ meta: { type: 'object', properties: { trace: { type: 'string' } } } }),
    )
    expect(marks(diffTrees(after, before))).toEqual({ trace: 'added' })
  })

  it('ghosts the descendants of a removed object', () => {
    const before = tree(
      doc({ meta: { type: 'object', properties: { trace: { type: 'string' } } } }),
    )
    const after = tree(doc({}))
    const diff = diffTrees(after, before)
    const meta = diff!.children.find((c) => c.name === 'meta')
    expect(meta?.ghost).toBe(true)
    expect(meta?.children[0]?.ghost).toBe(true)
    expect(meta?.children[0]?.change).toBe('removed')
  })

  it('marks nothing at all without a baseline', () => {
    // A brand-new schema has nothing to compare against; painting every field
    // green would be noise rather than information.
    const after = tree(doc({ id: { type: 'string' }, email: { type: 'string' } }))
    expect(marks(diffTrees(after, null))).toEqual({})
  })

  it('returns null when there is no draft tree', () => {
    expect(diffTrees(null, tree(doc({})))).toBeNull()
  })
})

describe('countChanges', () => {
  it('totals each kind', () => {
    const before = tree(
      doc({ a: { type: 'string' }, b: { type: 'string' }, c: { type: 'string' } }),
    )
    const after = tree(
      doc({ a: { type: 'integer' }, c: { type: 'string' }, d: { type: 'string' } }),
    )
    expect(countChanges(diffTrees(after, before))).toEqual({
      added: 1,
      removed: 1,
      changed: 1,
    })
  })

  it('is all zeroes for an untouched document', () => {
    const same = tree(doc({ a: { type: 'string' } }))
    expect(countChanges(diffTrees(same, tree(doc({ a: { type: 'string' } }))))).toEqual({
      added: 0,
      removed: 0,
      changed: 0,
    })
  })
})
