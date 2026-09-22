import { useEffect, useMemo, useRef, useState } from 'react'
import {
  Box,
  Copy,
  Check,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Pencil,
  Plus,
  Trash2,
  Bug,
} from 'lucide-react'
import {
  addComponentSchema,
  addProperty,
  buildTree,
  componentSchemas,
  payloadSchemaName,
  refReferrers,
  refUsage,
  removeComponentSchema,
  removeProperty,
  renameComponentSchema,
  renameProperty,
  setAcceptsNull,
  setKeywords,
  setNodeType,
  setRefTarget,
  setRequired,
  type NodeType,
  type SchemaNode,
} from '@/lib/schema-model'
import { buildSampleEvent, putEventsCommand } from '@/lib/sample-event'
import { stringify } from '@/lib/format'
import type { Finding, TicketContext } from '@/lib/types'
import { ConcernDialog, type ConcernSubject } from '@/features/jira/concern-dialog'
import { fieldPathFromPointer } from '@/lib/schema-model'
import {
  Badge,
  Button,
  EmptyState,
  Input,
  Modal,
  ModalDescription,
  ModalTitle,
  Segmented,
  cn,
} from '@/components/ui'
import { countChanges, diffTrees, type ChangeStatus } from '@/lib/schema-diff'
import type { FieldHistory } from '@/lib/schema-history'
import { AnalysisPanel } from './analysis-panel'
import { OriginPanel } from '@/features/origin/origin-panel'
import type { PanelImperativeHandle } from 'react-resizable-panels'
import {
  ResizableGroup,
  ResizablePanel,
  ResizeHandle,
} from '@/components/resizable'
import { SchemaTree } from './tree'
import { Inspector } from './inspector'

/** Summary badges for pending changes, in the order they read best. */
const CHANGE_BADGES: {
  kind: ChangeStatus
  tone: 'ok' | 'danger' | 'warn'
  sign: string
  title: string
}[] = [
  { kind: 'added', tone: 'ok', sign: '+', title: 'Fields this draft adds' },
  { kind: 'removed', tone: 'danger', sign: '−', title: 'Fields this draft removes' },
  { kind: 'changed', tone: 'warn', sign: '~', title: 'Fields this draft modifies' },
]

/** The three things the right-hand column can show about the tree. */
type SideTab = 'details' | 'analysis' | 'origin' | 'sample'

/** Every id from the root down to `target`, so revealing a node is one call. */
function pathTo(root: SchemaNode, targetId: string): string[] | null {
  if (root.id === targetId) return [root.id]
  for (const child of root.children) {
    const path = pathTo(child, targetId)
    if (path) return [root.id, ...path]
  }
  return null
}

function collectIds(node: SchemaNode, depth: number, acc: Set<string>) {
  if (node.depth < depth) {
    acc.add(node.id)
    node.children.forEach((c) => collectIds(c, depth, acc))
  }
}

export interface SchemaEditorProps {
  /** The parsed OpenAPI 3 document. */
  document: unknown
  onChange: (next: unknown) => void
  findings?: Finding[]
  /** Bus name used in the generated `put-events` command. */
  busName?: string
  region?: string
  /** Registry schema name, needed to sample matching events. */
  schemaName?: string
  envId?: string
  /** Named on any ticket filed from the Analysis tab. */
  environmentLabel?: string
  registryName?: string
  /**
   * The document as it currently stands in the registry.
   *
   * Supplied so the tree can mark what an unsaved draft would add, remove or
   * change. Omitted for a schema that does not exist yet, where every field
   * would be an addition and colouring them all says nothing.
   */
  baseline?: unknown
  /**
   * Whether the side panel is showing Analysis rather than the inspector.
   *
   * Optional: supply it — with `onAnalysisOpenChange` — to keep the tab
   * selected across screen changes. Left out, the editor keeps the state
   * itself.
   */
  analysisOpen?: boolean
  onAnalysisOpenChange?: (open: boolean) => void
  /**
   * Per-field change history from previous versions, keyed by node id.
   *
   * Shown while editing so a field this schema has already been changed for
   * carries that fact where the next edit is being made.
   */
  fieldHistory?: Map<string, FieldHistory>
}

