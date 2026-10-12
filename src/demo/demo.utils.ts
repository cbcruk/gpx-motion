import { Cause, Effect } from 'effect'
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

/**
 * Runs `effect` in the background until the scope closes, logging anything
 * it throws instead of dropping it.
 *
 * Expected failures should be recovered inside `effect` (see
 * {@linkcode fallBack}); what reaches here is a bug.
 *
 * @example Load a font without waiting for it
 * ```ts
 * import { Effect } from 'effect'
 * import { inBackground } from '@/demo/demo.utils'
 *
 * declare const loadFont: Effect.Effect<void>
 *
 * const started = Effect.scoped(inBackground(loadFont))
 * ```
 */
export function inBackground<A, E, R>(
  effect: Effect.Effect<A, E, R>,
): Effect.Effect<void, never, R | Scope.Scope> {
  return effect.pipe(
    Effect.catchCause((cause) =>
      Effect.sync(() => {
        if (!Cause.hasInterruptsOnly(cause)) console.error(Cause.squash(cause))
      }),
    ),
    Effect.forkScoped,
    Effect.asVoid,
  )
}
