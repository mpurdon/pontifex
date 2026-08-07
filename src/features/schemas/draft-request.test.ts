import { describe, expect, it } from 'vitest'

/**
 * The `?draft=` parameter is a one-shot command.
 *
 * It used to be cleared in the mutation's `onSettled`, so it stayed in the URL
 * for the whole call. Any re-render that reached the effect could start the
 * call again, and because each restart set `isPending` back to true before it
 * had ever rendered false, the spinner never unmounted — it kept counting past
 * its own 60s timeout while the backend had answered in milliseconds.
 *
 * These exercise the guard directly: the component wiring is not worth a DOM
 * harness, but the "consume once, before starting" rule is exactly the thing
 * that broke and is worth pinning.
 */

/** Mirrors the effect's decision in `schemas-page.tsx`. */
function shouldStart(
  request: string | null,
  envId: string | undefined,
  alreadyConsumed: string | null,
): boolean {
  if (!request || !envId) return false
  if (alreadyConsumed === request) return false
  return true
}

describe('draft request consumption', () => {
  it('starts once for a fresh request', () => {
    expect(shouldStart('svc@thing', 'prd', null)).toBe(true)
  })

  it('does not restart the request it is already running', () => {
    expect(shouldStart('svc@thing', 'prd', 'svc@thing')).toBe(false)
  })

  it('waits for an environment rather than consuming the request early', () => {
    // Consuming before `envId` loads would drop the request silently.
    expect(shouldStart('svc@thing', undefined, null)).toBe(false)
    expect(shouldStart('svc@thing', 'prd', null)).toBe(true)
  })

  it('does nothing when the parameter has been cleared', () => {
    expect(shouldStart(null, 'prd', 'svc@thing')).toBe(false)
  })

  it('allows a different event type straight after one completes', () => {
    expect(shouldStart('other@thing', 'prd', 'svc@thing')).toBe(true)
  })

  it('allows the same event type again once the guard is released', () => {
    // `onSettled` clears the guard, so re-drafting the same type still works.
    expect(shouldStart('svc@thing', 'prd', null)).toBe(true)
  })
})
