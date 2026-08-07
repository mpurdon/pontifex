import { useMutation } from '@tanstack/react-query'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import { useState } from 'react'
import { FolderOpen } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  ApplyImportResult,
  ExportResult,
  ImportPlan,
  ImportPlanEntry,
  IpcError,
} from '@/lib/types'
import {
  Badge,
  Button,
  Checkbox,
  ErrorBox,
  Input,
  Modal,
  ModalDescription,
  ModalTitle,
  Note,
  Spinner,
  cn,
} from '@/components/ui'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'

type Mode = 'import' | 'export'

const actionTone = {
  create: 'ok',
  update: 'warn',
  unchanged: 'neutral',
  error: 'danger',
} as const

/**
 * Directory-based import/export against the registry.
 *
 * Import is always two-phase: a plan showing create/update/unchanged per file,
 * then an explicit apply of only the ticked entries. The registry is the
 * source of truth, so bulk-writing a directory without review would be the
 * wrong default.
 */
/** Per-item failures from a batch operation. Renders nothing when empty. */
function FailedList({ failed }: { failed: [string, string][] }) {
  if (failed.length === 0) return null
  return (
    <ul className="mt-1 text-danger">
      {failed.map(([name, message]) => (
        <li key={name}>
          <span className="font-mono">{name}</span>: {message}
        </li>
      ))}
    </ul>
  )
}

export function TransferDialog({
  open,
  mode,
  onOpenChange,
  onImported,
}: {
  open: boolean
  mode: Mode
  onOpenChange: (open: boolean) => void
  onImported: () => void
}) {
  const { envId, activeEnvironment, settings } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)

  const [directory, setDirectory] = useState('')
  const [simplify, setSimplify] = useState(mode === 'import')
  const [plan, setPlan] = useState<ImportPlan | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())

  const browse = async () => {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      defaultPath: settings?.eventBusRepoPath ?? undefined,
      title: mode === 'import' ? 'Choose a schema directory' : 'Choose an export directory',
    })
    if (typeof picked === 'string') setDirectory(picked)
  }

  const buildPlan = useMutation<ImportPlan, IpcError>({
    mutationFn: () => ipc.planImport(directory, simplify, envId),
    onSuccess: (result) => {
      setPlan(result)
      // Pre-select everything that would actually change and is valid; the
      // user opts out rather than in.
      setSelected(
        new Set(
          result.entries
            .filter(
              (e) =>
                (e.action === 'create' || e.action === 'update') &&
                (e.validation?.valid ?? false),
            )
            .map((e) => e.name),
        ),
      )
    },
  })

  const apply = useMutation<ApplyImportResult, IpcError>({
    mutationFn: () =>
      ipc.applyImport(
        plan!.entries
          .filter((e) => selected.has(e.name))
          .map((e) => ({ name: e.name, content: e.content })),
        envId,
      ),
    onSuccess: onImported,
  })

  const exportAll = useMutation<ExportResult, IpcError>({
    mutationFn: () => ipc.exportSchemas(directory, [], simplify, envId),
  })

  const toggle = (name: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(name)) next.delete(name)
      else next.add(name)
      return next
    })
  }

  const reset = () => {
    setPlan(null)
    setSelected(new Set())
    buildPlan.reset()
    apply.reset()
    exportAll.reset()
  }

  return (
    <Modal
      open={open}
      onClose={() => {
        reset()
        onOpenChange(false)
      }}
      width={680}
      className="flex max-h-[82vh] flex-col"
    >
          <div className="border-b border-edge p-4">
            <ModalTitle>
              {mode === 'import' ? 'Import schemas' : 'Export schemas'}
            </ModalTitle>
            <ModalDescription>
              {mode === 'import'
                ? `Read a directory of schema files and register them to ${activeEnvironment?.registryName}.`
                : `Write every schema in ${activeEnvironment?.registryName} to a directory, in the repo's file format.`}
            </ModalDescription>
          </div>

          <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-4">
            <div className="flex items-end gap-2">
              <label className="flex flex-1 flex-col gap-1">
                <span className="text-[11px] font-medium text-ink-muted">
                  Directory
                </span>
                <Input
                  value={directory}
                  onChange={(e) => setDirectory(e.target.value)}
                  placeholder="~/Projects/trajector/global-event-bus/schemas-simplified"
                  spellCheck={false}
                />
              </label>
              <Button variant="secondary" onClick={browse}>
                <FolderOpen className="size-3" />
                Browse
              </Button>
            </div>

            <Checkbox
              checked={simplify}
              onChange={(e) => setSimplify(e.target.checked)}
              label={
                mode === 'import'
                  ? 'Simplify before registering (relax non-ID required fields)'
                  : 'Simplify on export'
              }
            />

            {mode === 'import' && simplify && (
              <Note>
                The registry holds simplified schemas. Leave this on when
                importing from the repo's full{' '}
                <span className="font-mono">schemas/</span> directory; turn it
                off when importing from{' '}
                <span className="font-mono">schemas-simplified/</span>.
              </Note>
            )}

            {mode === 'import' && (
              <>
                <Button
                  variant="secondary"
                  className="self-start"
                  disabled={!directory.trim()}
                  loading={buildPlan.isPending}
                  onClick={() => buildPlan.mutate()}
                >
                  Preview changes
                </Button>

                {buildPlan.isError && (
                  <ErrorBox error={buildPlan.error} {...credentials} />
                )}

                {buildPlan.isPending && <Spinner label="Comparing against the registry…" />}

                {plan && <ImportPlanTable plan={plan} selected={selected} onToggle={toggle} />}

                {apply.isError && <ErrorBox error={apply.error} {...credentials} />}

                {apply.data && (
                  <div className="rounded-md border border-ok/40 bg-ok/10 px-3 py-2 text-xs text-ok">
                    Registered {apply.data.applied.length} schema
                    {apply.data.applied.length === 1 ? '' : 's'}.
                    <FailedList failed={apply.data.failed} />
                  </div>
                )}
              </>
            )}

            {mode === 'export' && (
              <>
                {exportAll.isError && (
                  <ErrorBox error={exportAll.error} {...credentials} />
                )}
                {exportAll.data && (
                  <div className="rounded-md border border-ok/40 bg-ok/10 px-3 py-2 text-xs text-ok">
                    Wrote {exportAll.data.written.length} file
                    {exportAll.data.written.length === 1 ? '' : 's'} to{' '}
                    <span className="font-mono">{exportAll.data.directory}</span>
                    <FailedList failed={exportAll.data.failed} />
                  </div>
                )}
              </>
            )}
          </div>

          <div className="flex items-center justify-end gap-2 border-t border-edge p-4">
            <Button variant="ghost" onClick={() => onOpenChange(false)}>
              Close
            </Button>
            {mode === 'import' ? (
              <Button
                variant="primary"
                disabled={!plan || selected.size === 0}
                loading={apply.isPending}
                onClick={() => apply.mutate()}
              >
                Register {selected.size} schema{selected.size === 1 ? '' : 's'}
              </Button>
            ) : (
              <Button
                variant="primary"
                disabled={!directory.trim()}
                loading={exportAll.isPending}
                onClick={() => exportAll.mutate()}
              >
                Export all
              </Button>
            )}
          </div>
    </Modal>
  )
}

