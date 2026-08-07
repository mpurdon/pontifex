import { useEffect, useState } from 'react'
import { AlertTriangle, ArrowDown } from 'lucide-react'
import type { IpcError } from '@/lib/types'
import {
  Button,
  Checkbox,
  ErrorBox,
  Modal,
  ModalDescription,
  ModalTitle,
  Note,
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
  onConfirm: () => void
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

  useEffect(() => {
    if (open) {
      setAcknowledged(false)
      setReviewed(false)
    }
  }, [open, schemaName])

  const confirmed = acknowledged

  return (
    <Modal
      open={open}
      onClose={onCancel}
      width={900}
      tone="danger"
      className="flex h-[80vh] flex-col"
    >
          <div className="border-b border-edge p-4">
            <ModalTitle tone="danger">
              <AlertTriangle className="size-4" />
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

            {error && <ErrorBox error={error} />}

            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={onCancel}>
                Cancel
              </Button>
              <Button
                variant="danger"
                disabled={!confirmed}
                loading={saving}
                onClick={onConfirm}
              >
                {isNew ? 'Create' : 'Save new version'}
              </Button>
            </div>
          </div>
    </Modal>
  )
}
