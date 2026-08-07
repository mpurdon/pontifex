/**
 * Parsing for the application log.
 *
 * The backend writes `[rfc3339][LEVEL][category] message`. Filtering by
 * category is the whole point of logging heavily — without it, turning the
 * level up to debug produces a wall of text you cannot read — so the parse has
 * to be reliable enough to filter on, and forgiving enough that a line it does
 * not recognise still shows up rather than vanishing.
 */

export type LogLevelName = 'error' | 'warn' | 'info' | 'debug' | 'trace'

export interface LogLine {
  /** The original text, always rendered as-is. */
  raw: string
  timestamp: string | null
  level: LogLevelName | null
  /** The category the backend tagged this with, e.g. `ipc`, `aws`. */
  category: string | null
  /** Message with the bracketed prefix stripped, for readability. */
  message: string
}

/** `[2026-07-31T20:15:00Z][INFO][ipc] list_schemas(envId) ok — 812ms` */
const LINE = /^\[([^\]]*)\]\[([A-Z]+)\]\[([^\]]*)\]\s?([\s\S]*)$/

function toLevel(name: string): LogLevelName | null {
  switch (name.toUpperCase()) {
    case 'ERROR':
      return 'error'
    case 'WARN':
      return 'warn'
    case 'INFO':
      return 'info'
    case 'DEBUG':
      return 'debug'
    case 'TRACE':
      return 'trace'
    default:
      return null
  }
}

export function parseLogLine(raw: string): LogLine {
  const match = LINE.exec(raw)
  if (!match) {
    // Panic backtraces and anything written before the logger was installed
    // land here. Showing them unparsed beats dropping them.
    return { raw, timestamp: null, level: null, category: null, message: raw }
  }
  const [, timestamp, level, category, message] = match
  return {
    raw,
    timestamp: timestamp || null,
    level: toLevel(level),
    category: category || null,
    message,
  }
}

export function parseLogLines(lines: string[]): LogLine[] {
  return lines.map(parseLogLine)
}

/** Categories actually present, with counts, for the filter chips. */
export function categoryCounts(lines: LogLine[]): Map<string, number> {
  const counts = new Map<string, number>()
  for (const line of lines) {
    const key = line.category ?? '(none)'
    counts.set(key, (counts.get(key) ?? 0) + 1)
  }
  return counts
}

export function levelCounts(lines: LogLine[]): Map<string, number> {
  const counts = new Map<string, number>()
  for (const line of lines) {
    const key = line.level ?? '(none)'
    counts.set(key, (counts.get(key) ?? 0) + 1)
  }
  return counts
}

/**
 * Apply the Developer tab's filters.
 *
 * An empty category set means "no category filter", not "match nothing" —
 * deselecting the last chip should show everything again rather than an empty
 * pane you have to guess your way out of.
 */
export function filterLogLines(
  lines: LogLine[],
  {
    categories,
    minLevel,
    text,
  }: { categories: Set<string>; minLevel: LogLevelName | null; text: string },
): LogLine[] {
  const needle = text.trim().toLowerCase()
  return lines.filter((line) => {
    if (categories.size > 0 && !categories.has(line.category ?? '(none)')) return false
    if (minLevel && !atLeast(line.level, minLevel)) return false
    if (needle && !line.raw.toLowerCase().includes(needle)) return false
    return true
  })
}

const SEVERITY: Record<LogLevelName, number> = {
  error: 0,
  warn: 1,
  info: 2,
  debug: 3,
  trace: 4,
}

/**
 * True when `level` is at least as severe as `floor`.
 *
 * A threshold rather than an exact match: picking "warn" to hunt a problem
 * should not hide the errors above it.
 */
export function atLeast(level: LogLevelName | null, floor: LogLevelName): boolean {
  if (!level) return false
  return SEVERITY[level] <= SEVERITY[floor]
}