/**
 * Structured editor for an EventBridge schema document.
 *
 * Three panes: the document's named *types* on the left, the selected type's
 * shape as a tree in the middle (with `$ref`s resolved inline), and a tabbed
 * side panel on the right — the selected field's details, a sample event, and
 * the analysis against real traffic. They share one column because they are
 * all commentary on the tree, and each used to steal space from it.
 *
 * Vocabulary is deliberate, because "schema" otherwise means two things one
 * click apart: a **schema** is the registry entry, a **type** is a named object
 * inside it, and a **field** is a property on a type.
 */
export function SchemaEditor({
  document,
  onChange,
  findings = [],
  busName,
  region = 'us-east-2',
  schemaName,
  envId,
  environmentLabel,
  registryName,
  baseline,
  analysisOpen,
  onAnalysisOpenChange,
  fieldHistory,
}: SchemaEditorProps) {
  const schemas = useMemo(() => componentSchemas(document), [document])
  const schemaNames = useMemo(() => Object.keys(schemas), [schemas])
  const usage = useMemo(() => refUsage(document), [document])

  // Open on the payload, not the envelope: the envelope is generated
  // boilerplate and nobody edits it by hand.
  const defaultSchema = useMemo(() => payloadSchemaName(document), [document])
  const [activeSchema, setActiveSchema] = useState<string | undefined>(defaultSchema)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  const [removingType, setRemovingType] = useState<string | null>(null)
  const [renamingType, setRenamingType] = useState<string | null>(null)
  // The tab is the state; `analysisOpen` only seeds it and hears about
  // changes, so the caller can keep the choice across screens — walking to the
  // health report and back used to reset it. A second flag to hold in sync
  // would only be a way for the two to disagree.
  const [sideTab, setSideTab] = useState<SideTab>(
    analysisOpen ? 'analysis' : 'details',
  )
  const selectTab = (next: SideTab) => {
    setSideTab(next)
    onAnalysisOpenChange?.(next === 'analysis')
  }
  // Collapsing a side pane is the fastest way to give the tree room, and the
  // side panel is empty until a field is selected.
  const typesPanel = useRef<PanelImperativeHandle>(null)
  const sidePanel = useRef<PanelImperativeHandle>(null)
  const [typesCollapsed, setTypesCollapsed] = useState(false)
  const [sideCollapsed, setSideCollapsed] = useState(false)
  /** Coverage from the last analysis run, annotated onto the tree. */
  const [coverage, setCoverage] = useState<{
    counts: Record<string, number>
    sampled: number
  } | null>(null)
  /** A concern being written up, about the event type or one field. */
  const [concern, setConcern] = useState<ConcernSubject | null>(null)

  /**
   * Where a concern files to. The schema name carries the event's identity,
   * so a draft that is not registered yet can still name its producer.
   */
  const ticketContext: TicketContext | null = useMemo(() => {
    if (!schemaName) return null
    const at = schemaName.indexOf('@')
    return {
      schemaName,
      environment: environmentLabel ?? envId ?? 'unknown',
      registry: registryName ?? null,
      source: at > 0 ? schemaName.slice(0, at) : schemaName,
      detailType: at > 0 ? schemaName.slice(at + 1) : '',
      logGroup: null,
      minutes: null,
      typeName: activeSchema ?? null,
    }
  }, [schemaName, environmentLabel, envId, registryName, activeSchema])

  const raiseFieldConcern = (node: SchemaNode) => {
    const path = fieldPathFromPointer(node.ownPointer)
    const declared = [
      node.refTarget ?? node.type,
      node.format && `format ${node.format}`,
      node.required ? 'required' : 'optional',
      node.acceptsNull && 'nullable',
    ]
      .filter(Boolean)
      .join(', ')
    const seen = coverage?.counts[node.id]
    const observed =
      coverage && seen !== undefined
        ? `present in ${seen} of ${coverage.sampled} sampled events (${Math.round((100 * seen) / Math.max(coverage.sampled, 1))}%)`
        : null
    setConcern({ path, label: `the field ${path}`, declared, observed })
  }

  useEffect(() => {
    if (!activeSchema || !schemaNames.includes(activeSchema)) {
      setActiveSchema(defaultSchema)
    }
  }, [activeSchema, schemaNames, defaultSchema])

  // Built separately from the draft: `document` is a new object on every
  // keystroke, and rebuilding the baseline alongside it re-resolved every
  // `$ref` in the saved document for an input that had not changed.
  const baselineTree = useMemo(
    () => (baseline && activeSchema ? buildTree(baseline, activeSchema) : null),
    [baseline, activeSchema],
  )

  const tree = useMemo(() => {
    if (!activeSchema) return null
    // Diffed against the saved document, so the tree shows pending changes
    // where they are being made rather than only in the JSON diff view.
    return diffTrees(buildTree(document, activeSchema), baselineTree)
  }, [document, baselineTree, activeSchema])

  const changes = useMemo(() => countChanges(tree), [tree])

  // Open the first couple of levels: enough to see the shape without an
  // overwhelming wall on a 61-schema document.
  useEffect(() => {
    if (!tree) return
    setExpanded((prev) => {
      if (prev.size > 0) return prev
      const ids = new Set<string>()
      collectIds(tree, 2, ids)
      return ids
    })
  }, [tree])

  const selected = useMemo(() => {
    if (!tree || !selectedId) return null
    const stack: SchemaNode[] = [tree]
    while (stack.length) {
      const node = stack.pop()!
      if (node.id === selectedId) return node
      stack.push(...node.children)
    }
    return null
  }, [tree, selectedId])

  const errorPointers = useMemo(() => {
    // Findings carry JSON Pointers, so a row can be marked without re-deriving
    // the location from the message.
    return new Set(
      findings.filter((f) => f.severity === 'error' && f.path).map((f) => f.path),
    )
  }, [findings])

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const reveal = (node: SchemaNode) => {
    if (!tree) return
    const path = pathTo(tree, node.id)
    if (path) setExpanded((prev) => new Set([...prev, ...path]))
  }

  const followRef = (target: string) => {
    if (schemaNames.includes(target)) {
      setActiveSchema(target)
      setSelectedId(null)
    }
  }

  const addField = (node: SchemaNode) => {
    const existing = new Set(node.children.map((c) => c.name))
    let name = 'newField'
    let n = 2
    while (existing.has(name)) name = `newField${n++}`
    onChange(addProperty(document, node.pointer, name, 'string'))
    setExpanded((prev) => new Set([...prev, node.id]))
    setSelectedId(`${node.id}/${name}`)
  }

  const addSchema = () => {
    const existing = new Set(schemaNames)
    let name = 'NewType'
    let n = 2
    while (existing.has(name)) name = `NewType${n++}`
    onChange(addComponentSchema(document, name))
    setActiveSchema(name)
  }

  if (schemaNames.length === 0) {
    return (
      <EmptyState
        icon={<Box className="size-8" />}
        title="No types defined"
        detail="This schema has no object types to edit. Use the JSON view to add one."
      />
    )
  }

  const sideTabs: { id: SideTab; label: string; title: string; disabled?: boolean }[] = [
    {
      id: 'details',
      label: 'Details',
      title: 'The selected field’s type, description and constraints',
    },
    {
      id: 'analysis',
      label: 'Analysis',
      disabled: !schemaName,
      title: schemaName
        ? 'Compare this schema with events actually on the bus'
        : 'Needs a schema in the registry to sample against',
    },
    {
      id: 'origin',
      label: 'Origin',
      disabled: !schemaName,
      title: schemaName
        ? 'Who wired this event type in, and who publishes it'
        : 'Needs a schema in the registry to look up',
    },
    // Last: a generated example is the fallback for when there is no real
    // event to look at, and Analysis shows real ones.
    { id: 'sample', label: 'Sample', title: 'Preview a generated sample event' },
  ]
  // A tab that needs a registry name would be a dead end without one, so the
  // column falls back to the inspector.
  const activeTab: SideTab = sideTabs.find((t) => t.id === sideTab)?.disabled
    ? 'details'
    : sideTab

  const treePane = (
    <div className="min-h-0 flex-1 overflow-auto" role="tree">
      {tree ? (
        <SchemaTree
          node={tree}
          selectedId={selectedId}
          expanded={expanded}
          onSelect={(node) => {
            setSelectedId(node.id)
            reveal(node)
            // Picking a field is a request to see it: show Details rather than
            // leaving the selection invisible behind another tab.
            selectTab('details')
          }}
          onToggle={toggle}
          onAddChild={addField}
          onFollowRef={followRef}
          errorPointers={errorPointers}
          coverage={coverage ?? undefined}
          history={fieldHistory}
        />
      ) : (
        <EmptyState title="Select a schema" />
      )}
    </div>
  )

  return (
    <>
      <ResizableGroup
        id="schema-editor"
        panelIds={['types', 'shape', 'inspector']}
        className="h-full"
      >
      {/*
       * Types — the named objects inside this schema document.
       *
       * Deliberately not called "schemas": the registry entry is already a
       * "schema", and using the word for both levels makes it impossible to
       * say which one you mean.
       */}
      <ResizablePanel
        id="types"
        defaultSize="18"
        minSize="8"
        collapsible
        collapsedSize="0"
        panelRef={typesPanel}
        // `onResize` reports a {asPercentage, inPixels} object, not a number:
        // coercing it gave NaN, so the collapsed flag never flipped and the
        // toggle button kept calling collapse() on an already-collapsed pane.
        onResize={(size) => setTypesCollapsed(size.asPercentage === 0)}
        className="flex flex-col border-r border-edge"
      >
        <div className="flex h-8 shrink-0 items-center gap-1 border-b border-edge px-2">
          <span
            className="text-[11px] font-semibold text-ink-muted"
            title="Named object types defined inside this schema"
          >
            Types
          </span>
          <span className="text-[10px] text-ink-faint">{schemaNames.length}</span>
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            onClick={addSchema}
            title="Add a type"
          >
            <Plus className="size-3" />
          </Button>
        </div>
        <ul className="min-h-0 flex-1 overflow-auto py-1">
          {schemaNames.map((typeName) => {
            const count = usage[typeName] ?? 0
            const isEnvelope = typeName === 'AWSEvent'
            const isActive = activeSchema === typeName
            return (
              <li key={typeName} className="group/type relative">
                <button
                  type="button"
                  onClick={() => {
                    setActiveSchema(typeName)
                    setSelectedId(null)
                  }}
                  className={cn(
                    'flex w-full items-center gap-1.5 px-2 py-1 pr-6 text-left text-xs',
                    isActive
                      ? 'bg-accent/15 text-accent'
                      : 'text-ink-muted hover:bg-surface-2 hover:text-ink',
                    isEnvelope && !isActive && 'text-ink-faint',
                  )}
                  title={
                    isEnvelope
                      ? 'The EventBridge envelope — generated boilerplate'
                      : count === 0
                        ? 'Not used by any field'
                        : `Used by ${count} field${count === 1 ? '' : 's'}`
                  }
                >
                  <span className="truncate font-mono">{typeName}</span>
                  {isEnvelope ? (
                    <span className="ml-auto shrink-0 text-[9px] text-ink-faint">
                      envelope
                    </span>
                  ) : count === 0 ? (
                    <span className="ml-auto shrink-0 text-[9px] text-warn" title="Unused">
                      unused
                    </span>
                  ) : (
                    <span className="ml-auto shrink-0 text-[9px] text-ink-faint">
                      ×{count}
                    </span>
                  )}
                </button>

                {/* The envelope is required by EventBridge, so it has no
                    delete affordance at all. */}
                {!isEnvelope && (
                  <button
                    type="button"
                    onClick={() => setRenamingType(typeName)}
                    title={`Rename ${typeName}`}
                    className={cn(
                      'absolute right-6 top-1/2 -translate-y-1/2 rounded p-0.5',
                      'text-ink-faint opacity-0 hover:bg-surface-3 hover:text-ink',
                      'group-hover/type:opacity-100 focus:opacity-100',
                    )}
                  >
                    <Pencil className="size-3" />
                  </button>
                )}

                {!isEnvelope && (
                  <button
                    type="button"
                    onClick={() => setRemovingType(typeName)}
                    title={`Remove ${typeName}`}
                    className={cn(
                      'absolute right-1 top-1/2 -translate-y-1/2 rounded p-0.5',
                      'text-ink-faint opacity-0 hover:bg-surface-3 hover:text-danger',
                      'group-hover/type:opacity-100 focus:opacity-100',
                    )}
                  >
                    <Trash2 className="size-3" />
                  </button>
                )}
              </li>
            )
          })}
        </ul>
      </ResizablePanel>

      <ResizeHandle />

      {/* Shape */}
      <ResizablePanel id="shape" defaultSize="52" minSize="25" className="flex flex-col">
        <div className="flex h-8 shrink-0 items-center gap-2 border-b border-edge px-2">
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              typesCollapsed
                ? typesPanel.current?.expand()
                : typesPanel.current?.collapse()
            }
            title={typesCollapsed ? 'Show the Types list' : 'Hide the Types list'}
          >
            {typesCollapsed ? (
              <PanelLeftOpen className="size-3" />
            ) : (
              <PanelLeftClose className="size-3" />
            )}
          </Button>
          {/* Labelled so the header reads "type X", distinguishing it from the
              registry schema named in the toolbar above. */}
          <span className="shrink-0 text-[10px] uppercase tracking-wide text-ink-faint">
            type
          </span>
          <span className="truncate font-mono text-[11px] text-ink">
            {activeSchema}
          </span>
          {activeSchema && (usage[activeSchema] ?? 0) > 1 && (
            <Badge tone="info">shared ×{usage[activeSchema]}</Badge>
          )}
          {activeSchema === 'AWSEvent' && (
            <Badge tone="warn" title="Generated boilerplate — edit the payload instead">
              envelope
            </Badge>
          )}
          {/* Counts as well as colour: a change deep inside a collapsed branch
              is invisible otherwise. */}
          {CHANGE_BADGES.map(({ kind, tone, sign, title }) =>
            changes[kind] > 0 ? (
              <Badge key={kind} tone={tone} title={title}>
                {sign}
                {changes[kind]}
              </Badge>
            ) : null,
          )}
          {/* The root row is not rendered, so top-level fields need their own
              add affordance up here. */}
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto"
            disabled={!tree}
            onClick={() => tree && addField(tree)}
            title="Add a top-level field"
          >
            <Plus className="size-3" />
            Field
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() =>
              sideCollapsed
                ? sidePanel.current?.expand()
                : sidePanel.current?.collapse()
            }
            title={sideCollapsed ? 'Show the side panel' : 'Hide the side panel'}
          >
            {sideCollapsed ? (
              <PanelRightOpen className="size-3" />
            ) : (
              <PanelRightClose className="size-3" />
            )}
          </Button>
        </div>

        {treePane}
      </ResizablePanel>

      <ResizeHandle />

      {/*
        Side panel — details, sample, analysis.

        Keeps the `inspector` id even though it now holds three tabs: the id is
        the key the pane's saved width is stored under, and renaming it would
        reset everyone's layout for nothing.
      */}
      <ResizablePanel
        id="inspector"
        defaultSize="30"
        minSize="14"
        collapsible
        collapsedSize="0"
        panelRef={sidePanel}
        onResize={(size) => setSideCollapsed(size.asPercentage === 0)}
        className="flex flex-col border-l border-edge"
      >
        <div className="flex h-8 shrink-0 items-center gap-2 border-b border-edge px-2">
          <Segmented
            size="sm"
            value={activeTab}
            options={sideTabs}
            onChange={selectTab}
          />
          {ticketContext && (
            <Button
              variant="ghost"
              size="sm"
              className="ml-auto"
              onClick={() =>
                setConcern({
                  path: '',
                  label: `the event type ${ticketContext.detailType || ticketContext.schemaName}`,
                  declared: null,
                  observed: null,
                })
              }
              title="Raise a concern about this event type — its name, its source, its shape — with the team that owns the producer"
            >
              <Bug className="size-3" />
            </Button>
          )}
        </div>

        {/*
          Analysis stays mounted behind the other tabs: switching to Details
          mid-analysis should not throw away a sample that cost an AWS call.
        */}
        <div className="relative min-h-0 flex-1">
          <div className={cn('absolute inset-0', activeTab !== 'details' && 'hidden')}>
            {selected ? (
              <Inspector
                node={selected}
                schemaNames={schemaNames}
                onConcern={ticketContext ? raiseFieldConcern : undefined}
                sharedUsage={selected.refTarget ? usage[selected.refTarget] : undefined}
                onFollowRef={followRef}
                onRename={(node, name) => {
                  if (!node.parentPointer) return
                  onChange(renameProperty(document, node.parentPointer, node.name, name))
                  setSelectedId(
                    node.id.replace(new RegExp(`${node.name}$`), name),
                  )
                }}
                // Retyping replaces the field, so it must land on the field
                // itself — through a `$ref`, `pointer` is the shared
                // definition and rewriting that corrupts every other user.
                onSetType={(node, type: NodeType) =>
                  onChange(setNodeType(document, node.ownPointer, type))
                }
                onSetRequired={(node, required) => {
                  if (!node.parentPointer) return
                  onChange(setRequired(document, node.parentPointer, node.name, required))
                }}
                onSetKeyword={(node, key, value) =>
                  onChange(setKeywords(document, node.pointer, { [key]: value }))
                }
                onSetAcceptsNull={(node, accepts) =>
                  onChange(setAcceptsNull(document, node.pointer, accepts))
                }
                onSetRefTarget={(node, target) =>
                  onChange(setRefTarget(document, node.ownPointer, target))
                }
                onRemove={(node) => {
                  if (!node.parentPointer) return
                  onChange(removeProperty(document, node.parentPointer, node.name))
                  setSelectedId(null)
                }}
              />
            ) : (
              <div className="flex h-full items-center justify-center p-4 text-center text-[11px] text-ink-faint">
                Select a field to edit its type, description and constraints.
              </div>
            )}
          </div>

          {/* Mounted only while visible, unlike the other two: it holds nothing
              worth keeping, and hidden it would rebuild the sample event on
              every keystroke. */}
          {activeTab === 'sample' && activeSchema && (
            <div className="absolute inset-0">
              <SamplePanel
                document={document}
                detailSchema={activeSchema}
                busName={busName}
                region={region}
              />
            </div>
          )}

          {/* Mounted only while visible: the lookup is cached, so coming
              back is instant, and an unopened tab should cost no search. */}
          {activeTab === 'origin' && schemaName && (
            <div className="absolute inset-0 overflow-auto">
              <OriginPanel schemaName={schemaName} compact />
            </div>
          )}

          {schemaName && (
            <div className={cn('absolute inset-0', activeTab !== 'analysis' && 'hidden')}>
              <AnalysisPanel
                schemaName={schemaName}
                document={document}
                envId={envId}
                environmentLabel={environmentLabel}
                registryName={registryName}
                active={activeTab === 'analysis'}
                onApplySuggestions={onChange}
                onCoverage={(counts, sampled) => setCoverage({ counts, sampled })}
              />
            </div>
          )}
        </div>
      </ResizablePanel>
      </ResizableGroup>

      {renamingType && (
        <RenameTypeDialog
          name={renamingType}
          taken={schemaNames}
          usage={usage[renamingType] ?? 0}
          onCancel={() => setRenamingType(null)}
          onConfirm={(next) => {
            onChange(renameComponentSchema(document, renamingType, next))
            if (activeSchema === renamingType) setActiveSchema(next)
            setRenamingType(null)
          }}
        />
      )}

      {removingType && (
        <RemoveTypeDialog
          name={removingType}
          referrers={refReferrers(document, removingType)}
          onCancel={() => setRemovingType(null)}
          onConfirm={() => {
            onChange(removeComponentSchema(document, removingType))
            if (activeSchema === removingType) {
              setActiveSchema(defaultSchema)
              setSelectedId(null)
            }
            setRemovingType(null)
          }}
        />
      )}

      {ticketContext && (
        <ConcernDialog
          open={!!concern}
          subject={concern}
          context={ticketContext}
          onClose={() => setConcern(null)}
        />
      )}
    </>
  )
}

