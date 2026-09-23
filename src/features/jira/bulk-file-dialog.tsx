import { useEffect, useMemo, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { AlertTriangle, Bug, CheckCircle2, MinusCircle } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  FileOutcome,
  FileTicketResult,
  IpcError,
  Issue,
  TicketContext,
} from '@/lib/types'
import {
  Badge,
  Button,
  Checkbox,
  ErrorBox,
  Marked,
  Modal,
  ModalDescription,
  ModalTitle,
  Note,
  Spinner,
  cn,
} from '@/components/ui'

/** One candidate ticket: an issue plus where it was found. */
interface Candidate {
  id: string
  issue: Issue
  context: TicketContext
}

const OUTCOMES: Record<FileOutcome, { label: string; tone: 'ok' | 'neutral' | 'danger' }> = {
  created: { label: 'created', tone: 'ok' },
  commented: { label: 'commented', tone: 'ok' },
  skipped: { label: 'already open', tone: 'neutral' },
  failed: { label: 'failed', tone: 'danger' },
}

/**
 * Files tickets for several schemas at once.
 *
 * The preview is the point: a bulk action that goes straight to twenty writes
 * in other teams' backlogs is a spam cannon. Everything is listed, everything
 * is deselectable, and the backend re-checks for an already-open ticket per
 * row as it goes, so nothing is filed twice even if this list is minutes old.
 */
