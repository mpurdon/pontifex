/**
 * The theme registry.
 *
 * A theme is a CSS file under `src/theme/` that values every token for its
 * `[data-theme='<id>']` selector, plus an entry here that names it for the
 * picker. Everything visual, the editor's colours included, comes from the
 * CSS; see the README under "Themes" for how to add one.
 */

export interface ThemeDefinition {
  id: string
  label: string
  /** One line for the picker card. */
  description: string
  /**
   * Light or dark, which decides Monaco's base theme. Must agree with the
   * `color-scheme` the CSS file declares; the test checks that it does.
   */
  scheme: 'light' | 'dark'
}

export const THEMES: readonly ThemeDefinition[] = [
  {
    id: 'dark',
    label: 'Dark',
    description: 'Slate surfaces with an amber accent. The original.',
    scheme: 'dark',
  },
  {
    id: 'light',
    label: 'Light',
    description: 'The same palette on paper, for a bright room.',
    scheme: 'light',
  },
  {
    id: 'pontifex',
    label: 'Pontifex',
    description: 'White ink on blueprint blue, ruled like a drafting sheet.',
    scheme: 'dark',
  },
]

export const DEFAULT_THEME = 'dark'

/**
 * What the setting can hold: a registered id, or `system` to follow the OS
 * between the light and dark themes.
 */
export const SYSTEM_THEME = 'system'

/**
 * The theme to draw for a stored preference.
 *
 * Unknown ids fall back to the default rather than throwing: a theme removed
 * from the registry must not brick the settings of anyone who had chosen it.
 */
export function resolveTheme(
  preference: string | null | undefined,
  systemPrefersDark: boolean,
): ThemeDefinition {
  const id =
    preference === SYSTEM_THEME
      ? systemPrefersDark
        ? 'dark'
        : 'light'
      : preference
  return THEMES.find((t) => t.id === id) ?? THEMES.find((t) => t.id === DEFAULT_THEME)!
}
