/**
 * Display formatting shared across views.
 *
 * These started life duplicated in the report page and the reality panel, and
 * drifted: the same cached sample rendered as `2h` in one and `120m` in the
 * other. Anything that turns a number into words for the user belongs here.
 */

/** Sampling windows offered wherever events are fetched by time range. */
export const WINDOWS = [
  { label: 'Last 1h', minutes: 60 },
  { label: 'Last 6h', minutes: 360 },
  { label: 'Last 24h', minutes: 1440 },
  { label: 'Last 7d', minutes: 10080 },
  { label: 'Last 30d', minutes: 43200 },
] as const

/** Compact age, e.g. `40s`, `3m`, `2h`, `5d`. */
export function formatAge(ms: number): string {
  const seconds = Math.round(ms / 1000)
  if (seconds < 60) return `${seconds}s`
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes}m`
  const hours = Math.round(minutes / 60)
  return hours < 24 ? `${hours}h` : `${Math.round(hours / 24)}d`
}

/** A clock time with milliseconds, `HH:MM:SS.mmm`, for event rows. */
export function formatTime(ms: number | null): string {
  if (ms === null) return '—'
  const date = new Date(ms)
  return `${date.toLocaleTimeString([], { hour12: false })}.${String(date.getMilliseconds()).padStart(3, '0')}`
}

/** A sampling window as a span, e.g. `24h` or `7d`. */
export function formatWindow(minutes: number): string {
  return minutes >= 1440 ? `${Math.round(minutes / 1440)}d` : `${Math.round(minutes / 60)}h`
}

/** Byte count at one decimal place, e.g. `1.4 MB`. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

/**
 * Pretty-print for display; never throws on odd input.
 *
 * Lives here rather than beside the editor because every schema screen needs
 * it — importing it from the editor module dragged all of Monaco into the
 * startup path even on screens that never open one.
 */
export function stringify(value: unknown): string {
  try {
    return JSON.stringify(value, null, 2)
  } catch {
    return String(value)
  }
}

/**
 * Scan time budgets, in seconds.
 *
 * Unevenly spaced on purpose: the interesting range is short, and the steps
 * roughly double until the point where you would rather narrow the window than
 * wait longer. Rendered as a slider, so the spacing is in the values, not the
 * travel. Mirrors `BUDGET_CHOICES` in `aws/log_scan.rs`.
 */
export const SCAN_BUDGETS = [15, 30, 60, 90, 120] as const

/**
 * Event-count caps offered in Settings → Scanning.
 *
 * Roughly doubling, because the useful range spans two orders of magnitude and
 * a linear slider would spend most of its travel on differences that do not
 * matter. Mirrors the 100–50,000 clamp in `ScanSettings::effective_events`.
 */
export const SCAN_EVENT_CAPS = [500, 1000, 2500, 5000, 10000, 25000, 50000] as const

/** Cache disk budgets in MB, matching the 4–1024 clamp in `ScanSettings`. */
export const CACHE_BUDGETS_MB = [8, 16, 32, 64, 128, 256] as const

/** Compact count, e.g. `2.5k`. */
export function formatCount(n: number): string {
  return n >= 1000 ? `${(n / 1000).toFixed(n % 1000 === 0 ? 0 : 1)}k` : String(n)
}