/**
 * Renames a type, rewriting every reference to it.
 *
 * Shows the reference count up front, since a rename here is not a local edit —
 * it rewrites every `$ref` in the document that points at this type.
 */
function RenameTypeDialog({
  name,
  taken,
  usage,
  onCancel,
  onConfirm,
}: {
  name: string
  taken: string[]
  usage: number
  onCancel: () => void
  onConfirm: (next: string) => void
}) {
  const [next, setNext] = useState(name)
  const trimmed = next.trim()
  const collides = trimmed !== name && taken.includes(trimmed)
  const valid = trimmed !== '' && !collides

  return (
    <Modal onClose={onCancel} width={420} className="p-4">
          <ModalTitle>Rename type</ModalTitle>
          <ModalDescription>
            {usage > 0
              ? `${usage} reference${usage === 1 ? '' : 's'} will be updated to match.`
              : 'Nothing references this type yet.'}
          </ModalDescription>

          <div className="mt-3">
            <Input
              value={next}
              onChange={(e) => setNext(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && valid) onConfirm(trimmed)
              }}
              className="font-mono"
              autoFocus
              spellCheck={false}
            />
            {collides && (
              <p className="mt-1 text-[10px] text-danger">
                A type called {trimmed} already exists.
              </p>
            )}
          </div>

          <div className="mt-4 flex justify-end gap-2">
            <Button variant="ghost" onClick={onCancel}>
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={!valid || trimmed === name}
              onClick={() => onConfirm(trimmed)}
            >
              Rename
            </Button>
          </div>
    </Modal>
  )
}

