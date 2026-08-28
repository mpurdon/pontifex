import { describe, expect, it } from 'vitest'
import { buildSampleEvent, putEventsCommand } from './sample-event'

function doc() {
  return {
    components: {
      schemas: {
        Sync: {
          type: 'object',
          properties: {
            veteranId: { type: 'string' },
            caseRecordIds: { type: 'string' },
            publishedAt: { type: 'string', format: 'date-time' },
            attempts: { type: 'integer' },
            active: { type: 'boolean' },
            status: { type: 'string', enum: ['queued', 'done'] },
            metadata: { $ref: '#/components/schemas/Metadata' },
            items: { type: 'array', items: { $ref: '#/components/schemas/Item' } },
          },
        },
        Metadata: {
          type: 'object',
          properties: { trackingId: { type: 'string' } },
        },
        Item: {
          type: 'object',
          properties: { id: { type: 'string' }, qty: { type: 'integer' } },
        },
      },
    },
  }
}

describe('buildSampleEvent', () => {
  it('wraps the payload in a complete EventBridge envelope', () => {
    const event = buildSampleEvent(doc(), {
      source: 'orders-api',
      detailType: 'orderNotification-assigned',
      detailSchema: 'Sync',
    })

    expect(event.source).toBe('orders-api')
    expect(event['detail-type']).toBe('orderNotification-assigned')
    for (const key of [
      'version',
      'id',
      'account',
      'time',
      'region',
      'resources',
      'detail',
    ]) {
      expect(event).toHaveProperty(key)
    }
  })

  it('generates values that suit the field name and format', () => {
    const detail = buildSampleEvent(doc(), {
      source: 's',
      detailType: 'd',
      detailSchema: 'Sync',
    }).detail as Record<string, unknown>

    expect(detail.veteranId).toBe('veteran-123')
    expect(detail.publishedAt).toBe('2026-01-01T12:00:00Z')
    expect(detail.attempts).toBe(1)
    expect(detail.active).toBe(true)
    // An enum should use a value the schema actually permits.
    expect(detail.status).toBe('queued')
  })

  it('follows refs and fills arrays with one element', () => {
    const detail = buildSampleEvent(doc(), {
      source: 's',
      detailType: 'd',
      detailSchema: 'Sync',
    }).detail as Record<string, unknown>

    expect(detail.metadata).toEqual({ trackingId: 'tracking-123' })
    // `qty` reads as a quantity, so it gets a count-like value rather than 1.
    expect(detail.items).toEqual([{ id: 'id-123', qty: 3 }])
  })

  it('stops at a cycle rather than recursing forever', () => {
    const cyclic = {
      components: {
        schemas: {
          Node: {
            type: 'object',
            properties: {
              name: { type: 'string' },
              child: { $ref: '#/components/schemas/Node' },
            },
          },
        },
      },
    }
    const detail = buildSampleEvent(cyclic, {
      source: 's',
      detailType: 'd',
      detailSchema: 'Node',
    }).detail as Record<string, unknown>

    expect(detail.name).toBe('Example Name')
    expect(detail.child).toBeNull()
  })

  it('produces an empty detail for a schema that does not exist', () => {
    const event = buildSampleEvent(doc(), {
      source: 's',
      detailType: 'd',
      detailSchema: 'Missing',
    })
    expect(event.detail).toEqual({})
  })
})

describe('null-accepting types', () => {
  it('samples the named type, not the null alternative', () => {
    // `type: [T, "null"]` is what inference now writes for every sometimes-null
    // field, so reading only the string form would have sampled most of the
    // registry's numeric fields as the string "example".
    const widened = {
      components: {
        schemas: {
          Sync: {
            type: 'object',
            properties: {
              attempts: { type: ['integer', 'null'] },
              note: { type: ['string', 'null'] },
              tags: { type: ['array', 'null'], items: { type: 'string' } },
            },
          },
        },
      },
    }
    const event = buildSampleEvent(widened, {
      source: 'orders-api',
      detailType: 'thing-happened',
      detailSchema: 'Sync',
    })

    const detail = event.detail as Record<string, unknown>
    expect(typeof detail.attempts).toBe('number')
    expect(typeof detail.note).toBe('string')
    expect(Array.isArray(detail.tags)).toBe(true)
  })
})

describe('putEventsCommand', () => {
  it('escapes the detail as the JSON string the CLI expects', () => {
    const event = buildSampleEvent(doc(), {
      source: 'orders-api',
      detailType: 'thing-happened',
      detailSchema: 'Metadata',
    })
    const command = putEventsCommand(event, 'prd-global-bus', 'us-east-2')

    expect(command).toContain('aws events put-events')
    expect(command).toContain('--region us-east-2')

    // The --entries payload must parse, with Detail as an escaped string.
    const json = command.slice(command.indexOf('['), command.lastIndexOf(']') + 1)
    const entries = JSON.parse(json)
    expect(entries[0].Source).toBe('orders-api')
    expect(entries[0].EventBusName).toBe('prd-global-bus')
    expect(JSON.parse(entries[0].Detail)).toEqual({ trackingId: 'tracking-123' })
  })
})
