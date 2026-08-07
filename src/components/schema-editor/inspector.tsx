import { useEffect, useState } from 'react'
import { AlertTriangle, Link2, Trash2 } from 'lucide-react'
import {
  CONSTRAINTS,
  NODE_TYPES,
  type NodeType,
  type SchemaNode,
} from '@/lib/schema-model'
import { Badge, Button, Checkbox, Field, Input, Select } from '@/components/ui'

/**
 * The string formats the bus actually asserts.
 *
 * Exactly `ajv-formats`' string set, because that is what `addFormats(ajv)`
 * installs — anything outside it is logged as an unknown format and enforced
 * on nothing, so offering it here would be offering a constraint that does not
 * exist. `iri`, `idn-email` and friends are absent for that reason, however
 * standard they look.
 *
 * Kept in step with `FORMATS` in `src-tauri/src/schema/ajv.rs`, which is the
 * source of truth — it decides both what the validator asserts and what the
 * report calls unrecognised. A format present there and missing here cannot be
 * picked and renders as "none" on a field that already declares it; one present
 * here and missing there gets offered and then flagged as unenforced. The
 * numeric formats (`int32`, `int64`, `float`, `double`) are deliberately
 * omitted: Ajv scopes them to numbers, so they do nothing on a string.
 */
const STRING_FORMATS = [
  '',
  'date-time',
  'date',
  'time',
  'iso-date-time',
  'iso-time',
  'duration',
  'email',
  'uuid',
  'uri',
  'uri-reference',
  'uri-template',
  'url',
  'hostname',
  'ipv4',
  'ipv6',
  'regex',
  'json-pointer',
  'json-pointer-uri-fragment',
  'relative-json-pointer',
  'byte',
  'password',
  'binary',
]

/**
 * How each constraint is presented and edited.
 *
 * One row per keyword. This was three structures keyed by the same thirteen
 * strings — a label map, a set naming the numeric ones, and a hardcoded
 * `uniqueItems` branch inside the editor — so adding a constraint meant
 * remembering all three.
 */
const CONSTRAINT_FIELDS: Record<
  string,
  { label: string; hint: string; kind: 'number' | 'text' | 'boolean' }
> = {
  minLength: { label: 'Min length', hint: 'characters', kind: 'number' },
  maxLength: { label: 'Max length', hint: 'characters', kind: 'number' },
  pattern: {
    label: 'Pattern',
    hint: 'regular expression the value must match',
    kind: 'text',
  },
  minimum: { label: 'Minimum', hint: 'inclusive', kind: 'number' },
  maximum: { label: 'Maximum', hint: 'inclusive', kind: 'number' },
  exclusiveMinimum: { label: 'Above', hint: 'exclusive lower bound', kind: 'number' },
  exclusiveMaximum: { label: 'Below', hint: 'exclusive upper bound', kind: 'number' },
  multipleOf: { label: 'Multiple of', hint: 'e.g. 0.01 for currency', kind: 'number' },
  minItems: { label: 'Min items', hint: '', kind: 'number' },
  maxItems: { label: 'Max items', hint: '', kind: 'number' },
  uniqueItems: { label: 'Unique items', hint: 'no duplicates', kind: 'boolean' },
  minProperties: { label: 'Min fields', hint: '', kind: 'number' },
  maxProperties: { label: 'Max fields', hint: '', kind: 'number' },
}

const COMPOSITION_BLURB: Record<string, string> = {
  allOf: 'must satisfy every branch',
  anyOf: 'must satisfy at least one branch',
  oneOf: 'must satisfy exactly one branch',
  not: 'must not satisfy this branch',
}

export interface InspectorProps {
  node: SchemaNode
  /** Every type name in the document, for the reference picker. */
  schemaNames: string[]
  /** How many places reference this node's definition, when reached via a ref. */
  sharedUsage?: number
  onRename: (node: SchemaNode, name: string) => void
  onSetType: (node: SchemaNode, type: NodeType) => void
  onSetRequired: (node: SchemaNode, required: boolean) => void
  onSetKeyword: (node: SchemaNode, key: string, value: unknown) => void
  /** Widen or narrow `type` to include `"null"`. */
  onSetAcceptsNull: (node: SchemaNode, accepts: boolean) => void
  onSetRefTarget: (node: SchemaNode, target: string) => void
  onRemove: (node: SchemaNode) => void
  onFollowRef: (target: string) => void
}

