import {
  AlertTriangle,
  CheckCircle2,
  HelpCircle,
  MinusCircle,
  XCircle,
} from 'lucide-react'
import type { ReportStatus } from '@/lib/types'

/**
 * What the report can say about an event type.
 *
 * `missing` is not a `ReportStatus` — a schema that does not exist cannot be
 * graded — but it is the most serious thing the report can find, so it belongs
 * in the same vocabulary as the rest for filtering and charting.
 */
export type FilterStatus = ReportStatus | 'missing'

export interface StatusStyle {
  label: string
  tone: 'danger' | 'violet' | 'orange' | 'warn' | 'ok' | 'neutral'
  /** Text colour for a table cell. */
  text: string
  /** Solid fill for chart segments, distinct per status even where the tone
   *  repeats — three different reds are unreadable in a stacked bar. */
  bar: string
  /** Legend swatch — same hue as `bar`, sized for a dot. */
  dot: string
  icon: typeof XCircle
  /** Why this status matters, for the chip tooltip. */
  hint: string
}

export const STATUS: Record<FilterStatus, StatusStyle> = {
  missing: {
    label: 'missing',
    tone: 'danger',
    text: 'text-danger',
    bar: 'bg-danger',
    dot: 'bg-danger',
    icon: HelpCircle,
    hint: 'On the bus with no schema at all — published but undocumented',
  },
  failing: {
    label: 'failing',
    tone: 'violet',
    text: 'text-violet',
    bar: 'bg-violet',
    dot: 'bg-violet',
    icon: XCircle,
    hint: 'Real events do not validate against the schema',
  },
  error: {
    label: 'error',
    tone: 'orange',
    text: 'text-orange',
    bar: 'bg-orange',
    dot: 'bg-orange',
    icon: AlertTriangle,
    hint: 'The schema itself could not be read or graded',
  },
  drifting: {
    label: 'drifting',
    tone: 'warn',
    text: 'text-warn',
    bar: 'bg-warn',
    dot: 'bg-warn',
    icon: AlertTriangle,
    hint: 'Events validate, but carry fields or values the schema does not describe',
  },
  ok: {
    label: 'ok',
    tone: 'ok',
    text: 'text-ok',
    bar: 'bg-ok',
    dot: 'bg-ok',
    icon: CheckCircle2,
    hint: 'Every sampled event validated, with no drift',
  },
  noTraffic: {
    label: 'no traffic',
    tone: 'neutral',
    text: 'text-ink-faint',
    bar: 'bg-ink-faint/40',
    dot: 'bg-ink-faint/50',
    icon: MinusCircle,
    hint: 'Registered, but nothing matching it appeared in this sample',
  },
}

/** Worst first — the order chips, charts and the legend all read in. */
export const STATUS_ORDER: FilterStatus[] = [
  'missing',
  'failing',
  'error',
  'drifting',
  'ok',
  'noTraffic',
]
