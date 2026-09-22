/**
 * Monaco's colours, derived from the theme's CSS.
 *
 * Monaco takes hex strings rather than CSS variables, so the editor cannot
 * pick tokens up on its own. Rather than keep a second, hand-mirrored palette
 * in TypeScript, each theme file values the `--editor-*` and `--type-*`
 * tokens in hex and the editor reads them back through `getComputedStyle`.
 * A palette change in CSS reaches the editor, and `themes.test.ts` fails if a
 * theme leaves one of these out.
 *
 * Kept free of any Monaco import so the test (and the theme registry) can
 * use it without pulling the editor onto the startup path.
 */

/** Monaco colour key → the theme token that supplies it. */
export const MONACO_COLOR_TOKENS = {
  'editor.background': '--editor-bg',
  'editorGutter.background': '--editor-bg',
  'editor.foreground': '--editor-fg',
  'editor.lineHighlightBackground': '--editor-line',
  'editor.selectionBackground': '--editor-selection',
  'editorCursor.foreground': '--editor-fg',
  'editorLineNumber.foreground': '--editor-gutter-fg',
  'editorLineNumber.activeForeground': '--editor-gutter-fg-active',
  'editorIndentGuide.background1': '--editor-guide',
  'editorWidget.background': '--editor-widget-bg',
  'editorWidget.border': '--editor-widget-edge',
  'scrollbarSlider.background': '--editor-scrollbar',
  'scrollbarSlider.hoverBackground': '--editor-scrollbar-hover',
  'diffEditor.insertedTextBackground': '--diff-added',
  'diffEditor.removedTextBackground': '--diff-removed',
} as const

/** JSON token class → the type colour the schema tree uses for the same thing. */
export const MONACO_RULE_TOKENS = {
  'string.key.json': '--type-object',
  'string.value.json': '--type-string',
  number: '--type-number',
  'keyword.json': '--type-boolean',
  delimiter: '--editor-fg',
} as const

/** Every token the editor reads, for the completeness test. */
export const MONACO_TOKENS: readonly string[] = [
  ...new Set([
    ...Object.values(MONACO_COLOR_TOKENS),
    ...Object.values(MONACO_RULE_TOKENS),
  ]),
]

/** The shape Monaco's `defineTheme` takes, spelled out to avoid importing it. */
export interface MonacoThemeData {
  base: 'vs' | 'vs-dark'
  inherit: boolean
  rules: { token: string; foreground: string }[]
  colors: Record<string, string>
}

/**
 * Assemble a Monaco theme from a token reader.
 *
 * `read` returns a token's value as written in the theme CSS — a hex string,
 * possibly with alpha. Monaco rule foregrounds are written without `#`.
 */
export function buildMonacoTheme(
  scheme: 'light' | 'dark',
  read: (token: string) => string,
): MonacoThemeData {
  return {
    base: scheme === 'light' ? 'vs' : 'vs-dark',
    inherit: true,
    rules: Object.entries(MONACO_RULE_TOKENS).map(([token, source]) => ({
      token,
      foreground: read(source).replace(/^#/, ''),
    })),
    colors: Object.fromEntries(
      Object.entries(MONACO_COLOR_TOKENS).map(([key, source]) => [key, read(source)]),
    ),
  }
}
