import { useEffect, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { Bug, CheckCircle2, ExternalLink, MessageSquarePlus } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  FiledTicket,
  IpcError,
  Issue,
  RequiredField,
  TicketContext,
} from '@/lib/types'
import {
  Badge,
  Button,
  Checkbox,
  ErrorBox,
  Field,
  Input,
  Modal,
  ModalDescription,
  ModalTitle,
  Note,
  Select,
  Spinner,
} from '@/components/ui'

/**
 * The id inside a stored field value, for driving a select.
 *
 * Values are kept in the shape Jira wants — `{"id": "10500"}`, an array of
 * those, or a bare string — because that is what varies per field type.
 */
function valueId(value: unknown): string {
  if (Array.isArray(value)) return valueId(value[0])
  if (value && typeof value === 'object' && 'id' in value) {
    return String((value as { id: unknown }).id)
  }
  return value === undefined || value === null ? '' : String(value)
}

/** Wrap a chosen id the way the field's type expects it. */
function shapeValue(field: RequiredField, id: string): unknown {
  if (!id) return undefined
  return field.fieldType === 'array' ? [{ id }] : { id }
}

/** Type a free-text answer the way the field's schema expects it. */
function shapeText(field: RequiredField, text: string): unknown {
  if (!text) return undefined
  // A number field sent as a string is rejected, and the rejection talks about
  // the field rather than about the quotes.
  if (field.fieldType === 'number') {
    const parsed = Number(text)
    return Number.isNaN(parsed) ? text : parsed
  }
  return text
}

/**
 * Reviews a ticket before it reaches Jira.
 *
 * The same rule as saving to a protected environment: a write that leaves this
 * machine and lands in another team's backlog should not be one keystroke
 * away. The body shown here is rendered by the backend — the same function
 * that files it — so this is the ticket, not an approximation of it.
 */
