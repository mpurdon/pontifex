import { useEffect, useState } from 'react'
import { Bug } from 'lucide-react'
import type { FiledTicket, Issue, TicketContext } from '@/lib/types'
import { concernIssue } from '@/lib/issues'
import {
  Button,
  Field,
  Input,
  Modal,
  ModalDescription,
  ModalTitle,
  Textarea,
} from '@/components/ui'
import { FileTicketDialog } from './file-ticket-dialog'

/** What the concern is about, in the words the ticket will use. */
export interface ConcernSubject {
  /** `payload.matterId`, or empty for the event type itself. */
  path: string
  /** For the title: "the event type `x`" or "the field `payload.y`". */
  label: string
  /** What the schema says about it, when there is something to say. */
  declared?: string | null
  /** What the traffic says about it, when an analysis has run. */
  observed?: string | null
}

/**
 * A ticket about something the validator cannot see.
 *
 * The findings on the Analysis tab are one kind of discrepancy: the events
 * and the schema disagree. The other kind is a person looking at either and
 * seeing it is wrong — a name that misleads, a field that should not exist,
 * a type that happens to fit the traffic and is still the wrong contract.
 * This asks for the two sentences a ticket needs, then hands off to the same
 * preview every other ticket goes through.
 */
export function ConcernDialog({
  open,
  subject,
  context,
  onClose,
  onFiled,
}: {
  open: boolean
  subject: ConcernSubject | null
  context: TicketContext
  onClose: () => void
  onFiled?: (issueKey: string, ticket: FiledTicket) => void
}) {
  const [summary, setSummary] = useState('')
  const [detail, setDetail] = useState('')
  const [issue, setIssue] = useState<Issue | null>(null)

  // Fresh words for a fresh subject; the previous concern's text is not a
  // useful starting point for a different field.
  useEffect(() => {
    if (!open) return
    setSummary('')
    setDetail('')
    setIssue(null)
  }, [open, subject?.path])

  if (!subject) return null

  const ready = summary.trim().length > 0 && detail.trim().length > 0

  const preview = () => {
    setIssue(
      concernIssue({
        path: subject.path,
        summary: summary.trim(),
        detail: detail.trim(),
        declared: subject.declared,
        observed: subject.observed,
      }),
    )
  }

  return (
    <>
      <Modal open={open && !issue} onClose={onClose} width={560} className="flex flex-col gap-3 p-4">
        <ModalTitle>
          Raise a concern about <span className="normal-case">{subject.label}</span>
        </ModalTitle>
        <ModalDescription>
          For what the validator cannot see: a misleading name, a field that should not be
          there, a type that fits the traffic but is the wrong contract. It files to the team
          that owns the producer, like any other ticket.
        </ModalDescription>
        <Field label="In one line" hint="becomes the ticket title">
          <Input
            value={summary}
            onChange={(e) => setSummary(e.target.value)}
            placeholder={
              subject.path
                ? `${subject.path} should be …`
                : 'This event type should be named …'
            }
            autoFocus
          />
        </Field>
        <Field label="What is wrong, and what should change">
          <Textarea
            value={detail}
            onChange={(e) => setDetail(e.target.value)}
            rows={6}
            placeholder="Why it matters to consumers, and what you would like the producer to do instead."
          />
        </Field>
        {(subject.declared || subject.observed) && (
          <p className="text-[10px] text-ink-faint">
            The ticket will also say
            {subject.declared && (
              <>
                {' '}
                the schema declares <span className="font-mono">{subject.declared}</span>
              </>
            )}
            {subject.declared && subject.observed && ' and'}
            {subject.observed && <> the events show {subject.observed}</>}.
          </p>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" size="sm" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={preview} disabled={!ready}>
            <Bug className="size-3" />
            Preview ticket
          </Button>
        </div>
      </Modal>
      <FileTicketDialog
        open={open && !!issue}
        issue={issue}
        context={context}
        onClose={onClose}
        onFiled={(key, ticket) => {
          onFiled?.(key, ticket)
          onClose()
        }}
      />
    </>
  )
}
