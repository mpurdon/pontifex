import { describe, expect, it } from 'vitest'
import {
  atLeast,
  categoryCounts,
  filterLogLines,
  parseLogLine,
  parseLogLines,
} from './log-line'

const LINE = '[2026-07-31T20:15:00.123456789Z][INFO][ipc] list_schemas(envId) ok — 812ms'

describe('parseLogLine', () => {
  it('splits timestamp, level, category and message', () => {
    const parsed = parseLogLine(LINE)
    expect(parsed.timestamp).toBe('2026-07-31T20:15:00.123456789Z')
    expect(parsed.level).toBe('info')
    expect(parsed.category).toBe('ipc')
    expect(parsed.message).toBe('list_schemas(envId) ok — 812ms')
  })

  it('keeps an unparseable line rather than dropping it', () => {
    // Panics and pre-logger output have no prefix; losing them would hide the
    // exact failures this screen exists to show.
    const parsed = parseLogLine('thread panicked at src/lib.rs:12')
    expect(parsed.category).toBeNull()
    expect(parsed.level).toBeNull()
    expect(parsed.message).toBe('thread panicked at src/lib.rs:12')
    expect(parsed.raw).toBe('thread panicked at src/lib.rs:12')
  })

  it('handles a message containing brackets', () => {
    const parsed = parseLogLine('[t][WARN][aws] profile [default] not found')
    expect(parsed.category).toBe('aws')
    expect(parsed.message).toBe('profile [default] not found')
  })

  it('handles an empty message', () => {
    const parsed = parseLogLine('[t][INFO][app]')
    expect(parsed.category).toBe('app')
    expect(parsed.message).toBe('')
  })

  it('recognises every level the backend emits', () => {
    for (const [name, expected] of [
      ['ERROR', 'error'],
      ['WARN', 'warn'],
      ['INFO', 'info'],
      ['DEBUG', 'debug'],
      ['TRACE', 'trace'],
    ] as const) {
      expect(parseLogLine(`[t][${name}][app] x`).level).toBe(expected)
    }
  })
})

describe('categoryCounts', () => {
  it('counts by category, bucketing unparsed lines', () => {
    const counts = categoryCounts(
      parseLogLines([
        '[t][INFO][ipc] a',
        '[t][INFO][ipc] b',
        '[t][WARN][aws] c',
        'unparseable',
      ]),
    )
    expect(counts.get('ipc')).toBe(2)
    expect(counts.get('aws')).toBe(1)
    expect(counts.get('(none)')).toBe(1)
  })
})

describe('filterLogLines', () => {
  const lines = parseLogLines([
    '[t][ERROR][aws] credentials failed',
    '[t][WARN][cache] evicted 12 types',
    '[t][INFO][ipc] list_schemas ok',
    '[t][DEBUG][ipc] describe_schema ok',
  ])

  it('treats no selected categories as no filter', () => {
    // Deselecting the last chip must not leave an empty pane.
    const out = filterLogLines(lines, {
      categories: new Set(),
      minLevel: null,
      text: '',
    })
    expect(out).toHaveLength(4)
  })

  it('filters to the selected categories', () => {
    const out = filterLogLines(lines, {
      categories: new Set(['ipc']),
      minLevel: null,
      text: '',
    })
    expect(out.map((l) => l.category)).toEqual(['ipc', 'ipc'])
  })

  it('combines several selected categories', () => {
    const out = filterLogLines(lines, {
      categories: new Set(['aws', 'cache']),
      minLevel: null,
      text: '',
    })
    expect(out).toHaveLength(2)
  })

  it('treats level as a floor, not an exact match', () => {
    // Choosing "warn" while hunting a problem must still show errors.
    const out = filterLogLines(lines, {
      categories: new Set(),
      minLevel: 'warn',
      text: '',
    })
    expect(out.map((l) => l.level)).toEqual(['error', 'warn'])
  })

  it('matches text against the whole line, not just the message', () => {
    const out = filterLogLines(lines, {
      categories: new Set(),
      minLevel: null,
      text: 'CACHE',
    })
    expect(out).toHaveLength(1)
  })

  it('applies category, level and text together', () => {
    const out = filterLogLines(lines, {
      categories: new Set(['ipc']),
      minLevel: 'info',
      text: 'list',
    })
    expect(out).toHaveLength(1)
    expect(out[0].message).toBe('list_schemas ok')
  })
})

describe('atLeast', () => {
  it('orders levels by severity', () => {
    expect(atLeast('error', 'warn')).toBe(true)
    expect(atLeast('warn', 'warn')).toBe(true)
    expect(atLeast('info', 'warn')).toBe(false)
    expect(atLeast('trace', 'debug')).toBe(false)
    expect(atLeast('debug', 'trace')).toBe(true)
  })

  it('excludes lines with no recognised level when a floor is set', () => {
    expect(atLeast(null, 'error')).toBe(false)
  })
})
