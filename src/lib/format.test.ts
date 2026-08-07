import { describe, expect, it } from 'vitest'
import {
  CACHE_BUDGETS_MB,
  SCAN_BUDGETS,
  SCAN_EVENT_CAPS,
  formatAge,
  formatBytes,
  formatCount,
  formatWindow,
} from './format'

describe('SCAN_BUDGETS', () => {
  it('matches the Rust BUDGET_CHOICES exactly', () => {
    // `aws/log_scan.rs` clamps against these; a drift would let the UI offer a
    // budget the backend silently rewrites.
    expect([...SCAN_BUDGETS]).toEqual([15, 30, 60, 90, 120])
  })

  it('is strictly ascending, so slider travel maps to increasing time', () => {
    for (let i = 1; i < SCAN_BUDGETS.length; i++) {
      expect(SCAN_BUDGETS[i]).toBeGreaterThan(SCAN_BUDGETS[i - 1])
    }
  })
})

describe('formatAge', () => {
  it('uses the largest unit that keeps the number small', () => {
    expect(formatAge(45_000)).toBe('45s')
    expect(formatAge(90_000)).toBe('2m')
    expect(formatAge(3 * 3_600_000)).toBe('3h')
    expect(formatAge(50 * 3_600_000)).toBe('2d')
  })
})

describe('formatWindow', () => {
  it('switches to days at 24h', () => {
    expect(formatWindow(60)).toBe('1h')
    expect(formatWindow(1440)).toBe('1d')
    expect(formatWindow(10080)).toBe('7d')
  })
})

describe('formatBytes', () => {
  it('scales through B, KB and MB', () => {
    expect(formatBytes(512)).toBe('512 B')
    expect(formatBytes(2048)).toBe('2.0 KB')
    expect(formatBytes(5 * 1024 * 1024)).toBe('5.0 MB')
  })
})

describe('SCAN_EVENT_CAPS', () => {
  it('spans the backend clamp without exceeding it', () => {
    // `ScanSettings::effective_events` clamps to 100..50_000; offering a value
    // outside that would be silently rewritten.
    expect(Math.min(...SCAN_EVENT_CAPS)).toBeGreaterThanOrEqual(100)
    expect(Math.max(...SCAN_EVENT_CAPS)).toBeLessThanOrEqual(50_000)
  })

  it('is strictly ascending', () => {
    for (let i = 1; i < SCAN_EVENT_CAPS.length; i++) {
      expect(SCAN_EVENT_CAPS[i]).toBeGreaterThan(SCAN_EVENT_CAPS[i - 1])
    }
  })

  it('includes the default the backend falls back to', () => {
    expect([...SCAN_EVENT_CAPS]).toContain(5000)
  })
})

describe('CACHE_BUDGETS_MB', () => {
  it('stays inside the 4..1024 MB clamp', () => {
    expect(Math.min(...CACHE_BUDGETS_MB)).toBeGreaterThanOrEqual(4)
    expect(Math.max(...CACHE_BUDGETS_MB)).toBeLessThanOrEqual(1024)
  })

  it('includes the 32 MB default', () => {
    expect([...CACHE_BUDGETS_MB]).toContain(32)
  })
})

describe('formatCount', () => {
  it('abbreviates thousands without trailing noise', () => {
    expect(formatCount(500)).toBe('500')
    expect(formatCount(5000)).toBe('5k')
    expect(formatCount(2500)).toBe('2.5k')
    expect(formatCount(50000)).toBe('50k')
  })
})
