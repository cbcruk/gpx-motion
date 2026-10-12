import { describe, expect, it, vi } from 'vite-plus/test'
import { createLifetime } from './lifetime'

describe('createLifetime', () => {
  it('runs cleanups newest first, once', () => {
    const life = createLifetime()
    const order: number[] = []
    life.defer(() => order.push(1))
    life.defer(() => order.push(2))
    life.end()
    life.end()
    expect(order).toEqual([2, 1])
  })

  it('is ended and aborted before any cleanup runs', () => {
    const life = createLifetime()
    let seen: [boolean, boolean] | null = null
    life.defer(() => (seen = [life.ended, life.signal.aborted]))
    expect(life.ended).toBe(false)
    life.end()
    expect(seen).toEqual([true, true])
  })

  it('runs a cleanup deferred after the end right away', () => {
    const life = createLifetime()
    life.end()
    const cleanup = vi.fn()
    life.defer(cleanup)
    expect(cleanup).toHaveBeenCalledOnce()
  })

  it('keeps going past a cleanup that throws', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const life = createLifetime()
    const cleanup = vi.fn()
    life.defer(cleanup)
    life.defer(() => {
      throw new Error('boom')
    })
    life.end()
    expect(cleanup).toHaveBeenCalledOnce()
    expect(warn).toHaveBeenCalledOnce()
    warn.mockRestore()
  })

  it('removes listeners added with its signal', () => {
    const life = createLifetime()
    const target = new EventTarget()
    const heard = vi.fn()
    target.addEventListener('ping', heard, { signal: life.signal })
    target.dispatchEvent(new Event('ping'))
    life.end()
    target.dispatchEvent(new Event('ping'))
    expect(heard).toHaveBeenCalledOnce()
  })
})
