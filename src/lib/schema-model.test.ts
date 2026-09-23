import { describe, expect, it } from 'vitest'
import {
  addProperty,
  buildTree,
  getAtPointer,
  componentSchemas,
  declaredPaths,
  payloadSchemaName,
  refReferrers,
  removeComponentSchema,
  renameComponentSchema,
  refUsage,
  removeProperty,
  renameProperty,
  setKeywords,
  isNonEmpty,
  nonEmptyPatch,
  setAcceptsNull,
  setNodeType,
  setRefTarget,
  setRequired,
  type SchemaNode,
} from './schema-model'

/**
 * Modelled on the shape of the real registry documents: a detail schema whose
 * fields are `$ref`s into sibling component schemas, which is how
 * ClientProfileV1Sync (61 component schemas, refs all the way down) is built.
 */
function doc() {
  return {
    openapi: '3.0.0',
    components: {
      schemas: {
        AWSEvent: {
          type: 'object',
          required: ['detail', 'source'],
          properties: { detail: { $ref: '#/components/schemas/Sync' } },
        },
        Sync: {
          type: 'object',
          required: ['clientId'],
          properties: {
            clientId: { type: 'string', description: 'Client identifier' },
            metadata: { $ref: '#/components/schemas/Metadata' },
            items: {
              type: 'array',
              items: { $ref: '#/components/schemas/Item' },
            },
            note: { type: 'string', nullable: true },
          },
        },
        Metadata: {
          type: 'object',
          required: ['trackingId'],
          properties: {
            trackingId: { type: 'string' },
            publishedAt: { type: 'string', format: 'date-time' },
          },
        },
        Item: {
          type: 'object',
          properties: { id: { type: 'string' }, qty: { type: 'integer' } },
        },
      },
    },
  }
}

function find(node: SchemaNode, id: string): SchemaNode | null {
  if (node.id === id) return node
  for (const child of node.children) {
    const hit = find(child, id)
    if (hit) return hit
  }
  return null
}