function ImportPlanTable({
  plan,
  selected,
  onToggle,
}: {
  plan: ImportPlan
  selected: Set<string>
  onToggle: (name: string) => void
}) {
  if (plan.entries.length === 0) {
    return (
      <p className="text-xs text-ink-faint">
        No schema files found in {plan.directory}
      </p>
    )
  }

  const counts = plan.entries.reduce<Record<string, number>>((acc, entry) => {
    acc[entry.action] = (acc[entry.action] ?? 0) + 1
    return acc
  }, {})

  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-2">
        {Object.entries(counts).map(([action, count]) => (
          <Badge key={action} tone={actionTone[action as ImportPlanEntry['action']]}>
            {count} {action}
          </Badge>
        ))}
      </div>

      <div className="max-h-64 overflow-auto rounded-md border border-edge">
        <table className="w-full text-[11px]">
          <tbody>
            {plan.entries.map((entry) => {
              const selectable = entry.action === 'create' || entry.action === 'update'
              const invalid = entry.validation && !entry.validation.valid
              return (
                <tr
                  key={entry.file}
                  className="border-b border-edge/50 last:border-0 hover:bg-surface-2"
                >
                  <td className="w-6 pl-2">
                    <input
                      type="checkbox"
                      checked={selected.has(entry.name)}
                      disabled={!selectable}
                      onChange={() => onToggle(entry.name)}
                      className="size-3 accent-[var(--color-accent)]"
                    />
                  </td>
                  <td className="py-1">
                    <span
                      className={cn(
                        'font-mono',
                        invalid ? 'text-danger' : 'text-ink-muted',
                      )}
                      title={entry.file}
                    >
                      {entry.name}
                    </span>
                    {entry.message && (
                      <span className="ml-2 text-danger">{entry.message}</span>
                    )}
                    {invalid && (
                      <span className="ml-2 text-danger">
                        {entry.validation!.findings.find((f) => f.severity === 'error')
                          ?.message}
                      </span>
                    )}
                    {entry.simplifyChanges.length > 0 && (
                      <span className="ml-2 text-ink-faint">
                        simplified {entry.simplifyChanges.length} object
                        {entry.simplifyChanges.length === 1 ? '' : 's'}
                      </span>
                    )}
                  </td>
                  <td className="w-20 pr-2 text-right">
                    <Badge tone={actionTone[entry.action]}>{entry.action}</Badge>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    </div>
  )
}
