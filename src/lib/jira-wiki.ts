/**
 * Jira wiki markup, as far as a ticket body uses it.
 *
 * Not a general parser: the bodies here are written by `jira::ticket`, so the
 * vocabulary is known — headings, bullets, a numbered list, monospace, bold,
 * links, a code block, a quote, a rule. That is what the preview has to
 * render, and a general implementation of a markup this old would be a much
 * larger thing that answered the same question no better.
 *
 * Anything outside the vocabulary is left as text rather than guessed at,
 * which is also the honest answer for a body someone has edited by hand.
 */

export type Inline =
  | { kind: 'text'; text: string }
  | { kind: 'strong'; text: string }
  | { kind: 'code'; text: string }
  | { kind: 'link'; text: string; href: string }

export type Block =
  | { kind: 'heading'; level: number; spans: Inline[] }
  | { kind: 'paragraph'; spans: Inline[] }
  | { kind: 'list'; ordered: boolean; items: Inline[][] }
  | { kind: 'code'; text: string }
  | { kind: 'quote'; spans: Inline[] }
  | { kind: 'rule' }

/**
 * `{{monospace}}`, `*bold*`, `[label|url]` and `[url]`, in one pass.
 *
 * Ordered so the greedier forms cannot swallow the others: a bare link is
 * only a bare link once `[label|url]` has had its chance.
 */
const INLINE =
  /\{\{(.+?)\}\}|\*([^*\n]+)\*|\[([^\]|\n]+)\|([^\]\n]+)\]|\[((?:https?|mailto):[^\]\n]+)\]/g

export function parseInline(text: string): Inline[] {
  const spans: Inline[] = []
  let last = 0
  for (const match of text.matchAll(INLINE)) {
    const at = match.index
    if (at > last) spans.push({ kind: 'text', text: text.slice(last, at) })
    const [, mono, bold, label, href, bare] = match
    if (mono !== undefined) spans.push({ kind: 'code', text: mono })
    else if (bold !== undefined) spans.push({ kind: 'strong', text: bold })
    else if (label !== undefined) spans.push({ kind: 'link', text: label, href })
    else spans.push({ kind: 'link', text: bare, href: bare })
    last = at + match[0].length
  }
  if (last < text.length) spans.push({ kind: 'text', text: text.slice(last) })
  return spans
}

const HEADING = /^h([1-6])\.\s+(.*)$/
const BULLET = /^\*+\s+(.*)$/
const NUMBER = /^#+\s+(.*)$/

export function parseWiki(body: string): Block[] {
  const lines = body.replace(/\r\n/g, '\n').split('\n')
  const blocks: Block[] = []
  /** Plain lines waiting to become a paragraph. */
  let pending: string[] = []

  const flush = () => {
    const text = pending.join('\n').trim()
    pending = []
    if (text) blocks.push({ kind: 'paragraph', spans: parseInline(text) })
  }

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]

    // A code block runs to its closing marker, or to the end of the body if
    // the author is still typing one.
    if (line.trim() === '{code}' || line.trim().startsWith('{code:')) {
      flush()
      const held: string[] = []
      i++
      while (i < lines.length && lines[i].trim() !== '{code}') held.push(lines[i++])
      blocks.push({ kind: 'code', text: held.join('\n') })
      continue
    }

    const quote = line.match(/^\{quote\}(.*)\{quote\}$/)
    if (quote) {
      flush()
      blocks.push({ kind: 'quote', spans: parseInline(quote[1]) })
      continue
    }

    if (/^-{4,}$/.test(line.trim())) {
      flush()
      blocks.push({ kind: 'rule' })
      continue
    }

    const heading = line.match(HEADING)
    if (heading) {
      flush()
      blocks.push({
        kind: 'heading',
        level: Number(heading[1]),
        spans: parseInline(heading[2]),
      })
      continue
    }

    const bullet = line.match(BULLET)
    const numbered = line.match(NUMBER)
    if (bullet || numbered) {
      flush()
      const ordered = !bullet
      const items: Inline[][] = []
      // Consecutive markers of the same kind are one list.
      while (i < lines.length) {
        const item = lines[i].match(ordered ? NUMBER : BULLET)
        if (!item) break
        items.push(parseInline(item[1]))
        i++
      }
      i--
      blocks.push({ kind: 'list', ordered, items })
      continue
    }

    if (line.trim() === '') flush()
    else pending.push(line)
  }

  flush()
  return blocks
}
