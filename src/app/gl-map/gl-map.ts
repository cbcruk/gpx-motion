import { createGroundLoader } from '../../map/gl/ground-loader/ground-loader'
import { createGlScene } from '../../map/gl/gl-scene/gl-scene'
import { loadTerrainTiles } from '../../map/terrain/terrain'
import { loadVectorTiles } from '../../map/tiles/tiles'
import type { SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { GlScene } from '../../map/gl/gl-scene/gl-scene.types'
import type { TileSink, TileSource } from '../../map/tiles/tiles.types'
import type { FollowLabels, GlMapOptions } from './gl-map.types'

/**
 * Loads the WebGL scene's map from a caching `source` (see `cachedSource`):
 * ground tiles as the camera needs them, label tiles around the route, and
 * the terrain, which the ground then drapes over.
 *
 * A terrain that fails to load is logged and the ground stays flat.
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
 * await loadGlMap(source, { scene, view, style: 'dark', terrainUrl: null, onTerrain: () => true })
 * ```
 */
export async function loadGlMap(source: TileSource, options: GlMapOptions): Promise<void> {
  const { scene, view, style, terrainUrl, onTerrain } = options
  view.setGround(createGroundLoader(source, style))
  const labels: TileSink = { addVectorTile: (tile, bytes) => scene.addLabelTile(tile, bytes) }
  const loadTerrain = async (url: string): Promise<void> => {
    if (!onTerrain((await loadTerrainTiles(scene, url)) > 0)) return
    scene.setTerrainHeights()
    view.setTerrain()
  }
  await Promise.all([
    loadVectorTiles(labels, source, scene.labelTiles(source.maxzoom)),
    terrainUrl !== null &&
      loadTerrain(terrainUrl).catch((err: unknown) =>
        console.warn('terrain failed; drawing flat ground', err),
      ),
  ])
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

/**
 * Creates the WebGL view of `scene` on `canvas`, or returns `null` (the
 * demo shows the 2D map only) where WebGL 2 is unavailable.
 *
 * @example Fall back to 2D
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { tryGlScene } from '@/app/gl-map/gl-map'
 *
 * declare const canvas: HTMLCanvasElement
 * declare const scene: SceneRenderer
 *
 * const glView = tryGlScene(canvas, scene, window.devicePixelRatio)
 * if (!glView) console.info('drawing the 2D map only')
 * ```
 */
export function tryGlScene(
  canvas: HTMLCanvasElement,
  scene: SceneRenderer,
  ratio: number,
): GlScene | null {
  try {
    return createGlScene(canvas, scene, ratio)
  } catch (err) {
    console.warn('WebGL 2 unavailable; showing the 2D map only', err)
    return null
  }
}