/**
 * One bound, committed on blur.
 *
 * A blank field removes the keyword rather than writing `0` or `""` — an empty
 * "Min length" means unbounded, and writing `minLength: 0` would be a real
 * constraint saying something the user did not.
 *
 * Mounted with a key that includes the node, so selecting a different field
 * remounts this with the right initial text instead of rendering the previous
 * node's value and correcting it in an effect.
 */
function ConstraintField({
  keyword,
  value,
  onCommit,
}: {
  keyword: string
  value: unknown
  onCommit: (value: unknown) => void
}) {
  const { label, hint, kind } = CONSTRAINT_FIELDS[keyword] ?? {
    label: keyword,
    hint: '',
    kind: 'text' as const,
  }
  const [text, setText] = useState(value === undefined ? '' : String(value))

  if (kind === 'boolean') {
    return (
      <Checkbox
        checked={value === true}
        onChange={(e) => onCommit(e.target.checked ? true : undefined)}
        label={label}
      />
    )
  }

  const numeric = kind === 'number'
  const commit = () => {
    const trimmed = text.trim()
    if (trimmed === '') return onCommit(undefined)
    if (!numeric) return onCommit(trimmed)
    const parsed = Number(trimmed)
    // Not a number: leave the document alone and snap the input back, rather
    // than writing a `minimum` the validator would reject the schema over.
    if (Number.isNaN(parsed)) return setText(value === undefined ? '' : String(value))
    onCommit(parsed)
  }

  return (
    <Field label={label} hint={hint || undefined}>
      <Input
        value={text}
        inputMode={numeric ? 'decimal' : undefined}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
        placeholder="any"
        className={numeric ? undefined : 'font-mono'}
        spellCheck={false}
      />
    </Field>
  )
}