export function BulkFileDialog({
  open,
  schemaNames,
  minutes,
  envId,
  onClose,
}: {
  open: boolean
  schemaNames: string[]
  minutes: number
  envId?: string
  onClose: () => void
}) {
  const [chosen, setChosen] = useState<Set<string>>(new Set())
  const [results, setResults] = useState<FileTicketResult[] | null>(null)

  const candidates = useQuery({
    queryKey: ['jira', 'bulk', envId, minutes, schemaNames.join(',')],
    // No log group: the backend reads whichever of the environment's groups
    // the report cached each schema under.
    queryFn: () => ipc.issuesForSchemas(schemaNames, minutes, undefined, envId),
    enabled: open && schemaNames.length > 0,
    retry: false,
    staleTime: 0,
  })

  const rows: Candidate[] = useMemo(
    () =>
      (candidates.data ?? []).flatMap((schema) =>
        schema.issues
          // Rejections and drift, never the "declared but quiet" notes: those
          // are observations, not bugs anyone should receive a ticket for.
          .filter((issue) => issue.severity !== 'info')
          .map((issue) => ({
            id: `${schema.schemaName}::${issue.key}`,
            issue,
            context: schema.context,
          })),
      ),
    [candidates.data],
  )

  // Default to the ones that are actually costing events — and to the types
  // with no schema at all, which is the whole finding for such a row — so
  // hitting File straight away does the defensible thing.
  useEffect(() => {
    setChosen(
      new Set(
        rows
          .filter((r) => r.issue.rejects || r.issue.kind === 'unregistered')
          .map((r) => r.id),
      ),
    )
    setResults(null)
  }, [rows])

  const file = useMutation<FileTicketResult[], IpcError>({
    mutationFn: () =>
      ipc.fileJiraTickets(
        rows
          .filter((row) => chosen.has(row.id))
          // One ticket per row here, deliberately: these are different
          // schemas, so they are different producers' backlogs.
          .map((row) => ({ issues: [row.issue], context: row.context })),
      ),
    onSuccess: setResults,
  })

  /** Results by candidate id, so the list is not a scan per row. */
  const resultsById = useMemo(
    () =>
      new Map<string, FileTicketResult>(
        (results ?? []).map((r) => [`${r.schemaName}::${r.issueKey}`, r]),
      ),
    [results],
  )

  const failures = (candidates.data ?? []).filter((s) => s.error)
  const empty = (candidates.data ?? []).filter((s) => s.note)

  return (
    <Modal open={open} onClose={onClose} width={780} className="flex max-h-[85vh] flex-col p-4">
      <ModalTitle>
        <Bug className="size-4" />
        File tickets for {schemaNames.length} schema{schemaNames.length === 1 ? '' : 's'}
      </ModalTitle>
      <ModalDescription>
        One ticket per problem, routed to the project that owns each source. Anything
        already open is left alone.
      </ModalDescription>

      {candidates.isLoading && <Spinner label="Working out what to file…" />}
      {candidates.isError && (
        <div className="mt-3">
          <ErrorBox error={ipc.asIpcError(candidates.error)} />
        </div>
      )}

      {candidates.data && (
        <div className="mt-3 flex min-h-0 flex-1 flex-col gap-2 overflow-auto">
          {empty.length > 0 && (
            <Note tone="info">
              {empty.length} schema{empty.length === 1 ? ' has' : 's have'} no cached
              sample — run the report again to include {empty.length === 1 ? 'it' : 'them'}.
            </Note>
          )}
          {failures.map((schema) => (
            <Note key={schema.schemaName} tone="warn">
              <span className="font-mono">{schema.schemaName}</span>:{' '}
              {schema.error?.message}
            </Note>
          ))}

          {rows.length === 0 ? (
            <p className="py-6 text-center text-[11px] text-ink-faint">
              Nothing worth filing in the selected schemas.
            </p>
          ) : (
            <ul className="flex flex-col gap-1">
              {rows.map((row) => {
                const result = resultsById.get(row.id)
                return (
                  <li
                    key={row.id}
                    className="flex items-start gap-2 rounded-md border border-edge px-2 py-1.5"
                  >
                    <Checkbox
                      checked={chosen.has(row.id)}
                      disabled={!!results}
                      onChange={(e) =>
                        setChosen((prev) => {
                          const next = new Set(prev)
                          if (e.target.checked) next.add(row.id)
                          else next.delete(row.id)
                          return next
                        })
                      }
                      label=""
                      className="mt-0.5"
                    />
                    <div className="min-w-0 flex-1">
                      <p className="truncate font-mono text-[10px] text-ink-faint">
                        {row.context.schemaName}
                      </p>
                      <p className="text-[11px] leading-snug text-ink-muted">
                        <Marked text={row.issue.summary} />
                      </p>
                    </div>
                    <div className="flex shrink-0 items-center gap-1.5">
                      {row.issue.rejects && (
                        <Badge tone="danger" title="The registered schema rejects these events; EventBridge delivers them regardless">
                          rejected by schema
                        </Badge>
                      )}
                      {row.issue.kind === 'unregistered' && (
                        <Badge tone="warn">no schema</Badge>
                      )}
                      <span className="whitespace-nowrap text-[10px] text-ink-faint">
                        {row.issue.affected}/{row.issue.sampled}
                      </span>
                      {result && (
                        <Badge tone={OUTCOMES[result.outcome].tone}>
                          {result.ticket?.url ? (
                            <a href={result.ticket.url} target="_blank" rel="noreferrer">
                              {result.ticket.key}
                            </a>
                          ) : (
                            OUTCOMES[result.outcome].label
                          )}
                        </Badge>
                      )}
                    </div>
                  </li>
                )
              })}
            </ul>
          )}
        </div>
      )}

      {file.isError && (
        <div className="mt-3">
          <ErrorBox error={file.error} />
        </div>
      )}

      {results && <Summary results={results} />}

      <div className="mt-4 flex items-center justify-end gap-2">
        {!results && rows.length > 0 && (
          <span className="mr-auto text-[10px] text-ink-faint">
            {chosen.size} of {rows.length} selected
          </span>
        )}
        <Button variant="ghost" onClick={onClose}>
          {results ? 'Done' : 'Cancel'}
        </Button>
        {!results && (
          <Button
            variant="primary"
            disabled={chosen.size === 0}
            loading={file.isPending}
            onClick={() => file.mutate()}
          >
            <Bug className="size-3" />
            File {chosen.size} ticket{chosen.size === 1 ? '' : 's'}
          </Button>
        )}
      </div>
    </Modal>
  )
}

function Summary({ results }: { results: FileTicketResult[] }) {
  const count = (outcome: FileOutcome) =>
    results.filter((r) => r.outcome === outcome).length

  const created = count('created') + count('commented')
  const skipped = count('skipped')
  const failed = count('failed')

  return (
    <div className="mt-3 flex flex-wrap items-center gap-3 border-t border-edge pt-3 text-[11px]">
      <span className={cn('inline-flex items-center gap-1', created > 0 && 'text-ok')}>
        <CheckCircle2 className="size-3" />
        {created} filed
      </span>
      {skipped > 0 && (
        <span className="inline-flex items-center gap-1 text-ink-muted">
          <MinusCircle className="size-3" />
          {skipped} already open
        </span>
      )}
      {failed > 0 && (
        <span className="inline-flex items-center gap-1 text-danger">
          <AlertTriangle className="size-3" />
          {failed} failed —{' '}
          {results.find((r) => r.outcome === 'failed')?.error?.message}
        </span>
      )}
    </div>
  )
}
