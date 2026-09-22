import {
  ChevronDown,
  ChevronRight,
  CornerDownRight,
  History,
  Link2,
  Lock,
  Plus,
  Repeat,
} from 'lucide-react'
import type { ChangeStatus, DiffNode } from '@/lib/schema-diff'
import { summarizeHistory, type FieldHistory } from '@/lib/schema-history'
import type { SchemaNode, NodeType } from '@/lib/schema-model'
import { cn } from '@/components/ui'

/** One colour per type, so shape is readable by scanning rather than reading. */
const TYPE_STYLES: Record<NodeType, string> = {
  string: 'text-type-string bg-type-string/10',
  number: 'text-type-number bg-type-number/10',
  integer: 'text-type-number bg-type-number/10',
  boolean: 'text-type-boolean bg-type-boolean/10',
  object: 'text-type-object bg-type-object/10',
  array: 'text-type-array bg-type-array/10',
  ref: 'text-type-object bg-type-object/10',
  unknown: 'text-ink-faint bg-surface-3',
}

const TYPE_LABELS: Record<NodeType, string> = {
  string: 'str',
  number: 'num',
  integer: 'int',
  boolean: 'bool',
  object: '{ }',
  array: '[ ]',
  ref: '{ }',
  unknown: '?',
}

/**
 * One entry per pending-change kind, so the row border, the name colour and
 * the tooltip cannot drift apart — they are three views of one value.
 *
 * A left bar rather than a full-row tint: the row already carries selection,
 * error and coverage colour, and a background fill would fight all three.
 */
const CHANGE_STYLES: Record<
  ChangeStatus,
  { row: string; text: string; title: string }
> = {
  added: {
    row: 'border-l-2 border-ok bg-ok/10',
    text: 'text-ok',
    title: 'Added — will be created when you save',
  },
  removed: {
    row: 'border-l-2 border-danger bg-danger/10',
    text: 'text-danger line-through',
    title: 'Removed — will be deleted when you save',
  },
  changed: {
    row: 'border-l-2 border-warn bg-warn/10',
    text: 'text-warn',
    title: 'Modified — will be updated when you save',
  },
}

/** Glow colour per change kind, matching the row's own border. */
const GLOW_TINTS: Record<ChangeStatus, string> = {
  added: 'color-mix(in oklch, var(--ok) 35%, transparent)',
  removed: 'color-mix(in oklch, var(--danger) 35%, transparent)',
  changed: 'color-mix(in oklch, var(--warn) 35%, transparent)',
}

function TypeBadge({ node }: { node: SchemaNode }) {
  return (
    <span
      className={cn(
        'shrink-0 rounded px-1 py-px font-mono text-[10px] leading-none',
        TYPE_STYLES[node.type],
      )}
    >
      {TYPE_LABELS[node.type]}
    </span>
  )
}

export interface TreeProps {
  node: DiffNode
  selectedId: string | null
  expanded: Set<string>
  onSelect: (node: SchemaNode) => void
  onToggle: (id: string) => void
  onAddChild: (node: SchemaNode) => void
  /** Jump to a `$ref` target's own definition. */
  onFollowRef: (target: string) => void
  /** Validation error paths, keyed by JSON Pointer prefix. */
  errorPointers?: Set<string>
  /**
   * How often each declared field appeared in a sample of real events, keyed by
   * dotted path, plus the sample size. Turns the tree into a map of what
   * traffic actually contains rather than only what is declared.
   */
  coverage?: { counts: Record<string, number>; sampled: number }
  /** Dotted path of this node within the type, for coverage lookup. */
  path?: string
  /**
   * Per-field change history, keyed by node id.
   *
   * A field the schema has been edited before is worth knowing about while
   * reading it: repeated edits to one field mean the producer keeps moving.
   */
  history?: Map<string, FieldHistory>
  /** Animate changed rows on mount — used when stepping between versions. */
  glow?: boolean
}

/** How often a field has been changed, and when. */
function HistoryBadge({ history }: { history: FieldHistory }) {
  const times = history.changes.length
  return (
    <span
      className={cn(
        'shrink-0 rounded px-1 py-px font-mono text-[9px]',
        // Repeatedly edited is the case worth flagging: once is a fix, three
        // times is a field nobody has pinned down.
        times > 1 ? 'bg-warn/15 text-warn' : 'bg-surface-3 text-ink-faint',
      )}
      title={`Changed in ${times} version${times === 1 ? '' : 's'}:\n${summarizeHistory(history)}`}
    >
      <History className="mr-0.5 inline size-2" />
      {times}
    </span>
  )
}

