import { describe, expect, it } from 'vitest'
import { globMatches, matchingSchemas } from './payload-paths'

describe('globMatches', () => {
  it('is exact without a wildcard, and any when blank', () => {
    expect(globMatches('orders', 'orders')).toBe(true)
    expect(globMatches('orders', 'orders-v2')).toBe(false)
    expect(globMatches('', 'anything')).toBe(true)
    expect(globMatches(null, 'anything')).toBe(true)
  })

  it('treats * as any run of characters', () => {
    expect(globMatches('orders*', 'orders-v2')).toBe(true)
    expect(globMatches('*assigned', 'ticket-assigned')).toBe(true)
    expect(globMatches('*Notification*', 'orderNotification-sent')).toBe(true)
    expect(globMatches('a*c', 'abc')).toBe(true)
    expect(globMatches('a*c', 'ab')).toBe(false)
  })
})

describe('matchingSchemas', () => {
  const names = [
    'cadence-outreachLegal@cadence-sessionEnded',
    'cadence-outreachLegal@cadence-sessionStarted',
    'billing-medical@invoice-assigned',
  ]

  it('narrows by both halves of the name', () => {
    expect(matchingSchemas(names, 'cadence-*', '*Ended')).toEqual([
      'cadence-outreachLegal@cadence-sessionEnded',
    ])
    expect(matchingSchemas(names, '', '*assigned')).toEqual(['billing-medical@invoice-assigned'])
    expect(matchingSchemas(names, null, null)).toHaveLength(3)
  })
})
