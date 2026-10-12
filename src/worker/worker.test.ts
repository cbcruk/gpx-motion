import { describe, expect, it, vi } from 'vite-plus/test'
import { serve } from './serve'
import { createWorkerClient } from './worker-client'
import type { WorkerHandlers, WorkerLike, WorkerPort } from './worker.types'
import { fromWorkerError, toWorkerError, transfer } from './worker.utils'

type Api = {
  init: (style: string) => void
  echo: (text: string) => string
  bytes: (length: number) => Uint8Array
  fail: (message: string) => void
}

/** A worker and its global scope joined by an in-memory channel that copies (or moves) like `postMessage`. */
function fakeWorker(): {
  worker: WorkerLike & { terminated: boolean }
  scope: WorkerPort
  crash: (error: Error) => void
} {
  const toWorker: ((event: MessageEvent) => void)[] = []
  const toMain = new Map<string, ((event: Event) => void)[]>()
  const deliver = (listeners: ((event: MessageEvent) => void)[], data: unknown): void => {
    setTimeout(() => {
      if (worker.terminated) return
      for (const listener of listeners) listener({ data } as MessageEvent)
    })
  }
  const worker = {
    terminated: false,
    postMessage(message: unknown, options?: StructuredSerializeOptions): void {
      deliver(toWorker, structuredClone(message, options))
    },
    addEventListener(type: string, listener: (event: Event) => void): void {
      toMain.set(type, [...(toMain.get(type) ?? []), listener])
    },
    terminate(): void {
      worker.terminated = true
    },
  }
  const scope: WorkerPort = {
    postMessage(message, options) {
      deliver(toMain.get('message') ?? [], structuredClone(message, options))
    },
    addEventListener(_type, listener) {
      toWorker.push(listener)
    },
  }
  const crash = (error: Error): void => {
    for (const listener of toMain.get('error') ?? [])
      listener({ error, message: error.message } as unknown as Event)
  }
  return { worker, scope, crash }
}

const handlers = (log: string[] = []): WorkerHandlers<Api> => ({
  async init(style) {
    await new Promise((resolve) => setTimeout(resolve, 5))
    log.push(`init ${style}`)
  },
  echo(text) {
    log.push(`echo ${text}`)
    return text.toUpperCase()
  },
  bytes(length) {
    const bytes = new Uint8Array(length).fill(7)
    log.push(`bytes ${bytes.byteLength}`)
    return transfer(bytes, [bytes.buffer])
  },
  fail(message) {
    throw new RangeError(message)
  },
})

const start = (
  log: string[] = [],
): {
  client: ReturnType<typeof createWorkerClient<Api>>
  worker: WorkerLike & { terminated: boolean }
  crash: (error: Error) => void
  onFailure: ReturnType<typeof vi.fn>
} => {
  const { worker, scope, crash } = fakeWorker()
  serve<Api>(handlers(log), scope)
  const onFailure = vi.fn()
  const client = createWorkerClient<Api>({ name: 'test worker', create: () => worker, onFailure })
  return { client, worker, crash, onFailure }
}

describe('worker protocol', () => {
  it('answers calls in the order sent, after an async init', async () => {
    const log: string[] = []
    const { client } = start(log)
    client.post('init', 'dark')
    const [a, b] = await Promise.all([client.call('echo', 'a'), client.call('echo', 'b')])
    expect([a, b]).toEqual(['A', 'B'])
    expect(log).toEqual(['init dark', 'echo a', 'echo b'])
  })

  it('moves transferred buffers instead of copying them', async () => {
    const { client } = start()
    const bytes = await client.call('bytes', 3)
    expect(bytes).toEqual(new Uint8Array([7, 7, 7]))
    const sent = new Uint8Array([1, 2])
    client.post('echo', 'x', [sent.buffer])
    expect(sent.byteLength).toBe(0)
  })

  it('rejects a failed call with the worker error, then fails the worker once', async () => {
    const { client, worker, onFailure } = start()
    const waiting = client.call('echo', 'later')
    const err = await client.call('fail', 'bad tile').then(
      () => new Error('resolved'),
      (e: unknown) => e as Error,
    )
    expect(err).toBeInstanceOf(Error)
    expect(err.name).toBe('RangeError')
    expect(err.message).toBe('test worker: bad tile')
    expect(err.stack).toContain('bad tile')
    expect(onFailure).toHaveBeenCalledExactlyOnceWith(err)
    expect(worker.terminated).toBe(true)
    expect(client.state).toBe('failed')
    await expect(waiting).resolves.toBe('LATER')
    await expect(client.call('echo', 'x')).rejects.toThrow('test worker failed')
  })

  it('fails the worker when a posted message throws', async () => {
    const { client, onFailure } = start()
    const waiting = client.call('echo', 'first')
    client.post('fail', 'in post')
    await vi.waitFor(() => expect(onFailure).toHaveBeenCalledOnce())
    expect(client.state).toBe('failed')
    await expect(waiting).resolves.toBe('FIRST')
  })

  it('rejects waiting calls when the worker crashes', async () => {
    const { client, crash, onFailure } = start()
    const waiting = client.call('echo', 'x')
    crash(new Error('out of memory'))
    await expect(waiting).rejects.toThrow('out of memory')
    expect(onFailure).toHaveBeenCalledOnce()
  })

  it('fails at once when the worker cannot be created', async () => {
    const onFailure = vi.fn()
    const client = createWorkerClient<Api>({
      name: 'test worker',
      create: () => {
        throw new Error('workers disabled')
      },
      onFailure,
    })
    expect(onFailure).toHaveBeenCalledExactlyOnceWith(new Error('workers disabled'))
    expect(client.state).toBe('failed')
    client.post('init', 'dark')
    await expect(client.call('echo', 'x')).rejects.toThrow('test worker failed')
  })

  it('rejects waiting calls on dispose without reporting a failure', async () => {
    const { client, worker, onFailure } = start()
    const waiting = client.call('echo', 'x')
    client.dispose()
    await expect(waiting).rejects.toThrow('test worker disposed')
    await expect(client.call('echo', 'y')).rejects.toThrow('test worker disposed')
    expect(worker.terminated).toBe(true)
    expect(client.state).toBe('disposed')
    expect(onFailure).not.toHaveBeenCalled()
  })
})

describe('worker errors', () => {
  it('keep the name, message and stack of an Error', () => {
    const err = new TypeError('nope')
    expect(toWorkerError(err)).toEqual({ name: 'TypeError', message: 'nope', stack: err.stack })
  })

  it('turn anything else into its string', () => {
    expect(toWorkerError('plain')).toEqual({ name: 'Error', message: 'plain' })
  })

  it('come back as an Error naming where they came from', () => {
    const err = fromWorkerError({ name: 'RangeError', message: 'bad', stack: 'at w.js:1' }, 'w')
    expect(err).toBeInstanceOf(Error)
    expect([err.name, err.message, err.stack]).toEqual(['RangeError', 'w: bad', 'at w.js:1'])
  })
})
