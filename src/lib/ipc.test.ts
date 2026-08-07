import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { asIpcError, needsLogin, withTimeout } from './ipc'

describe('withTimeout', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('resolves normally when the call returns in time', async () => {
    const promise = withTimeout(Promise.resolve('done'), 60_000, 'Inferring')
    await expect(promise).resolves.toBe('done')
  })

  it('rejects once the deadline passes', async () => {
    // The spinner outlived a 60s timeout by minutes in practice; this pins the
    // primitive so a future hang cannot be blamed on it without evidence.
    const never = new Promise<string>(() => {})
    const promise = withTimeout(never, 60_000, 'Inferring the schema')
    const assertion = expect(promise).rejects.toMatchObject({
      kind: 'internal',
      message: expect.stringContaining('did not respond within 60s'),
    })
    await vi.advanceTimersByTimeAsync(60_001)
    await assertion
  })

  it('does not reject a call that beat the deadline', async () => {
    let settle: (v: string) => void = () => {}
    const slow = new Promise<string>((resolve) => {
      settle = resolve
    })
    const promise = withTimeout(slow, 60_000, 'Inferring')
    settle('in time')
    await vi.advanceTimersByTimeAsync(120_000)
    await expect(promise).resolves.toBe('in time')
  })

  it('propagates the original rejection rather than the timeout', async () => {
    const failed = Promise.reject({ kind: 'auth', message: 'expired' })
    const promise = withTimeout(failed, 60_000, 'Inferring')
    await expect(promise).rejects.toMatchObject({ kind: 'auth' })
  })
})

describe('asIpcError', () => {
  it('passes through a typed rejection', () => {
    expect(asIpcError({ kind: 'auth', message: 'nope' })).toEqual({
      kind: 'auth',
      message: 'nope',
    })
  })

  it('wraps anything else as internal', () => {
    expect(asIpcError(new Error('boom')).kind).toBe('internal')
    expect(asIpcError('boom')).toEqual({ kind: 'internal', message: 'boom' })
  })
})

describe('needsLogin', () => {
  it('is true only for auth failures', () => {
    expect(needsLogin({ kind: 'auth', message: '' })).toBe(true)
    expect(needsLogin({ kind: 'aws', message: '' })).toBe(false)
  })
})
