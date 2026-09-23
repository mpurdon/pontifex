import { declaredPaths } from '@/lib/schema-model'

/**
 * Which registered schemas a watch's source and detail type reach.
 *
 * The same wildcard rule the pattern uses: `*` is any run of characters,
 * everything else literal. Blank means any.
 */
export function globMatches(pattern: string | null | undefined, text: string): boolean {
  const p = (pattern ?? '').trim()
  if (!p) return true
  if (!p.includes('*')) return p === text
  const parts = p.split('*')
  if (!text.startsWith(parts[0])) return false
  let at = parts[0].length
  for (let i = 1; i < parts.length; i++) {
    const part = parts[i]
    if (i === parts.length - 1) return part === '' || text.endsWith(part) && text.length - part.length >= at
    const found = text.indexOf(part, at)
    if (found === -1) return false
    at = found + part.length
  }
  return true
}

/** Schema names whose `source@detail-type` the filters would match. */
export function matchingSchemas(
  names: readonly string[],
  source: string | null | undefined,
  detailType: string | null | undefined,
): string[] {
  return names.filter((name) => {
    const at = name.indexOf('@')
    if (at <= 0) return false
    return globMatches(source, name.slice(0, at)) && globMatches(detailType, name.slice(at + 1))
  })
}

/** The union of every field path the given documents declare, sorted. */
export function pathsAcross(documents: readonly unknown[]): string[] {
  const all = new Set<string>()
  for (const doc of documents) for (const path of declaredPaths(doc)) all.add(path)
  return [...all].sort()
}
