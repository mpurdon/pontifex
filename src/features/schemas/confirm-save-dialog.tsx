import { useCallback, useEffect, useState } from 'react'
import { AlertTriangle, ArrowDown, RefreshCw, Sparkles } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { IpcError } from '@/lib/types'
import {
  Button,
  Checkbox,
  ErrorBox,
  Modal,
  ModalDescription,
  ModalTitle,
  Note,
  Textarea,
  cn,
} from '@/components/ui'
import { JsonDiff } from '@/components/lazy-editor'

/**
 * Reviews a write before it reaches a protected environment.
 *
 * Production changes are allowed — they just should not be one keystroke away.
 * Showing the actual diff makes "am I about to do what I think I am" answerable
 * without leaving the dialog, which a plain "are you sure?" cannot.
 */
export function ConfirmSaveDialog({
  open,
  schemaName,
  environmentLabel,
  registryName,
  before,
  after,
  isNew,
  saving,
  error,
  onCancel,
  onConfirm,
  requiresAcknowledgement,
}: {
  open: boolean
  schemaName: string
  environmentLabel: string
  registryName: string
  /** Live document, as JSON text. Empty for a new schema. */
  before: string
  /** The draft being saved. */
  after: string
  isNew: boolean
  saving: boolean
  /**
   * A failed write, shown here rather than behind the dialog.
   *
   * The dialog stays open on failure so the diff and the confirmation are
   * still in front of you — which meant an error rendered on the panel
   * underneath was invisible, and the button looked like it did nothing.
   */
  error?: IpcError | null
  onCancel: () => void
  /** Carries the version description, which the registry stores per version. */
  onConfirm: (description?: string) => void
  /**
   * Whether the write needs the diff read and acknowledged.
   *
   * True for protected environments. Elsewhere the dialog is still the publish
   * step — it is where the version description is written — but it does not
   * stand in the way.
   */
  requiresAcknowledgement: boolean
}) {
  const [acknowledged, setAcknowledged] = useState(false)
  /**
   * Whether the whole document has been scrolled through.
   *
   * The acknowledgement is worth something only if the diff was actually in
   * front of you — so it stays disabled until you reach the bottom, the way a
   * terms-of-service accept button does. A document short enough not to scroll
   * counts as read immediately.
   */
  const [reviewed, setReviewed] = useState(false)

  /**
   * The version description, written to the registry alongside the document.
   *
   * EventBridge has stored one per version all along and nothing has ever
   * written to it, so every version in the registry reads the same: nothing.
   * A summary generated from the diff is not a substitute for a considered
   * message, which is why it lands in an editable box rather than going
   * straight to the API.
   */
  const [description, setDescription] = useState('')
  const [summarising, setSummarising] = useState(false)
  const [summaryError, setSummaryError] = useState<IpcError | null>(null)
  /** Whether the box holds something typed, which must not be overwritten. */
  const [edited, setEdited] = useState(false)

  useEffect(() => {
    if (open) {
      setAcknowledged(false)
      setReviewed(false)
      setDescription('')
      setEdited(false)
      setSummaryError(null)
    }
  }, [open, schemaName])

  const summarise = useCallback(async () => {
    setSummarising(true)
    setSummaryError(null)
    try {
      const parse = (text: string) => {
        try {
          return JSON.parse(text) as unknown
        } catch {
          return null
        }
      }
      const summary = await ipc.aiSummarizeChanges(
        isNew ? null : parse(before),
        parse(after),
      )
      setDescription(summary)
      setEdited(false)
    } catch (e) {
      // A summary is a convenience, not a precondition: the write is still
      // available with the box empty or hand-written. Failing loudly here
      // would block a publish over a model being unreachable.
      //
      // Narrowed rather than stringified: a rejection from a Rust command is
      // `{kind, message}`, and `String()` on it renders "[object Object]" —
      // which says a summary failed while withholding the only part that says
      // why.
      setSummaryError(ipc.asIpcError(e))
    } finally {
      setSummarising(false)
    }
  }, [before, after, isNew])

  // Generated once per opening, and never over something already typed.
  useEffect(() => {
    if (!open || edited || description || summarising) return
    void summarise()
    // Deliberately keyed on the opening alone: regenerating as the draft
    // changes would overwrite the box while it is being read.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const confirmed = acknowledged || !requiresAcknowledgement

  return (
    <Modal
      open={open}
      onClose={onCancel}
      width={900}
      tone={requiresAcknowledgement ? 'danger' : undefined}
      className="flex h-[80vh] flex-col"
    >
          <div className="border-b border-edge p-4">
            <ModalTitle tone={requiresAcknowledgement ? 'danger' : undefined}>
              {requiresAcknowledgement && <AlertTriangle className="size-4" />}
              {isNew ? 'Create in' : 'Update'}{' '}
              <span className="font-mono">{environmentLabel}</span>
            </ModalTitle>
            <ModalDescription>
              {isNew ? 'Creating' : 'Saving a new version of'}{' '}
              <span className="font-mono text-ink">{schemaName}</span> in{' '}
              <span className="font-mono text-ink">{registryName}</span>.
            </ModalDescription>
          </div>

          <div className="min-h-0 flex-1">
            {isNew ? (
              <div
                className="h-full overflow-auto p-3"
                onScroll={(e) => {
                  const el = e.currentTarget
                  if (el.scrollTop + el.clientHeight >= el.scrollHeight - 4) {
                    setReviewed(true)
                  }
                }}
                ref={(el) => {
                  // Short documents never fire a scroll event, so check on
                  // mount or the gate could never open.
                  if (el && el.scrollHeight <= el.clientHeight + 4) setReviewed(true)
                }}
              >
                <Note className="mb-2">
                  This schema does not exist yet, so there is nothing to compare
                  against — the whole document is new.
                </Note>
                <pre className="whitespace-pre-wrap font-mono text-[11px] text-ink-muted">
                  {after}
                </pre>
              </div>
            ) : (
              <JsonDiff
                original={before}
                modified={after}
                originalLabel={`Currently live in ${registryName}`}
                modifiedLabel="What you are about to write"
                onReachedEnd={() => setReviewed(true)}
              />
            )}
          </div>

          <div className="flex flex-col gap-3 border-t border-edge p-4">
            <div>
              <div className="mb-1 flex items-center gap-2">
                <label
                  htmlFor="version-description"
                  className="text-[10px] uppercase tracking-wide text-ink-faint"
                >
                  Version description
                </label>
                <button
                  type="button"
                  onClick={() => void summarise()}
                  disabled={summarising}
                  title="Summarise the diff again, discarding what is in the box"
                  className="inline-flex items-center gap-1 rounded px-1 py-0.5 text-[10px] text-ink-faint hover:bg-surface-3 hover:text-accent disabled:opacity-40"
                >
                  {summarising ? (
                    <RefreshCw className="size-2.5 animate-spin" />
                  ) : (
                    <Sparkles className="size-2.5" />
                  )}
                  {summarising ? 'Summarising…' : 'Regenerate'}
                </button>
              </div>
              <Textarea
                id="version-description"
                rows={2}
                value={description}
                placeholder={
                  summarising
                    ? 'Summarising the diff…'
                    : 'What changed, and why — stored with this version.'
                }
                onChange={(e) => {
                  setDescription(e.target.value)
                  setEdited(true)
                }}
              />
              {summaryError && (
                <p className="mt-1 text-[10px] leading-snug text-ink-faint">
                  Could not summarise automatically — {summaryError.message}.
                  Write one yourself, or publish without.
                </p>
              )}
            </div>

            {requiresAcknowledgement && (
              <div
                className={cn(
                  'rounded-md border p-2 transition-colors',
                  reviewed
                    ? 'border-danger/40 bg-danger/10'
                    : 'border-edge bg-surface-2',
                )}
              >
                <Checkbox
                  checked={acknowledged}
                  disabled={!reviewed}
                  onChange={(e) => setAcknowledged(e.target.checked)}
                  className={reviewed ? 'text-danger' : 'text-ink-faint'}
                  label={
                    <>
                      I have reviewed {isNew ? 'this document' : 'this diff'} and want to
                      apply it to <span className="font-mono">{environmentLabel}</span>
                    </>
                  }
                />
                {!reviewed && (
                  <p className="mt-1 flex items-center gap-1.5 pl-5 text-[10px] text-ink-faint">
                    <ArrowDown className="size-2.5" />
                    Scroll to the end of the {isNew ? 'document' : 'diff'} to enable this.
                  </p>
                )}
              </div>
            )}

            {error && <ErrorBox error={error} />}

            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={onCancel}>
                Cancel
              </Button>
              <Button
                variant={requiresAcknowledgement ? 'danger' : 'primary'}
                disabled={!confirmed}
                loading={saving}
                onClick={() => onConfirm(description.trim() || undefined)}
              >
                {isNew ? 'Create' : 'Save new version'}
              </Button>
            </div>
          </div>
    </Modal>
  )
}
