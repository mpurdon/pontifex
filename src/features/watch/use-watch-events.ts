import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import * as ipc from '@/lib/ipc'
import type { WatchHit, WatchStatus } from '@/lib/types'

/** Query keys the poller's events write into. */
export const watchKeys = {
  status: ['watch', 'status'] as const,
  list: (envId: string | undefined) => ['watch', 'list', envId] as const,
  hits: (envId: string | undefined) => ['watch', 'hits', envId] as const,
  marks: (envId: string | undefined) => ['watch', 'marks', envId] as const,
}

/** Newest hits kept in the webview's copy, matching what the backend keeps per environment. */
export const HITS_IN_VIEW = 1000

/** Replace one environment's status in the cached list, keeping list order. */
export function upsertStatus(queryClient: QueryClient, next: WatchStatus) {
  queryClient.setQueryData<WatchStatus[]>(watchKeys.status, (old) => {
    if (!old) return [next]
    const index = old.findIndex((s) => s.envId === next.envId)
    if (index === -1) return [...old, next]
    const copy = old.slice()
    copy[index] = next
    return copy
  })
}

/**
 * Keep the query cache in step with the Rust poller.
 *
 * The poller emits `watch:hits` and `watch:status` as it goes. Writing those
 * straight into the cache means the page and the nav badge update the moment
 * something matches, and nothing has to poll the backend for state the
 * backend already pushed.
 */
export function useWatchEvents() {
  const queryClient = useQueryClient()

  useEffect(() => {
    const hits = listen<WatchHit[]>('watch:hits', (event) => {
      const byEnv = new Map<string, WatchHit[]>()
      for (const hit of event.payload) {
        const list = byEnv.get(hit.envId) ?? []
        list.push(hit)
        byEnv.set(hit.envId, list)
      }
      for (const [envId, fresh] of byEnv) {
        // Only merge into a list that has been loaded; an unloaded page will
        // fetch the full list, hits included, when it mounts.
        queryClient.setQueryData<WatchHit[]>(watchKeys.hits(envId), (old) =>
          old ? [...fresh, ...old].slice(0, HITS_IN_VIEW) : old,
        )
      }
    })

    const status = listen<WatchStatus>('watch:status', (event) => {
      const next = event.payload
      // Arming and disarming write session marks. Every pass pushes a status,
      // so only a change of session is a reason to refetch them.
      const previous = queryClient
        .getQueryData<WatchStatus[]>(watchKeys.status)
        ?.find((s) => s.envId === next.envId)
      if (previous?.armed !== next.armed || previous?.watchingSince !== next.watchingSince) {
        void queryClient.invalidateQueries({ queryKey: watchKeys.marks(next.envId) })
      }
      upsertStatus(queryClient, next)
    })

    return () => {
      void hits.then((unlisten) => unlisten())
      void status.then((unlisten) => unlisten())
    }
  }, [queryClient])
}

/** Every environment's poller state. Refreshed by events; the interval is a backstop. */
export function useWatchStatuses() {
  return useQuery({
    queryKey: watchKeys.status,
    queryFn: ipc.watchStatus,
    refetchInterval: 60_000,
    retry: false,
  })
}

/** The current time, refreshed every `intervalMs`, for "3m ago" text that has to keep moving. */
export function useNow(intervalMs: number): number {
  const [now, setNow] = useState(Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs])
  return now
}
