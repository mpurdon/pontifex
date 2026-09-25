/**
 * A tree model over an OpenAPI 3 schema document, plus immutable edits.
 *
 * The registry's real documents compose through `$ref` far more than through
 * deep inline nesting — one has 61 component schemas and there are 874 refs
 * across the set. Reading such a document as raw JSON means jumping between
 * definitions constantly, so this model **resolves refs inline**: expanding a
 * `$ref` property shows the target's fields in place, and the whole effective
 * event shape is visible in one tree.
 *
 * Every edit returns a new document and **merges into** the existing node
 * rather than replacing it, so keywords this model does not understand
 * (`oneOf`, `pattern`, `minimum`, …) survive untouched.
 */

export type JsonObject = Record<string, unknown>

/** Types the inspector offers directly. Anything else shows as `unknown`. */
export const SCALAR_TYPES = ['string', 'number', 'integer', 'boolean'] as const
export const NODE_TYPES = [...SCALAR_TYPES, 'object', 'array'] as const
export type NodeType = (typeof NODE_TYPES)[number] | 'ref' | 'unknown'

/**
 * Constraints Ajv enforces, grouped by the type they apply to.
 *
 * These used to fall through to the inspector's "Also defines / JSON only"
 * box, which put a `pattern` — a keyword that rejects live traffic — in the
 * same grey box as `title`. Anything the bus can reject an event over deserves
 * a control.
 */
// No `exclusiveMinimum`/`exclusiveMaximum`: OpenAPI 3.0 spells them as
// booleans modifying `minimum`/`maximum`, so the registry refuses the number,
// and Ajv — which the bus validates with — refuses the boolean. Offering them
// is offering a document that cannot be saved. `Fix` rewrites one that is
// already there into its inclusive neighbour.
const NUMERIC_BOUNDS = ['minimum', 'maximum', 'multipleOf'] as const

export const CONSTRAINTS = {
  string: ['minLength', 'maxLength', 'pattern'],
  number: NUMERIC_BOUNDS,
  integer: NUMERIC_BOUNDS,
  array: ['minItems', 'maxItems', 'uniqueItems'],
  object: ['minProperties', 'maxProperties'],
  boolean: [],
  ref: [],
  unknown: [],
} as const satisfies Record<NodeType, readonly string[]>

/** Keywords that combine subschemas. Rendered as branches, not as text. */
export const COMPOSITION_KEYWORDS = ['allOf', 'anyOf', 'oneOf', 'not'] as const
export type CompositionKeyword = (typeof COMPOSITION_KEYWORDS)[number]

/**
 * Keywords the structured UI models for every type.
 *
 * Type-specific constraints are not listed here — they come from
 * [`CONSTRAINTS`] for the node's own type, so a `minLength` on a boolean is
 * correctly *not* modelled and falls through to `extraKeywords`.
 */
const ALWAYS_MODELLED = new Set<string>([
  'type',
  'properties',
  'items',
  'required',
  'description',
  'format',
  'enum',
  'nullable',
  '$ref',
  'additionalProperties',
  'title',
  'const',
  'default',
])