export function FileTicketDialog({
  open,
  issue,
  context,
  onClose,
  onFiled,
}: {
  open: boolean
  issue: Issue | null
  context: TicketContext
  onClose: () => void
  /** Lets the caller show the ticket key where the button was. */
  onFiled: (issueKey: string, ticket: FiledTicket) => void
}) {
  const [summary, setSummary] = useState('')
  const [description, setDescription] = useState('')
  const [projectKey, setProjectKey] = useState('')
  /** Answers for whatever this project makes mandatory, keyed by field id. */
  const [fields, setFields] = useState<Record<string, unknown>>({})
  const [remember, setRemember] = useState(true)

  const preview = useQuery({
    queryKey: ['jira', 'preview', context.schemaName, context.environment, issue?.key],
    queryFn: () => ipc.previewJiraTicket(issue!, context),
    enabled: open && !!issue,
    retry: false,
    // Always re-render on open: the ticket describes a sample, and a stale
    // preview would file yesterday's numbers.
    staleTime: 0,
  })

  useEffect(() => {
    if (!preview.data) return
    setSummary(preview.data.summary)
    setDescription(preview.data.description)
    setProjectKey(preview.data.projectKey)
    setCommittedProject(preview.data.projectKey)
    // Whatever was answered for this project last time.
    setFields(preview.data.fields ?? {})
  }, [preview.data])

  /**
   * What this project demands beyond the obvious.
   *
   * Projects can make any field mandatory, and a create call that omits one is
   * rejected with a message naming a custom field id — so ask Jira what it
   * wants rather than finding out at the write.
   */
  // Keyed on the committed project, not on every keystroke: the field is free
  // text, and each lookup is two Jira round trips — typing `IPP` fired six.
  const [committedProject, setCommittedProject] = useState('')
  const required = useQuery({
    queryKey: ['jira', 'requiredFields', committedProject, preview.data?.issueType],
    queryFn: () => ipc.jiraRequiredFields(committedProject, preview.data!.issueType),
    enabled: open && !!committedProject && !!preview.data?.issueType,
    retry: false,
    staleTime: 5 * 60_000,
  })

  // Blocking only on the ones Jira will not fill itself: a defaulted field is
  // worth offering, but refusing to file without it would be wrong more often
  // than right.
  const unanswered = (required.data ?? []).filter(
    (field) => !field.hasDefault && fields[field.fieldId] === undefined,
  )

  const file = useMutation<
    { issueKey: string; ticket: FiledTicket },
    IpcError,
    { commentOn?: string }
  >({
    mutationFn: async ({ commentOn }) => {
      // Saved before the write, so a rejected create does not also lose the
      // answers that took a trip to Jira's field metadata to discover.
      if (remember && Object.keys(fields).length > 0) {
        await ipc.setJiraFieldDefaults(projectKey, fields)
      }

      const result = await ipc.fileJiraTicket({
        issue: issue!,
        context,
        summary,
        description,
        projectKey,
        issueType: preview.data?.issueType,
        fields,
        commentOn,
      })
      // A bulk run reports failures per row; a single filing has nowhere to put
      // one but the dialog, so it becomes a rejection.
      if (!result.ticket) {
        throw (
          result.error ?? {
            kind: 'internal' as const,
            message: 'Jira accepted the request but returned no ticket.',
          }
        )
      }
      return { issueKey: issue!.key, ticket: result.ticket }
    },
    onSuccess: ({ issueKey, ticket }) => {
      onFiled(issueKey, ticket)
      onClose()
    },
  })

  if (!issue) return null
  const existing = preview.data?.existing

  return (
    <Modal open={open} onClose={onClose} width={640} className="flex max-h-[85vh] flex-col p-4">
      <ModalTitle>
        <Bug className="size-4" />
        File this with the producer team
      </ModalTitle>
      <ModalDescription>
        Creates a Jira ticket in{' '}
        <span className="font-mono text-ink">{projectKey || '—'}</span>
        {preview.data && ` — ${preview.data.routedBy}`}
      </ModalDescription>

      {preview.isLoading && <Spinner label="Preparing the ticket…" />}
      {preview.isError && (
        <div className="mt-3">
          <ErrorBox error={ipc.asIpcError(preview.error)} />
        </div>
      )}

      {preview.data && (
        <div className="mt-3 flex min-h-0 flex-1 flex-col gap-3 overflow-auto">
          {/* The duplicate check runs before the dialog is useful, because
              "file it again" is the wrong default when it is already filed. */}
          {existing && (
            <Note tone="warn">
              <span>
                Already filed as{' '}
                <a
                  href={existing.url}
                  target="_blank"
                  rel="noreferrer"
                  className="font-mono underline"
                >
                  {existing.key}
                </a>
                {existing.status && ` (${existing.status})`}. Comment on it instead of
                opening a second ticket.
              </span>
            </Note>
          )}

          <Field label="Title">
            <Input value={summary} onChange={(e) => setSummary(e.target.value)} />
          </Field>

          <div className="grid grid-cols-[1fr_auto] gap-2">
            <Field label="Project" hint="from the routing rules">
              <Input
                value={projectKey}
                onChange={(e) => setProjectKey(e.target.value)}
                onBlur={(e) => setCommittedProject(e.target.value.trim())}
                className="font-mono"
              />
            </Field>
            <Field label="Type">
              <Input value={preview.data.issueType} readOnly className="font-mono" />
            </Field>
          </div>

          {/* Discovery failing is worth saying out loud: without it the dialog
              looks complete and the create fails naming a field that never
              appeared on screen. */}
          {required.isError && (
            <Note tone="warn">
              <span>
                Could not read what {projectKey} requires (
                {ipc.asIpcError(required.error).message}). Filing may be rejected for a
                missing field.
              </span>
            </Note>
          )}

          {/* Only what this project actually demands — an empty section here
              is the normal case. */}
          {required.data && required.data.length > 0 && (
            <div className="flex flex-col gap-2 rounded-md border border-edge bg-surface-0 p-2">
              <p className="text-[10px] text-ink-faint">
                {projectKey} requires {required.data.length === 1 ? 'this' : 'these'} before
                it will accept a ticket.
              </p>
              {required.data.map((field) => (
                <Field
                  key={field.fieldId}
                  label={field.name}
                  hint={field.hasDefault ? 'Jira may supply this one' : undefined}
                >
                  {field.allowedValues.length > 0 ? (
                    <Select
                      value={valueId(fields[field.fieldId])}
                      onChange={(e) =>
                        setFields((prev) => ({
                          ...prev,
                          [field.fieldId]: shapeValue(field, e.target.value),
                        }))
                      }
                    >
                      <option value="">choose…</option>
                      {field.allowedValues.map((allowed) => (
                        <option key={allowed.id} value={allowed.id}>
                          {allowed.label}
                        </option>
                      ))}
                    </Select>
                  ) : (
                    <Input
                      value={String(fields[field.fieldId] ?? '')}
                      onChange={(e) =>
                        setFields((prev) => ({
                          ...prev,
                          [field.fieldId]: shapeText(field, e.target.value),
                        }))
                      }
                      placeholder={field.fieldType}
                      spellCheck={false}
                    />
                  )}
                </Field>
              ))}
              <Checkbox
                checked={remember}
                onChange={(e) => setRemember(e.target.checked)}
                label={`Remember for every ticket filed into ${projectKey}`}
              />
            </div>
          )}

          <Field label="Description" hint="Jira wiki markup">
            <textarea
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              rows={14}
              spellCheck={false}
              className="resize-none rounded-md border border-edge bg-surface-0 p-2 font-mono text-[11px] leading-relaxed text-ink focus:border-accent focus:outline-none"
            />
          </Field>

          <div className="flex flex-wrap items-center gap-1.5">
            <span className="text-[10px] text-ink-faint">Labels</span>
            {preview.data.labels.map((label) => (
              <Badge key={label} tone="neutral">
                {label}
              </Badge>
            ))}
          </div>
        </div>
      )}

      {file.isError && (
        <div className="mt-3">
          <ErrorBox error={file.error} />
        </div>
      )}

      <div className="mt-4 flex justify-end gap-2">
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        {existing && (
          <Button
            variant="secondary"
            loading={file.isPending}
            onClick={() => file.mutate({ commentOn: existing.key })}
          >
            <MessageSquarePlus className="size-3" />
            Comment on {existing.key}
          </Button>
        )}
        <Button
          variant="primary"
          title={
            unanswered.length > 0
              ? `${projectKey} requires ${unanswered.map((f) => f.name).join(', ')}`
              : undefined
          }
          disabled={
            !preview.data ||
            !summary.trim() ||
            !projectKey.trim() ||
            // Filing without them is a guaranteed rejection.
            unanswered.length > 0
          }
          loading={file.isPending}
          onClick={() => file.mutate({})}
        >
          <Bug className="size-3" />
          {existing ? 'File anyway' : 'Create ticket'}
        </Button>
      </div>
    </Modal>
  )
}

/** A ticket that was just filed, shown where the button used to be. */
export function FiledChip({ ticket }: { ticket: FiledTicket }) {
  return (
    <a
      href={ticket.url}
      target="_blank"
      rel="noreferrer"
      title={ticket.summary ?? ticket.key}
      className="shrink-0"
    >
      <Badge tone="ok" className="hover:bg-ok/25">
        <CheckCircle2 className="size-2.5" />
        {ticket.key}
        <ExternalLink className="size-2.5" />
      </Badge>
    </a>
  )
}
