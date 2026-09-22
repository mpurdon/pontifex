import { getCurrentWebview } from '@tauri-apps/api/webview'

/**
 * Text size, as a webview zoom factor.
 *
 * The webview does not zoom on its own — there is no browser chrome to own
 * the shortcut — so the app does it: ⌘/Ctrl with `=`, `-` and `0`, on the
 * ladder browsers use, applied through Tauri's `setZoom`. The level is a
 * setting, saved on the light path and reapplied on launch, and cached in
 * localStorage so the first frames are already at size.
 */
export const ZOOM_STEPS = [0.67, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2] as const

export const DEFAULT_ZOOM = 1

const CACHE_KEY = 'pontifex.zoom'

/** The nearest step to a stored value, so an odd number cannot strand the ladder. */
function nearestStep(zoom: number): number {
  return ZOOM_STEPS.reduce((best, step) =>
    Math.abs(step - zoom) < Math.abs(best - zoom) ? step : best,
  )
}

/** The step above or below `zoom`, clamped at the ends of the ladder. */
export function stepZoom(zoom: number, direction: 1 | -1): number {
  const index = ZOOM_STEPS.indexOf(nearestStep(zoom) as (typeof ZOOM_STEPS)[number])
  return ZOOM_STEPS[Math.min(Math.max(index + direction, 0), ZOOM_STEPS.length - 1)]
}

/** The zoom a keyboard shortcut asks for, or null if the key is not one. */
export function zoomForKey(
  event: Pick<KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey' | 'altKey'>,
  current: number,
): number | null {
  if (!(event.metaKey || event.ctrlKey) || event.altKey) return null
  switch (event.key) {
    case '=':
    case '+':
      return stepZoom(current, 1)
    case '-':
    case '_':
      return stepZoom(current, -1)
    case '0':
      return DEFAULT_ZOOM
    default:
      return null
  }
}

export function formatZoom(zoom: number): string {
  return `${Math.round(zoom * 100)}%`
}

export function readCachedZoom(): number | null {
  try {
    const raw = localStorage.getItem(CACHE_KEY)
    const zoom = raw === null ? NaN : Number(raw)
    return Number.isFinite(zoom) && zoom > 0 ? zoom : null
  } catch {
    return null
  }
}

/** Zoom the webview, and remember it for the next launch's first frames. */
export function applyZoom(zoom: number): void {
  void getCurrentWebview()
    .setZoom(zoom)
    .catch(() => {
      // Outside Tauri (a plain browser tab in development) there is no
      // webview to zoom; the shortcut simply does nothing there.
    })
  try {
    localStorage.setItem(CACHE_KEY, String(zoom))
  } catch {
    // Not remembering is harmless: the next launch reads the setting.
  }
}
