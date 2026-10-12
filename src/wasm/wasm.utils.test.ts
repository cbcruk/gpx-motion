import { describe, expect, it } from 'vite-plus/test'
import { withFree } from './wasm.utils'

describe('withFree', () => {
  const freeable = (): { free: () => void; freed: number } => {
    const owned = {
      freed: 0,
      free(): void {
        owned.freed += 1
      },
    }
    return owned
  }

  it('returns what the callback returns, then frees once', () => {
    const owned = freeable()
    expect(withFree(owned, (o) => (o.freed === 0 ? 'in use' : 'freed early'))).toBe('in use')
    expect(owned.freed).toBe(1)
  })

  it('frees when the callback throws', () => {
    const owned = freeable()
    expect(() =>
      withFree(owned, () => {
        throw new Error('boom')
      }),
    ).toThrow('boom')
    expect(owned.freed).toBe(1)
  })

  it('passes null and undefined through without freeing', () => {
    expect(withFree(undefined, (o) => o)).toBeUndefined()
    expect(withFree(null, (o) => o)).toBeNull()
  })
})