export function Inspector({
  node,
  schemaNames,
  sharedUsage,
  onRename,
  onSetType,
  onSetRequired,
  onSetKeyword,
  onSetAcceptsNull,
  onSetRefTarget,
  onRemove,
  onFollowRef,
}: InspectorProps) {
  // Name and description are free text, so they commit on blur rather than on
  // every keystroke — otherwise each character would be an undo step.
  const [name, setName] = useState(node.name)
  const [description, setDescription] = useState(node.description ?? '')
  const [enumText, setEnumText] = useState((node.enumValues ?? []).join(', '))

  useEffect(() => {
    setName(node.name)
    setDescription(node.description ?? '')
    setEnumText((node.enumValues ?? []).join(', '))
  }, [node.id, node.name, node.description, node.enumValues])

  const isRoot = node.parentPointer === null
  const isArrayItem = node.name === 'items' && !isRoot
  const canRename = !isRoot && !isArrayItem
  const effectiveType: NodeType = node.refTarget ? 'ref' : node.type

  /**
   * Types worth pointing a field at.
   *
   * The envelope is excluded: it is the EventBridge wrapper around the
   * payload, so a field inside the payload referencing it is always a mistake
   * — and it used to be the default target purely because it sorts first.
   * A ref that already points somewhere odd stays listed, so the picker keeps
   * showing the truth about the document.
   */
  const refTargets = schemaNames.filter((name) => name !== 'AWSEvent')
  const refOptions =
    node.refTarget && !refTargets.includes(node.refTarget)
      ? [node.refTarget, ...refTargets]
      : refTargets

  const commitEnum = () => {
    const values = enumText
      .split(',')
      .map((v) => v.trim())
      .filter(Boolean)
    onSetKeyword(node, 'enum', values.length > 0 ? values : undefined)
  }

  return (
    <div className="flex h-full flex-col gap-3 overflow-auto p-3">
      <div className="flex items-center gap-2">
        <span className="truncate font-mono text-xs text-ink" title={node.pointer}>
          {node.name}
        </span>
        {node.viaRef && (
          <Badge tone="info" title={node.pointer}>
            shared
          </Badge>
        )}
        {!isRoot && (
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto text-danger"
            onClick={() => onRemove(node)}
            title="Remove this field"
          >
            <Trash2 className="size-3" />
          </Button>
        )}
      </div>

      {/* Editing through a ref changes a shared definition; saying so up front
          prevents a surprise elsewhere in the document. */}
      {node.viaRef && (
        <div className="flex items-start gap-1.5 rounded-md border border-info/30 bg-info/10 px-2 py-1.5 text-[10px] text-info">
          <AlertTriangle className="mt-px size-3 shrink-0" />
          <span>
            Defined by{' '}
            <button
              type="button"
              className="underline"
              onClick={() => node.refTarget && onFollowRef(node.refTarget)}
            >
              {node.refTarget}
            </button>
            {sharedUsage && sharedUsage > 1
              ? ` — used in ${sharedUsage} places, so edits affect all of them.`
              : ' — edits change that definition.'}
          </span>
        </div>
      )}

      {canRename && (
        <Field label="Name">
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            onBlur={() => name !== node.name && onRename(node, name)}
            onKeyDown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
            className="font-mono"
            spellCheck={false}
          />
        </Field>
      )}

      <div className="grid grid-cols-2 gap-2">
        <Field label="Type">
          <Select
            value={effectiveType === 'unknown' ? '' : effectiveType}
            onChange={(e) => {
              const next = e.target.value as NodeType
              if (next === 'ref') onSetRefTarget(node, refTargets[0] ?? '')
              else onSetType(node, next)
            }}
            className="w-full"
          >
            {effectiveType === 'unknown' && <option value="">untyped</option>}
            {NODE_TYPES.map((type) => (
              <option key={type} value={type}>
                {type}
              </option>
            ))}
            <option value="ref" disabled={refOptions.length === 0}>
              another type →
            </option>
          </Select>
        </Field>

        {effectiveType === 'ref' ? (
          <Field label="Reuses" hint="a named type in this schema">
            <Select
              value={node.refTarget ?? ''}
              onChange={(e) => onSetRefTarget(node, e.target.value)}
              className="w-full"
            >
              <option value="">choose…</option>
              {refOptions.map((schemaName) => (
                <option key={schemaName} value={schemaName}>
                  {schemaName}
                </option>
              ))}
            </Select>
          </Field>
        ) : effectiveType === 'string' ? (
          <Field label="Format">
            <Select
              value={node.format ?? ''}
              onChange={(e) =>
                onSetKeyword(node, 'format', e.target.value || undefined)
              }
              className="w-full"
            >
              {STRING_FORMATS.map((format) => (
                <option key={format} value={format}>
                  {format || 'none'}
                </option>
              ))}
            </Select>
          </Field>
        ) : (
          <span />
        )}
      </div>

      <div className="flex flex-wrap items-center gap-3">
        {!isRoot && !isArrayItem && (
          <Checkbox
            checked={node.required}
            onChange={(e) => onSetRequired(node, e.target.checked)}
            label="Required"
          />
        )}
        {/* Writes `type: [T, "null"]`, not `nullable: true` — see the warning
            below for why the two are not interchangeable. Disabled where there
            is no type to widen: an untyped field already accepts null, and a
            `$ref`'s nullability belongs to the type it points at. */}
        <Checkbox
          checked={node.acceptsNull}
          disabled={effectiveType === 'ref' || effectiveType === 'unknown'}
          onChange={(e) => onSetAcceptsNull(node, e.target.checked)}
          label="Accepts null"
        />
      </div>

      {node.nullable && (
        <div className="flex items-start gap-1.5 rounded-md border border-warn/30 bg-warn/10 px-2 py-1.5 text-[10px] text-warn">
          <AlertTriangle className="mt-px size-3 shrink-0" />
          <span>
            This field is marked <code className="font-mono">nullable: true</code>, which
            does nothing. It is an OpenAPI keyword and the bus validates with Ajv, which
            ignores it — so nulls are rejected today.{' '}
            <button
              type="button"
              className="underline"
              onClick={() => onSetAcceptsNull(node, true)}
            >
              Rewrite it as “accepts null”
            </button>
            {node.acceptsNull ? ' to drop the dead keyword.' : ' to make that true.'}
          </span>
        </div>
      )}

      {/* `node.type` is the *resolved* type, so this also appears for a field
          that reaches an object through a $ref. */}
      {node.type === 'object' && (
        <Field
          label="Extra fields"
          hint={
            node.additionalProperties === false
              ? 'An event carrying an undeclared field will fail validation.'
              : 'The registry stores relaxed schemas, so this is normally left open.'
          }
        >
          <Select
            value={
              node.additionalProperties === undefined
                ? 'default'
                : String(node.additionalProperties)
            }
            onChange={(e) => {
              const choice = e.target.value
              onSetKeyword(
                node,
                'additionalProperties',
                choice === 'default' ? undefined : choice === 'true',
              )
            }}
            className="w-full"
            disabled={node.additionalProperties === 'schema'}
          >
            <option value="default">Allowed — not specified</option>
            <option value="true">Allowed</option>
            <option value="false">Not allowed</option>
            {node.additionalProperties === 'schema' && (
              <option value="schema">
                Constrained by a sub-schema — edit in JSON
              </option>
            )}
          </Select>
        </Field>
      )}

      <Field label="Description">
        <textarea
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          onBlur={() =>
            description !== (node.description ?? '') &&
            onSetKeyword(node, 'description', description || undefined)
          }
          rows={2}
          placeholder="What this field holds"
          className="resize-none rounded-md border border-edge bg-surface-1 p-2 text-xs text-ink placeholder:text-ink-faint focus:border-accent focus:outline-none"
        />
      </Field>

      {(effectiveType === 'string' ||
        effectiveType === 'integer' ||
        effectiveType === 'number') && (
        <Field label="Allowed values" hint="comma separated; blank for any">
          <Input
            value={enumText}
            onChange={(e) => setEnumText(e.target.value)}
            onBlur={commitEnum}
            onKeyDown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
            placeholder="queued, running, done"
            className="font-mono"
            spellCheck={false}
          />
        </Field>
      )}

      {/* Bounds the bus enforces. These used to sit in the grey "Also defines"
          box below, alongside `title` — which put a `pattern` that rejects live
          traffic in the same visual place as a label nobody validates. */}
      {CONSTRAINTS[effectiveType].length > 0 && (
        <Field label="Constraints" hint="blank means no limit">
          <div className="grid grid-cols-2 gap-2">
            {CONSTRAINTS[effectiveType].map((keyword) => (
              <ConstraintField
                key={`${node.id}:${keyword}`}
                keyword={keyword}
                value={node.constraints[keyword]}
                onCommit={(value) => onSetKeyword(node, keyword, value)}
              />
            ))}
          </div>
        </Field>
      )}

      {/* A `const` is an enum of one, but Ajv reports it differently and people
          write it deliberately, so it gets its own row rather than being folded
          into "Allowed values". Shown only when present: offering it next to
          the enum would invite setting both, which can only ever contradict. */}
      {node.constValue !== undefined && (
        <Field label="Fixed value" hint="the only value that validates">
          <Input
            value={
              typeof node.constValue === 'string'
                ? node.constValue
                : JSON.stringify(node.constValue)
            }
            readOnly
            className="font-mono"
            spellCheck={false}
          />
        </Field>
      )}

      {/* Composition is where a schema gets genuinely conditional, and it is
          the one thing here the tree cannot show in place. Naming the keyword
          and its branch count says what the field means; editing the branches
          still belongs to the JSON view, because a form for arbitrary nested
          alternatives is a worse editor than the JSON. */}
      {node.composition.length > 0 && (
        <div className="rounded-md border border-info/30 bg-info/10 p-2 text-[10px] text-info">
          <p className="font-medium">Enforced by composition</p>
          <ul className="mt-1 space-y-0.5">
            {node.composition.map(({ keyword, count }) => (
              <li key={keyword}>
                <code className="font-mono">{keyword}</code>
                {keyword !== 'not' && ` · ${count} ${count === 1 ? 'branch' : 'branches'}`}
                <span className="text-ink-muted"> — {COMPOSITION_BLURB[keyword]}</span>
              </li>
            ))}
          </ul>
          <p className="mt-1 text-ink-muted">
            The validator applies these on top of everything above. Edit the branches
            in the JSON view.
          </p>
        </div>
      )}

      {node.extraKeywords.length > 0 && (
        <div className="rounded-md border border-edge bg-surface-1 p-2 text-[10px] text-ink-muted">
          <p className="font-medium text-ink">Also defines:</p>
          <p className="mt-0.5 font-mono">{node.extraKeywords.join(', ')}</p>
          <p className="mt-1">
            Preserved exactly, but only editable in the JSON view.
          </p>
        </div>
      )}

      <div className="mt-auto flex items-center gap-1 pt-2 text-[10px] text-ink-faint">
        <Link2 className="size-2.5 shrink-0" />
        <span className="truncate font-mono" title={node.pointer}>
          {node.pointer}
        </span>
      </div>
    </div>
  )
}
