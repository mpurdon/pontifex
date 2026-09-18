import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Radar } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { WatchStatus } from '@/lib/types'
import { Button, Modal, ModalDescription, ModalTitle } from '@/components/ui'
import { formatAge } from '@/lib/format'
import { useWatchStatuses, watchKeys } from './use-watch-events'

/** How long the question stays open before silence is taken as "no". */
const GRACE_MS = 5 * 60_000

/** "Continue for…" choices, in hours. */
const CONTINUE_HOURS = [1, 4, 8]

/**
 * The idle check: watching is meant to run unattended, but not forgotten.
 *
 * Any environment armed, and nobody has touched the app for the configured
 * time — a click, a key, a scroll — so the window comes forward and asks
 * whether to keep going. Continue picks a fresh allowance, after which it
 * asks again. No answer within the grace period stops every watch, which
 * writes the usual stop marks, so the hit list shows exactly when
 * unattended watching ended.
 *
 * Activity is a person using the app, not hits arriving: a busy bus is the
 * one thing this check must not mistake for someone being there.
 */
export function IdleWatchGuard() {
  const queryClient = useQueryClient()
  const { data: statuses } = useWatchStatuses()
  const armed = (statuses ?? []).filter((s) => s.armed)
  const idleMinutes = statuses?.[0]?.idleTimeoutMinutes ?? 0

  const lastActivity = useRef(Date.now())
  /** A "continue for N hours" answer, as the time at which to ask again. */
  const allowanceUntil = useRef<number | null>(null)
  const [open, setOpen] = useState<{ since: number; deadline: number } | null>(null)

  useEffect(() => {
    const touch = () => {
      lastActivity.current = Date.now()
    }
    const events: Array<keyof WindowEventMap> = ['pointerdown', 'keydown', 'wheel', 'touchstart']
    for (const e of events) window.addEventListener(e, touch, { passive: true })
    return () => {
      for (const e of events) window.removeEventListener(e, touch)
    }
  }, [])

  const pauseAll = useMutation<WatchStatus[], unknown, WatchStatus[]>({
    mutationFn: (envs) => Promise.all(envs.map((s) => ipc.setWatching(false, s.envId))),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: watchKeys.status })
    },
  })

  const ask = useCallback(() => {
    const now = Date.now()
    setOpen({ since: lastActivity.current, deadline: now + GRACE_MS })
    // Someone who closed the window and walked away cannot see a modal in it.
    void ipc.showMainWindow().catch(() => {})
  }, [])

  useEffect(() => {
    if (armed.length === 0 || idleMinutes <= 0) return
    const id = setInterval(() => {
      const now = Date.now()
      if (open) {
        if (now >= open.deadline) {
          setOpen(null)
          pauseAll.mutate(armed)
        }
        return
      }
      const allowance = allowanceUntil.current
      if (allowance != null) {
        if (now >= allowance) {
          allowanceUntil.current = null
          ask()
        }
        return
      }
      if (now - lastActivity.current >= idleMinutes * 60_000) ask()
    }, 15_000)
    return () => clearInterval(id)
    // `armed` is derived each render; its ids are what matter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [armed.map((s) => s.envId).join(','), idleMinutes, open, ask])

  if (!open) return null

  const keep = (hours: number) => {
    allowanceUntil.current = Date.now() + hours * 3_600_000
    lastActivity.current = Date.now()
    setOpen(null)
  }

  return (
    <Modal open onClose={() => keep(CONTINUE_HOURS[0])} width={440}>
      <div className="flex flex-col gap-3 p-5">
        <ModalTitle>
          <span className="flex items-center gap-2">
            <Radar className="size-4 text-ok" />
            Still watching?
          </span>
        </ModalTitle>
        <ModalDescription>
          Nobody has touched Pontifex for {formatAge(Date.now() - open.since)}. Watching{' '}
          {armed.length === 1 ? armed[0].envId : `${armed.length} environments`} continues
          unattended, or stops in {formatAge(Math.max(0, open.deadline - Date.now()))} if
          nothing is chosen.
        </ModalDescription>
        <div className="flex flex-wrap items-center gap-2">
          {CONTINUE_HOURS.map((h) => (
            <Button key={h} variant={h === CONTINUE_HOURS[0] ? 'primary' : 'secondary'} onClick={() => keep(h)}>
              Continue for {h}h
            </Button>
          ))}
          <Button
            variant="ghost"
            className="ml-auto"
            onClick={() => {
              setOpen(null)
              pauseAll.mutate(armed)
            }}
          >
            Stop watching
          </Button>
        </div>
      </div>
    </Modal>
  )
}
