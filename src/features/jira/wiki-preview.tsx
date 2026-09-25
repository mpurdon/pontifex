import { useMemo } from 'react'
import { parseWiki, type Block, type Inline } from '@/lib/jira-wiki'
import { OpenLink, cn } from '@/components/ui'

/**
 * The body as Jira will draw it.
 *
 * Not a faithful copy of Jira's own stylesheet — it is the app's type and
 * colours — but the structure is the structure: which lines are headings,
 * what is a list, where the code blocks are. That is what someone is checking
 * before they put this in another team's backlog.
 */
export function WikiPreview({ text, className }: { text: string; className?: string }) {
  const blocks = useMemo(() => parseWiki(text), [text])

  if (blocks.length === 0) {
    return (
      <div
        className={cn(
          'rounded-md border border-edge bg-surface-0 p-2 text-[11px] text-ink-faint',
          className,
        )}
      >
        Nothing to preview.
      </div>
    )
  }

  return (
    // Blocks are spaced by margin, not by a flex gap. In a flex column with a
    // height, every child shrinks to fit — which silently squashed the code
    // blocks: a paragraph that shrinks still paints, but a `<pre>` scrolls its
    // own overflow, so the example value was clipped to a sliver of one line.
    <div
      className={cn(
        'space-y-2 rounded-md border border-edge bg-surface-0 p-2 text-[11px] leading-relaxed text-ink-muted',
        className,
      )}
    >
      {blocks.map((block, i) => (
        <Rendered key={i} block={block} />
      ))}
    </div>
  )
}

function Rendered({ block }: { block: Block }) {
  switch (block.kind) {
    case 'heading':
      return (
        <p
          className={cn(
            'font-semibold text-ink',
            block.level <= 2 ? 'mt-1 text-xs' : 'text-[11px]',
          )}
        >
          <Spans spans={block.spans} />
        </p>
      )
    case 'paragraph':
      return (
        <p className="whitespace-pre-wrap">
          <Spans spans={block.spans} />
        </p>
      )
    case 'list':
      return (
        <ol
          className={cn('space-y-0.5 pl-4', block.ordered ? 'list-decimal' : 'list-disc')}
        >
          {block.items.map((item, i) => (
            <li key={i}>
              <Spans spans={item} />
            </li>
          ))}
        </ol>
      )
    case 'code':
      return (
        <pre className="overflow-x-auto rounded border border-edge bg-surface-1 p-2 font-mono text-[11px] leading-snug text-ink">
          {block.text}
        </pre>
      )
    case 'quote':
      return (
        <p className="border-l-2 border-edge pl-2 italic">
          <Spans spans={block.spans} />
        </p>
      )
    case 'rule':
      return <hr className="border-edge" />
  }
}

function Spans({ spans }: { spans: Inline[] }) {
  return (
    <>
      {spans.map((span, i) => {
        switch (span.kind) {
          case 'strong':
            return (
              <strong key={i} className="font-semibold text-ink">
                {span.text}
              </strong>
            )
          case 'code':
            return (
              <span key={i} className="rounded bg-surface-2 px-1 font-mono text-ink">
                {span.text}
              </span>
            )
          case 'link':
            return (
              <OpenLink key={i} url={span.href} className="text-accent underline">
                {span.text}
              </OpenLink>
            )
          default:
            return <span key={i}>{span.text}</span>
        }
      })}
    </>
  )
}