describe('buildTree', () => {
  it('resolves $ref inline so the whole shape is visible at once', () => {
    const tree = buildTree(doc(), 'Sync')!
    const metadata = find(tree, 'Sync/metadata')!

    expect(metadata.refTarget).toBe('Metadata')
    expect(metadata.viaRef).toBe(true)
    // The target's fields appear in place rather than as an opaque ref.
    expect(metadata.children.map((c) => c.name)).toEqual([
      'trackingId',
      'publishedAt',
    ])
  })

  it('points ref-resolved nodes at the definition they would actually edit', () => {
    const tree = buildTree(doc(), 'Sync')!
    const trackingId = find(tree, 'Sync/metadata/trackingId')!
    expect(trackingId.pointer).toBe(
      '/components/schemas/Metadata/properties/trackingId',
    )
  })

  it('keeps a ref node aimed at its own property as well as the definition', () => {
    const tree = buildTree(doc(), 'Sync')!
    const metadata = find(tree, 'Sync/metadata')!

    // `pointer` reads the shape, `ownPointer` writes the field. Retyping this
    // field through `pointer` would rewrite Metadata for every user of it.
    expect(metadata.pointer).toBe('/components/schemas/Metadata')
    expect(metadata.ownPointer).toBe('/components/schemas/Sync/properties/metadata')

    const clientId = find(tree, 'Sync/clientId')!
    expect(clientId.ownPointer).toBe(clientId.pointer)
  })

  it('retyping a ref field replaces the field, not the type it points at', () => {
    const tree = buildTree(doc(), 'Sync')!
    const metadata = find(tree, 'Sync/metadata')!

    const next = setNodeType(doc(), metadata.ownPointer, 'string')
    expect(getAtPointer(next, '/components/schemas/Sync/properties/metadata')).toEqual({
      type: 'string',
    })
    // The shared definition is untouched.
    expect(getAtPointer(next, '/components/schemas/Metadata')).toEqual(
      doc().components.schemas.Metadata,
    )
  })

  it('follows refs through array items', () => {
    const tree = buildTree(doc(), 'Sync')!
    const items = find(tree, 'Sync/items')!
    expect(items.type).toBe('array')

    const element = items.children[0]
    expect(element.name).toBe('items')
    expect(element.refTarget).toBe('Item')
    expect(element.children.map((c) => c.name)).toEqual(['id', 'qty'])
  })

  it('reads required, nullable, format and description', () => {
    const tree = buildTree(doc(), 'Sync')!
    expect(find(tree, 'Sync/clientId')!.required).toBe(true)
    expect(find(tree, 'Sync/clientId')!.description).toBe('Client identifier')
    expect(find(tree, 'Sync/note')!.required).toBe(false)
    expect(find(tree, 'Sync/note')!.acceptsNull).toBe(true)
    expect(find(tree, 'Sync/metadata/publishedAt')!.format).toBe('date-time')
  })

  it('stops at a cycle instead of recursing forever', () => {
    const cyclic = {
      components: {
        schemas: {
          Node: {
            type: 'object',
            properties: {
              child: { $ref: '#/components/schemas/Node' },
            },
          },
        },
      },
    }
    const tree = buildTree(cyclic, 'Node')!
    const child = find(tree, 'Node/child')!
    expect(child.cyclic).toBe(true)
    expect(child.children).toEqual([])
  })

  it('models the constraints Ajv enforces rather than listing them as extras', () => {
    const withConstraints = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: {
              weird: { type: 'string', pattern: '^x', minLength: 2, 'x-owner': 'me' },
            },
          },
        },
      },
    }
    const node = find(buildTree(withConstraints, 'A')!, 'A/weird')!
    // `pattern` rejects live traffic; it belongs in a control, not in a list of
    // things the editor shrugs at.
    expect(node.constraints).toEqual({ pattern: '^x', minLength: 2 })
    // Genuinely unmodelled keywords still surface.
    expect(node.extraKeywords).toEqual(['x-owner'])
  })

  it('reads a null-accepting type as its named type', () => {
    const widened = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: { note: { type: ['string', 'null'] } },
          },
        },
      },
    }
    const node = find(buildTree(widened, 'A')!, 'A/note')!
    // Would have read as `unknown` before, which is what the inference change
    // now writes for every sometimes-null field.
    expect(node.type).toBe('string')
    expect(node.acceptsNull).toBe(true)
  })

  it('reads `nullable: true` and a null type list as the same fact', () => {
    const both = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: {
              spelled: { type: 'string', nullable: true },
              listed: { type: ['string', 'null'] },
            },
          },
        },
      },
    }
    const tree = buildTree(both, 'A')!
    // The bus's Ajv honours `nullable`; the list is what a pasted JSON
    // Schema says, and the registry will not store it.
    expect(find(tree, 'A/spelled')!.acceptsNull).toBe(true)
    expect(find(tree, 'A/listed')!.acceptsNull).toBe(true)
  })

  it('surfaces a constraint that does not apply to the declared type', () => {
    // `minLength` means nothing on a boolean, so it is not a control — but it
    // must still appear somewhere. It used to match neither the type's
    // constraint list nor the global modelled set and so showed in neither the
    // Constraints section nor "Also defines": invisible in the structured
    // editor, and easy to delete by accident.
    const mismatched = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: { flag: { type: 'boolean', minLength: 3 } },
          },
        },
      },
    }
    const node = find(buildTree(mismatched, 'A')!, 'A/flag')!
    expect(node.constraints).toEqual({})
    expect(node.extraKeywords).toEqual(['minLength'])
  })

  it('records composition branches', () => {
    const composed = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: {
              either: { oneOf: [{ type: 'string' }, { type: 'number' }] },
            },
          },
        },
      },
    }
    const node = find(buildTree(composed, 'A')!, 'A/either')!
    expect(node.composition).toEqual([{ keyword: 'oneOf', count: 2 }])
    expect(node.extraKeywords).toEqual([])
  })

  it('returns null for a schema that does not exist', () => {
    expect(buildTree(doc(), 'Nope')).toBeNull()
  })
})

