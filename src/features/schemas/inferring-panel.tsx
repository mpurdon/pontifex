import { listen } from '@tauri-apps/api/event'
import { useEffect, useState } from 'react'
import { Loader2, Wand2 } from 'lucide-react'
import { Button } from '@/components/ui'

/**
 * Progress while a schema is being inferred from traffic.
 *
 * An indefinite spinner cannot distinguish "working" from "hung", which is the
 * worst possible state for an operation that reaches out to AWS. This shows
 * what step is running and how long it has been going, so a stall is visible
 * rather than merely suspected.
 */
export function InferringPanel({
  schemaName,
  onCancel,
}: {
  schemaName: string
  onCancel: () => void
}) {
  const [stage, setStage] = useState<string | null>(null)
  const [elapsed, setElapsed] = useState(0)

  useEffect(() => {
    const unlisten = listen<string>('draft://progress', (event) =>
      setStage(event.payload),
    )
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [])

  useEffect(() => {
    const timer = setInterval(() => setElapsed((s) => s + 1), 1000)
    return () => clearInterval(timer)
  }, [])

  // Past this point something is wrong. *What* is wrong depends on how far it
  // got: blaming credentials while the backend is already inferring from
  // events it has in hand sends you to check a badge that is perfectly green.
  const slow = elapsed >= 15
  const reachedAws = stage !== null
  const inferring = stage?.startsWith('Inferring') ?? false

  return (
    <div className="flex h-full flex-col items-center justify-center gap-3 p-8 text-center">
      <Wand2 className="size-8 text-ink-faint" />
      <div>
        <p className="text-sm text-ink-muted">
          Inferring a schema for{' '}
          <span className="font-mono text-ink">{schemaName}</span>
        </p>
        <p className="mt-1 flex items-center justify-center gap-1.5 text-xs text-ink-faint">
          <Loader2 className="size-3 animate-spin" />
          {stage ?? 'Starting…'} · {elapsed}s
        </p>
      </div>

      {slow && (
        <p className="max-w-md text-[11px] text-warn">
          {inferring ? (
            <>
              The events are already fetched and shaping them takes milliseconds,
              so this is not the event bus. Cancel and try again; if it keeps
              happening, the application log under Developer will say whether
              the call ever finished.
            </>
          ) : reachedAws ? (
            <>
              Still waiting on CloudWatch. Widen the time window if this event
              type is rare, or cancel and retry.
            </>
          ) : (
            <>
              No progress from the backend at all. The usual cause is credentials
              that cannot be resolved for this environment — check the badge in
              the header.
            </>
          )}
        </p>
      )}

      <Button variant="ghost" size="sm" onClick={onCancel}>
        Cancel
      </Button>
    </div>
  )
}
