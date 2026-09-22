import { STATUS_ORDER, type FilterStatus } from './status'

/**
 * Ordering the Health table.
 *
 * The report arrives worst-first, which is the right default, but "which
 * schema fails most" and "where is the drift" are questions the table can
 * answer only if a column can be sorted by. Numbers sort descending on the
 * first click, because the big number is what you came for; the name sorts
 * ascending; status keeps the report's own worst-first order.
 */
export type SortKey = 'status' | 'name' | 'failed' | 'drift'

export interface SortState {
  key: SortKey
  dir: 'asc' | 'desc'
}

export const DEFAULT_SORT: SortState = { key: 'status', dir: 'asc' }

const FIRST_DIR: Record<SortKey, SortState['dir']> = {
  status: 'asc',
  name: 'asc',
  failed: 'desc',
  drift: 'desc',
}

/** Clicking a header: a new column starts on its natural direction, the same one flips. */
export function nextSort(current: SortState, key: SortKey): SortState {
  if (current.key !== key) return { key, dir: FIRST_DIR[key] }
  return { key, dir: current.dir === 'asc' ? 'desc' : 'asc' }
}

/** The fields a row needs to be sorted. */
export interface SortableRow {
  name: string
  status: FilterStatus
  failed: number
  undeclared: number
  typeMismatches: number
  enumDrift: number
  missingRequired: number
  emptyRequired: number
}

export function driftOf(row: SortableRow): number {
  return (
    row.undeclared +
    row.typeMismatches +
    row.enumDrift +
    row.missingRequired +
    row.emptyRequired
  )
}

const COMPARE: Record<SortKey, (a: SortableRow, b: SortableRow) => number> = {
  // Stable sort keeps the report's order within a status, which is the
  // backend's own severity ranking.
  status: (a, b) => STATUS_ORDER.indexOf(a.status) - STATUS_ORDER.indexOf(b.status),
  name: (a, b) => a.name.localeCompare(b.name),
  failed: (a, b) => a.failed - b.failed,
  drift: (a, b) => driftOf(a) - driftOf(b),
}

/** A sorted copy; ties fall back to the name so the order is reproducible. */
export function sortRows<T extends SortableRow>(rows: readonly T[], sort: SortState): T[] {
  const compare = COMPARE[sort.key]
  const sign = sort.dir === 'asc' ? 1 : -1
  return [...rows].sort(
    (a, b) => sign * compare(a, b) || (sort.key === 'name' ? 0 : a.name.localeCompare(b.name)),
  )
}
