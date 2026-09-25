import { describe, expect, it } from 'vitest'
import { examplesFor, repairLabel } from './repair'
import type { Issue, RealityCheckResult } from './types'

const issue = (over: Partial<Issue> = {}): Issue => ({
  key: 'wrongType:country',
  kind: 'wrongType',
  severity: 'error',
  path: 'country',
  summary: '`country` is declared object but 100% of events send null | string',
  action: 'Fix the producer, or redeclare `country` as null | string.',
  declared: 'object',
  observed: 'null | string',
  affected: 200,
  sampled: 200,
  rejects: true,
  example: null,
  message: '"" is not of type "object"',
  ...over,
})

const result = (drift: Partial<RealityCheckResult['drift']>): RealityCheckResult =>
  ({
    drift: {
      undeclared: [],
      unused: [],
      missingRequired: [],
      typeMismatches: [],
      enumDrift: [],
      ...drift,
    },
  }) as RealityCheckResult

describe('repairLabel', () => {
  it('says the type it is about to write, not just "fix"', () => {
    expect(repairLabel({ kind: 'widenType', types: ['string', 'null'] })).toBe(
      'Redeclare as string | null',
    )
  })

  it('counts the values an enum repair would admit', () => {
    expect(repairLabel({ kind: 'extendEnum', values: ['archived'] })).toBe(
      'Allow this value',
    )
    expect(repairLabel({ kind: 'extendEnum', values: ['a', 'b', 'c'] })).toBe(
      'Allow 3 values',
    )
  })

  it('shows the pattern it would pin, because that is the claim', () => {
    // "Constrain to the observed shape" is a button whose effect you cannot
    // read, and therefore cannot disagree with before clicking.
    expect(
      repairLabel({ kind: 'constrainPattern', pattern: '^\\d{7}-[0-9a-f]{16}$' }),
    ).toBe('Match ^\\d{7}-[0-9a-f]{16}$')
  })

  it('labels the repairs that take no operands', () => {
    expect(repairLabel({ kind: 'dropRequired' })).toBe('Make optional')
    expect(repairLabel({ kind: 'requireNonEmpty' })).toBe('Require a value')
    expect(repairLabel({ kind: 'declareField', types: ['string'] })).toBe(
      'Declare as string',
    )
  })
})

describe('examplesFor', () => {
  it('sends every value an enum drift names, not one of them', () => {
    // For enum drift the offending values *are* the finding, so a single
    // example would be a model reasoning about a third of the evidence.
    const examples = examplesFor(
      result({
        enumDrift: [
          {
            path: 'status',
            declared: ['open'],
            unexpected: ['archived', 'merged', 'void'],
            seenIn: 200,
            unexpectedIn: 12,
          },
        ],
      }),
      issue({ path: 'status', kind: 'outsideEnum' }),
    )

    expect(examples).toEqual(['archived', 'merged', 'void'])
  })

  it('takes the example recorded for the path that drifted', () => {
    const examples = examplesFor(
      result({
        typeMismatches: [
          {
            path: 'country',
            declared: 'object',
            observed: ['string'],
            seenIn: 200,
            mismatchedIn: 200,
            example: 'PO Box 694',
            suggested: ['string'],
          },
        ],
      }),
      issue(),
    )

    expect(examples).toContain('PO Box 694')
  })

  it('ignores values recorded for a different field', () => {
    const examples = examplesFor(
      result({
        typeMismatches: [
          {
            path: 'street',
            declared: 'object',
            observed: ['string'],
            seenIn: 200,
            mismatchedIn: 200,
            example: 'somewhere else',
            suggested: ['string'],
          },
        ],
      }),
      issue({ path: 'country' }),
    )

    expect(examples).not.toContain('somewhere else')
  })

  it('drops nulls, which say nothing a type name has not already said', () => {
    const examples = examplesFor(result({}), issue({ example: null }))
    expect(examples).toEqual([])
  })

  it('is capped, so a wide sample cannot become the whole prompt', () => {
    const examples = examplesFor(
      result({
        enumDrift: [
          {
            path: 'status',
            declared: [],
            unexpected: Array.from({ length: 40 }, (_, i) => `value-${i}`),
            seenIn: 200,
            unexpectedIn: 40,
          },
        ],
      }),
      issue({ path: 'status' }),
    )

    expect(examples).toHaveLength(10)
  })

  it('has nothing to offer before a check has run', () => {
    expect(examplesFor(null, issue())).toEqual([])
  })
})
