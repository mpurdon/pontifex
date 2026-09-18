import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate, useSearchParams } from 'react-router-dom'
import { Braces, Download, Plus, RefreshCw, Upload } from 'lucide-react'
import * as ipc from '@/lib/ipc'
import type {
  InferredDraft,
  IpcError,
  NewSchemaDraft,
  SchemaDetail,
  SchemaSummary,
} from '@/lib/types'
import {
  Badge,
  Button,
  EmptyState,
  ErrorBox,
  Input,
  Note,
  Segmented,
  Spinner,
  Toolbar,
} from '@/components/ui'
import { JsonEditor } from '@/components/lazy-editor'
import { stringify } from '@/lib/format'
import { invalidNameChars, sanitizeSchemaName } from '@/lib/schema-name'
import { SchemaEditor } from '@/components/schema-editor/schema-editor'
import { useSettings } from '@/app/settings-context'
import { useLoginForEnvironment } from '@/app/login-dialog'
import {
  ResizableGroup,
  ResizablePanel,
  ResizeHandle,
} from '@/components/resizable'
import { InferringPanel } from './inferring-panel'
import { SchemaList } from './schema-list'
import { SchemaDetailPanel } from './schema-detail'
import { ConfirmSaveDialog } from './confirm-save-dialog'
import { NewSchemaDialog } from './new-schema-dialog'
import { TransferDialog } from './transfer-dialog'
import { useAiDraft } from '../ai/ai-draft-context'
import { useWorkbench } from './workbench-context'

