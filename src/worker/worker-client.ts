import type {
  WorkerApi,
  WorkerClient,
  WorkerClientOptions,
  WorkerLike,
  WorkerRequest,
  WorkerResponse,
  WorkerResult,
} from './worker.types'
import { fromWorkerError } from './worker.utils'

/**
 * Starts a worker that {@linkcode serve}s `Api` and returns the main
 * thread's side of it: `post` for messages whose result isn't needed,
 * `call` for those whose result is awaited.
 *
 * The worker fails, once, if it can't be created, crashes, can't send a
 * message, or a message throws in it: it is stopped, waiting calls reject,
 * and `options.onFailure` hears why (a worker-side error keeps its name and
 * stack), so the caller can carry on without it.
 *
 * @example Draw in a worker, on the main thread if it fails
 * ```ts
 * import { createWorkerClient } from '@/worker/worker-client'
 *
 * type Api = { init: (style: string) => void; render: (arg: undefined) => Uint8Array }
 *
 * const worker = createWorkerClient<Api>({
 *   name: '2D map worker',
 *   create: () => new Worker(new URL('./flat-worker.ts', import.meta.url), { type: 'module' }),
 *   onFailure: (reason) => console.warn('drawing on the main thread', reason),
 * })
 * worker.post('init', 'dark')
 * const bytes = await worker.call('render', undefined)
 * ```
 */
export function createWorkerClient<Api extends WorkerApi<Api>>(
  options: WorkerClientOptions,
): WorkerClient<Api> {
  const { name, onFailure } = options
  const waiting = new Map<
    number,
    { resolve: (value: never) => void; reject: (err: Error) => void }
  >()
  let nextId = 0
  let state: WorkerClient<Api>['state'] = 'running'
  let worker: WorkerLike | null = null

  const stop = (next: 'failed' | 'disposed', reason: Error): void => {
    state = next
    worker?.terminate()
    worker = null
    const calls = [...waiting.values()]
    waiting.clear()
    for (const { reject } of calls) reject(reason)
  }
  const fail = (reason: Error): void => {
    if (state !== 'running') return
    stop('failed', reason)
    onFailure(reason)
  }
  const send = (request: WorkerRequest, transfer: Transferable[]): void => {
    try {
      worker?.postMessage(request, { transfer })
    } catch (err) {
      fail(asError(err, `${name} could not be sent "${request.type}"`))
    }
  }

  try {
    worker = options.create()
    worker.addEventListener('error', (event) => {
      const { error, message } = event as Partial<ErrorEvent>
      fail(asError(error ?? message ?? event, `${name} crashed`))
    })
    worker.addEventListener('messageerror', () =>
      fail(new Error(`${name} sent an unreadable message`)),
    )
    worker.addEventListener('message', (event) => {
      const response = (event as MessageEvent<WorkerResponse>).data
      const call = waiting.get(response.id)
      waiting.delete(response.id)
      if (response.ok) {
        call?.resolve(response.value as never)
        return
      }
      const err = fromWorkerError(response.error, name)
      call?.reject(err)
      fail(err)
    })
  } catch (err) {
    fail(asError(err, `${name} could not start`))
  }

  return {
    get state(): WorkerClient<Api>['state'] {
      return state
    },
    post(type, arg, transfer = []): void {
      if (state === 'running') send({ id: nextId++, type, arg, reply: false }, transfer)
    },
    call(type, arg, transfer = []): Promise<WorkerResult<Api[typeof type]>> {
      if (state !== 'running') return Promise.reject(new Error(`${name} ${state}`))
      const id = nextId++
      return new Promise((resolve, reject) => {
        waiting.set(id, { resolve, reject })
        send({ id, type, arg, reply: true }, transfer)
      })
    },
    dispose(): void {
      if (state === 'disposed') return
      stop('disposed', new Error(`${name} disposed`))
    },
  }
}

/** `err` itself when it is an `Error`, else an `Error` saying `what`, with `err` as its cause. */
function asError(err: unknown, what: string): Error {
  return err instanceof Error ? err : new Error(what, { cause: err })
}