describe('payloadSchemaName', () => {
  it('follows the envelope detail ref rather than guessing by key order', () => {
    // AWSEvent is declared first, so order-based guessing would pick wrong.
    expect(payloadSchemaName(doc())).toBe('Sync')
  })

  it('falls back to the first non-envelope schema when detail has no ref', () => {
    const noRef = {
      components: {
        schemas: {
          AWSEvent: { type: 'object', properties: { detail: { type: 'object' } } },
          Payload: { type: 'object' },
        },
      },
    }
    expect(payloadSchemaName(noRef)).toBe('Payload')
  })

  it('ignores a detail ref that points at a missing schema', () => {
    const dangling = {
      components: {
        schemas: {
          AWSEvent: {
            properties: { detail: { $ref: '#/components/schemas/Gone' } },
          },
          Real: { type: 'object' },
        },
      },
    }
    expect(payloadSchemaName(dangling)).toBe('Real')
  })

  it('returns undefined for an empty document', () => {
    expect(payloadSchemaName({})).toBeUndefined()
  })
})

describe('refUsage', () => {
  it('counts how many places depend on each component schema', () => {
    const usage = refUsage(doc())
    expect(usage).toEqual({ Sync: 1, Metadata: 1, Item: 1 })
  })
})

describe('additionalProperties', () => {
  it('distinguishes absent, explicit true, explicit false and a sub-schema', () => {
    const variants = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: {
              absent: { type: 'object', properties: {} },
              open: { type: 'object', properties: {}, additionalProperties: true },
              strict: { type: 'object', properties: {}, additionalProperties: false },
              constrained: {
                type: 'object',
                properties: {},
                additionalProperties: { type: 'string' },
              },
            },
          },
        },
      },
    }
    const tree = buildTree(variants, 'A')!
    expect(find(tree, 'A/absent')!.additionalProperties).toBeUndefined()
    expect(find(tree, 'A/open')!.additionalProperties).toBe(true)
    expect(find(tree, 'A/strict')!.additionalProperties).toBe(false)
    expect(find(tree, 'A/constrained')!.additionalProperties).toBe('schema')
  })

  it('is not reported as an unmodelled keyword', () => {
    const tree = buildTree(
      {
        components: {
          schemas: {
            A: {
              type: 'object',
              properties: { f: { type: 'object', additionalProperties: false } },
            },
          },
        },
      },
      'A',
    )!
    expect(find(tree, 'A/f')!.extraKeywords).toEqual([])
  })

  it('round-trips through setKeywords', () => {
    let next = setKeywords(doc(), '/components/schemas/Sync', {
      additionalProperties: false,
    })
    expect(buildTree(next, 'Sync')!.additionalProperties).toBe(false)

    next = setKeywords(next, '/components/schemas/Sync', {
      additionalProperties: undefined,
    })
    expect(buildTree(next, 'Sync')!.additionalProperties).toBeUndefined()
  })
})

describe('component schema removal', () => {
  it('names every schema that references a definition', () => {
    expect(refReferrers(doc(), 'Metadata')).toEqual({ Sync: 1 })
    expect(refReferrers(doc(), 'Sync')).toEqual({ AWSEvent: 1 })
    // Item is referenced from inside an array's items.
    expect(refReferrers(doc(), 'Item')).toEqual({ Sync: 1 })
  })

  it('reports no referrers for an orphan', () => {
    const withOrphan = {
      components: {
        schemas: { A: { type: 'object' }, Orphan: { type: 'object' } },
      },
    }
    expect(refReferrers(withOrphan, 'Orphan')).toEqual({})
  })

  it('counts multiple references from the same schema', () => {
    const twice = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: {
              one: { $ref: '#/components/schemas/B' },
              two: { $ref: '#/components/schemas/B' },
            },
          },
          B: { type: 'object' },
        },
      },
    }
    expect(refReferrers(twice, 'B')).toEqual({ A: 2 })
  })

  it('removes the schema and leaves the rest intact', () => {
    const next = removeComponentSchema(doc(), 'Item')
    expect(Object.keys(componentSchemas(next))).toEqual([
      'AWSEvent',
      'Sync',
      'Metadata',
    ])
  })

  it('does not mutate the input', () => {
    const original = doc()
    const snapshot = JSON.stringify(original)
    removeComponentSchema(original, 'Item')
    expect(JSON.stringify(original)).toBe(snapshot)
  })
})