export function SchemasPage() {
  const { envId, activeEnvironment } = useSettings()
  const queryClient = useQueryClient()
  const navigate = useNavigate()
  const credentials = useLoginForEnvironment(activeEnvironment)
  const { setDraft: setAiDraft } = useAiDraft()

  // The health report links here with ?select=<schema>, so a finding is one
  // click from the schema that needs fixing. The selection itself lives above
  // the router, so walking to another screen and back does not lose it.
  const [searchParams, setSearchParams] = useSearchParams()
  const { selected, select: setSelected } = useWorkbench()

  useEffect(() => {
    const requested = searchParams.get('select')
    if (requested && requested !== selected) setSelected(requested)
  }, [searchParams, selected, setSelected])

  /**
   * `?draft=source@detail-type` arrives from the health report's "on the bus
   * with no schema" list. The shape is inferred from that event type's own
   * traffic, so the draft opens already describing what is really being sent.
   */
  const draftRequest = searchParams.get('draft')
  const [inferredFrom, setInferredFrom] = useState<InferredDraft | null>(null)
  /** The event type being inferred right now, or null. Gates the progress panel. */
  const [inferring, setInferring] = useState<string | null>(null)
  const [inferError, setInferError] = useState<IpcError | null>(null)
  /** The `?draft=` value already acted on, so a re-render cannot re-fire it. */
  const inferRef = useRef<string | null>(null)
  /**
   * Which request may still apply its result. Cancel clears it, so a call
   * that comes back afterwards is dropped rather than opening a draft nobody
   * asked for any more.
   */
  const activeRef = useRef<symbol | null>(null)

  useEffect(() => {
    if (!draftRequest || !envId || inferRef.current === draftRequest) return
    inferRef.current = draftRequest

    // Read the companions before the parameters are consumed below.
    const [source, ...rest] = draftRequest.split('@')
    const minutes = Number(searchParams.get('minutes')) || undefined
    // `group` names the log group the requester saw the type in; the Watch
    // screen passes it so the draft reads one group instead of all of them.
    const logGroup = searchParams.get('group') || undefined
    // `around` is the hit's own timestamp, so the sample is the two minutes
    // around an event known to exist rather than a day's scan.
    const aroundMs = Number(searchParams.get('around')) || undefined

    // Consume the parameter *before* starting the call, not after it settles:
    // `?draft=` is a one-shot command, and leaving it in the URL let any
    // re-render that got past the guard start the call again.
    setSearchParams({}, { replace: true })

    // Deliberately a plain promise with local state rather than `useMutation`.
    // React's development double-mount unsubscribes and re-subscribes the
    // mutation observer, and TanStack detaches an unsubscribed observer from
    // its in-flight mutation without re-attaching it — so a mutation started
    // in a mount effect finished in milliseconds while `isPending` stayed
    // true forever and the spinner counted on. A promise has no observer to
    // lose.
    const token = Symbol(draftRequest)
    activeRef.current = token
    setInferring(draftRequest)
    setInferError(null)
    ipc
      .draftFromEvents(source, rest.join('@'), envId, minutes, logGroup, aroundMs)
      .then((draft) => {
        if (activeRef.current !== token) return
        setSelected(null)
        setPendingDraft({
          name: draft.name,
          content: draft.content,
          fileName: draft.fileName,
          validation: draft.validation,
        })
        setInferredFrom(draft)
      })
      .catch((e: unknown) => {
        if (activeRef.current !== token) return
        setInferError(ipc.asIpcError(e))
      })
      .finally(() => {
        if (activeRef.current !== token) return
        activeRef.current = null
        setInferring(null)
        // Free the request so the same event type can be drafted again later.
        inferRef.current = null
      })
  }, [draftRequest, envId, searchParams, setSearchParams, setSelected])
  const [creating, setCreating] = useState(false)
  const [transfer, setTransfer] = useState<'import' | 'export' | null>(null)
  /** An unsaved, not-yet-registered draft from the New Schema wizard. */
  const [pendingDraft, setPendingDraft] = useState<NewSchemaDraft | null>(null)

  const schemas = useQuery({
    queryKey: ['schemas', envId, 'list'],
    queryFn: () => ipc.listSchemas(envId),
    enabled: !!envId,
    retry: false,
  })

  /** Registry contents by name, so a draft can tell whether it is really new. */
  const registeredByName = useMemo(
    () => new Map((schemas.data ?? []).map((s) => [s.name, s])),
    [schemas.data],
  )

  const sendToAi = (detail: SchemaDetail) => {
    setAiDraft({ name: detail.name, content: detail.content })
    navigate('/generate')
  }

  return (
    <>
      <ResizableGroup
      id="schemas-page"
      panelIds={['registry', 'detail']}
      className="h-full"
    >
      <ResizablePanel
        id="registry"
        defaultSize="22"
        minSize="12"
        className="flex flex-col border-r border-edge bg-surface-1"
      >
        <div className="flex h-9 shrink-0 items-center gap-1 border-b border-edge px-2">
          <span className="text-xs font-semibold text-ink">Registry</span>
          <div className="ml-auto flex items-center gap-0.5">
            <Button
              variant="ghost"
              size="sm"
              title="Reload from AWS"
              onClick={() =>
                queryClient.invalidateQueries({ queryKey: ['schemas', envId] })
              }
            >
              <RefreshCw
                className={schemas.isFetching ? 'size-3 animate-spin' : 'size-3'}
              />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="Import from a directory"
              onClick={() => setTransfer('import')}
            >
              <Upload className="size-3" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="Export to a directory"
              onClick={() => setTransfer('export')}
            >
              <Download className="size-3" />
            </Button>
            <Button
              variant="ghost"
              size="sm"
              title="New schema"
              onClick={() => setCreating(true)}
            >
              <Plus className="size-3" />
            </Button>
          </div>
        </div>

        {schemas.isLoading && <Spinner label="Loading schemas…" />}

        {schemas.isError && (
          <div className="p-2">
            <ErrorBox error={ipc.asIpcError(schemas.error)} {...credentials} />
          </div>
        )}

        {schemas.data && (
          <SchemaList
            schemas={schemas.data}
            selected={selected}
            onSelect={(name) => {
              setPendingDraft(null)
              setSelected(name)
              // Drop the deep-link param so a later reload does not fight the
              // user's current selection.
              if (searchParams.has('select')) setSearchParams({}, { replace: true })
            }}
          />
        )}
      </ResizablePanel>

      <ResizeHandle />

      <ResizablePanel id="detail" defaultSize="78" minSize="30" className="flex flex-col">
        {inferring ? (
          <InferringPanel
            schemaName={inferring}
            onCancel={() => {
              // The call itself cannot be aborted; its result is simply dropped.
              activeRef.current = null
              inferRef.current = null
              setInferring(null)
              setSearchParams({}, { replace: true })
            }}
          />
        ) : inferError ? (
          <div className="p-3">
            <ErrorBox error={inferError} {...credentials} />
          </div>
        ) : pendingDraft ? (
          <PendingDraftPanel
            draft={pendingDraft}
            inferredFrom={inferredFrom}
            registered={registeredByName}
            onOpenExisting={(name) => {
              setPendingDraft(null)
              setInferredFrom(null)
              setSelected(name)
            }}
            onDiscard={() => {
              setPendingDraft(null)
              setInferredFrom(null)
            }}
            onSaved={(name) => {
              setPendingDraft(null)
              setInferredFrom(null)
              setSelected(name)
              queryClient.invalidateQueries({ queryKey: ['schemas', envId] })
            }}
          />
        ) : selected ? (
          <SchemaDetailPanel
            key={selected}
            name={selected}
            onDeleted={() => setSelected(null)}
            onSendToAi={sendToAi}
          />
        ) : (
          <EmptyState
            icon={<Braces className="size-8" />}
            title="No schema selected"
            detail={
              schemas.data
                ? `${schemas.data.length} schemas in ${activeEnvironment?.registryName}. Pick one from the list, or create a new one.`
                : undefined
            }
            action={
              <Button variant="primary" onClick={() => setCreating(true)}>
                <Plus className="size-3" />
                New schema
              </Button>
            }
          />
        )}
      </ResizablePanel>

    </ResizableGroup>

      <NewSchemaDialog
        open={creating}
        onOpenChange={setCreating}
        onCreated={(draft) => {
          setSelected(null)
          setPendingDraft(draft)
        }}
      />

      <TransferDialog
        open={transfer !== null}
        mode={transfer ?? 'import'}
        onOpenChange={(open) => !open && setTransfer(null)}
        onImported={() =>
          queryClient.invalidateQueries({ queryKey: ['schemas', envId] })
        }
      />
    </>
  )
}

