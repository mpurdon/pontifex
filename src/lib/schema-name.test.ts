import { describe, expect, it } from 'vitest'
import {
  describeInvalidChars,
  invalidNameChars,
  isValidSchemaNameChar,
  sanitizeSchemaName,
} from './schema-name'

describe('invalidNameChars', () => {
  it('accepts the characters EventBridge allows', () => {
    expect(invalidNameChars('milo-medical@packet_v1.2')).toEqual([])
    expect(invalidNameChars('ABC123@xyz')).toEqual([])
  })

  it('reports a space in a real source name', () => {
    // This is the case that made "Create" fail with a BadRequestException.
    expect(invalidNameChars('Atomic Forms@CASE_STATUS_CHANGED')).toEqual([' '])
  })

  it('lists each offending character once, in order', () => {
    expect(invalidNameChars('a b/c d/e')).toEqual([' ', '/'])
  })
})

describe('sanitizeSchemaName', () => {
  it('produces a name EventBridge accepts', () => {
    const name = sanitizeSchemaName('Atomic Forms@CASE_STATUS_CHANGED')
    expect(name).toBe('Atomic-Forms@CASE_STATUS_CHANGED')
    expect(invalidNameChars(name)).toEqual([])
  })

  it('does not leave runs of separators', () => {
    expect(sanitizeSchemaName('Atomic   Forms@x')).toBe('Atomic-Forms@x')
  })

  it('leaves an already-valid name untouched', () => {
    const name = 'milo-medical@packetNotification-assigned'
    expect(sanitizeSchemaName(name)).toBe(name)
  })

  it('agrees with the Rust implementation on the reported case', () => {
    // `model.rs` has the same assertion; the two must not drift.
    expect(sanitizeSchemaName('Atomic Forms@CASE_STATUS_CHANGED')).toBe(
      'Atomic-Forms@CASE_STATUS_CHANGED',
    )
  })
})

describe('isValidSchemaNameChar', () => {
  it('allows exactly the documented set', () => {
    for (const c of ['a', 'Z', '0', '_', '.', '-', '@']) {
      expect(isValidSchemaNameChar(c)).toBe(true)
    }
    for (const c of [' ', '/', '#', '(', ')', ':']) {
      expect(isValidSchemaNameChar(c)).toBe(false)
    }
  })
})

describe('describeInvalidChars', () => {
  it('names a space rather than printing an invisible quote', () => {
    expect(describeInvalidChars('Atomic Forms@x')).toBe('space')
    expect(describeInvalidChars('a/b c')).toBe("'/', space")
  })
})
