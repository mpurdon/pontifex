import { useMutation } from '@tanstack/react-query'
import { useEffect, useState } from 'react'
import { AlertTriangle } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { IpcError } from '@/lib/types'
import {
  Button,
  Checkbox,
  ErrorBox,
  Input,
  Modal,
  ModalDescription,
  ModalTitle,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'

/**
 * Deleting a schema is irreversible and, in prd, consequential. Two gates:
 * the user must type the schema name, and protected environments require an
 * explicit acknowledgement. The backend enforces both again — this dialog is
 * the affordance, not the security boundary.
 */
export function DeleteSchemaDialog({
  open,
  name,
  onOpenChange,
  onDeleted,
}: {
  open: boolean
  name: string
  onOpenChange: (open: boolean) => void
  onDeleted: () => void
}) {
  const { envId, activeEnvironment } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)
  const [typed, setTyped] = useState('')
  const [acknowledged, setAcknowledged] = useState(false)

  const isProtected = activeEnvironment?.protected ?? false

  useEffect(() => {
    if (open) {
      setTyped('')
      setAcknowledged(false)
    }
  }, [open, name])

  const remove = useMutation<void, IpcError>({
    mutationFn: () =>
      ipc.deleteSchema({
        name,
        confirmName: typed,
        confirmProtected: acknowledged,
        envId,
      }),
    onSuccess: onDeleted,
  })

  const canDelete = typed === name && (!isProtected || acknowledged)

  return (
    <Modal open={open} onClose={() => onOpenChange(false)} width={460} className="p-4">
          <ModalTitle tone="danger">
            <AlertTriangle className="size-4" />
            Delete schema
          </ModalTitle>
          <ModalDescription className="mt-2">
            This removes <span className="font-mono text-ink">{name}</span> and
            all of its versions from{' '}
            <span className="font-mono text-ink">
              {activeEnvironment?.registryName}
            </span>
            . It cannot be undone.
          </ModalDescription>

          <div className="mt-4 flex flex-col gap-3">
            <label className="flex flex-col gap-1">
              <span className="text-[11px] text-ink-muted">
                Type the schema name to confirm
              </span>
              <Input
                value={typed}
                onChange={(e) => setTyped(e.target.value)}
                placeholder={name}
                autoFocus
                spellCheck={false}
              />
            </label>

            {isProtected && (
              <div className="rounded-md border border-danger/40 bg-danger/10 p-2">
                <Checkbox
                  checked={acknowledged}
                  onChange={(e) => setAcknowledged(e.target.checked)}
                  className="text-danger"
                  label={
                    <>
                      I understand{' '}
                      <span className="font-mono">{activeEnvironment?.label}</span>{' '}
                      is a protected environment
                    </>
                  }
                />
              </div>
            )}

            {remove.isError && <ErrorBox error={remove.error} {...credentials} />}

            <div className="flex justify-end gap-2">
              <Button variant="ghost" onClick={() => onOpenChange(false)}>
                Cancel
              </Button>
              <Button
                variant="danger"
                loading={remove.isPending}
                disabled={!canDelete}
                onClick={() => remove.mutate()}
              >
                Delete permanently
              </Button>
            </div>
          </div>
    </Modal>
  )
}