/**
 * Confirms removing a type.
 *
 * Removing one that is still referenced leaves dangling `$ref`s — legal JSON
 * that breaks the moment anything reads the schema. Rather than silently
 * refusing or silently allowing it, this names the fields that depend on it and
 * makes the consequence explicit.
 */
function RemoveTypeDialog({
  name,
  referrers,
  onCancel,
  onConfirm,
}: {
  name: string
  referrers: Record<string, number>
  onCancel: () => void
  onConfirm: () => void
}) {
  const owners = Object.entries(referrers)
  const inUse = owners.length > 0

  return (
    <Modal onClose={onCancel} width={440} className="p-4">
          <ModalTitle>
            <Trash2 className="size-4" />
            Remove <span className="font-mono normal-case">{name}</span>
          </ModalTitle>

          <ModalDescription className="mt-2">
            {inUse
              ? 'This type is still used. Removing it leaves references pointing at nothing.'
              : 'Nothing references this type, so removing it is safe.'}
          </ModalDescription>

          {inUse && (
            <div className="mt-3 rounded-md border border-danger/40 bg-danger/10 p-2">
              <p className="text-[10px] font-medium text-danger">Used by:</p>
              <ul className="mt-1 flex flex-col gap-0.5">
                {owners.map(([owner, count]) => (
                  <li key={owner} className="font-mono text-[11px] text-danger">
                    {owner}
                    {count > 1 && (
                      <span className="text-ink-muted"> ×{count}</span>
                    )}
                  </li>
                ))}
              </ul>
              <p className="mt-1.5 text-[10px] text-ink-muted">
                Repoint or remove those fields first, or the schema will fail
                validation on save.
              </p>
            </div>
          )}

          <div className="mt-4 flex justify-end gap-2">
            <Button variant="ghost" onClick={onCancel}>
              Cancel
            </Button>
            <Button
              variant={inUse ? 'danger' : 'primary'}
              onClick={onConfirm}
            >
              {inUse ? 'Remove anyway' : 'Remove'}
            </Button>
          </div>
    </Modal>
  )
}

