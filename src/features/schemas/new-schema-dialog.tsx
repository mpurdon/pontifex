import { useMutation } from '@tanstack/react-query'
import { useState } from 'react'
import { Plus, X } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { DetailProperty, IpcError, NewSchemaDraft } from '@/lib/types'
import {
  Button,
  ErrorBox,
  Field,
  Input,
  Modal,
  ModalDescription,
  ModalTitle,
  Note,
  Select,
} from '@/components/ui'

const PROPERTY_TYPES = ['string', 'number', 'integer', 'boolean', 'object', 'array']

/**
 * Collects the source, detail-type and payload fields, then asks the backend
 * to generate a complete envelope-correct document.
 *
 * The generator lives in Rust so the wizard, the AI panel and the import path
 * all produce identical structure — there is no second copy of the boilerplate
 * to drift.
 */
export function NewSchemaDialog({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onCreated: (draft: NewSchemaDraft) => void
}) {
  const [source, setSource] = useState('')
  const [detailType, setDetailType] = useState('')
  const [properties, setProperties] = useState<DetailProperty[]>([
    { name: 'veteranId', type: 'string' },
  ])

  const create = useMutation<NewSchemaDraft, IpcError>({
    mutationFn: () => ipc.newSchemaDraft(source, detailType, properties),
    onSuccess: (draft) => {
      onCreated(draft)
      onOpenChange(false)
      setSource('')
      setDetailType('')
      setProperties([{ name: 'veteranId', type: 'string' }])
    },
  })

  const update = (index: number, patch: Partial<DetailProperty>) => {
    setProperties((prev) =>
      prev.map((p, i) => (i === index ? { ...p, ...patch } : p)),
    )
  }

  return (
    <Modal
      open={open}
      onClose={() => onOpenChange(false)}
      width={520}
      className="flex max-h-[80vh] flex-col"
    >
          <div className="border-b border-edge p-4">
            <ModalTitle>New schema</ModalTitle>
            <ModalDescription>
              Generates the EventBridge envelope for you. Nothing is written
              until you save.
            </ModalDescription>
          </div>

          <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-4">
            <div className="grid grid-cols-2 gap-3">
              <Field label="Source" hint="kebab-case service, e.g. milo-medical">
                <Input
                  value={source}
                  onChange={(e) => setSource(e.target.value)}
                  placeholder="my-service"
                  autoFocus
                  spellCheck={false}
                />
              </Field>
              <Field
                label="Detail type"
                hint="camelCase words joined by hyphens"
              >
                <Input
                  value={detailType}
                  onChange={(e) => setDetailType(e.target.value)}
                  placeholder="thing-happened"
                  spellCheck={false}
                />
              </Field>
            </div>

            {source && detailType && (
              <Note>
                Registers as{' '}
                <span className="font-mono">
                  {source}@{detailType}
                </span>
              </Note>
            )}

            <div className="flex flex-col gap-2">
              <span className="text-[11px] font-medium text-ink-muted">
                Payload fields
              </span>
              <p className="text-[10px] text-ink-faint">
                Fields ending in id, ids, uuid or arn become required, matching
                the registry's convention.
              </p>

              {properties.map((property, index) => (
                <div key={index} className="flex items-center gap-2">
                  <Input
                    value={property.name}
                    onChange={(e) => update(index, { name: e.target.value })}
                    placeholder="fieldName"
                    spellCheck={false}
                  />
                  <Select
                    value={property.type}
                    onChange={(e) => update(index, { type: e.target.value })}
                  >
                    {PROPERTY_TYPES.map((type) => (
                      <option key={type} value={type}>
                        {type}
                      </option>
                    ))}
                  </Select>
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() =>
                      setProperties((prev) => prev.filter((_, i) => i !== index))
                    }
                    title="Remove field"
                  >
                    <X className="size-3" />
                  </Button>
                </div>
              ))}

              <Button
                variant="secondary"
                size="sm"
                className="self-start"
                onClick={() =>
                  setProperties((prev) => [...prev, { name: '', type: 'string' }])
                }
              >
                <Plus className="size-3" />
                Add field
              </Button>
            </div>

            {create.isError && <ErrorBox error={create.error} />}
          </div>

          <div className="flex justify-end gap-2 border-t border-edge p-4">
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button
              variant="primary"
              loading={create.isPending}
              disabled={!source.trim() || !detailType.trim()}
              onClick={() => create.mutate()}
            >
              Create draft
            </Button>
          </div>
    </Modal>
  )
}
