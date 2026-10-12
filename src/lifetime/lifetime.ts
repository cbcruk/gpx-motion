import type { Lifetime } from './lifetime.types'

/**
 * Creates a {@linkcode Lifetime}: what is made under it registers how to
 * undo itself where it is made, and one `end()` undoes it all.
 *
 * Ending sets `ended` and aborts `signal` first, so listeners are gone and
 * late async work sees it before any cleanup runs. A cleanup that throws is
 * logged and the rest still run.
 *
 * @example Undo a listener and a resource together
 * ```ts
 * import { createLifetime } from '@/lifetime/lifetime'
 *
 * declare const canvas: HTMLCanvasElement
 * declare function resize(): void
 *
 * const life = createLifetime()
 * window.addEventListener('resize', resize, { signal: life.signal })
 * const bitmap = await createImageBitmap(canvas)
 * life.defer(() => bitmap.close())
 * life.end()
 * ```
 */
export function createLifetime(): Lifetime {
  const controller = new AbortController()
  const cleanups: (() => void)[] = []

  const run = (cleanup: () => void): void => {
    try {
      cleanup()
    } catch (err) {
      console.warn('cleanup failed', err)
    }
  }

  return {
    signal: controller.signal,
    get ended(): boolean {
      return controller.signal.aborted
    },
    defer(cleanup): void {
      if (controller.signal.aborted) run(cleanup)
      else cleanups.push(cleanup)
    },
    end(): void {
      if (controller.signal.aborted) return
      controller.abort()
      for (let cleanup = cleanups.pop(); cleanup; cleanup = cleanups.pop()) run(cleanup)
    },
  }
}
