import { GpxRenderer, Tile } from '../../../crates/wasm/pkg/gpx_motion.js'
import { tileKey } from '../tiles.utils'
import { withFree } from '../../wasm/wasm.utils'
import type { FlatChange, FlatSetup, TilePlace } from './flat-map.types'

/** A `Tile`'s id and placement as plain data, to send to a worker. */
export function tilePlace(tile: Tile): TilePlace {
  const { zoom, x, y, dx, dy, size } = tile
  return { zoom, x, y, dx, dy, size }
}

/** Creates the renderer drawing the 2D map of `setup`, with no tiles yet (needs the wasm core started). */
export function createFlatRenderer({ gpx, size }: FlatSetup): GpxRenderer {
  return new GpxRenderer(gpx, size.width, size.height, size.ratio)
}

/**
 * Applies `change` to the renderer drawing the 2D map. A tile that fails to
 * decode is skipped with a warning, leaving the style background there.
 */
export function applyChange(renderer: GpxRenderer, change: FlatChange): void {
  switch (change.type) {
    case 'tile': {
      const { zoom, x, y, dx, dy, size } = change.tile
      withFree(new Tile(zoom, x, y, dx, dy, size), (tile) => {
        try {
          renderer.addVectorTile(tile, change.bytes)
        } catch (err) {
          console.warn(`tile decode failed: ${tileKey(zoom, x, y)}`, err)
        }
      })
      return
    }
    case 'labels':
      if (change.font) renderer.setLabels(change.font)
      else renderer.clearLabels()
      return
    case 'keepout':
      renderer.setLabelKeepout(change.rects)
  }
}
