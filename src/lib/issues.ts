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
