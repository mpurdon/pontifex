import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { Braces, CheckCircle2, GitCompare, Save, Trash2, Wand2 } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type { Finding, IpcError, SchemaDetail, ValidationReport } from '@/lib/types'
import {
  Badge,
  Button,
  EmptyState,
  ErrorBox,
  FindingList,
  Select,
  Spinner,
  Toolbar,
  Segmented,
} from '@/components/ui'
import { JsonDiff, JsonEditor } from '@/components/lazy-editor'
import { stringify } from '@/lib/format'
import { SchemaEditor } from '@/components/schema-editor/schema-editor'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'
import { ConfirmSaveDialog } from './confirm-save-dialog'
import { DeleteSchemaDialog } from './delete-dialog'
import { useWorkbench } from './workbench-context'
import { HistoryPanel } from './history-panel'
import { VersionTimeline } from './version-timeline'
import { useSchemaHistory } from './use-schema-history'

type View = 'structure' | 'edit' | 'diffLive' | 'diffVersion' | 'history' | 'timeline'

export function SchemaDetailPanel({
  name,
  onDeleted,
  onSendToAi,
}: {
  name: string
  onDeleted: () => void
  onSendToAi: (detail: SchemaDetail) => void
}) {
  const { envId, activeEnvironment } = useSettings()
  const queryClient = useQueryClient()
  const credentials = useLoginForEnvironment(activeEnvironment)

  // Structure is the default: it is the view that makes a ref-composed
  // document legible without jumping between definitions.
  const [view, setView] = useState<View>('structure')
  const [draft, setDraft] = useState<string | null>(null)
  const [compareVersion, setCompareVersion] = useState<string | null>(null)
  const [deleting, setDeleting] = useState(false)
  const [confirmingSave, setConfirmingSave] = useState(false)
  const { analysisOpen, setAnalysisOpen } = useWorkbench()

  const detail = useQuery({
    queryKey: ['schemas', envId, 'detail', name],
    queryFn: () => ipc.describeSchema(name, undefined, envId),
    enabled: !!envId,
    retry: false,
  })

  const versions = useQuery({
    queryKey: ['schemas', envId, 'versions', name],
    queryFn: () => ipc.listSchemaVersions(name, envId),
    enabled: !!envId,
    retry: false,
  })

  const liveText = useMemo(
    () => (detail.data ? stringify(detail.data.content) : ''),
    [detail.data],
  )
  const text = draft ?? liveText
  const dirty = draft !== null && draft !== liveText

  /**
   * The draft as a document, for the structure editor. `value` is null while
   * the JSON is mid-edit and unparseable, which is why the structure view can
   * be empty even though there is text.
   *
   * The syntax error rides along so the validation effect below does not have
   * to parse the same string a second time to produce it.
   */
  const parsed = useMemo<{ value: unknown; error: string | null }>(() => {
    if (!text) return { value: null, error: null }
    try {
      return { value: JSON.parse(text) as unknown, error: null }
    } catch (e) {
      return { value: null, error: e instanceof Error ? e.message : String(e) }
    }
  }, [text])

  const parseError = parsed.error

  // Validate the draft locally as it changes. Cheap (pure Rust, no AWS) so a
  // short debounce is enough to keep typing smooth.
  const [validation, setValidation] = useState<ValidationReport | null>(null)

  useEffect(() => {
    if (!text || parsed.value === null) {
      setValidation(null)
      return
    }
    const timer = setTimeout(async () => {
      setValidation(await ipc.validateSchema(parsed.value, name))
    }, 250)
    return () => clearTimeout(timer)
  }, [text, name, parsed])

  const save = useMutation<unknown, IpcError, string | undefined>({
    mutationFn: (description) =>
      ipc.putSchema(name, JSON.parse(text), description, envId),
    onSuccess: () => {
      setDraft(null)
      setConfirmingSave(false)
      queryClient.invalidateQueries({ queryKey: ['schemas', envId] })
    },
  })

  /**
   * Every write goes through the dialog; only protected ones are gated by it.
   *
   * It was protected-only because its job was the acknowledgement, and making
   * dev ask "are you sure" for every keystroke would have taught everyone to
   * click through it. It now also carries the version description, which every
   * version wants and which has nowhere else to be written — so the dialog
   * opens for all writes and asks for the acknowledgement only where that was
   * always the point.
   */
  const requestSave = () => setConfirmingSave(true)

  /**
   * Version history, indexed by field.
   *
   * Fetched alongside the schema so the structure editor can mark fields this
   * schema has already been changed for — the point being that a field edited
   * three times before is worth a second look on the fourth.
   */
  const { ledger } = useSchemaHistory(name, envId)
  const fieldHistory = ledger.byField

  const compare = useQuery({
    queryKey: ['schemas', envId, 'detail', name, compareVersion],
    queryFn: () => ipc.describeSchema(name, compareVersion!, envId),
    enabled: !!envId && !!compareVersion && view === 'diffVersion',
    retry: false,
  })

  if (detail.isLoading) return <Spinner label="Loading schema…" />
  if (detail.isError) {
    return (
      <div className="p-3">
        <ErrorBox error={ipc.asIpcError(detail.error)} {...credentials} />
      </div>
    )
  }
  if (!detail.data) return null

  const findings: Finding[] = validation?.findings ?? detail.data.validation.findings
  const errorCount = findings.filter((f) => f.severity === 'error').length
  const warningCount = findings.filter((f) => f.severity === 'warning').length
  const canSave = dirty && !parseError && (validation?.valid ?? false)

  return (
    <div className="flex h-full flex-col">
      <Toolbar>
        <div className="flex min-w-0 flex-1 items-center gap-2">
          <span className="truncate font-mono text-xs text-ink" title={detail.data.name}>
            {detail.data.name}
          </span>
          <Badge tone="neutral">v{detail.data.version}</Badge>
          {dirty && <Badge tone="accent">unsaved</Badge>}
          {errorCount > 0 ? (
            <Badge tone="danger">
              {errorCount} error{errorCount === 1 ? '' : 's'}
            </Badge>
          ) : warningCount > 0 ? (
            <Badge tone="warn">
              {warningCount} warning{warningCount === 1 ? '' : 's'}
            </Badge>
          ) : (
            <Badge tone="ok">
              <CheckCircle2 className="size-2.5" />
              valid
            </Badge>
          )}
        </div>

        <div className="flex items-center gap-1">
          <Segmented
            value={view}
            options={[
              { id: 'structure', label: 'Structure' },
              { id: 'edit', label: 'JSON' },
              { id: 'diffLive', label: 'vs live', disabled: !dirty },
              { id: 'diffVersion', label: 'vs version' },
              { id: 'history', label: 'History' },
              { id: 'timeline', label: 'Timeline' },
            ]}
            onChange={(id) => {
              setView(id)
              if (id === 'diffVersion' && !compareVersion) {
                // Default to the previous version, the usual comparison.
                const previous = versions.data?.find(
                  (v) => v.version !== detail.data!.version,
                )
                setCompareVersion(previous?.version ?? null)
              }
            }}
          />

          {view === 'diffVersion' && (
            <Select
              value={compareVersion ?? ''}
              onChange={(e) => setCompareVersion(e.target.value)}
            >
              <option value="">choose…</option>
              {versions.data
                ?.filter((v) => v.version !== detail.data!.version)
                .map((v) => (
                  <option key={v.version} value={v.version}>
                    v{v.version}
                  </option>
                ))}
            </Select>
          )}

          <Button
            variant="ghost"
            size="sm"
            title="Send to the AI panel"
            onClick={() => onSendToAi(detail.data!)}
          >
            <Wand2 className="size-3" />
          </Button>

          <Button
            variant="ghost"
            size="sm"
            title="Relax required fields the way the registry expects"
            onClick={async () => {
              try {
                const preview = await ipc.simplifySchema(JSON.parse(text))
                setDraft(stringify(preview.content))
                setView('diffLive')
              } catch {
                // A parse failure is already surfaced by the editor banner.
              }
            }}
          >
            Simplify
          </Button>

          {dirty && (
            <Button variant="ghost" size="sm" onClick={() => setDraft(null)}>
              Revert
            </Button>
          )}

          <Button
            variant="primary"
            size="sm"
            loading={save.isPending}
            disabled={!canSave}
            onClick={requestSave}
            title={
              !dirty
                ? 'No changes to save'
                : parseError
                  ? 'Fix the JSON syntax error first'
                  : !validation?.valid
                    ? 'Fix the validation errors first'
                    : 'Save as a new schema version'
            }
          >
            <Save className="size-3" />
            Save
          </Button>

          <Button
            variant="danger"
            size="sm"
            onClick={() => setDeleting(true)}
            title="Delete this schema"
          >
            <Trash2 className="size-3" />
          </Button>
        </div>
      </Toolbar>

      {save.isError && (
        <div className="px-3 pt-2">
          <ErrorBox error={save.error} {...credentials} />
        </div>
      )}
      {parseError && (
        <div className="px-3 pt-2">
          <ErrorBox error={{ kind: 'invalid', message: `JSON syntax: ${parseError}` }} />
        </div>
      )}

      <div className="min-h-0 flex-1">
        {view === 'structure' &&
          (parsed.value ? (
            <SchemaEditor
              document={parsed.value}
              onChange={(next) => setDraft(stringify(next))}
              findings={findings}
              busName={`${activeEnvironment?.label ?? 'dev'}-global-bus`}
              region={activeEnvironment?.region}
              schemaName={name}
              envId={envId}
              environmentLabel={activeEnvironment?.label}
              registryName={activeEnvironment?.registryName}
              // The registry's current document, so the tree marks what this
              // draft would add, remove or change.
              baseline={detail.data.content}
              // Held above the router so changing screens does not reset it.
              analysisOpen={analysisOpen}
              onAnalysisOpenChange={setAnalysisOpen}
              fieldHistory={fieldHistory}
            />
          ) : (
            <EmptyState
              icon={<Braces className="size-8" />}
              title="Structure view needs valid JSON"
              detail="Fix the syntax error in the JSON view, then come back."
              action={
                <Button variant="secondary" onClick={() => setView('edit')}>
                  Open JSON view
                </Button>
              }
            />
          ))}

        {view === 'edit' && (
          <JsonEditor value={text} onChange={setDraft} findings={findings} />
        )}

        {view === 'diffLive' && (
          <JsonDiff
            original={liveText}
            modified={text}
            originalLabel={`Live in ${activeEnvironment?.registryName ?? 'the registry'} · v${detail.data.version}`}
            modifiedLabel={dirty ? 'Your unsaved draft' : 'Your copy — no changes yet'}
          />
        )}

        {view === 'diffVersion' &&
          (compare.isLoading ? (
            <Spinner label="Loading version…" />
          ) : compare.data ? (
            <JsonDiff
              original={stringify(compare.data.content)}
              modified={text}
              originalLabel={`Previous · v${compare.data.version}`}
              modifiedLabel={
                dirty
                  ? `Your unsaved draft (from v${detail.data.version})`
                  : `Live · v${detail.data.version}`
              }
            />
          ) : (
            <div className="flex h-full items-center justify-center gap-2 text-xs text-ink-faint">
              <GitCompare className="size-4" />
              Choose a version to compare against
            </div>
          ))}

        {view === 'timeline' && <VersionTimeline name={name} envId={envId} />}

        {view === 'history' && (
          <HistoryPanel
            name={name}
            envId={envId}
            currentVersion={detail.data.version}
            selectedVersion={compareVersion}
            // Picking a version from the timeline jumps straight to the diff —
            // seeing what changed is the reason you were reading the list.
            onSelectVersion={(version) => {
              setCompareVersion(version)
              setView('diffVersion')
            }}
          />
        )}
      </div>

      {findings.length > 0 && (
        <div className="max-h-32 shrink-0 overflow-auto border-t border-edge bg-surface-1 p-2">
          <FindingList findings={findings} />
          {/* Offered beside the finding rather than in the toolbar, so the
              repair stays attached to the reason for it. The validator says
              which findings have one; the editor does not guess. */}
          {findings.some((f) => f.fix === 'widenNullable') && (
            <div className="mt-1 pl-2">
              <Button
                variant="ghost"
                size="sm"
                title={
                  'Rewrite every `nullable: true` as a type that includes "null", ' +
                  'which draft-07 honours — then review the diff before saving.'
                }
                onClick={async () => {
                  try {
                    const repair = await ipc.widenNullableSchema(JSON.parse(text))
                    setDraft(stringify(repair.content))
                    setView('diffLive')
                  } catch {
                    // A parse failure is already surfaced by the editor banner.
                  }
                }}
              >
                Fix all <span className="font-mono">nullable</span> fields
              </Button>
            </div>
          )}
        </div>
      )}

      <ConfirmSaveDialog
        open={confirmingSave}
        schemaName={name}
        environmentLabel={activeEnvironment?.label ?? ''}
        registryName={activeEnvironment?.registryName ?? ''}
        before={liveText}
        after={text}
        isNew={false}
        saving={save.isPending}
        error={save.error}
        requiresAcknowledgement={activeEnvironment?.protected ?? false}
        onCancel={() => setConfirmingSave(false)}
        onConfirm={(description) => save.mutate(description)}
      />

      <DeleteSchemaDialog
        open={deleting}
        name={name}
        onOpenChange={setDeleting}
        onDeleted={() => {
          setDeleting(false)
          queryClient.invalidateQueries({ queryKey: ['schemas', envId] })
          onDeleted()
        }}
      />
    </div>
  )
}