describe('renameComponentSchema', () => {
  it('rewrites every ref that points at the old name', () => {
    const next = renameComponentSchema(doc(), 'Metadata', 'EventMetadata')
    expect(Object.keys(componentSchemas(next))).toContain('EventMetadata')
    expect(Object.keys(componentSchemas(next))).not.toContain('Metadata')
    expect(
      getAtPointer(next, '/components/schemas/Sync/properties/metadata'),
    ).toEqual({ $ref: '#/components/schemas/EventMetadata' })
    // Nothing is left pointing at the old name.
    expect(refReferrers(next, 'Metadata')).toEqual({})
    expect(refReferrers(next, 'EventMetadata')).toEqual({ Sync: 1 })
  })

  it('rewrites refs nested inside array items', () => {
    const next = renameComponentSchema(doc(), 'Item', 'LineItem')
    expect(
      getAtPointer(next, '/components/schemas/Sync/properties/items/items'),
    ).toEqual({ $ref: '#/components/schemas/LineItem' })
  })

  it('keeps the type in its original position', () => {
    const next = renameComponentSchema(doc(), 'Sync', 'Renamed')
    expect(Object.keys(componentSchemas(next))).toEqual([
      'AWSEvent',
      'Renamed',
      'Metadata',
      'Item',
    ])
  })

  it('follows the envelope so the payload is still discoverable', () => {
    const next = renameComponentSchema(doc(), 'Sync', 'Renamed')
    expect(payloadSchemaName(next)).toBe('Renamed')
  })

  it('refuses a collision, a blank name, or a missing source', () => {
    const before = JSON.stringify(doc())
    for (const [from, to] of [
      ['Metadata', 'Item'],
      ['Metadata', '  '],
      ['Ghost', 'Anything'],
    ]) {
      expect(JSON.stringify(renameComponentSchema(doc(), from, to))).toBe(before)
    }
  })

  it('does not mutate the input', () => {
    const original = doc()
    const snapshot = JSON.stringify(original)
    renameComponentSchema(original, 'Metadata', 'Other')
    expect(JSON.stringify(original)).toBe(snapshot)
  })
})

