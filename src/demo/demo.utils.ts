import { Effect } from 'effect'
import type { Scope } from 'effect'

/**
 * Makes an `AbortSignal` that aborts when the surrounding scope closes, for
 * the listeners and requests that should stop with it.
 *
 * @example Listen until the demo stops
 * ```ts
 * import { Effect } from 'effect'
 * import { scopeSignal } from '@/demo/demo.utils'
 *
 * declare function resize(): void
 *
 * const listen = Effect.gen(function* () {
 *   const signal = yield* scopeSignal
 *   window.addEventListener('resize', resize, { signal })
 * })
 * ```
 */
export const scopeSignal: Effect.Effect<AbortSignal, never, Scope.Scope> = Effect.acquireRelease(
  Effect.sync(() => new AbortController()),
  (controller) => Effect.sync(() => controller.abort()),
).pipe(Effect.map((controller) => controller.signal))

/**
 * Recovers from a failure by logging `message` with its cause and going on
 * with `fallback`.
 *
 * @example Draw without a map when it fails
 * ```ts
 * import { Effect } from 'effect'
 * import { MapFailed } from '@/demo/demo.errors'
 * import { fallBack } from '@/demo/demo.utils'
 *
 * declare const loadMap: Effect.Effect<void, MapFailed>
 *
 * const shown = loadMap.pipe(
 *   Effect.catchTag('MapFailed', fallBack('map tiles failed; drawing without a map', undefined)),
 * )
 * ```
 */
export function fallBack<A>(
  message: string,
  fallback: A,
): (failure: { readonly cause: unknown }) => Effect.Effect<A> {
  return (failure) =>
    Effect.sync(() => {
      console.warn(message, failure.cause)
      return fallback
    })
}
