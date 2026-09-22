import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { useSettings } from '@/app/settings-context'
import { DEFAULT_THEME, SYSTEM_THEME, resolveTheme, type ThemeDefinition } from './themes'

export type { ThemeDefinition }

/**
 * Where the last chosen theme is remembered between launches.
 *
 * Settings arrive over IPC after the first paint, so without this a light
 * theme user would see a flash of dark chrome on every launch. The setting is
 * still the source of truth; this is only a hint for the first frames.
 */
const CACHE_KEY = 'pontifex.theme'

const DARK_QUERY = '(prefers-color-scheme: dark)'

/** Stamp the theme on the document so the CSS tokens resolve. */
function applyTheme(theme: ThemeDefinition): void {
  const root = document.documentElement
  if (root.dataset.theme !== theme.id) root.dataset.theme = theme.id
}

function systemPrefersDark(): boolean {
  return typeof matchMedia === 'function' && matchMedia(DARK_QUERY).matches
}

function readCachedPreference(): string | null {
  try {
    return localStorage.getItem(CACHE_KEY)
  } catch {
    // Storage can be unavailable; the default theme is fine until settings load.
    return null
  }
}

/**
 * Apply the remembered theme before React renders.
 *
 * Called once from `main.tsx`. Always stamps something — the default when
 * nothing is remembered — so the CSS never has to guess.
 */
export function applyCachedTheme(): void {
  applyTheme(resolveTheme(readCachedPreference(), systemPrefersDark()))
}

interface ThemeContextValue {
  /** The stored choice: a theme id, or `system`. */
  preference: string
  /** The theme actually drawn — `system` resolved to light or dark. */
  theme: ThemeDefinition
  /** What `system` would draw right now, whether or not it is chosen. */
  systemTheme: ThemeDefinition
  setPreference: (preference: string) => void
}

const ThemeContext = createContext<ThemeContextValue | null>(null)

export function ThemeProvider({ children }: { children: ReactNode }) {
  const { settings, setTheme } = useSettings()
  // Until settings load, stay on what `applyCachedTheme` chose rather than
  // snapping to the default and back.
  const preference = settings?.theme ?? readCachedPreference() ?? DEFAULT_THEME

  // Tracked whether or not `system` is chosen, so the picker can say what it
  // would give you before you pick it.
  const [prefersDark, setPrefersDark] = useState(systemPrefersDark)
  useEffect(() => {
    const query = matchMedia(DARK_QUERY)
    const onChange = (e: MediaQueryListEvent) => setPrefersDark(e.matches)
    query.addEventListener('change', onChange)
    return () => query.removeEventListener('change', onChange)
  }, [])

  const theme = useMemo(
    () => resolveTheme(preference, prefersDark),
    [preference, prefersDark],
  )
  const systemTheme = useMemo(
    () => resolveTheme(SYSTEM_THEME, prefersDark),
    [prefersDark],
  )

  useEffect(() => applyTheme(theme), [theme])

  // Written only when the preference actually moves: on mount it is what
  // `applyCachedTheme` just read, and an OS light/dark flip does not change it.
  const cached = useRef(readCachedPreference())
  useEffect(() => {
    if (cached.current === preference) return
    cached.current = preference
    try {
      localStorage.setItem(CACHE_KEY, preference)
    } catch {
      // Not remembering is harmless: the next launch reads the setting.
    }
  }, [preference])

  const value = useMemo<ThemeContextValue>(
    () => ({ preference, theme, systemTheme, setPreference: setTheme }),
    [preference, theme, systemTheme, setTheme],
  )

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>
}

export function useTheme(): ThemeContextValue {
  const context = useContext(ThemeContext)
  if (!context) {
    throw new Error('useTheme must be used inside a ThemeProvider')
  }
  return context
}