/**
 * A generated-but-unregistered schema. Kept separate from the saved-schema
 * panel because there is no live version to diff against yet, and discarding
 * it should not touch AWS.
 */
function PendingDraftPanel({
  draft,
  inferredFrom,
  registered,
  onDiscard,
  onSaved,
  onOpenExisting,
}: {
  draft: NewSchemaDraft
  /** Set when the draft was inferred from traffic rather than hand-built. */
  inferredFrom?: InferredDraft | null
  /**
   * What the registry already holds, by name.
   *
   * A draft inferred from traffic is named after the event's own source, which
   * may have to be sanitized before it can be registered — so the name it will
   * actually be written under can already exist even though the draft looks new.
   */
  registered: Map<string, SchemaSummary>
  onDiscard: () => void
  onSaved: (name: string) => void
  onOpenExisting: (name: string) => void
}) {
  const { envId, activeEnvironment } = useSettings()
  const credentials = useLoginForEnvironment(activeEnvironment)
  const [text, setText] = useState(() => stringify(draft.content))
  /**
   * The name to register under, separate from the draft's own name.
   *
   * Sources on the bus can contain characters EventBridge forbids in a schema
   * name — `Atomic Forms` has a space — so an inferred draft can arrive with a
   * name that can never be registered. Defaulting to the sanitized form makes
   * the write succeed; the document keeps the real source either way.
   */
  const [name, setName] = useState(() => sanitizeSchemaName(draft.name))
  const nameWasSanitized = name !== draft.name
  /** The schema this draft would write over, if the name is already taken. */
  const existing = registered.get(name)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<IpcError | null>(null)
  const [confirmingSave, setConfirmingSave] = useState(false)
  // A brand-new schema is mostly about shaping the payload, so open on the
  // structure editor rather than the generated boilerplate.
  const [view, setView] = useState<'structure' | 'json'>('structure')

  const parsed = useMemo(() => {
    try {
      return JSON.parse(text) as unknown
    } catch {
      return null
    }
  }, [text])

  const save = async (description?: string) => {
    setSaving(true)
    setError(null)
    try {
      await ipc.putSchema(name, JSON.parse(text), description, envId)
      setConfirmingSave(false)
      onSaved(name)
    } catch (e) {
      setError(ipc.asIpcError(e))
    } finally {
      setSaving(false)
    }
  }

  // The dialog is where the version description is written, so a create goes
  // through it too. Only a protected environment is gated by it.
  const requestSave = () => setConfirmingSave(true)

  return (
    <div className="flex h-full flex-col">
      <Toolbar>
        <span className="truncate font-mono text-xs text-ink">{name}</span>
        {existing ? (
          <Badge
            tone="warn"
            title={`${name} is already in ${activeEnvironment?.registryName}. Saving adds a version rather than creating a schema.`}
          >
            already registered · v{existing.version}
          </Badge>
        ) : (
          <Badge tone="accent">new · not registered</Badge>
        )}
        {inferredFrom && (
          <Badge
            tone="info"
            title={
              inferredFrom.fromCache
                ? 'Shape inferred from cached events'
                : 'Shape inferred from events fetched just now'
            }
          >
            inferred from {inferredFrom.sampled} event
            {inferredFrom.sampled === 1 ? '' : 's'}
          </Badge>
        )}
        <div className="ml-auto flex items-center gap-1.5">
          <Segmented
            value={view}
            options={[
              { id: 'structure', label: 'Structure' },
              { id: 'json', label: 'JSON' },
            ]}
            onChange={setView}
          />
          <Button variant="ghost" size="sm" onClick={onDiscard}>
            Discard
          </Button>
          <Button
            variant="primary"
            size="sm"
            loading={saving}
            disabled={invalidNameChars(name).length > 0}
            onClick={requestSave}
          >
            {existing ? 'Save as a new version' : `Register to ${activeEnvironment?.label}`}
          </Button>
        </div>
      </Toolbar>

      {existing && (
        <div className="px-3 pt-2">
          <Note tone="warn">
            <span className="font-mono text-ink">{name}</span> is already registered in{' '}
            <span className="font-mono text-ink">{activeEnvironment?.registryName}</span>{' '}
            at <span className="font-mono text-ink">v{existing.version}</span>. Saving
            this draft adds a new version rather than creating a schema — you will not
            see what changed, because a draft has nothing to diff against.
            <button
              type="button"
              onClick={() => onOpenExisting(name)}
              className="mt-1 block text-[11px] text-accent hover:underline"
            >
              Open the registered schema instead →
            </button>
          </Note>
        </div>
      )}

      {nameWasSanitized && (
        <div className="px-3 pt-2">
          <Note>
            EventBridge only allows letters, digits, <span className="font-mono">_</span>{' '}
            <span className="font-mono">.</span> <span className="font-mono">-</span> and{' '}
            <span className="font-mono">@</span> in a schema name, and{' '}
            <span className="font-mono text-ink">{draft.name}</span> contains characters
            it rejects. It will be registered as the name below; the envelope keeps the
            real source, so events still match.
            <label className="mt-2 flex items-center gap-2">
              <span className="shrink-0 text-[11px] text-ink-muted">Register as</span>
              <Input
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="flex-1 font-mono"
                spellCheck={false}
              />
            </label>
            {invalidNameChars(name).length > 0 && (
              <span className="mt-1 block text-[11px] text-danger">
                Still contains characters EventBridge rejects.
              </span>
            )}
          </Note>
        </div>
      )}

      {error && (
        <div className="px-3 pt-2">
          <ErrorBox error={error} {...credentials} />
        </div>
      )}

      <ConfirmSaveDialog
        open={confirmingSave}
        schemaName={name}
        environmentLabel={activeEnvironment?.label ?? ''}
        registryName={activeEnvironment?.registryName ?? ''}
        before=""
        after={text}
        isNew
        saving={saving}
        error={error}
        requiresAcknowledgement={activeEnvironment?.protected ?? false}
        onCancel={() => setConfirmingSave(false)}
        onConfirm={(description) => void save(description)}
      />

      <div className="min-h-0 flex-1">
        {view === 'structure' && parsed ? (
          <SchemaEditor
            document={parsed}
            onChange={(next) => setText(stringify(next))}
            findings={draft.validation.findings}
            busName={`${activeEnvironment?.label ?? 'dev'}-global-bus`}
            region={activeEnvironment?.region}
            schemaName={draft.name}
            envId={envId}
            environmentLabel={activeEnvironment?.label}
            registryName={activeEnvironment?.registryName}
            // For an unsaved draft the baseline is the document as first
            // generated, so the tree marks what you have changed since —
            // there is no registry version to compare against yet.
            baseline={draft.content}
          />
        ) : (
          <JsonEditor
            value={text}
            onChange={setText}
            findings={draft.validation.findings}
          />
        )}
      </div>
    </div>
  )
}
