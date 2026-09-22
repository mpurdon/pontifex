/// <reference types="node" />
// Read from disk rather than `?raw`-imported: the Tailwind plugin transforms
// CSS on import, and the source is what this test is about.
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { MONACO_TOKENS } from './monaco'
import { DEFAULT_THEME, SYSTEM_THEME, THEMES, resolveTheme } from './themes'

const here = new URL('.', import.meta.url)
const read = (file: string) => readFileSync(new URL(file, here), 'utf8')
const indexCss = read('../index.css')

/** Variables `index.css` sets itself or reads with an inline value. */
const LOCAL = new Set(['--glow-tint', '--fill', '--frac', '--thumb-w'])

/**
 * Every variable `index.css` reads without defining, plus what the editor
 * reads: the full set a theme must value. A theme file that leaves one out
 * would render that token as nothing at all — a transparent border or an
 * invisible label — which is easy to miss by eye in a screen that happens not
 * to use it.
 */
function requiredTokens(): string[] {
  const read = [...indexCss.matchAll(/var\((--[\w-]+)/g)].map((m) => m[1])
  return [...new Set([...read, ...MONACO_TOKENS])].filter((t) => !LOCAL.has(t))
}

function themeBlock(id: string): string {
  const css = read(`./${id}.css`)
  const block = css.match(new RegExp(`\\[data-theme='${id}'\\]\\s*\\{([\\s\\S]*?)\\n\\}`))?.[1]
  if (!block) throw new Error(`no [data-theme='${id}'] block in ${id}.css`)
  return block
}

describe('theme files', () => {
  const tokens = requiredTokens()

  it('has something to check', () => {
    expect(tokens).toContain('--surface-0')
    expect(tokens).toContain('--editor-bg')
    expect(tokens).toContain('--canvas-texture')
    expect(tokens).toContain('--panel-frame')
    expect(tokens.length).toBeGreaterThan(40)
  })

  it.each(THEMES.map((t) => t.id))('%s values every token', (id) => {
    const block = themeBlock(id)
    const missing = tokens.filter((token) => !new RegExp(`${token}:`).test(block))
    expect(missing).toEqual([])
  })

  it('is imported by index.css', () => {
    for (const theme of THEMES) {
      expect(indexCss).toContain(`@import './theme/${theme.id}.css'`)
    }
  })

  it('declares the colour scheme the registry says', () => {
    // The CSS drives native controls; the registry drives Monaco's base.
    for (const theme of THEMES) {
      expect(themeBlock(theme.id)).toContain(`color-scheme: ${theme.scheme}`)
    }
  })

  it('writes what the editor reads in hex', () => {
    for (const theme of THEMES) {
      const block = themeBlock(theme.id)
      for (const token of MONACO_TOKENS) {
        const value = block.match(new RegExp(`${token}:\\s*([^;]+);`))?.[1]
        expect(value, `${theme.id} ${token}`).toMatch(/^#[0-9a-f]{6}([0-9a-f]{2})?$/i)
      }
    }
  })
})

describe('resolveTheme', () => {
  it('returns the named theme', () => {
    expect(resolveTheme('pontifex', false).id).toBe('pontifex')
  })

  it('follows the OS for system', () => {
    expect(resolveTheme(SYSTEM_THEME, true).id).toBe('dark')
    expect(resolveTheme(SYSTEM_THEME, false).id).toBe('light')
  })

  it('falls back to the default for an unknown or missing id', () => {
    // A theme removed from the registry must not strand anyone who chose it.
    expect(resolveTheme('sepia', true).id).toBe(DEFAULT_THEME)
    expect(resolveTheme(undefined, true).id).toBe(DEFAULT_THEME)
  })
})
