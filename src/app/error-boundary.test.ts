import { describe, expect, it } from 'vitest'
import { crashReport, describeError } from './error-boundary'

describe('describeError', () => {
  it('reads message and stack off an Error', () => {
    const error = new Error('Can’t find variable: cn')
    const { message, stack } = describeError(error)
    expect(message).toBe('Can’t find variable: cn')
    expect(stack).toContain('Error')
  })

  it('falls back to the name when the message is empty', () => {
    // `throw new TypeError()` produces an empty message; showing a blank red
    // box would say nothing at all.
    const { message } = describeError(new TypeError())
    expect(message).toBe('TypeError')
  })

  it('handles a thrown string', () => {
    expect(describeError('boom')).toEqual({ message: 'boom', stack: null })
  })

  it('handles a thrown object with a message', () => {
    // Rejected IPC calls are `{kind, message}`, not Errors.
    expect(describeError({ kind: 'auth', message: 'expired' }).message).toBe('expired')
  })

  it('handles null and undefined without throwing', () => {
    // A boundary that crashes while reporting a crash is the worst case.
    expect(describeError(null).message).toBe('null')
    expect(describeError(undefined).message).toBe('undefined')
  })
})

describe('crashReport', () => {
  it('names the scope and the error', () => {
    const report = crashReport('The Schemas screen', new Error('kaboom'))
    expect(report).toContain('Pontifex crashed in The Schemas screen')
    expect(report).toContain('error: kaboom')
  })

  it('includes the component stack when there is one', () => {
    const report = crashReport('The app', new Error('kaboom'), '\n  at ConfirmSaveDialog')
    expect(report).toContain('components:')
    expect(report).toContain('ConfirmSaveDialog')
  })

  it('omits empty sections rather than printing blank headings', () => {
    const report = crashReport('The app', 'plain string')
    expect(report).not.toContain('stack:')
    expect(report).not.toContain('components:')
  })
})
