import { describe, expect, it } from 'vitest'
import { DEFAULT_ZOOM, ZOOM_STEPS, formatZoom, stepZoom, zoomForKey } from './zoom'

const meta = (key: string) => ({ key, metaKey: true, ctrlKey: false, altKey: false })

describe('stepZoom', () => {
  it('walks the ladder', () => {
    expect(stepZoom(1, 1)).toBe(1.1)
    expect(stepZoom(1, -1)).toBe(0.9)
  })

  it('stops at the ends', () => {
    expect(stepZoom(2, 1)).toBe(2)
    expect(stepZoom(ZOOM_STEPS[0], -1)).toBe(ZOOM_STEPS[0])
  })

  it('snaps an off-ladder value before stepping', () => {
    // A hand-edited settings file must not leave the shortcut stuck.
    expect(stepZoom(1.02, 1)).toBe(1.1)
    expect(stepZoom(1.3, -1)).toBe(1.1)
  })
})

describe('zoomForKey', () => {
  it('handles the three shortcuts with either modifier', () => {
    expect(zoomForKey(meta('='), 1)).toBe(1.1)
    expect(zoomForKey(meta('+'), 1)).toBe(1.1)
    expect(zoomForKey(meta('-'), 1)).toBe(0.9)
    expect(zoomForKey(meta('0'), 1.5)).toBe(DEFAULT_ZOOM)
    expect(zoomForKey({ key: '=', metaKey: false, ctrlKey: true, altKey: false }, 1)).toBe(1.1)
  })

  it('ignores other keys and unmodified keys', () => {
    expect(zoomForKey(meta('a'), 1)).toBeNull()
    expect(zoomForKey({ key: '=', metaKey: false, ctrlKey: false, altKey: false }, 1)).toBeNull()
    // ⌥ combinations type characters on macOS; leave them to the field.
    expect(zoomForKey({ key: '=', metaKey: true, ctrlKey: false, altKey: true }, 1)).toBeNull()
  })
})

describe('formatZoom', () => {
  it('rounds to a whole percentage', () => {
    expect(formatZoom(0.67)).toBe('67%')
    expect(formatZoom(1)).toBe('100%')
  })
})
