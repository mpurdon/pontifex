/**
 * Filtering and alphabetical indexing for the schema list.
 *
 * This used to be a subsequence ("fuzzy") match, the kind an editor file picker
 * uses. Over `source@detail-type` names of forty-odd characters it matched
 * nearly everything: searching `call` returned `ais-medical@medical-extraction
 * Complete`, because a `c`, then a later `a`, then two later `l`s exist
 * somewhere in it. Results looked arbitrary, which is worse than too few.
 *
 * Substring matching is predictable, and splitting on whitespace keeps the
 * abbreviation case working — `milo pack` still finds
 * `milo-medical@packetNotification-assigned` — without the false positives.
 */

export interface Searchable {
  name: string
  source: string | null
  detailType: string | null
}

/** Every term must appear somewhere in the name. Case-insensitive. */
export function matchesQuery(schema: Searchable, query: string): boolean {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean)
  if (terms.length === 0) return true
  const haystack = `${schema.source ?? ''} ${schema.detailType ?? ''} ${schema.name}`.toLowerCase()
  return terms.every((term) => haystack.includes(term))
}

/**
 * How well a schema matches, lowest first.
 *
 * Used to order *sources*, so that searching `billing` puts the `billing-*`
 * sources above ones that merely mention it in a detail type.
 */
export function matchRank(schema: Searchable, query: string): number {
  const q = query.trim().toLowerCase()
  if (!q) return 0
  const source = (schema.source ?? '').toLowerCase()
  const detail = (schema.detailType ?? '').toLowerCase()

  if (source.startsWith(q)) return 0
  if (detail.startsWith(q)) return 1
  if (source.includes(q)) return 2
  if (detail.includes(q)) return 3
  return 4
}

/** The bucket a source sorts into on the A–Z rail. */
export function alphaKeyFor(source: string): string {
  const first = source.trim().charAt(0).toUpperCase()
  return first >= 'A' && first <= 'Z' ? first : '#'
}

/** `#` first, because non-alphabetic sources sort ahead of `A` in the list. */
export const ALPHA_KEYS: string[] = [
  '#',
  ...Array.from({ length: 26 }, (_, i) => String.fromCharCode(65 + i)),
]

/** Which rail letters have at least one source behind them. */
export function availableAlphaKeys(sources: string[]): Set<string> {
  return new Set(sources.map(alphaKeyFor))
}

/**
 * Index of the first row for a letter, or -1.
 *
 * Takes the already-built row list rather than the sources, because that is
 * what the virtualizer scrolls by — deriving it separately would drift the
 * moment pinning or filtering reorders things.
 */
export function firstRowForAlphaKey(
  rows: { kind: string; source?: string }[],
  key: string,
): number {
  return rows.findIndex(
    (row) => row.kind === 'group' && row.source !== undefined && alphaKeyFor(row.source) === key,
  )
}
