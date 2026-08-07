import { useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import * as ipc from '@/lib/ipc'
import { buildLedger, type Ledger } from '@/lib/schema-history'

/**
 * A schema's version history, and the same history indexed by field.
 *
 * One hook because three screens want it — the structure editor for its
 * per-field badges, the History list, and the Timeline — and each fetch is up
 * to 25 `DescribeSchema` calls. Sharing the query key means opening a schema
 * pays for that once, not three times.
 *
 * Never goes stale on its own: published versions are immutable, and a new one
 * only appears when this app writes it, which already invalidates
 * `['schemas', envId]`.
 */
export function useSchemaHistory(name: string, envId: string | undefined) {
  const query = useQuery({
    queryKey: ['schemas', envId, 'history', name],
    queryFn: () => ipc.schemaHistory(name, envId),
    enabled: !!envId,
    retry: false,
    staleTime: Infinity,
  })

  const ledger: Ledger = useMemo(() => buildLedger(query.data ?? []), [query.data])

  return { ...query, entries: query.data ?? [], ledger }
}
