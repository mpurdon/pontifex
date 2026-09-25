import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import * as ipc from '@/lib/ipc'

/**
 * Getting from an event to its schema, from wherever the event is shown.
 *
 * Watch and Logs both show events as rows, and both want the same two
 * answers: open the schema for this event type, or — when there is none —
 * draft one from the traffic around this event. The links are built here
 * rather than at each row, because the query string is a contract with
 * `schemas-page.tsx`, which parses `select`, `draft`, `group` and `around`.
 */
export function schemaLink(schemaName: string): string {
  return `/schemas?select=${encodeURIComponent(schemaName)}`
}

/**
 * The draft link for an event type seen at a moment in a group.
 *
 * `group` and `around` narrow the inference to a two-minute scan of one log
 * group rather than a day of every group: near-instant, and enough to draft
 * from. The Health report is the wide view.
 */
export function draftLink(identity: string, logGroup: string, timestamp: number): string {
  return `/schemas?draft=${encodeURIComponent(identity)}&group=${encodeURIComponent(logGroup)}&around=${timestamp}`
}

/**
 * The registry, keyed by the identity an event carries.
 *
 * A schema's registered name is not always the `source@detail-type` on the
 * wire — a source with a character the registry cannot hold is registered
 * under a sanitized spelling — so the lookup goes through what the event
 * says, not what the schema is called.
 */
export function useSchemaIndex(envId: string | undefined) {
  const schemas = useQuery({
    queryKey: ['schemas', envId, 'list'],
    queryFn: () => ipc.listSchemas(envId),
    enabled: !!envId,
    retry: false,
  })

  const byIdentity = useMemo(() => {
    const map = new Map<string, string>()
    for (const schema of schemas.data ?? []) {
      if (schema.source && schema.detailType) {
        map.set(`${schema.source}@${schema.detailType}`, schema.name)
      }
    }
    return map
  }, [schemas.data])

  return {
    /** False while the list is loading, so neither action is offered yet. */
    known: schemas.isSuccess,
    /** The registered schema for an event type, or null. */
    nameFor: (source: string | null, detailType: string | null) =>
      source && detailType ? (byIdentity.get(`${source}@${detailType}`) ?? null) : null,
  }
}
