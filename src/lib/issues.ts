import type { Issue, IssueKind, IssueSeverity } from './types'

/** What each kind of issue is called, in the reader's terms. */
export const KIND_LABELS: Record<IssueKind, string> = {
  wrongType: 'wrong type',
  outsideEnum: 'value not allowed',
  missingRequired: 'required field missing',
  undeclared: 'undeclared field',
  neverSeen: 'never seen',
  rejected: 'rejected',
  unregistered: 'no schema',
  concern: 'concern',
}

/** The badge tone that says how bad an issue is. */
export const SEVERITY_TONE: Record<IssueSeverity, 'danger' | 'warn' | 'neutral'> = {
  error: 'danger',
  warning: 'warn',
  info: 'neutral',
}

const SEVERITY_WORD: Record<IssueSeverity, string> = {
  error: 'error',
  warning: 'drifting',
  info: 'note',
}

/**
 * What the severity is grounded in: a word or two for the badge, and the
 * sentence behind it for the tooltip.
 *
 * "Error" and "warning" read as impact levels, and until the origin lookup
 * has found who reads a field they are not: the validator only knows whether
 * the bus rejects the events. So the badge says the thing that is actually
 * known — rejected by the schema, read by consumers, drifting unread — rather than a level
 * the reader would have to guess the meaning of.
 */
export function describeSeverity(issue: Issue): { label: string; title: string } {
  if (issue.rejects && !issue.impact) {
    return {
      label: 'rejected by schema',
      title:
        'The registered schema rejects these events. EventBridge still delivers them; the bus\u2019s validator alerts on failures rather than blocking. Who reads the field is unknown until the origin lookup has run.',
    }
  }
  const impact = issue.impact
  if (impact) {
    const readers = impact.readers.length
    const indirect = impact.indirect.length
    const found = `the ${impact.handlers} consumer file${impact.handlers === 1 ? '' : 's'} the origin lookup found`
    const rejected = issue.rejects ? ' The schema rejects these events; EventBridge delivers them regardless.' : ''
    if (readers > 0) {
      return {
        label: `read by ${readers} consumer${readers === 1 ? '' : 's'}`,
        title: `${readers} of ${found} read this field.${rejected}`,
      }
    }
    if (indirect > 0) {
      return {
        label: issue.rejects ? 'rejected by schema' : SEVERITY_WORD[issue.severity],
        title: `${indirect} of ${found} pass this field's parent along whole, so whether they read it is not visible.${rejected}`,
      }
    }
    return {
      label: issue.rejects ? 'rejected, unread' : 'no consumer reads it',
      title: `None of ${found} reads this field.${rejected}`,
    }
  }
  return {
    label: SEVERITY_WORD[issue.severity],
    title:
      issue.severity === 'info'
        ? 'Worth knowing; nothing to do on its own'
        : 'The schema and the traffic disagree, but the schema does not reject these events. Who reads the field is unknown until the origin lookup has run.',
  }
}

/**
 * An issue a person raised rather than one the validator found — about the
 * event type as a whole (`path` empty) or one field of it. Same shape as a
 * finding so it files through the same preview and dedupes the same way.
 */
export function concernIssue(input: {
  path: string
  summary: string
  detail: string
  declared?: string | null
  observed?: string | null
}): Issue {
  return {
    key: `concern:${input.path || 'event'}`,
    kind: 'concern',
    severity: 'warning',
    path: input.path,
    summary: input.summary,
    action: input.detail,
    declared: input.declared ?? null,
    observed: input.observed ?? null,
    affected: 0,
    sampled: 0,
    rejects: false,
    example: null,
    message: null,
  }
}
