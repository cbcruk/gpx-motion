import type { Transfer } from './worker.utils'

/**
 * The messages a worker takes, as one function per message type: its one
 * argument (`undefined` when it needs none) and what it answers.
 *
 * Use with an object type alias rather than an interface, e.g.
 * `type Api = { render: (zoom: number) => Uint8Array }`.
 */
export type WorkerApi<Api> = { [K in keyof Api]: (arg: never) => unknown }

/** The argument of a worker message handler type. */
export type WorkerArg<F> = F extends (arg: infer A) => unknown ? A : never

/** What a worker message handler type answers. */
export type WorkerResult<F> = F extends (arg: never) => infer R ? R : never

/** The worker side of a {@linkcode WorkerApi}: one handler per message type, run one at a time. */
export type WorkerHandlers<Api extends WorkerApi<Api>> = {
  [K in keyof Api]: (
    arg: WorkerArg<Api[K]>,
  ) =>
    | WorkerResult<Api[K]>
    | Transfer<WorkerResult<Api[K]>>
    | Promise<WorkerResult<Api[K]> | Transfer<WorkerResult<Api[K]>>>
}

/** An error thrown in a worker, as plain data that survives `postMessage`. */
export interface WorkerError {
  /** The error's class name, such as `RangeError`. */
  name: string
  /** The error's message. */
  message: string
  /** The worker-side stack trace, when the error had one. */
  stack?: string
}

/** A message from the main thread to a worker. */
export interface WorkerRequest {
  /** Increasing id the worker's answer or error refers to. */
  id: number
  /** The message type: a key of the worker's API. */
  type: string
  /** The handler's argument. */
  arg: unknown
  /** Whether the main thread waits for the result; errors are reported either way. */
  reply: boolean
}

/** A message from a worker back to the main thread: a result, or why a message failed. */
export type WorkerResponse =
  | { id: number; ok: true; value: unknown }
  | { id: number; ok: false; error: WorkerError }

/** One end of a message channel: a worker's global scope, or a `Worker` seen from the main thread. */
export interface WorkerPort {
  /** Sends `message`, moving `options.transfer` buffers instead of copying them. */
  postMessage(message: unknown, options?: StructuredSerializeOptions): void
  /** Hears each message from the other end, or one that could not be read (`messageerror`). */
  addEventListener(type: 'message' | 'messageerror', listener: (event: MessageEvent) => void): void
}

/** The main thread's handle on a worker, such as a `Worker`: a message port that can also fail and be stopped. */
export interface WorkerLike {
  /** Sends `message` to the worker, moving `options.transfer` buffers instead of copying them. */
  postMessage(message: unknown, options?: StructuredSerializeOptions): void
  /** Hears messages (`MessageEvent`s), and the worker failing to load or run (`error`) or to send (`messageerror`). */
  addEventListener(
    type: 'message' | 'error' | 'messageerror',
    listener: (event: Event) => void,
  ): void
  /** Stops the worker at once. */
  terminate(): void
}

/** How {@linkcode createWorkerClient} starts its worker and reports it failing. */
export interface WorkerClientOptions {
  /** Names the worker in error messages, such as `2D map`. */
  name: string
  /** Starts the worker, e.g. `() => new Worker(new URL("./x-worker.ts", import.meta.url), { type: "module" })`; it may throw. */
  create: () => WorkerLike
  /**
   * Hears the worker fail, once: it could not start, a message failed in it,
   * or it crashed. The worker is stopped by then and every waiting call
   * rejects; this may run before `createWorkerClient` returns.
   */
  onFailure: (reason: Error) => void
}

/** The main thread's side of a worker speaking {@linkcode WorkerApi} `Api`. */
export interface WorkerClient<Api extends WorkerApi<Api>> {
  /** `running` until the worker fails or is disposed. */
  readonly state: 'running' | 'failed' | 'disposed'
  /** Sends a message without waiting for its result; one that fails in the worker fails the worker. Does nothing once not running. */
  post<K extends keyof Api & string>(
    type: K,
    arg: WorkerArg<Api[K]>,
    transfer?: Transferable[],
  ): void
  /** Sends a message and resolves with its result; rejects once the worker fails or is disposed. */
  call<K extends keyof Api & string>(
    type: K,
    arg: WorkerArg<Api[K]>,
    transfer?: Transferable[],
  ): Promise<WorkerResult<Api[K]>>
  /** Stops the worker; waiting calls reject and later ones do nothing (calls reject). */
  dispose(): void
}
