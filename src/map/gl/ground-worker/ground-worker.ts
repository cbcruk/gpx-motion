import { GroundTiles } from '../../../../crates/wasm/pkg/gpx_motion.js'
import { initWasm } from '../../../wasm/wasm'
import { serve } from '../../../worker/serve'
import { transfer } from '../../../worker/worker.utils'
import { tileKey } from '../../tiles/tiles.utils'
import type { GroundApi } from '../ground/ground.types'

let ground: GroundTiles | null = null

const current = (): GroundTiles => {
  if (!ground) throw new Error('ground worker used before init')
  return ground
}

serve<GroundApi>({
  async init(style) {
    await initWasm()
    ground = new GroundTiles(style)
  },
  style(style) {
    current().free()
    ground = null
    ground = new GroundTiles(style)
  },
  tile({ zoom, x, y, bytes }) {
    const tiles = current()
    try {
      tiles.addTile(zoom, x, y, bytes)
    } catch (err) {
      console.warn(`tile decode failed: ${tileKey(zoom, x, y)}`, err)
    }
  },
  render({ zoom, x, y, dataZoom }) {
    const tiles = current()
    const texture = tiles.renderTile(zoom, x, y, dataZoom)
    const buildings = tiles.buildings(zoom, x, y, dataZoom)
    return transfer({ texture, buildings }, [texture.buffer, buildings.buffer])
  },
})