/**
 * Renders the schema's fields.
 *
 * The root row itself is omitted — the schema name already labels the pane, so
 * showing it again would cost every field a level of indentation for nothing.
 */
export function SchemaTree({ node, ...rest }: TreeProps) {
  if (node.children.length === 0) {
    return (
      <div className="p-4 text-center text-[11px] text-ink-faint">
        <p>
          <span className="font-mono text-ink-muted">{node.name}</span> has no
          fields yet.
        </p>
        <button
          type="button"
          onClick={() => rest.onAddChild(node)}
          className="mt-1 text-accent hover:underline"
        >
          Add the first one
        </button>
      </div>
    )
  }

  return (
    <ul className="select-none py-1">
      {node.children.map((child) => (
        <TreeRow key={child.id} {...rest} node={child} path={child.name} />
      ))}
    </ul>
  )
}

/**
 * Coverage badge: how much of real traffic actually carried this field.
 *
 * A field declared but present in 3% of events is a very different thing from
 * one present in 100%, and neither is visible from the schema alone.
 */
function CoverageBadge({
  count,
  sampled,
  required,
}: {
  count: number
  sampled: number
  required: boolean
}) {
  const percent = Math.round((count / sampled) * 100)
  // Required-but-sometimes-absent is the case worth flagging loudly.
  const tone =
    count === 0
      ? 'bg-surface-3 text-ink-faint'
      : required && count < sampled
        ? 'bg-danger/15 text-danger'
        : percent === 100
          ? 'bg-ok/15 text-ok'
          : 'bg-surface-3 text-ink-muted'

  return (
    <span
      className={cn('shrink-0 rounded px-1 py-px font-mono text-[9px]', tone)}
      title={`Present in ${count} of ${sampled} sampled events${
        required && count < sampled ? ' — but declared required' : ''
      }`}
    >
      {percent}%
    </span>
  )
}

