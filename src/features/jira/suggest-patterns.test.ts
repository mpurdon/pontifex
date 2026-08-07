import { describe, expect, it } from 'vitest'
import { suggestPatterns } from './jira-settings'

describe('suggestPatterns', () => {
  it('offers a wildcard for a family more than one source belongs to', () => {
    const options = suggestPatterns([
      'billing-invoices',
      'billing-ledger',
      'milo-disability',
    ])
    expect(options).toContain('billing-*')
    // One source is not a family — `milo-*` would just be a slower way of
    // writing the name that is already on the list.
    expect(options).not.toContain('milo-*')
  })

  it('keeps every source, so an exact rule is one pick away', () => {
    const options = suggestPatterns(['billing-invoices', 'billing-ledger'])
    expect(options).toContain('billing-invoices')
    expect(options).toContain('billing-ledger')
  })

  it('puts the wildcards first, since that is the rule usually wanted', () => {
    const options = suggestPatterns(['billing-a', 'billing-b'])
    expect(options[0]).toBe('billing-*')
  })

  it('splits on a dot or a space as well as a dash', () => {
    expect(suggestPatterns(['ais.legal', 'ais.medical'])).toContain('ais-*')
    // Sources are not all dash-separated — `Atomic Forms` is a real one.
    expect(suggestPatterns(['Atomic Forms', 'Atomic Records'])).toContain('Atomic-*')
  })

  it('ignores a source with no family part rather than offering `-*`', () => {
    // Sources come through in the order given — the caller sorts them.
    expect(suggestPatterns(['standalone', 'other'])).toEqual(['standalone', 'other'])
  })
})