describe('edits', () => {
  it('preserves unmodelled keywords when setting one', () => {
    const original = {
      components: {
        schemas: {
          A: {
            type: 'object',
            properties: {
              f: { type: 'string', pattern: '^x', minLength: 3 },
            },
          },
        },
      },
    }
    const next = setKeywords(original, '/components/schemas/A/properties/f', {
      description: 'hello',
    })
    expect(getAtPointer(next, '/components/schemas/A/properties/f')).toEqual({
      type: 'string',
      pattern: '^x',
      minLength: 3,
      description: 'hello',
    })
  })

  it('removes a keyword when set to undefined', () => {
    const next = setKeywords(doc(), '/components/schemas/Sync/properties/note', {
      nullable: undefined,
    })
    expect(
      getAtPointer(next, '/components/schemas/Sync/properties/note'),
    ).toEqual({ type: 'string' })
  })

  it('does not mutate the input document', () => {
    const original = doc()
    const snapshot = JSON.stringify(original)
    setKeywords(original, '/components/schemas/Sync/properties/note', {
      description: 'x',
    })
    addProperty(original, '/components/schemas/Sync', 'added')
    removeProperty(original, '/components/schemas/Sync', 'clientId')
    expect(JSON.stringify(original)).toBe(snapshot)
  })

  it('adds and removes required without leaving an empty array', () => {
    let next = setRequired(doc(), '/components/schemas/Sync', 'note', true)
    expect(
      (getAtPointer(next, '/components/schemas/Sync') as { required: string[] })
        .required,
    ).toEqual(['clientId', 'note'])

    next = setRequired(next, '/components/schemas/Sync', 'note', false)
    next = setRequired(next, '/components/schemas/Sync', 'clientId', false)
    expect(
      getAtPointer(next, '/components/schemas/Sync'),
    ).not.toHaveProperty('required')
  })

  it('renames a property in place and updates required', () => {
    const next = renameProperty(
      doc(),
      '/components/schemas/Sync',
      'clientId',
      'customerId',
    )
    const sync = getAtPointer(next, '/components/schemas/Sync') as {
      properties: Record<string, unknown>
      required: string[]
    }
    // Order matters: a rename must not move the field to the end.
    expect(Object.keys(sync.properties)).toEqual([
      'customerId',
      'metadata',
      'items',
      'note',
    ])
    expect(sync.required).toEqual(['customerId'])
  })

  it('refuses a rename that would collide or blank the name', () => {
    const before = JSON.stringify(doc())
    for (const to of ['note', '']) {
      const next = renameProperty(doc(), '/components/schemas/Sync', 'clientId', to)
      expect(JSON.stringify(next)).toBe(before)
    }
  })

  it('adds a property with a usable starting shape', () => {
    const next = addProperty(doc(), '/components/schemas/Item', 'tags', 'array')
    expect(getAtPointer(next, '/components/schemas/Item/properties/tags')).toEqual({
      type: 'array',
      items: { type: 'string' },
    })
  })

  it('turns a non-object into an object when a field is added', () => {
    const scalar = {
      components: { schemas: { A: { type: 'string' } } },
    }
    const next = addProperty(scalar, '/components/schemas/A', 'f')
    expect(getAtPointer(next, '/components/schemas/A')).toMatchObject({
      type: 'object',
      properties: { f: { type: 'string' } },
    })
  })

  it('removes a property and its required entry together', () => {
    const next = removeProperty(doc(), '/components/schemas/Sync', 'clientId')
    const sync = getAtPointer(next, '/components/schemas/Sync') as {
      properties: Record<string, unknown>
    }
    expect(sync).not.toHaveProperty('required')
    expect(Object.keys(sync.properties)).toEqual(['metadata', 'items', 'note'])
  })

  it('keeps a compatible sub-shape when retyping', () => {
    // object -> array -> object should not lose the properties.
    let next = setNodeType(doc(), '/components/schemas/Metadata', 'array')
    expect(getAtPointer(next, '/components/schemas/Metadata')).toMatchObject({
      type: 'array',
    })

    next = setNodeType(
      doc(),
      '/components/schemas/Sync/properties/items',
      'array',
    )
    // Existing items shape survives a no-op retype.
    expect(
      getAtPointer(next, '/components/schemas/Sync/properties/items/items'),
    ).toEqual({ $ref: '#/components/schemas/Item' })
  })

  it('writes `nullable: true`, the spelling the registry stores', () => {
    const next = setAcceptsNull(
      doc(),
      '/components/schemas/Sync/properties/clientId',
      true,
    )
    expect(getAtPointer(next, '/components/schemas/Sync/properties/clientId')).toEqual({
      type: 'string',
      nullable: true,
      description: 'Client identifier',
    })
  })

  it('folds a pasted null type list into `nullable`', () => {
    const listed = {
      components: {
        schemas: { A: { type: 'object', properties: { n: { type: ['string', 'null'] } } } },
      },
    }
    const on = setAcceptsNull(listed, '/components/schemas/A/properties/n', true)
    expect(getAtPointer(on, '/components/schemas/A/properties/n')).toEqual({
      type: 'string',
      nullable: true,
    })
    const off = setAcceptsNull(listed, '/components/schemas/A/properties/n', false)
    expect(getAtPointer(off, '/components/schemas/A/properties/n')).toEqual({ type: 'string' })
  })

  it('narrows back to a plain type rather than leaving a one-element array', () => {
    const widened = setAcceptsNull(
      doc(),
      '/components/schemas/Sync/properties/clientId',
      true,
    )
    const narrowed = setAcceptsNull(
      widened,
      '/components/schemas/Sync/properties/clientId',
      false,
    )
    expect(getAtPointer(narrowed, '/components/schemas/Sync/properties/clientId')).toEqual({
      type: 'string',
      description: 'Client identifier',
    })
  })

  it('leaves an untyped field alone rather than asserting it is always null', () => {
    const untyped = {
      components: {
        schemas: { A: { type: 'object', properties: { any: { description: 'x' } } } },
      },
    }
    const next = setAcceptsNull(untyped, '/components/schemas/A/properties/any', true)
    // `type: ["null"]` would be a new constraint, and a wrong one.
    expect(getAtPointer(next, '/components/schemas/A/properties/any')).toEqual({
      description: 'x',
    })
  })

  it('carries null acceptance across a type change', () => {
    const next = setNodeType(
      doc(),
      '/components/schemas/Sync/properties/note',
      'integer',
    )
    expect(getAtPointer(next, '/components/schemas/Sync/properties/note')).toEqual({
      type: 'integer',
      nullable: true,
    })
  })

  it('replaces the node wholesale when pointing at a ref', () => {
    const next = setRefTarget(
      doc(),
      '/components/schemas/Sync/properties/note',
      'Item',
    )
    // Sibling keywords must go: a $ref is exclusive.
    expect(getAtPointer(next, '/components/schemas/Sync/properties/note')).toEqual({
      $ref: '#/components/schemas/Item',
    })
  })

  it('leaves the document alone for pointers that do not resolve', () => {
    const before = JSON.stringify(doc())
    expect(
      JSON.stringify(setKeywords(doc(), '/components/schemas/Ghost', { a: 1 })),
    ).toBe(before)
    expect(
      JSON.stringify(removeProperty(doc(), '/components/schemas/Ghost', 'x')),
    ).toBe(before)
  })
})

