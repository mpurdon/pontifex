import { describe, expect, it } from 'vitest'
import { DEFAULT_SORT, nextSort, sortRows, type SortableRow } from './sort'

const row = (
  name: string,
  status: SortableRow['status'],
  failed = 0,
  drift: Partial<
    Pick<SortableRow, 'undeclared' | 'typeMismatches' | 'enumDrift' | 'missingRequired' | 'emptyRequired'>
  > = {},
): SortableRow => ({
  name,
  status,
  failed,
  undeclared: 0,
  typeMismatches: 0,
  enumDrift: 0,
  missingRequired: 0,
  emptyRequired: 0,
  ...drift,
})

const rows = [
  row('b-ok', 'ok'),
  row('a-fail', 'failing', 12, { typeMismatches: 3 }),
  row('c-drift', 'drifting', 0, { undeclared: 40 }),
  row('d-fail', 'failing', 4),
  row('missing@x', 'missing'),
]

const names = (sorted: SortableRow[]) => sorted.map((r) => r.name)

describe('sortRows', () => {
  it('keeps worst-first by default, names breaking ties', () => {
    expect(names(sortRows(rows, DEFAULT_SORT))).toEqual([
      'missing@x',
      'a-fail',
      'd-fail',
      'c-drift',
      'b-ok',
    ])
  })

  it('sorts by name either way', () => {
    expect(names(sortRows(rows, { key: 'name', dir: 'asc' }))[0]).toBe('a-fail')
    expect(names(sortRows(rows, { key: 'name', dir: 'desc' }))[0]).toBe('missing@x')
  })

  it('puts the most failures first when descending', () => {
    expect(names(sortRows(rows, { key: 'failed', dir: 'desc' })).slice(0, 2)).toEqual([
      'a-fail',
      'd-fail',
    ])
  })

  it('totals every kind of drift', () => {
    expect(names(sortRows(rows, { key: 'drift', dir: 'desc' })).slice(0, 2)).toEqual([
      'c-drift',
      'a-fail',
    ])
  })

  it('does not mutate its input', () => {
    const copy = [...rows]
    sortRows(rows, { key: 'name', dir: 'desc' })
    expect(rows).toEqual(copy)
  })
})

describe('nextSort', () => {
  it('starts numbers descending and names ascending', () => {
    expect(nextSort(DEFAULT_SORT, 'failed')).toEqual({ key: 'failed', dir: 'desc' })
    expect(nextSort(DEFAULT_SORT, 'name')).toEqual({ key: 'name', dir: 'asc' })
  })

  it('flips the same column', () => {
    expect(nextSort({ key: 'failed', dir: 'desc' }, 'failed')).toEqual({
      key: 'failed',
      dir: 'asc',
    })
  })
})
