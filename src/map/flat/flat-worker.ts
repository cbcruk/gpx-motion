import init, { GpxRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import { applyChange } from './flat-map.utils'
import type { FlatReply, FlatRequest, FlatWorkerScope } from './flat-map.types'

let renderer: GpxRenderer | null = null
let style = ''
let queue: Promise<void> = Promise.resolve()
const scope = self as unknown as FlatWorkerScope

function post(reply: FlatReply, transfer: Transferable[] = []): void {
  scope.postMessage(reply, { transfer })
}

async function handle(request: FlatRequest): Promise<void> {
  if (request.type === 'init') {
    await init({
      module_or_path: new URL('../../../crates/wasm/pkg/gpx_motion_bg.wasm', import.meta.url),
    })
    renderer = new GpxRenderer(request.gpx, request.width, request.height, request.ratio)
    style = request.style
    return
  }
  if (!renderer) throw new Error('2D map worker used before init')
  if (request.type === 'render') {
    const bytes = renderer.renderMap(style)
    post({ type: 'map', id: request.id, bytes }, [bytes.buffer])
    return
  }
  applyChange(renderer, request)
}

self.addEventListener('message', (event: MessageEvent<FlatRequest>) => {
  queue = queue
    .then(() => handle(event.data))
    .catch((err: unknown) => post({ type: 'error', message: String(err) }))
})