describe('non-empty', () => {
  it('reads a string as non-empty from any positive minLength', () => {
    expect(isNonEmpty('string', {})).toBe(false)
    expect(isNonEmpty('string', { minLength: 0 })).toBe(false)
    expect(isNonEmpty('string', { minLength: 1 })).toBe(true)
    expect(isNonEmpty('string', { minLength: 4 })).toBe(true)
  })

  it('reads a number as non-empty when zero is excluded', () => {
    expect(isNonEmpty('number', {})).toBe(false)
    expect(isNonEmpty('integer', { minimum: 0 })).toBe(false)
    expect(isNonEmpty('number', { exclusiveMinimum: 0 })).toBe(true)
    expect(isNonEmpty('integer', { minimum: 1 })).toBe(true)
  })

  it('writes the smallest bound that refuses empty, and leaves a stricter one', () => {
    expect(nonEmptyPatch('string', {}, true)).toEqual({ minLength: 1 })
    expect(nonEmptyPatch('string', { minLength: 4 }, true)).toEqual({})
    expect(nonEmptyPatch('integer', {}, true)).toEqual({ exclusiveMinimum: 0 })
  })

  it('removes the lower bound when empty is allowed again', () => {
    expect(nonEmptyPatch('string', { minLength: 4 }, false)).toEqual({ minLength: undefined })
    expect(nonEmptyPatch('number', { minimum: 1 }, false)).toEqual({
      exclusiveMinimum: undefined,
      minimum: undefined,
    })
  })

  it('does not apply to types with no empty value', () => {
    expect(nonEmptyPatch('boolean', {}, true)).toBeNull()
    expect(nonEmptyPatch('object', {}, true)).toBeNull()
    expect(isNonEmpty('array', { minItems: 1 })).toBe(false)
  })
})

describe('declaredPaths', () => {
  it('lists payload fields through refs and arrays, in condition syntax', () => {
    const doc = {
      components: {
        schemas: {
          AWSEvent: { type: 'object', properties: { detail: { $ref: '#/components/schemas/Order' } } },
          Order: {
            type: 'object',
            properties: {
              id: { type: 'string' },
              client: { $ref: '#/components/schemas/Client' },
              items: { type: 'array', items: { $ref: '#/components/schemas/Line' } },
            },
          },
          Client: { type: 'object', properties: { dob: { type: 'string' }, parent: { $ref: '#/components/schemas/Client' } } },
          Line: { type: 'object', properties: { sku: { type: 'string' } } },
        },
      },
    }
    expect(declaredPaths(doc)).toEqual([
      'id',
      'client',
      'client.dob',
      'client.parent',
      'items',
      'items[0].sku',
    ])
  })

  it('is empty for a document with no payload type', () => {
    expect(declaredPaths({})).toEqual([])
  })
})
