import { describe, expect, it } from 'vitest'

/**
 * The confirm dialog enables its acknowledgement only once the document has
 * been scrolled through, like a terms-of-service accept button.
 *
 * The failure mode worth guarding is the opposite of the feature: a document
 * short enough not to scroll fires no scroll event, so a naive gate would stay
 * shut forever and the schema could never be saved.
 */

/** Mirrors the DOM check in `confirm-save-dialog.tsx`. */
export function scrolledToEnd(
  scrollTop: number,
  clientHeight: number,
  scrollHeight: number,
): boolean {
  return scrollTop + clientHeight >= scrollHeight - 4
}

describe('scrolledToEnd', () => {
  it('is false partway through a long document', () => {
    expect(scrolledToEnd(0, 400, 2000)).toBe(false)
    expect(scrolledToEnd(800, 400, 2000)).toBe(false)
  })

  it('is true at the bottom', () => {
    expect(scrolledToEnd(1600, 400, 2000)).toBe(true)
  })

  it('tolerates sub-pixel layout rounding', () => {
    // Fractional heights are routine; an exact comparison would leave the
    // checkbox disabled with the scrollbar visibly at the bottom.
    expect(scrolledToEnd(1597, 400, 2000)).toBe(true)
  })

  it('is true immediately when the content fits', () => {
    // No scroll event will ever fire here, so requiring one would make the
    // dialog impossible to confirm.
    expect(scrolledToEnd(0, 600, 400)).toBe(true)
    expect(scrolledToEnd(0, 400, 400)).toBe(true)
  })

  it('does not overshoot past the end', () => {
    expect(scrolledToEnd(2000, 400, 2000)).toBe(true)
  })
})
