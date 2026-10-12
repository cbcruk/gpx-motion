import type {
  WorkerApi,
  WorkerHandlers,
  WorkerPort,
  WorkerRequest,
  WorkerResponse,
} from './worker.types'
import { Transfer, toWorkerError } from './worker.utils'

/**
 * Answers the main thread's messages (see {@linkcode createWorkerClient}) with
 * `handlers`, one message at a time in the order sent, so an async `init`
 * finishes before anything after it runs.
 *
 * A handler's result goes back for messages sent with `call` (wrap it with
 * `transfer` to move its buffers); anything a handler throws goes back as a
 * {@linkcode WorkerError} with its name and stack, which fails the worker on
 * the main thread.
 *
 * @param scope Where messages come from: the worker's global scope by default.
 *
 * @example Serve a worker's messages
 * ```ts
 * import { GpxRenderer } from '@wasm'
 * import { initWasm } from '@/wasm/wasm'
 * import { serve } from '@/worker/serve'
 * import { transfer } from '@/worker/worker.utils'
 *
 * type Api = { init: (gpx: string) => void; render: (style: string) => Uint8Array }
 * let renderer: GpxRenderer | null = null
 *
 * serve<Api>({
 *   init: async (gpx) => {
 *     await initWasm()
 *     renderer = new GpxRenderer(gpx, 640, 640, 1)
 *   },
 *   render: (style) => {
 *     if (!renderer) throw new Error('not initialized')
 *     const bytes = renderer.renderMap(style)
 *     return transfer(bytes, [bytes.buffer])
 *   },
 * })
 * ```
 */
export function serve<Api extends WorkerApi<Api>>(
  handlers: WorkerHandlers<Api>,
  scope: WorkerPort = self as unknown as WorkerPort,
): void {
  let queue: Promise<void> = Promise.resolve()
  const respond = (response: WorkerResponse, buffers: Transferable[] = []): void =>
    scope.postMessage(response, { transfer: buffers })

  const handle = async ({ id, type, arg, reply }: WorkerRequest): Promise<void> => {
    const handler = (handlers as Record<string, ((arg: unknown) => unknown) | undefined>)[type]
    if (!handler) throw new Error(`no handler for "${type}" messages`)
    const result = await handler(arg)
    if (!reply) return
    if (result instanceof Transfer) respond({ id, ok: true, value: result.value }, result.buffers)
    else respond({ id, ok: true, value: result })
  }

  // An unreadable request's id is lost: report it under no id, which fails the worker (and every waiting call).
  scope.addEventListener('messageerror', () =>
    respond({
      id: -1,
      ok: false,
      error: { name: 'DataCloneError', message: 'a request could not be read' },
    }),
  )
  scope.addEventListener('message', (event: MessageEvent<WorkerRequest>) => {
    const request = event.data
    queue = queue
      .then(() => handle(request))
      .catch((err: unknown) => respond({ id: request.id, ok: false, error: toWorkerError(err) }))
  })
}