export interface SchemaNode {
  /** Stable identity for React keys and selection, unique within a tree. */
  id: string
  /** Property name, or a synthetic label for array element nodes. */
  name: string
  /**
   * JSON Pointer to this node's schema object within the document.
   *
   * For a node reached through a `$ref` this points into the *target*
   * definition, because that is what an edit would actually change.
   */
  pointer: string
  /**
   * JSON Pointer to where this field itself is written.
   *
   * The same as `pointer` except through a `$ref`, where this is the property
   * holding the `$ref` and `pointer` is the definition it resolves to. Edits
   * that replace the field — changing its type, or repointing the ref — must
   * use this one, or they overwrite the shared definition instead of the
   * field: retyping a `$ref` field to `string` used to turn the type it
   * pointed at into `{"type": "string"}`, taking every other user of that type
   * with it.
   */
  ownPointer: string
  /** Pointer to the object that owns this node's `required` array. */
  parentPointer: string | null
  type: NodeType
  required: boolean
  /**
   * Whether `null` is an accepted value: `nullable: true`, the OpenAPI 3.0
   * spelling the registry stores and the bus's Ajv honours, or a `type` list
   * naming `"null"`, which a pasted JSON Schema may carry.
   */
  acceptsNull: boolean
  description?: string
  format?: string
  enumValues?: unknown[]
  /** A single permitted value — draft-07's `const`. */
  constValue?: unknown
  /**
   * Ajv-enforced bounds present on this node, e.g. `{ pattern: '^A' }`.
   *
   * Only the keys in [`CONSTRAINTS`] for this node's type.
   */
  constraints: Record<string, unknown>
  /** Subschema branches, by keyword — `not` has exactly one. */
  composition: { keyword: CompositionKeyword; count: number }[]
  /**
   * Whether fields beyond `properties` are permitted.
   *
   * `undefined` means the keyword is absent, which JSON Schema treats as
   * permissive — a meaningful distinction from an explicit `true`, because the
   * registry's simplify step sets it deliberately. `'schema'` means extras are
   * allowed but constrained by a sub-schema, which only the JSON view can edit.
   */
  additionalProperties?: boolean | 'schema'
  /** Component schema name this node refs, when `type` is `ref`. */
  refTarget?: string
  /** True when this node was reached by following a `$ref`. */
  viaRef: boolean
  /** Set when expansion stopped because the ref cycles back on itself. */
  cyclic: boolean
  children: SchemaNode[]
  /** Keywords present but not modelled, so the UI can flag them. */
  extraKeywords: string[]
  /** Depth from the tree root, for indentation. */
  depth: number
}

const REF_PREFIX = '#/components/schemas/'

export function refName(ref: string): string | null {
  return ref.startsWith(REF_PREFIX) ? ref.slice(REF_PREFIX.length) : null
}

