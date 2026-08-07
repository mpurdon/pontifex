import { describe, expect, it } from 'vitest'
import type { SchemaHistoryEntry } from './types'
import { buildLedger, recurringFields, summarizeHistory } from './schema-history'

/** A document whose payload type carries the given properties. */
function doc(payload: Record<string, unknown>, required: string[] = []) {
  return {
    openapi: '3.0.0',
    components: {
      schemas: {
        AWSEvent: {
          type: 'object',
          properties: { detail: { $ref: '#/components/schemas/Payload' } },
        },
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

/** Newest first, as `schema_history` returns. */
function version(
  v: string,
  payload: Record<string, unknown>,
  required: string[] = [],
): SchemaHistoryEntry {
  return { version: v, createdAt: `2026-0${v}-01T00:00:00Z`, content: doc(payload, required) }
}

describe('buildLedger', () => {
  it('records an added field against the version that added it', () => {
    const ledger = buildLedger([
      version('2', { id: { type: 'string' }, email: { type: 'string' } }),
      version('1', { id: { type: 'string' } }),
    ])
    const email = ledger.byField.get('Payload/email')
    expect(email?.changes).toHaveLength(1)
    expect(email?.changes[0]).toMatchObject({ version: '2', kind: 'added' })
  })

  it('records a removal', () => {
    const ledger = buildLedger([
      version('2', { id: { type: 'string' } }),
      version('1', { id: { type: 'string' }, legacy: { type: 'string' } }),
    ])
    expect(ledger.byField.get('Payload/legacy')?.changes[0].kind).toBe('removed')
  })

  it('records what specifically changed on a retype', () => {
    const ledger = buildLedger([
      version('2', { count: { type: 'integer' } }),
      version('1', { count: { type: 'string' } }),
    ])
    const count = ledger.byField.get('Payload/count')
    expect(count?.changes[0].kind).toBe('changed')
    expect(count?.changes[0].detail).toContain('type string → integer')
  })

  it('records a requiredness flip in words', () => {
    const ledger = buildLedger([
      version('2', { id: { type: 'string' } }, ['id']),
      version('1', { id: { type: 'string' } }),
    ])
    expect(ledger.byField.get('Payload/id')?.changes[0].detail).toContain(
      'became required',
    )
  })

  it('accumulates repeated changes to one field across versions', () => {
    // The red flag: the same field edited again and again.
    const ledger = buildLedger([
      version('3', { stage: { type: 'string' } }),
      version('2', { stage: { type: 'integer' } }),
      version('1', { stage: { type: 'string' } }),
    ])
    expect(ledger.byField.get('Payload/stage')?.changes.map((c) => c.version)).toEqual([
      '3',
      '2',
    ])
  })

  it('ignores the oldest version, which has nothing to differ from', () => {
    const ledger = buildLedger([
      version('2', { id: { type: 'string' } }),
      version('1', { id: { type: 'string' } }),
    ])
    expect(ledger.comparedVersions).toEqual(['2'])
  })

  it('returns an empty ledger for a single-version history', () => {
    const ledger = buildLedger([version('1', { id: { type: 'string' } })])
    expect(ledger.byField.size).toBe(0)
    expect(ledger.comparedVersions).toEqual([])
  })

  it('records nothing when consecutive versions match', () => {
    const ledger = buildLedger([
      version('2', { id: { type: 'string' } }),
      version('1', { id: { type: 'string' } }),
    ])
    expect(ledger.byField.size).toBe(0)
  })
})

describe('recurringFields', () => {
  it('surfaces only fields changed more than once, worst first', () => {
    // `a` flips every version (3 changes); `b` flips twice then settles.
    const ledger = buildLedger([
      version('4', { a: { type: 'string' }, b: { type: 'integer' } }),
      version('3', { a: { type: 'integer' }, b: { type: 'string' } }),
      version('2', { a: { type: 'string' }, b: { type: 'integer' } }),
      version('1', { a: { type: 'integer' }, b: { type: 'integer' } }),
    ])
    const recurring = recurringFields(ledger)
    expect(recurring.map((f) => f.name)).toEqual(['a', 'b'])
    expect(recurring[0].changes).toHaveLength(3)
    expect(recurring[1].changes).toHaveLength(2)
  })

  it('is empty when every field was touched at most once', () => {
    const ledger = buildLedger([
      version('2', { a: { type: 'string' }, b: { type: 'string' } }),
      version('1', { a: { type: 'string' } }),
    ])
    expect(recurringFields(ledger)).toEqual([])
  })
})

describe('summarizeHistory', () => {
  it('lists each version and what happened', () => {
    const ledger = buildLedger([
      version('3', { stage: { type: 'string' } }),
      version('2', { stage: { type: 'integer' } }),
      version('1', {}),
    ])
    const summary = summarizeHistory(ledger.byField.get('Payload/stage')!)
    expect(summary).toContain('v3: type integer → string')
    expect(summary).toContain('v2: added')
  })
})
