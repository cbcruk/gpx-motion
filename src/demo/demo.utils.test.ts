import { Effect, Exit, Scope } from 'effect'
import { describe, expect, it, vi } from 'vite-plus/test'
import { MapFailed } from './demo.errors'
import { failAfter, fallBack, inBackground, scopeSignal } from './demo.utils'

describe('scopeSignal', () => {
  it('aborts when its scope closes, not before', async () => {
    const scope = await Effect.runPromise(Scope.make())
    const signal = await Effect.runPromise(Scope.provide(scope)(scopeSignal))
    expect(signal.aborted).toBe(false)
    await Effect.runPromise(Scope.close(scope, Exit.void))
    expect(signal.aborted).toBe(true)
  })
})

describe('fallBack', () => {
  it('logs the cause and goes on with the fallback', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const cause = new Error('offline')
    const shown = await Effect.runPromise(
      Effect.fail(new MapFailed({ cause })).pipe(
        Effect.catchTag('MapFailed', fallBack('map failed', 'no map')),
      ),
    )
    expect(shown).toBe('no map')
    expect(warn).toHaveBeenCalledWith('map failed', cause)
    warn.mockRestore()
  })
})

describe('failAfter', () => {
  it('fails with its error and aborts the request when time runs out', async () => {
    let request: AbortSignal | undefined
    const failure = await Effect.runPromise(
      Effect.tryPromise({
        try: (signal) => {
          request = signal
          return new Promise<never>(() => {})
        },
        catch: (cause) => new MapFailed({ cause }),
      }).pipe(
        failAfter(10, (cause) => new MapFailed({ cause })),
        Effect.flip,
      ),
    )
    expect(failure).toBeInstanceOf(MapFailed)
    expect(String(failure.cause)).toContain('timed out after 10 ms')
    expect(request?.aborted).toBe(true)
  })

  it('keeps the request alive after it succeeds, even when the scope closes', async () => {
    let request: AbortSignal | undefined
    const scope = await Effect.runPromise(Scope.make())
    await Effect.runPromise(
      Scope.provide(scope)(
        Effect.tryPromise({
          try: (signal) => {
            request = signal
            return Promise.resolve('source')
          },
          catch: (cause) => new MapFailed({ cause }),
        }).pipe(
          failAfter(10, (cause) => new MapFailed({ cause })),
          Effect.forkScoped,
        ),
      ).pipe(Effect.andThen(Effect.sleep(20))),
    )
    expect(request).toBeDefined()
    await Effect.runPromise(Scope.close(scope, Exit.void))
    expect(request?.aborted).toBe(false)
  })

  it('lets a quick effect through', async () => {
    const value = await Effect.runPromise(
      Effect.succeed('map').pipe(failAfter(10, (cause) => new MapFailed({ cause }))),
    )
    expect(value).toBe('map')
  })
})

describe('inBackground', () => {
  it('logs what the effect throws', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})
    const bug = new Error('bug')
    const crash = Effect.sync(() => {
      throw bug
    })
    await Effect.runPromise(
      Effect.scoped(inBackground(crash).pipe(Effect.andThen(Effect.yieldNow))),
    )
    expect(error).toHaveBeenCalledWith(bug)
    error.mockRestore()
  })

  it('stays quiet when the scope closes first', async () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => {})
    await Effect.runPromise(Effect.scoped(inBackground(Effect.never)))
    expect(error).not.toHaveBeenCalled()
    error.mockRestore()
  })
})
