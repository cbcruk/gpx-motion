import { createGroundLoader } from '../../map/gl/ground-loader/ground-loader'
import { loadVectorTiles } from '../../map/tiles/tiles'
import type { SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { TileSink, TileSource } from '../../map/tiles/tiles.types'
import type { FollowLabels, GlMapOptions } from './gl-map.types'

/**
 * Loads the WebGL scene's map from a caching `source` (see `cachedSource`):
 * ground tiles as the camera needs them, and label tiles around the route.
 *
 * @example Load the 3D map
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { loadGlMap } from '@/app/gl-map/gl-map'
 * import type { GlScene } from '@/map/gl/gl-scene/gl-scene.types'
 * import type { TileSource } from '@/map/tiles/tiles.types'
 *
 * declare const source: TileSource
 * declare const scene: SceneRenderer
 * declare const view: GlScene
 *
 * await loadGlMap(source, { scene, view, style: 'dark' })
 * ```
 */
export async function loadGlMap(source: TileSource, options: GlMapOptions): Promise<void> {
  const { scene, view, style } = options
  view.setGround(createGroundLoader(source, style))
  const labels: TileSink = { addVectorTile: (tile, bytes) => scene.addLabelTile(tile, bytes) }
  await loadVectorTiles(labels, source, scene.labelTiles(source.maxzoom))
}

/**
 * Creates the loader of the scene's finer label tiles along the route,
 * which fetches them once from the source `source()` gives, once there is one.
 *
 * @example Load them on first follow
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { createFollowLabels } from '@/app/gl-map/gl-map'
 * import type { TileSource } from '@/map/tiles/tiles.types'
 *
 * declare const scene: SceneRenderer
 * declare const mapSource: TileSource | null
 * declare const follow: boolean
 *
 * const followLabels = createFollowLabels(scene, () => mapSource)
 * if (follow) followLabels.load()
 * ```
 */
export function createFollowLabels(
  scene: SceneRenderer,
  source: () => TileSource | null,
): FollowLabels {
  let loaded = false
  return {
    load(): void {
      const from = source()
      if (loaded || !from) return
      loaded = true
      const along: TileSink = {
        addVectorTile: (tile, bytes) => scene.addFollowLabelTile(tile, bytes),
      }
      loadVectorTiles(along, from, scene.followLabelTiles(from.maxzoom)).catch((err: unknown) =>
        console.warn('labels along the route failed', err),
      )
    },
  }
}
