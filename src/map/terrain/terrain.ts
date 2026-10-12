import type { SceneRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import { fetchOk, isAbort } from '../../fetch/fetch.utils'
import { withFree } from '../../wasm/wasm.utils'
import { tileKey } from '../tiles/tiles.utils'

const DEM_TILE_SIZE = 256

/**
 * Fetches the Terrarium elevation tiles a 3D scene's terrain needs and adds
 * their decoded pixels to it.
 *
 * `template` is an XYZ URL with `{z}`, `{x}`, and `{y}` (e.g. the AWS
 * Terrain Tiles). PNGs are decoded without color management so the encoded
 * heights survive. Tiles that fail to load are skipped with a warning; the
 * ground there stays flat.
 *
 * @param signal Aborts the requests; tiles cut short are skipped without a warning.
 * @returns How many tiles were added; `0` means there is no relief to draw.
 *
 * @example Load the DEM for a scene
 * ```ts
 * import { createSceneRenderer } from '@/map/scene/scene'
 * import type { SceneOptions } from '@/map/scene/scene.types'
 * import { loadTerrainTiles } from '@/map/terrain/terrain'
 *
 * declare const gpx: string
 * declare const options: SceneOptions
 *
 * const { scene, terrain } = createSceneRenderer(gpx, { ...options, terrain: true })
 * if (terrain) {
 *   await loadTerrainTiles(
 *     scene,
 *     'https://elevation-tiles-prod.s3.amazonaws.com/terrarium/{z}/{x}/{y}.png',
 *   )
 * }
 * ```
 */
export async function loadTerrainTiles(
  scene: SceneRenderer,
  template: string,
  signal?: AbortSignal,
): Promise<number> {
  const tiles = scene.terrainTiles()
  const decoded = await Promise.all(
    tiles.map(async (tile) => {
      const url = template
        .replace('{z}', String(tile.zoom))
        .replace('{x}', String(tile.x))
        .replace('{y}', String(tile.y))
      try {
        const response = await fetchOk(url, signal)
        const bitmap = await createImageBitmap(await response.blob(), {
          colorSpaceConversion: 'none',
          premultiplyAlpha: 'none',
        })
        const canvas = new OffscreenCanvas(DEM_TILE_SIZE, DEM_TILE_SIZE)
        const ctx = canvas.getContext('2d')!
        ctx.drawImage(bitmap, 0, 0)
        bitmap.close()
        const rgba = ctx.getImageData(0, 0, DEM_TILE_SIZE, DEM_TILE_SIZE).data
        return { tile, rgba: new Uint8Array(rgba.buffer) }
      } catch (err) {
        if (!isAbort(err)) console.warn(`terrain tile failed: ${url}`, err)
        return { tile, rgba: null }
      }
    }),
  )
  let added = 0
  for (const { tile, rgba } of decoded) {
    withFree(tile, () => {
      if (!rgba) return
      if (scene.addTerrainTile(tile, rgba)) added += 1
      else
        console.warn(
          `terrain tile ${tileKey(tile.zoom, tile.x, tile.y)} is not a 256px Terrarium tile`,
        )
    })
  }
  return added
}
