/**
 * What EventBridge accepts as a registry schema name.
 *
 * Mirrors `schema/model.rs`. The API enforces `^[a-zA-Z0-9_\.\-\@]+$` and
 * rejects anything else with a `BadRequestException` whose message names the
 * pattern but not the offending character. Sources on the bus genuinely
 * contain spaces (`Atomic Forms`), so a schema inferred from real traffic can
 * carry a name that can never be registered — which is worth catching in the
 * editor rather than after the write.
 */

const ALLOWED = /^[a-zA-Z0-9_.\-@]$/

export function isValidSchemaNameChar(char: string): boolean {
  return ALLOWED.test(char)
}

/** The distinct characters EventBridge would reject, in order of appearance. */
export function invalidNameChars(name: string): string[] {
  const seen: string[] = []
  for (const char of name) {
    if (!isValidSchemaNameChar(char) && !seen.includes(char)) seen.push(char)
  }
  return seen
}

/**
 * A registrable name, with rejected characters replaced by `-`.
 *
 * Only the name is rewritten. The document keeps the true
 * `x-amazon-events-source`, so the schema still describes events as they are
 * actually published — the registry name is an identifier, not the contract.
 */
export function sanitizeSchemaName(name: string): string {
  let out = [...name].map((c) => (isValidSchemaNameChar(c) ? c : '-')).join('')
  while (out.includes('--')) out = out.replace(/--/g, '-')
  return out
}

/** Offending characters as prose, e.g. `space, '/'`. */
export function describeInvalidChars(name: string): string {
  return invalidNameChars(name)
    .map((c) => (c === ' ' ? 'space' : `'${c}'`))
    .join(', ')
}