function TreeRow({ node, path, ...rest }: TreeProps) {
  // Kept as one object so the recursive call below can forward it wholesale;
  // re-listing the props by hand meant a new one silently stopped propagating
  // past the first level.
  const {
    selectedId,
    expanded,
    onSelect,
    onToggle,
    onAddChild,
    onFollowRef,
    errorPointers,
    coverage,
    history,
    glow,
  } = rest
  const change = node.change ? CHANGE_STYLES[node.change] : undefined
  const hasChildren = node.children.length > 0
  const isOpen = expanded.has(node.id)
  const isSelected = selectedId === node.id
  const canAddChild = node.type === 'object' || node.type === 'ref'
  const hasError = errorPointers?.has(node.pointer) ?? false
  // A ghost row is a field that no longer exists in the draft. It is shown so
  // the deletion is visible, but nothing about it is editable.
  const ghost = node.ghost ?? false
  const observed =
    coverage && path !== undefined ? coverage.counts[path] : undefined
  const fieldHistory = history?.get(node.id)

  return (
    <li>
      <div
        role="treeitem"
        aria-selected={isSelected}
        aria-expanded={hasChildren ? isOpen : undefined}
        tabIndex={ghost ? -1 : 0}
        onClick={() => !ghost && onSelect(node)}
        onKeyDown={(e) => {
          if (ghost) return
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault()
            onSelect(node)
          }
          if (e.key === 'ArrowRight' && hasChildren && !isOpen) onToggle(node.id)
          if (e.key === 'ArrowLeft' && hasChildren && isOpen) onToggle(node.id)
        }}
        title={
          node.changeDetail?.length
            ? `${change?.title}\n${node.changeDetail.join('\n')}`
            : change?.title
        }
        className={cn(
          'group flex h-[26px] items-center gap-1.5 pr-2 text-xs',
          ghost ? 'cursor-default' : 'cursor-pointer',
          isSelected ? 'bg-accent/15 text-accent' : 'text-ink-muted hover:bg-surface-2',
          hasError && !isSelected && 'text-danger',
          change?.row,
          // Fires once on mount; the timeline remounts the tree per version.
          glow && change && 'glow-on-change',
        )}
        style={
          {
            paddingLeft: `${(node.depth - 1) * 14 + 8}px`,
            ...(glow && node.change ? { '--glow-tint': GLOW_TINTS[node.change] } : {}),
          } as React.CSSProperties
        }
      >
        {hasChildren ? (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              onToggle(node.id)
            }}
            className="-m-1 shrink-0 p-1 text-ink-faint hover:text-ink"
            aria-label={isOpen ? 'Collapse' : 'Expand'}
          >
            {isOpen ? (
              <ChevronDown className="size-3" />
            ) : (
              <ChevronRight className="size-3" />
            )}
          </button>
        ) : (
          <span className="w-3 shrink-0" />
        )}

        <span
          className={cn(
            'truncate font-mono',
            isSelected && 'text-accent',
            // A removed row is always a ghost, and ghosts cannot be selected,
            // so one guard covers all three kinds.
            !isSelected && change?.text,
          )}
        >
          {node.name}
        </span>

        {/* Required is the single most consulted attribute; keep it adjacent
            to the name rather than in a distant column. */}
        {node.required && (
          <span className="shrink-0 text-[13px] leading-none text-danger" title="Required">
            •
          </span>
        )}

        <TypeBadge node={node} />

        {observed !== undefined && coverage && coverage.sampled > 0 && (
          <CoverageBadge
            count={observed}
            sampled={coverage.sampled}
            required={node.required}
          />
        )}

        {fieldHistory && <HistoryBadge history={fieldHistory} />}

        {node.acceptsNull && (
          <span
            className="shrink-0 rounded bg-surface-3 px-1 py-px font-mono text-[9px] leading-none text-ink-faint"
            title="Accepts null"
          >
            null
          </span>
        )}

        {/* Only the closed case is flagged: the registry leaves objects open by
            convention, so a badge on every row would be noise. */}
        {node.additionalProperties === false && (
          <span
            className="flex shrink-0 items-center gap-0.5 rounded bg-warn/15 px-1 py-px text-[9px] text-warn"
            title="Extra fields are not allowed — an event with an undeclared field fails validation"
          >
            <Lock className="size-2.5" />
            strict
          </span>
        )}

        {node.refTarget && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              onFollowRef(node.refTarget!)
            }}
            title={`Go to ${node.refTarget} — shared definition`}
            className="flex shrink-0 items-center gap-0.5 rounded bg-surface-3 px-1 py-px text-[9px] text-info hover:bg-surface-3/70"
          >
            <Link2 className="size-2.5" />
            {node.refTarget}
          </button>
        )}

        {node.cyclic && (
          <span
            className="flex shrink-0 items-center gap-0.5 text-[9px] text-warn"
            title="Recurses into itself — follow the link to keep going"
          >
            <Repeat className="size-2.5" />
            cycle
          </span>
        )}

        {node.enumValues && (
          <span className="shrink-0 truncate font-mono text-[9px] text-ink-faint">
            {node.enumValues.slice(0, 3).map(String).join(' | ')}
            {node.enumValues.length > 3 ? ' …' : ''}
          </span>
        )}

        {node.description && (
          <span className="min-w-0 flex-1 truncate text-[10px] text-ink-faint">
            {node.description}
          </span>
        )}

        {node.extraKeywords.length > 0 && (
          <span
            className="ml-auto shrink-0 rounded bg-warn/15 px-1 py-px text-[9px] text-warn"
            title={`Also has: ${node.extraKeywords.join(', ')} — edit these in the JSON view`}
          >
            +{node.extraKeywords.length}
          </span>
        )}

        {canAddChild && (
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation()
              onAddChild(node)
            }}
            title="Add a field"
            className={cn(
              'shrink-0 rounded p-0.5 text-ink-faint opacity-0 hover:bg-surface-3 hover:text-ink',
              'group-hover:opacity-100 focus:opacity-100',
              !node.extraKeywords.length && 'ml-auto',
            )}
          >
            <Plus className="size-3" />
          </button>
        )}
      </div>

      {hasChildren && isOpen && (
        <ul>
          {node.children.map((child) => (
            <TreeRow
              key={child.id}
              {...rest}
              node={child}
              // Array elements collapse onto `parent[]`, matching how the
              // reality check flattens observed payloads.
              path={
                path === undefined
                  ? child.name
                  : node.type === 'array'
                    ? `${path}[]`
                    : `${path}.${child.name}`
              }
            />
          ))}
        </ul>
      )}

      {/* An expanded-but-empty object is otherwise indistinguishable from a
          leaf, which makes "where do I add the first field" unclear. */}
      {!hasChildren && (node.type === 'object' || node.type === 'ref') && (
        <li
          className="flex h-[22px] items-center gap-1 text-[10px] text-ink-faint"
          style={{ paddingLeft: `${node.depth * 14 + 22}px` }}
        >
          <CornerDownRight className="size-2.5" />
          <button
            type="button"
            onClick={() => onAddChild(node)}
            className="hover:text-ink hover:underline"
          >
            no fields — add one
          </button>
        </li>
      )}
    </li>
  )
}
