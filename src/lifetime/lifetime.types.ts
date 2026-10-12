/** How long something lives, and what to undo when it ends; make one with {@linkcode createLifetime}. */
export interface Lifetime {
  /** Aborts when the lifetime ends; pass it to `addEventListener`, `fetch` and the like to undo them with it. */
  readonly signal: AbortSignal
  /** Whether {@linkcode Lifetime.end} has run, so work finishing later knows to drop its result. */
  readonly ended: boolean
  /** Runs `cleanup` when the lifetime ends, before the cleanups deferred earlier; right away if it already ended. */
  defer(cleanup: () => void): void
  /** Aborts `signal`, then runs the deferred cleanups newest first; calling it again does nothing. */
  end(): void
}
