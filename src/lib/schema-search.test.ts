import { describe, expect, it } from 'vitest'
import {
  ALPHA_KEYS,
  alphaKeyFor,
  availableAlphaKeys,
  firstRowForAlphaKey,
  matchRank,
  matchesQuery,
  type Searchable,
} from './schema-search'

function schema(name: string): Searchable {
  const [source, ...rest] = name.split('@')
  return { name, source, detailType: rest.join('@') || null }
}

const REGISTRY = [
  'agent-rdlProcessor@agenticReviewOfRDL-completed',
  'ais-medical@medical-extractionComplete',
  'ais-outreachLegal@outreachLegal-extractionComplete',
  'billing-medical@CelebrationCallAssigned',
  'billing-medical@update-client',
  'callbot@va-pay-increase-reported',
  'clientPortal-billing@clientPortal-connectionsView',
  'orders-api@orderNotification-assigned',
].map(schema)

describe('matchesQuery', () => {
  it('only returns names that actually contain the query', () => {
    // The subsequence matcher returned all eight of these for `call`, because
    // a c…a…l…l can be picked out of almost any long name.
    const hits = REGISTRY.filter((s) => matchesQuery(s, 'call')).map((s) => s.name)
    expect(hits).toEqual([
      'billing-medical@CelebrationCallAssigned',
      'callbot@va-pay-increase-reported',
    ])
  })

  it('does not match a scattered subsequence', () => {
    expect(matchesQuery(schema('ais-medical@medical-extractionComplete'), 'call')).toBe(false)
  })

  it('is case-insensitive', () => {
    expect(matchesQuery(schema('billing-medical@CelebrationCallAssigned'), 'CALL')).toBe(true)
    expect(matchesQuery(schema('callbot@x'), 'CaLl')).toBe(true)
  })

  it('treats whitespace as separate terms that must all match', () => {
    // Keeps the abbreviation case working without the false positives.
    const s = schema('orders-api@orderNotification-assigned')
    expect(matchesQuery(s, 'orders order')).toBe(true)
    expect(matchesQuery(s, 'orders nope')).toBe(false)
  })

  it('matches on the source or the detail type', () => {
    expect(matchesQuery(schema('billing-medical@update-client'), 'billing')).toBe(true)
    expect(matchesQuery(schema('billing-medical@update-client'), 'update')).toBe(true)
  })

  it('matches everything when the query is empty or whitespace', () => {
    expect(REGISTRY.every((s) => matchesQuery(s, ''))).toBe(true)
    expect(REGISTRY.every((s) => matchesQuery(s, '   '))).toBe(true)
  })
})

describe('matchRank', () => {
  it('puts a source prefix ahead of a detail-type mention', () => {
    const bySource = schema('callbot@va-pay-increase-reported')
    const byDetail = schema('billing-medical@CelebrationCallAssigned')
    expect(matchRank(bySource, 'call')).toBeLessThan(matchRank(byDetail, 'call'))
  })

  it('ranks everything equally when there is no query', () => {
    expect(matchRank(REGISTRY[0], '')).toBe(0)
    expect(matchRank(REGISTRY[1], '')).toBe(0)
  })
})

describe('alphaKeyFor', () => {
  it('uses the uppercased first letter', () => {
    expect(alphaKeyFor('billing-medical')).toBe('B')
    expect(alphaKeyFor('Atomic Forms')).toBe('A')
  })

  it('buckets non-alphabetic sources under #', () => {
    expect(alphaKeyFor('(ungrouped)')).toBe('#')
    expect(alphaKeyFor('3rd-party')).toBe('#')
    expect(alphaKeyFor('')).toBe('#')
  })
})

describe('ALPHA_KEYS', () => {
  it('is # then A to Z', () => {
    expect(ALPHA_KEYS).toHaveLength(27)
    expect(ALPHA_KEYS[0]).toBe('#')
    expect(ALPHA_KEYS[1]).toBe('A')
    expect(ALPHA_KEYS[26]).toBe('Z')
  })
})

describe('availableAlphaKeys', () => {
  it('reports only letters that have a source', () => {
    const keys = availableAlphaKeys(['agent-rdlProcessor', 'billing-medical', 'callbot'])
    expect([...keys].sort()).toEqual(['A', 'B', 'C'])
    expect(keys.has('Z')).toBe(false)
  })
})

describe('firstRowForAlphaKey', () => {
  const rows = [
    { kind: 'group', source: 'agent-rdlProcessor' },
    { kind: 'schema' },
    { kind: 'group', source: 'ais-medical' },
    { kind: 'group', source: 'billing-medical' },
    { kind: 'schema' },
    { kind: 'group', source: 'callbot' },
  ]

  it('finds the first group for a letter, not a later one', () => {
    expect(firstRowForAlphaKey(rows, 'A')).toBe(0)
    expect(firstRowForAlphaKey(rows, 'B')).toBe(3)
    expect(firstRowForAlphaKey(rows, 'C')).toBe(5)
  })

  it('ignores schema rows when locating the letter', () => {
    // Schema rows carry no source; matching one would scroll to the wrong place.
    expect(firstRowForAlphaKey(rows, 'A')).not.toBe(1)
  })

  it('returns -1 for a letter with no sources', () => {
    expect(firstRowForAlphaKey(rows, 'Z')).toBe(-1)
  })
})
