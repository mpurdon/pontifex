import type { IssueKind, IssueSeverity } from './types'

/** What each kind of issue is called, in the reader's terms. */
export const KIND_LABELS: Record<IssueKind, string> = {
  wrongType: 'wrong type',
  outsideEnum: 'value not allowed',
  missingRequired: 'required field missing',
  undeclared: 'undeclared field',
  neverSeen: 'never seen',
  rejected: 'rejected',
  unregistered: 'no schema',
}

/** The badge tone that says how bad an issue is. */
export const SEVERITY_TONE: Record<IssueSeverity, 'danger' | 'warn' | 'neutral'> = {
  error: 'danger',
  warning: 'warn',
  info: 'neutral',
}
