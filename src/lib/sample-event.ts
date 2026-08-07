/**
 * Generate a realistic example event from a schema.
 *
 * Reading a 61-schema document tells you the shape but not what an actual
 * event looks like. A sample answers that instantly, and doubles as something
 * you can paste straight into `aws events put-events` to exercise a rule.
 */

import {
  componentSchemas,
  namedTypes,
  refName,
  type JsonObject,
} from './schema-model'

/**
 * The type to generate a value for.
 *
 * A sample event is meant to be representative, and `null` never is — so a
 * `["integer", "null"]` field samples as an integer. Reading only the string
 * form would have made every such field sample as `"example"`, which since
 * inference started writing the array spelling would be most of them.
 */
function sampleType(schema: JsonObject): string | undefined {
  return namedTypes(schema)[0]
}

/** Plausible values keyed on what the field is called, not just its type. */
function scalarFor(name: string, schema: JsonObject): unknown {
  const type = sampleType(schema) ?? 'string'
  const format = typeof schema.format === 'string' ? schema.format : undefined

  if (Array.isArray(schema.enum) && schema.enum.length > 0) return schema.enum[0]
  if (schema.example !== undefined) return schema.example
  if (schema.default !== undefined) return schema.default

  switch (type) {
    case 'boolean':
      return true
    case 'integer':
      return /count|qty|quantity|total|index|age/i.test(name) ? 3 : 1
    case 'number':
      return /amount|price|balance|rate/i.test(name) ? 42.5 : 1.5
    case 'string':
    default:
      if (format === 'date-time') return '2026-01-01T12:00:00Z'
      if (format === 'date') return '2026-01-01'
      if (format === 'email') return 'person@example.com'
      if (format === 'uuid' || /uuid$/i.test(name))
        return '3f2504e0-4f89-11d3-9a0c-0305e82c3301'
      if (/arn$/i.test(name)) return 'arn:aws:service:us-east-2:123456789012:resource/example'
      // ID-like names are the registry's required fields, so make them look
      // like identifiers rather than generic placeholder text.
      if (/ids$/i.test(name)) return 'id-1,id-2'
      if (/id$/i.test(name)) return `${name.replace(/Id$/, '') || 'example'}-123`
      if (/email/i.test(name)) return 'person@example.com'
      if (/url|uri/i.test(name)) return 'https://example.com/resource'
      if (/name/i.test(name)) return 'Example Name'
      if (/status|state/i.test(name)) return 'active'
      return 'example'
  }
}

interface SampleContext {
  doc: unknown
  /** Component names being expanded, to break `$ref` cycles. */
  stack: string[]
}

function sampleFor(
  schema: JsonObject | undefined,
  name: string,
  ctx: SampleContext,
): unknown {
  if (!schema || typeof schema !== 'object') return null

  if (typeof schema.$ref === 'string') {
    const target = refName(schema.$ref)
    if (!target) return null
    // A self-referencing schema would recurse forever; null is the honest
    // stand-in for "this nests further".
    if (ctx.stack.includes(target)) return null
    const resolved = componentSchemas(ctx.doc)[target]
    return sampleFor(resolved, name, { ...ctx, stack: [...ctx.stack, target] })
  }

  const type =
    sampleType(schema) ??
    (schema.properties ? 'object' : schema.items ? 'array' : 'string')

  if (type === 'object') {
    const properties = schema.properties
    if (!properties || typeof properties !== 'object') return {}
    const out: JsonObject = {}
    for (const [key, child] of Object.entries(properties as JsonObject)) {
      out[key] = sampleFor(child as JsonObject, key, ctx)
    }
    return out
  }

  if (type === 'array') {
    const items = schema.items
    if (!items || typeof items !== 'object') return []
    // One element is enough to convey the shape without bloating the sample.
    return [sampleFor(items as JsonObject, name, ctx)]
  }

  return scalarFor(name, schema)
}

export interface SampleEventOptions {
  source: string
  detailType: string
  /** Component schema name holding the event payload. */
  detailSchema: string
}

/**
 * Build a complete EventBridge event: the envelope AWS supplies, plus a
 * `detail` generated from the schema.
 */
export function buildSampleEvent(
  doc: unknown,
  { source, detailType, detailSchema }: SampleEventOptions,
): JsonObject {
  const detail = sampleFor(componentSchemas(doc)[detailSchema], detailSchema, {
    doc,
    stack: [detailSchema],
  })

  return {
    version: '0',
    id: '3f2504e0-4f89-11d3-9a0c-0305e82c3301',
    'detail-type': detailType,
    source,
    account: '123456789012',
    time: '2026-01-01T12:00:00Z',
    region: 'us-east-2',
    resources: [],
    detail: detail ?? {},
  }
}

/**
 * The `aws events put-events` invocation for a sample, ready to paste.
 *
 * The CLI takes the detail as an escaped JSON *string*, which is the fiddly
 * part to get right by hand.
 */
export function putEventsCommand(
  event: JsonObject,
  busName: string,
  region: string,
): string {
  const entry = {
    Source: event.source,
    DetailType: event['detail-type'],
    Detail: JSON.stringify(event.detail),
    EventBusName: busName,
  }
  return [
    'aws events put-events \\',
    `  --region ${region} \\`,
    `  --entries '${JSON.stringify([entry]).replace(/'/g, "'\\''")}'`,
  ].join('\n')
}
