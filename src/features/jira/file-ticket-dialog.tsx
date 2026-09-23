import { useEffect, useState } from 'react'
import { useMutation, useQuery } from '@tanstack/react-query'
import { Bug, CheckCircle2, ExternalLink, MessageSquarePlus } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  EventTicket,
  FiledTicket,
  IpcError,
  Issue,
  RequiredField,
  TicketContext,
} from '@/lib/types'
import { WikiPreview } from './wiki-preview'
import { formatAge } from '@/lib/format'
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
  Segmented,
  Select,
  Spinner,
  cn,
} from '@/components/ui'

/** Write the markup, or look at what it will become. */
const BODY_TABS = [
  { id: 'write' as const, label: 'Write' },
  { id: 'preview' as const, label: 'Preview' },
]

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
  findings,
  context,
  onClose,
  onFiled,
}: {
  open: boolean
  /**
   * What the ticket is about: one finding from a row's File button, or every
   * finding on the Analysis tab as a single roll-up. Empty means there is
   * nothing to file and the dialog does not render.
   */
  findings: Issue[]
  context: TicketContext
  onClose: () => void
  /** Lets the caller show the ticket key where the buttons were. */
  onFiled: (issueKeys: string[], ticket: FiledTicket) => void
}) {
  const [summary, setSummary] = useState('')
  const [description, setDescription] = useState('')
  const [projectKey, setProjectKey] = useState('')
  /** Answers for whatever this project makes mandatory, keyed by field id. */
  const [fields, setFields] = useState<Record<string, unknown>>({})
  const [remember, setRemember] = useState(true)
  const [bodyTab, setBodyTab] = useState<'write' | 'preview'>('write')

  const keys = findings.map((finding) => finding.key)
  const preview = useQuery({
    queryKey: ['jira', 'preview', context.schemaName, context.environment, keys.join('|')],
    queryFn: () => ipc.previewJiraTicket(findings, context),
    enabled: open && findings.length > 0,
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

  const file = useMutation<{ ticket: FiledTicket }, IpcError, { commentOn?: string }>({
    mutationFn: async ({ commentOn }) => {
      // Saved before the write, so a rejected create does not also lose the
      // answers that took a trip to Jira's field metadata to discover.
      if (remember && Object.keys(fields).length > 0) {
        await ipc.setJiraFieldDefaults(projectKey, fields)
      }

      const result = await ipc.fileJiraTicket({
        issues: findings,
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
      return { ticket: result.ticket }
    },
    onSuccess: ({ ticket }) => {
      // Every finding the ticket covers, so a roll-up marks all of its rows
      // rather than only the one that led it.
      onFiled(keys, ticket)
      onClose()
    },
  })

  if (findings.length === 0) return null
  const existing = preview.data?.existing

  return (
    <Modal open={open} onClose={onClose} width={640} className="flex max-h-[85vh] flex-col p-4">
      <ModalTitle>
        <Bug className="size-4" />
        {findings.length > 1
          ? `File all ${findings.length} findings with the producer team`
          : 'File this with the producer team'}
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
          {/* Saying so rather than filing blind: without the check, an open
              ticket for this exact problem would not be offered as somewhere
              to comment, and this would quietly become the second one. */}
          {preview.data.duplicateCheck && (
            <Note tone="warn">
              <span>
                Could not check whether this is already filed (
                {preview.data.duplicateCheck.message}). Filing may open a duplicate.
              </span>
            </Note>
          )}

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

          {/* Not a `Field`: that wraps its children in a label, and a tab
              strip inside one is a button that steals the label's click. */}
          <div className="flex flex-col gap-1">
            <div className="flex items-center gap-2">
              <span className="text-[11px] font-medium text-ink-muted">Description</span>
              <Segmented
                size="sm"
                className="ml-auto"
                value={bodyTab}
                options={BODY_TABS}
                onChange={setBodyTab}
              />
            </div>
            {bodyTab === 'write' ? (
              <textarea
                value={description}
                onChange={(e) => setDescription(e.target.value)}
                rows={14}
                spellCheck={false}
                className="resize-none rounded-md border border-edge bg-surface-0 p-2 font-mono text-[11px] leading-relaxed text-ink focus:border-accent focus:outline-none"
              />
            ) : (
              // Grows with the body rather than scrolling inside the dialog's
              // own scroll: one scrollbar, and the footer stays pinned anyway.
              <WikiPreview text={description} className="min-h-[17.5rem]" />
            )}
            <span className="text-[10px] text-ink-faint">
              {bodyTab === 'write'
                ? 'Jira wiki markup'
                : 'Roughly as Jira will draw it. The markup is what gets filed.'}
            </span>
          </div>

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

/**
 * A ticket that exists in Jira, with where it stands.
 *
 * Status is the project's own word for it — "Shipped", "Blocked" — and the
 * colour is Jira's three-way grouping of that word, which is the only part
 * that means the same thing in every project.
 */
export function TicketChip({ ticket }: { ticket: EventTicket }) {
  return (
    <a
      href={ticket.url}
      target="_blank"
      rel="noreferrer"
      title={[
        ticket.summary,
        ticket.updated ? `moved ${formatAge(Date.now() - ticket.updated)} ago` : null,
      ]
        .filter(Boolean)
        .join(' — ')}
      className="shrink-0"
    >
      <Badge
        tone={ticket.done ? 'neutral' : ticket.started ? 'info' : 'warn'}
        className={cn('hover:bg-surface-3', ticket.done && 'line-through decoration-1')}
      >
        {ticket.key}
        <span className="font-normal opacity-80">{ticket.status}</span>
        <ExternalLink className="size-2.5" />
      </Badge>
    </a>
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