function escapePointer(segment: string): string {
  return segment.replace(/~/g, '~0').replace(/\//g, '~1')
}

function unescapePointer(segment: string): string {
  return segment.replace(/~1/g, '/').replace(/~0/g, '~')
}

/** Read the value at a JSON Pointer, or `undefined` if the path is absent. */
export function getAtPointer(doc: unknown, pointer: string): unknown {
  if (pointer === '') return doc
  let current: unknown = doc
  for (const raw of pointer.split('/').slice(1)) {
    const key = unescapePointer(raw)
    if (current === null || typeof current !== 'object') return undefined
    current = (current as JsonObject)[key]
  }
  return current
}

export function componentSchemas(doc: unknown): Record<string, JsonObject> {
  const schemas = getAtPointer(doc, '/components/schemas')
  if (!schemas || typeof schemas !== 'object') return {}
  return schemas as Record<string, JsonObject>
}

/**
 * The declared type names, as a list.
 *
 * `type` is a string in every document the registry stores; the array form is
 * how a pasted JSON Schema spells "or null", and reading only the string form
 * would show such a field as untyped.
 */
export function typeNames(schema: JsonObject): string[] {
  const type = schema.type
  if (typeof type === 'string') return [type]
  if (Array.isArray(type)) return type.filter((t): t is string => typeof t === 'string')
  return []
}

/**
 * The named (non-`null`) types a schema declares.
 *
 * `null` is a nullability marker rather than a shape, so a
 * `["string", "null"]` field is a string that also accepts null. Exported
 * because every consumer that reads a type has to strip it the same way, and
 * the one place that forgot sampled every widened field as a string.
 */
export function namedTypes(schema: JsonObject): string[] {
  return typeNames(schema).filter((t) => t !== 'null')
}

function classify(schema: JsonObject, named: string[]): NodeType {
  if (typeof schema.$ref === 'string') return 'ref'
  if (named.length === 1 && (NODE_TYPES as readonly string[]).includes(named[0])) {
    return named[0] as NodeType
  }
  // A bare object with `properties` and no declared type is still an object.
  if (schema.properties && typeof schema.properties === 'object') return 'object'
  if (schema.items && typeof schema.items === 'object') return 'array'
  return 'unknown'
}

interface Keywords {
  constraints: Record<string, unknown>
  composition: SchemaNode['composition']
  /** Everything no bucket above claimed. */
  extras: string[]
}

/**
 * Partition a schema's keys into what the UI shows and what it merely
 * preserves.
 *
 * One pass that consumes each key exactly once, so `extras` is "whatever was
 * left" rather than "whatever is absent from a hand-maintained set". Those are
 * different, and the difference used to swallow keywords whole: `minLength` on
 * a boolean matched neither the type's constraint list nor the modelled set, so
 * it appeared in no control *and* in no "Also defines" box — invisible in the
 * structured editor despite being a keyword the bus can reject an event over.
 */
function classifyKeywords(schema: JsonObject, type: NodeType): Keywords {
  const constraints: Record<string, unknown> = {}
  const composition: SchemaNode['composition'] = []
  const extras: string[] = []
  const applicable: readonly string[] = CONSTRAINTS[type]

  for (const [key, value] of Object.entries(schema)) {
    if (ALWAYS_MODELLED.has(key)) continue

    if (applicable.includes(key)) {
      if (value !== undefined) constraints[key] = value
      continue
    }

    if ((COMPOSITION_KEYWORDS as readonly string[]).includes(key)) {
      const keyword = key as CompositionKeyword
      // An empty `oneOf` is modelled — it just has nothing to show.
      if (keyword === 'not') {
        if (value && typeof value === 'object') composition.push({ keyword, count: 1 })
      } else if (Array.isArray(value) && value.length > 0) {
        composition.push({ keyword, count: value.length })
      }
      continue
    }

    extras.push(key)
  }

  // Stable order regardless of key order in the document.
  composition.sort(
    (a, b) =>
      COMPOSITION_KEYWORDS.indexOf(a.keyword) - COMPOSITION_KEYWORDS.indexOf(b.keyword),
  )
  return { constraints, composition, extras }
}

interface BuildContext {
  doc: unknown
  /** Component names currently being expanded, to break `$ref` cycles. */
  stack: string[]
  maxDepth: number
}

function buildNode(
  schema: JsonObject,
  opts: {
    id: string
    name: string
    pointer: string
    parentPointer: string | null
    required: boolean
    viaRef: boolean
    depth: number
  },
  ctx: BuildContext,
): SchemaNode {
  // Read once and shared: `classify` and the null check both want it, and this
  // runs for every node of every rebuild.
  const declared = typeNames(schema)
  const type = classify(schema, declared.filter((t) => t !== 'null'))
  const { constraints, composition, extras } = classifyKeywords(schema, type)

  const node: SchemaNode = {
    ...opts,
    ownPointer: opts.pointer,
    type,
    acceptsNull: declared.includes('null') || schema.nullable === true,
    description:
      typeof schema.description === 'string' ? schema.description : undefined,
    format: typeof schema.format === 'string' ? schema.format : undefined,
    enumValues: Array.isArray(schema.enum) ? schema.enum : undefined,
    constValue: schema.const,
    constraints,
    composition,
    additionalProperties:
      typeof schema.additionalProperties === 'boolean'
        ? schema.additionalProperties
        : schema.additionalProperties && typeof schema.additionalProperties === 'object'
          ? 'schema'
          : undefined,
    cyclic: false,
    children: [],
    extraKeywords: extras,
  }

  if (type === 'ref') {
    const target = refName(schema.$ref as string)
    node.refTarget = target ?? (schema.$ref as string)

    if (!target) return node

    // A ref that reappears in its own expansion would recurse forever; stop and
    // let the UI offer a jump instead.
    if (ctx.stack.includes(target)) {
      node.cyclic = true
      return node
    }

    const resolved = componentSchemas(ctx.doc)[target]
    if (!resolved) return node

    // Present the target's shape in place, but keep pointers aimed at the
    // target definition so edits land where the data actually lives.
    const targetPointer = `/components/schemas/${escapePointer(target)}`
    const expanded = buildNode(
      resolved,
      {
        ...opts,
        pointer: targetPointer,
        parentPointer: opts.parentPointer,
        viaRef: true,
      },
      { ...ctx, stack: [...ctx.stack, target] },
    )

    // Keep the ref identity on the row, but adopt the target's shape.
    //
    // `expanded` was built from `opts`, so it already carries this node's id,
    // name, requiredness and `viaRef`. The two that genuinely differ:
    // `pointer` stays on the definition — that is where a description or an
    // enum lives — while `ownPointer` stays on the `$ref` itself.
    return { ...expanded, ownPointer: opts.pointer, refTarget: target }
  }

  if (opts.depth >= ctx.maxDepth) return node

  if (type === 'object') {
    const properties = schema.properties
    if (properties && typeof properties === 'object') {
      const requiredList = new Set(
        Array.isArray(schema.required)
          ? schema.required.filter((r): r is string => typeof r === 'string')
          : [],
      )
      node.children = Object.entries(properties as JsonObject).map(
        ([childName, childSchema]) =>
          buildNode(
            (childSchema ?? {}) as JsonObject,
            {
              id: `${opts.id}/${childName}`,
              name: childName,
              pointer: `${opts.pointer}/properties/${escapePointer(childName)}`,
              parentPointer: opts.pointer,
              required: requiredList.has(childName),
              viaRef: false,
              depth: opts.depth + 1,
            },
            ctx,
          ),
      )
    }
  } else if (type === 'array') {
    const items = schema.items
    if (items && typeof items === 'object') {
      node.children = [
        buildNode(
          items as JsonObject,
          {
            id: `${opts.id}/[]`,
            name: 'items',
            pointer: `${opts.pointer}/items`,
            parentPointer: opts.pointer,
            required: false,
            viaRef: false,
            depth: opts.depth + 1,
          },
          ctx,
        ),
      ]
    }
  }

  return node
}

/**
 * Build the tree for one component schema, resolving refs inline.
 *
 * `maxDepth` is a safety valve for pathological documents; the deepest real
 * schema in the registry is 6 levels.
 */
export function buildTree(
  doc: unknown,
  schemaName: string,
  maxDepth = 24,
): SchemaNode | null {
  const schema = componentSchemas(doc)[schemaName]
  if (!schema) return null

  return buildNode(
    schema,
    {
      id: schemaName,
      name: schemaName,
      pointer: `/components/schemas/${escapePointer(schemaName)}`,
      parentPointer: null,
      required: false,
      viaRef: false,
      depth: 0,
    },
    { doc, stack: [schemaName], maxDepth },
  )
}

/**
 * The component schema holding the event payload.
 *
 * Follows `AWSEvent.properties.detail.$ref`, which is the document's own
 * statement of where the payload lives, rather than guessing from key order.
 * Falls back to the first non-envelope schema.
 */
export function payloadSchemaName(doc: unknown): string | undefined {
  const schemas = componentSchemas(doc)
  const names = Object.keys(schemas)

  const detail = getAtPointer(doc, '/components/schemas/AWSEvent/properties/detail')
  if (detail && typeof detail === 'object') {
    const ref = (detail as JsonObject).$ref
    if (typeof ref === 'string') {
      const target = refName(ref)
      if (target && target in schemas) return target
    }
  }

  return names.find((n) => n !== 'AWSEvent') ?? names[0]
}

/**
 * Every field path the payload type declares, in the form a watch condition
 * takes: `clientId`, `client.dob`, `items[0].id`.
 *
 * Refs are followed into sibling component schemas; a type that refers back
 * to itself stops at the first repeat rather than recursing forever. Arrays
 * are spelled with `[0]` because that is what CloudWatch's pattern grammar
 * accepts — there is no "any element" selector.
 */
export function declaredPaths(doc: unknown, limit = 400): string[] {
  const schemas = componentSchemas(doc)
  const root = payloadSchemaName(doc)
  if (!root) return []
  const out: string[] = []

  const walk = (schema: JsonObject, prefix: string, seen: string[]) => {
    if (out.length >= limit) return
    const ref = schema.$ref
    if (typeof ref === 'string') {
      const name = refName(ref)
      if (!name || seen.includes(name) || !(name in schemas)) return
      return walk(schemas[name], prefix, [...seen, name])
    }
    const properties = schema.properties
    if (properties && typeof properties === 'object') {
      for (const [key, child] of Object.entries(properties as JsonObject)) {
        if (out.length >= limit) return
        const path = prefix ? `${prefix}.${key}` : key
        out.push(path)
        if (child && typeof child === 'object') walk(child as JsonObject, path, seen)
      }
    }
    const items = schema.items
    if (items && typeof items === 'object' && prefix) {
      walk(items as JsonObject, `${prefix}[0]`, seen)
    }
  }

  walk(schemas[root], '', [root])
  return out
}

/** How many times each component schema is referenced across the document. */
export function refUsage(doc: unknown): Record<string, number> {
  const counts: Record<string, number> = {}
  const walk = (value: unknown) => {
    if (Array.isArray(value)) {
      value.forEach(walk)
      return
    }
    if (!value || typeof value !== 'object') return
    for (const [key, child] of Object.entries(value as JsonObject)) {
      if (key === '$ref' && typeof child === 'string') {
        const name = refName(child)
        if (name) counts[name] = (counts[name] ?? 0) + 1
      } else {
        walk(child)
      }
    }
  }
  walk(doc)
  return counts
}

// ---------------------------------------------------------------------------
// Edits — every one returns a new document, preserving unmodelled keywords
// ---------------------------------------------------------------------------

function clone<T>(value: T): T {
  return structuredClone(value)
}

/** Resolve a pointer to its container and final key, for in-place mutation. */
function locate(
  doc: unknown,
  pointer: string,
): { parent: JsonObject; key: string } | null {
  const segments = pointer.split('/').slice(1).map(unescapePointer)
  if (segments.length === 0) return null
  const key = segments.pop()!
  let current: unknown = doc
  for (const segment of segments) {
    if (!current || typeof current !== 'object') return null
    current = (current as JsonObject)[segment]
  }
  if (!current || typeof current !== 'object') return null
  return { parent: current as JsonObject, key }
}

/**
 * Merge keywords into the schema object at `pointer`.
 *
 * Setting a value to `undefined` removes that keyword. Everything else in the
 * object is left alone — this is what keeps `oneOf`, `pattern` and friends
 * intact through a structured edit.
 */
export function setKeywords(
  doc: unknown,
  pointer: string,
  changes: Record<string, unknown>,
): unknown {
  const next = clone(doc)
  const target = getAtPointer(next, pointer)
  if (!target || typeof target !== 'object') return next

  const schema = target as JsonObject
  for (const [key, value] of Object.entries(changes)) {
    if (value === undefined) delete schema[key]
    else schema[key] = value
  }
  return next
}

/**
 * Make a node accept `null`, or stop it.
 *
 * Writes `nullable: true`, the OpenAPI 3.0 spelling: it is what the registry
 * stores (a `type` list is refused on save) and the bus's Ajv honours it. Any
 * `"null"` in a `type` list is folded away on the same edit. A node with no
 * declared type is left alone — it already accepts null, and OpenAPI 3.0 has
 * no way to assert a field is *always* null.
 */
export function setAcceptsNull(
  doc: unknown,
  pointer: string,
  accepts: boolean,
): unknown {
  const next = clone(doc)
  const target = getAtPointer(next, pointer)
  if (!target || typeof target !== 'object') return next

  const schema = target as JsonObject
  const named = namedTypes(schema)
  if (named.length === 0) return next

  // Back to the plain string form: a list is not a spelling the registry
  // will take, whatever it holds.
  schema.type = named.length === 1 ? named[0] : named
  if (accepts) schema.nullable = true
  else delete schema.nullable
  return next
}

/**
 * Whether a node refuses the empty value of its type: `""` for a string, zero
 * or less for a number.
 *
 * `required` alone does not — the key exists, so it is satisfied — which is
 * how a producer sends `{"ssn": ""}` past a schema that requires `ssn`.
 */
export function isNonEmpty(type: NodeType, constraints: Record<string, unknown>): boolean {
  switch (type) {
    case 'string': {
      const min = constraints.minLength
      return typeof min === 'number' && min >= 1
    }
    // Still reads an `exclusiveMinimum` left by an older draft, so a document
    // that has one shows the toggle on rather than silently unticked.
    case 'integer':
    case 'number': {
      const above = constraints.exclusiveMinimum
      const min = constraints.minimum
      return (
        (typeof above === 'number' && above >= 0) || (typeof min === 'number' && min > 0)
      )
    }
    default:
      return false
  }
}

/**
 * The keyword changes that make a node non-empty, or allow empty again.
 *
 * `null` for a type with no notion of empty. Turning it on leaves a stricter
 * bound alone — a string that must be four characters is already non-empty.
 * Turning it off removes the lower bound entirely, because that is what
 * allowing empty means, even where the bound was tighter than one.
 *
 * Offered for integers but not for floats, because only an integer can say
 * "greater than zero" in a spelling that survives the registry: `minimum: 1`.
 * The exclusive bound that would say it for a float is refused by OpenAPI 3.0
 * as a number and by Ajv as a boolean, so a toggle for one would write a
 * document that cannot be saved — which is what it used to do.
 */
export function nonEmptyPatch(
  type: NodeType,
  constraints: Record<string, unknown>,
  on: boolean,
): Record<string, unknown> | null {
  switch (type) {
    case 'string':
      if (on) return isNonEmpty(type, constraints) ? {} : { minLength: 1 }
      return { minLength: undefined }
    case 'integer':
      if (on) return isNonEmpty(type, constraints) ? {} : { minimum: 1 }
      return { exclusiveMinimum: undefined, minimum: undefined }
    // Nothing to offer a float: "greater than zero" needs the exclusive bound
    // the registry refuses. Turning it *off* still works, so a draft written
    // by the old toggle can be cleared rather than sitting there unsavable.
    case 'number':
      return on ? null : { exclusiveMinimum: undefined, minimum: undefined }
    default:
      return null
  }
}

/** Toggle a property's presence in its parent object's `required` array. */
export function setRequired(
  doc: unknown,
  parentPointer: string,
  name: string,
  required: boolean,
): unknown {
  const next = clone(doc)
  const parent = getAtPointer(next, parentPointer)
  if (!parent || typeof parent !== 'object') return next

  const schema = parent as JsonObject
  const current = Array.isArray(schema.required)
    ? (schema.required as unknown[]).filter((r): r is string => typeof r === 'string')
    : []

  if (required) {
    if (!current.includes(name)) schema.required = [...current, name]
  } else {
    const remaining = current.filter((r) => r !== name)
    // An empty `required` is legal but noisy; the registry's simplified
    // schemas drop it entirely, so match that.
    if (remaining.length === 0) delete schema.required
    else schema.required = remaining
  }
  return next
}

/**
 * Rename a property, preserving its position in the object.
 *
 * Key order is what determines field order in the tree and in generated
 * samples, so a rename must not send the field to the end.
 */
export function renameProperty(
  doc: unknown,
  parentPointer: string,
  from: string,
  to: string,
): unknown {
  if (from === to) return doc
  const next = clone(doc)
  const parent = getAtPointer(next, parentPointer)
  if (!parent || typeof parent !== 'object') return next

  const schema = parent as JsonObject
  const properties = schema.properties
  if (!properties || typeof properties !== 'object') return next

  const props = properties as JsonObject
  if (!(from in props) || to in props || to === '') return next

  schema.properties = Object.fromEntries(
    Object.entries(props).map(([key, value]) => (key === from ? [to, value] : [key, value])),
  )

  if (Array.isArray(schema.required)) {
    schema.required = (schema.required as unknown[]).map((r) => (r === from ? to : r))
  }
  return next
}

function blankSchemaFor(type: NodeType): JsonObject {
  switch (type) {
    case 'object':
      return { type: 'object', properties: {}, additionalProperties: true }
    case 'array':
      return { type: 'array', items: { type: 'string' } }
    case 'ref':
      return { $ref: '' }
    default:
      return { type }
  }
}

/** Add a property to the object at `pointer`. */
export function addProperty(
  doc: unknown,
  pointer: string,
  name: string,
  type: NodeType = 'string',
): unknown {
  const next = clone(doc)
  const target = getAtPointer(next, pointer)
  if (!target || typeof target !== 'object') return next

  const schema = target as JsonObject
  // Adding a field to something not yet an object should make it one.
  if (!schema.properties || typeof schema.properties !== 'object') {
    schema.type = 'object'
    schema.properties = {}
  }
  const props = schema.properties as JsonObject
  if (name in props || name === '') return next

  props[name] = blankSchemaFor(type)
  return next
}

/** Remove a property, and drop it from the parent's `required` list. */
export function removeProperty(
  doc: unknown,
  parentPointer: string,
  name: string,
): unknown {
  const next = clone(doc)
  const parent = getAtPointer(next, parentPointer)
  if (!parent || typeof parent !== 'object') return next

  const schema = parent as JsonObject
  const properties = schema.properties
  if (properties && typeof properties === 'object') {
    delete (properties as JsonObject)[name]
  }

  if (Array.isArray(schema.required)) {
    const remaining = (schema.required as unknown[]).filter((r) => r !== name)
    if (remaining.length === 0) delete schema.required
    else schema.required = remaining
  }
  return next
}

/**
 * Change a node's type, keeping the keywords that still make sense.
 *
 * Switching away from object/array/ref discards the sub-shape, which is
 * destructive but unambiguous; `description` and null-acceptance survive
 * because they are type-independent.
 *
 * Takes the *field's* pointer — [`SchemaNode.ownPointer`], not `pointer`:
 * this replaces the object at the pointer wholesale, so through a `$ref` the
 * wrong one rewrites the shared definition.
 */
export function setNodeType(
  doc: unknown,
  fieldPointer: string,
  type: NodeType,
): unknown {
  const next = clone(doc)
  const located = locate(next, fieldPointer)
  if (!located) return next

  const existing = located.parent[located.key]
  const previous = (existing && typeof existing === 'object' ? existing : {}) as JsonObject

  const preserved: JsonObject = {}
  if (typeof previous.description === 'string') preserved.description = previous.description

  const rebuilt: JsonObject = { ...blankSchemaFor(type), ...preserved }

  // Null acceptance survives a retype. A `ref` has no type of its own to
  // qualify.
  const wasNullable =
    previous.nullable === true || typeNames(previous).includes('null')
  if (wasNullable && type !== 'ref' && typeof rebuilt.type === 'string') {
    rebuilt.nullable = true
  }

  // Keep a compatible sub-shape rather than throwing it away.
  if (type === 'object' && previous.properties && typeof previous.properties === 'object') {
    rebuilt.properties = previous.properties
    if (Array.isArray(previous.required)) rebuilt.required = previous.required
  }
  if (type === 'array' && previous.items && typeof previous.items === 'object') {
    rebuilt.items = previous.items
  }
  if (type !== 'ref' && typeof previous.format === 'string') {
    rebuilt.format = previous.format
  }
  if (type !== 'ref' && Array.isArray(previous.enum)) {
    rebuilt.enum = previous.enum
  }

  located.parent[located.key] = rebuilt
  return next
}

/**
 * Point a `$ref` node at a different component schema.
 *
 * Takes the *field's* pointer — [`SchemaNode.ownPointer`], not `pointer`. Both
 * this and [`setNodeType`] replace the object they are given, so aiming either
 * at a `$ref`'s target rewrites the shared definition instead of the field.
 */
export function setRefTarget(
  doc: unknown,
  fieldPointer: string,
  target: string,
): unknown {
  const next = clone(doc)
  const located = locate(next, fieldPointer)
  if (!located) return next
  // A ref is exclusive: any sibling keywords would be ignored by validators.
  located.parent[located.key] = { $ref: `${REF_PREFIX}${target}` }
  return next
}

/**
 * Which component schemas reference `name`, and how many times.
 *
 * Deleting a referenced schema would leave dangling `$ref`s that only fail
 * later, so the UI needs to name the dependants before offering to remove it.
 */
export function refReferrers(doc: unknown, name: string): Record<string, number> {
  const target = `${REF_PREFIX}${name}`
  const referrers: Record<string, number> = {}

  for (const [owner, schema] of Object.entries(componentSchemas(doc))) {
    let count = 0
    const walk = (value: unknown) => {
      if (Array.isArray(value)) {
        value.forEach(walk)
        return
      }
      if (!value || typeof value !== 'object') return
      for (const [key, child] of Object.entries(value as JsonObject)) {
        if (key === '$ref' && child === target) count += 1
        else walk(child)
      }
    }
    walk(schema)
    if (count > 0) referrers[owner] = count
  }
  return referrers
}

/**
 * Rename a type, rewriting every `$ref` that points at it.
 *
 * Renaming the key alone would leave every referring field pointing at a
 * definition that no longer exists, so the two must happen together.
 */
export function renameComponentSchema(
  doc: unknown,
  from: string,
  to: string,
): unknown {
  if (from === to) return doc

  const next = clone(doc)
  const schemas = getAtPointer(next, '/components/schemas')
  if (!schemas || typeof schemas !== 'object') return next

  const map = schemas as JsonObject
  if (!(from in map) || to in map || to.trim() === '') return next

  // Rebuild the map in place so the type keeps its position in the list.
  const renamed = Object.fromEntries(
    Object.entries(map).map(([key, value]) => (key === from ? [to, value] : [key, value])),
  )
  const components = getAtPointer(next, '/components') as JsonObject
  components.schemas = renamed

  const oldRef = `${REF_PREFIX}${from}`
  const newRef = `${REF_PREFIX}${to}`
  const rewrite = (value: unknown) => {
    if (Array.isArray(value)) {
      value.forEach(rewrite)
      return
    }
    if (!value || typeof value !== 'object') return
    const obj = value as JsonObject
    for (const [key, child] of Object.entries(obj)) {
      if (key === '$ref' && child === oldRef) obj[key] = newRef
      else rewrite(child)
    }
  }
  rewrite(next)

  return next
}

/** Remove a component schema outright. */
export function removeComponentSchema(doc: unknown, name: string): unknown {
  const next = clone(doc) as JsonObject
  const schemas = getAtPointer(next, '/components/schemas')
  if (!schemas || typeof schemas !== 'object') return next
  delete (schemas as JsonObject)[name]
  return next
}

/** Add a new, empty component schema. */
export function addComponentSchema(doc: unknown, name: string): unknown {
  const next = clone(doc) as JsonObject
  const components = (next.components ??= {}) as JsonObject
  const schemas = (components.schemas ??= {}) as JsonObject
  if (name in schemas || name === '') return next
  schemas[name] = { type: 'object', properties: {}, additionalProperties: true }
  return next
}

/**
 * A field's dotted path as a producer would name it — `payload.matterId` —
 * from the JSON Pointer that addresses its schema: the segments that follow
 * each `properties`, with array items as `[]`.
 */
export function fieldPathFromPointer(pointer: string): string {
  const segments = pointer.split('/').slice(1)
  const out: string[] = []
  for (let i = 0; i < segments.length; i++) {
    if (segments[i] === 'properties' && i + 1 < segments.length) {
      out.push(segments[++i].replace(/~1/g, '/').replace(/~0/g, '~'))
    } else if (segments[i] === 'items') {
      out.push('[]')
    }
  }
  return out.join('.').replace(/\.\[\]/g, '[]')
}
