import type { WorkerError } from './worker.types'

/**
 * A worker handler's result together with the buffers to move to the main
 * thread instead of copying; make one with {@linkcode transfer}.
 */
export class Transfer<T> {
  /** The handler's result. */
  readonly value: T
  /** Buffers inside `value`, detached in the worker once sent. */
  readonly buffers: Transferable[]

  /** Wraps `value` with the `buffers` inside it to move. */
  constructor(value: T, buffers: Transferable[]) {
    this.value = value
    this.buffers = buffers
  }
}

/**
 * Answers a worker message with `value`, moving `buffers` instead of copying them.
 *
 * @example Move a rendered image out of the worker
 * ```ts
 * import type { GpxRenderer } from '@wasm'
 * import { transfer } from '@/worker/worker.utils'
 *
 * declare const renderer: GpxRenderer
 *
 * const handlers = {
 *   render: (style: string) => {
 *     const bytes = renderer.renderMap(style)
 *     return transfer(bytes, [bytes.buffer])
 *   },
 * }
 * ```
 */
export function transfer<T>(value: T, buffers: Transferable[]): Transfer<T> {
  return new Transfer(value, buffers)
}

/** Turns anything thrown into a {@linkcode WorkerError}, keeping an `Error`'s name and stack. */
export function toWorkerError(err: unknown): WorkerError {
  if (err instanceof Error) {
    return { name: err.name, message: err.message, ...(err.stack && { stack: err.stack }) }
  }
  return { name: 'Error', message: String(err) }
}

/**
 * Rebuilds an `Error` from a {@linkcode WorkerError}, with the worker's name
 * and stack, its message prefixed by where it came from.
 *
 * @example Report a failed message
 * ```ts
 * import { fromWorkerError } from '@/worker/worker.utils'
 *
 * const err = fromWorkerError({ name: 'RangeError', message: 'bad tile' }, 'ground worker')
 * err.message // 'ground worker: bad tile'
 * ```
 */
export function fromWorkerError(error: WorkerError, origin: string): Error {
  const err = new Error(`${origin}: ${error.message}`)
  err.name = error.name
  if (error.stack) err.stack = error.stack
  return err
}
