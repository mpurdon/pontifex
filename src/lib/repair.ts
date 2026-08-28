import type { Issue, RealityCheckResult, Repair } from './types'

/**
 * What a repair will do, in the words of the row it sits on.
 *
 * The button says the edit rather than "Fix", because `Redeclare as
 * string | null` is a claim you can disagree with before clicking and "Fix" is
 * not. Written from the repair's own operands, so it stays honest when a model
 * chose the repair instead of the planner.
 */
export function repairLabel(repair: Repair): string {
  switch (repair.kind) {
    case 'widenType':
      return `Redeclare as ${repair.types.join(' | ')}`
    case 'extendEnum':
      return repair.values.length === 1
        ? 'Allow this value'
        : `Allow ${repair.values.length} values`
    case 'dropRequired':
      return 'Make optional'
    case 'declareField':
      return `Declare as ${repair.types.join(' | ')}`
  }
}

/** How many observed values are worth sending as evidence. */
const MAX_EXAMPLES = 10

/**
 * Real values seen at an issue's path, as evidence for a model.
 *
 * Taken from the drift report rather than the summary line: the summary rounds
 * ("7% of events"), and what a repair decision turns on is the values
 * themselves. Enum drift comes first because for that kind the offending values
 * *are* the finding, and one example of it would be one value out of several.
 */
export function examplesFor(
  result: RealityCheckResult | null,
  issue: Issue,
): unknown[] {
  if (!result) return []

  const seen: unknown[] = []
  const push = (value: unknown) => {
    if (value !== undefined && value !== null && seen.length < MAX_EXAMPLES) {
      seen.push(value)
    }
  }

  for (const entry of result.drift.enumDrift) {
    if (entry.path === issue.path) entry.unexpected.forEach(push)
  }
  for (const entry of result.drift.typeMismatches) {
    if (entry.path === issue.path) push(entry.example)
  }
  for (const entry of [...result.drift.undeclared, ...result.drift.missingRequired]) {
    if (entry.path === issue.path) push(entry.example)
  }
  push(issue.example)

  return seen
}
