import type { GpxRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import { initWasm } from '../../wasm/wasm'
import { serve } from '../../worker/serve'
import { transfer } from '../../worker/worker.utils'
import { applyChange, createFlatRenderer } from './flat-map.utils'
import type { FlatApi, FlatSetup } from './flat-map.types'

let setup: FlatSetup | null = null
let renderer: GpxRenderer | null = null
let style = ''

const current = (): GpxRenderer => {
  if (!renderer) throw new Error('2D map worker used before init')
  return renderer
}

serve<FlatApi>({
  async init(arg) {
    await initWasm()
    setup = { gpx: arg.gpx, size: arg.size }
    renderer = createFlatRenderer(setup)
    style = arg.style
  },
  style(next) {
    if (!setup) throw new Error('2D map worker used before init')
    current().free()
    renderer = null
    renderer = createFlatRenderer(setup)
    style = next
  },
  change(change) {
    applyChange(current(), change)
  },
  render() {
    const bytes = current().renderMap(style)
    return transfer(bytes, [bytes.buffer])
  },
})
