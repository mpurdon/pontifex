import { describe, expect, it } from 'vitest'
import { parseInline, parseWiki, type Block } from './jira-wiki'

/** The shapes `jira::ticket` actually emits, in the order it emits them. */
const BODY = `The producer of orders-fulfilment sends a field with the wrong type on the prd event bus.

h3. What is wrong
callAttemptCount is declared integer but 4% of events send string

h3. Evidence
* Observed in *7 of 200 sampled events* over the last 1 day(s)
* Field: {{callAttemptCount}}
* Source: {{orders-fulfilment}} / detail-type {{lead-unreached}}

h3. Publisher
* [org/billing/src/handler.ts|https://github.com/org/billing/blob/main/src/handler.ts]
* First published by someone in [https://github.com/org/billing/pull/42]

h3. Example value
{code}
"14"
{code}

h3. Validator
{quote}"14" is not of type "integer"{quote}

----
Filed from Pontifex.
`

const kinds = (blocks: Block[]) => blocks.map((b) => b.kind)

describe('parseWiki', () => {
  it('reads a ticket body as the blocks it is made of', () => {
    expect(kinds(parseWiki(BODY))).toEqual([
      'paragraph',
      'heading',
      'paragraph',
      'heading',
      'list',
      'heading',
      'list',
      'heading',
      'code',
      'heading',
      'quote',
      'rule',
      'paragraph',
    ])
  })

  it('keeps a code block verbatim, including what looks like markup', () => {
    const blocks = parseWiki('{code}\n{"a": *not bold*}\n{code}')
    expect(blocks).toEqual([{ kind: 'code', text: '{"a": *not bold*}' }])
  })

  it('closes an unfinished code block at the end rather than losing it', () => {
    // Someone is still typing. Dropping the content would read as the
    // preview eating what they wrote.
    expect(parseWiki('{code}\nhalf a block')).toEqual([
      { kind: 'code', text: 'half a block' },
    ])
  })

  it('groups consecutive markers into one list, and keeps the two kinds apart', () => {
    const blocks = parseWiki('* one\n* two\n# first\n# second')
    expect(blocks).toEqual([
      {
        kind: 'list',
        ordered: false,
        items: [
          [{ kind: 'text', text: 'one' }],
          [{ kind: 'text', text: 'two' }],
        ],
      },
      {
        kind: 'list',
        ordered: true,
        items: [
          [{ kind: 'text', text: 'first' }],
          [{ kind: 'text', text: 'second' }],
        ],
      },
    ])
  })

  it('reads the heading level', () => {
    expect(parseWiki('h2. Where this was found')).toEqual([
      { kind: 'heading', level: 2, spans: [{ kind: 'text', text: 'Where this was found' }] },
    ])
  })

  it('leaves markup it does not know as text rather than guessing', () => {
    // A body is editable, and a preview that swallowed an unknown macro
    // would be lying about what gets filed.
    expect(parseWiki('{color:red}careful{color}')).toEqual([
      { kind: 'paragraph', spans: [{ kind: 'text', text: '{color:red}careful{color}' }] },
    ])
  })
})

describe('parseInline', () => {
  it('reads monospace, bold and both spellings of a link', () => {
    expect(parseInline('a {{mono}} b *bold* c [text|https://x.test] d [https://y.test]')).toEqual([
      { kind: 'text', text: 'a ' },
      { kind: 'code', text: 'mono' },
      { kind: 'text', text: ' b ' },
      { kind: 'strong', text: 'bold' },
      { kind: 'text', text: ' c ' },
      { kind: 'link', text: 'text', href: 'https://x.test' },
      { kind: 'text', text: ' d ' },
      { kind: 'link', text: 'https://y.test', href: 'https://y.test' },
    ])
  })

  it('does not mistake a field path for emphasis', () => {
    // `callAttemptHistory[].result` inside {{…}} has a bracket pair that a
    // link pattern could reach for.
    expect(parseInline('{{callAttemptHistory[].result}}')).toEqual([
      { kind: 'code', text: 'callAttemptHistory[].result' },
    ])
  })

  it('leaves a lone asterisk alone', () => {
    expect(parseInline('2 * 3')).toEqual([{ kind: 'text', text: '2 * 3' }])
  })
})
