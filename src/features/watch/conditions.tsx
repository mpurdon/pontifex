import { useQueries, useQuery } from '@tanstack/react-query'
import { useId, useMemo } from 'react'
import { Plus, X } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { WatchCondition } from '@/lib/types'
import { Button, Input, Select } from '@/components/ui'
import { matchingSchemas, pathsAcross } from './payload-paths'

/** How many matching schemas are read for suggestions; past this the list is noise. */
const SUGGEST_FROM = 8

/**
 * Field paths the registry declares for whatever the source and detail type
 * reach, for completing a condition's path the way an editor completes a
 * member name. Nothing is fetched until a schema matches; a blank pair
 * matches everything, which is too much to be useful, so it suggests nothing.
 */
export function usePayloadPaths(
  envId: string | undefined,
  source: string | null | undefined,
  detailType: string | null | undefined,
): string[] {
  const narrowed = !!(source?.trim() || detailType?.trim())
  const schemas = useQuery({
    queryKey: ['schemas', envId, 'list'],
    queryFn: () => ipc.listSchemas(envId),
    enabled: !!envId && narrowed,
    staleTime: 60_000,
  })
  const names = useMemo(
    () =>
      narrowed
        ? matchingSchemas((schemas.data ?? []).map((s) => s.name), source, detailType).slice(
            0,
            SUGGEST_FROM,
          )
        : [],
    [schemas.data, source, detailType, narrowed],
  )
  const documents = useQueries({
    queries: names.map((name) => ({
      queryKey: ['schemas', envId, 'detail', name],
      queryFn: () => ipc.describeSchema(name, undefined, envId),
      staleTime: 5 * 60_000,
    })),
  })
  return useMemo(
    () => pathsAcross(documents.flatMap((d) => (d.data ? [d.data.content] : []))),
    // The query objects are new each render; what matters is their data.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [documents.map((d) => d.dataUpdatedAt).join(',')],
  )
}

/**
 * The payload conditions of a watch or a log search: path, operator, value.
 *
 * One editor for both screens, so the grammar, the hint and the completions
 * cannot drift apart. Paths are inside `detail`; the compiler adds the prefix.
 */
export function ConditionEditor({
  conditions,
  onChange,
  envId,
  source,
  detailType,
}: {
  conditions: WatchCondition[]
  onChange: (conditions: WatchCondition[]) => void
  envId: string | undefined
  /** Narrow the completions to the schemas these would match. */
  source: string | null | undefined
  detailType: string | null | undefined
}) {
  const suggestions = usePayloadPaths(envId, source, detailType)
  const listId = useId()
  const set = (index: number, changes: Partial<WatchCondition>) =>
    onChange(conditions.map((c, i) => (i === index ? { ...c, ...changes } : c)))

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center justify-between">
        <span className="text-[11px] font-medium text-ink-muted">Payload conditions</span>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => onChange([...conditions, { path: '', op: 'eq', value: '' }])}
        >
          <Plus className="size-3" />
          Condition
        </Button>
      </div>
      {suggestions.length > 0 && (
        <datalist id={listId}>
          {suggestions.map((path) => (
            <option key={path} value={path} />
          ))}
        </datalist>
      )}
      {conditions.map((condition, index) => (
        // `min-w-0` on the two inputs, because an input's automatic minimum
        // width is the size it would like to be — around twenty characters —
        // and this editor lives in a panel the user can drag down to a couple
        // of hundred pixels. Without it the row cannot shrink, and the panel
        // grows a horizontal scrollbar that takes every other line with it.
        <div key={index} className="flex items-center gap-1">
          <Input
            value={condition.path}
            onChange={(e) => set(index, { path: e.target.value })}
            placeholder="clientId"
            list={suggestions.length > 0 ? listId : undefined}
            className="min-w-0 flex-1 font-mono"
            spellCheck={false}
            autoComplete="off"
          />
          <Select
            value={condition.op}
            onChange={(e) => set(index, { op: e.target.value as WatchCondition['op'] })}
            className="w-14 shrink-0"
          >
            <option value="eq">=</option>
            <option value="ne">≠</option>
          </Select>
          <Input
            value={condition.value}
            onChange={(e) => set(index, { value: e.target.value })}
            placeholder="abc-123"
            className="min-w-0 flex-1 font-mono"
            spellCheck={false}
          />
          <Button
            variant="ghost"
            size="sm"
            className="shrink-0"
            onClick={() => onChange(conditions.filter((_, i) => i !== index))}
            title="Remove condition"
          >
            <X className="size-3" />
          </Button>
        </div>
      ))}
      <span className="text-[10px] text-ink-faint">
        Paths are inside the payload: <span className="font-mono">clientId</span>,{' '}
        <span className="font-mono">items[0].id</span>; start with{' '}
        <span className="font-mono">$.</span> for the envelope, like{' '}
        <span className="font-mono">$.account</span>. Unquoted numbers compare numerically;{' '}
        <span className="font-mono">*</span> is a wildcard in strings; wrap a value in quotes to
        force a string match.
        {suggestions.length > 0 && ' Field names complete from the schemas the source and detail type match.'}
      </span>
    </div>
  )
}