/**
 * A generated example event, plus the `put-events` call to publish it.
 *
 * The shape tells you what fields exist; a sample tells you what an actual
 * event looks like, and the command makes it testable without hand-escaping
 * the detail payload.
 */
function SamplePanel({
  document,
  detailSchema,
  busName,
  region,
}: {
  document: unknown
  detailSchema: string
  busName?: string
  region: string
}) {
  const [tab, setTab] = useState<'event' | 'command'>('event')
  const [copied, setCopied] = useState(false)

  const envelope = useMemo(() => {
    const awsEvent = componentSchemas(document).AWSEvent ?? {}
    return {
      source: String(awsEvent['x-amazon-events-source'] ?? 'my-service'),
      detailType: String(awsEvent['x-amazon-events-detail-type'] ?? 'thing-happened'),
    }
  }, [document])

  const event = useMemo(
    () =>
      buildSampleEvent(document, {
        source: envelope.source,
        detailType: envelope.detailType,
        detailSchema,
      }),
    [document, envelope, detailSchema],
  )

  const text =
    tab === 'event'
      ? stringify(event)
      : putEventsCommand(event, busName ?? `${'<stage>'}-global-bus`, region)

  const copy = async () => {
    await navigator.clipboard.writeText(text)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  return (
    <div className="flex h-full min-h-0 flex-col bg-surface-0">
      <div className="flex h-7 shrink-0 items-center gap-1 border-b border-edge px-2">
        <Segmented
          size="sm"
          value={tab}
          options={[
            { id: 'event', label: 'Event' },
            { id: 'command', label: 'put-events' },
          ]}
          onChange={setTab}
        />
        <Button variant="ghost" size="sm" className="ml-auto" onClick={copy}>
          {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
          Copy
        </Button>
      </div>
      <pre className="min-h-0 flex-1 overflow-auto p-2 font-mono text-[10px] leading-relaxed text-ink-muted">
        {text}
      </pre>
    </div>
  )
}

