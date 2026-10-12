import { GroundTiles } from '../../../../crates/wasm/pkg/gpx_motion.js'
import { createWorkerClient } from '../../../worker/worker-client'
import type { TileSource } from '../../tiles/tiles.types'
import { tileKey } from '../../tiles/tiles.utils'
import type { GroundApi, GroundLoader, GroundTileData } from '../ground/ground.types'

/**
 * Draws ground tile textures, and lists the buildings the style raises on
 * them, in a Web Worker from `source`'s vector tiles (overzoomed past
 * `source.maxzoom`). If the worker fails, this is done on the main thread
 * instead. A map tile that fails to load fails the
 * texture, so it can be asked for again; pass a caching source (see
 * `cachedSource`) so tiles shared by several textures are fetched once.
 * `setStyle` restyles the textures in the same worker.
 *
 * @example Load a ground tile's texture
 * ```ts
 * import { createGroundLoader } from '@/map/gl/ground-loader/ground-loader'
 * import { cachedSource } from '@/map/tiles/tiles'
 * import type { TileSource } from '@/map/tiles/tiles.types'
 *
 * declare const source: TileSource
 *
 * const loader = createGroundLoader(cachedSource(source), 'dark')
 * const { texture, buildings } = await loader.load(15, 27940, 12689)
 * ```
 */
export function createGroundLoader(source: TileSource, style: string): GroundLoader {
  /** Map tiles handed to the worker, or to `local` once it has taken over, in the current style. */
  const sent = new Set<string>()
  let local: GroundTiles | null = null
  let disposed = false

  const worker = createWorkerClient<GroundApi>({
    name: 'ground worker',
    create: () =>
      new Worker(new URL('../ground-worker/ground-worker.ts', import.meta.url), { type: 'module' }),
    onFailure: (reason) => {
      console.warn('ground worker failed; drawing map tiles on the main thread', reason)
      local = new GroundTiles(style)
      sent.clear()
    },
  })
  worker.post('init', style)

  const inWorker = (
    tile: [number, number, number],
    data: [number, number, number],
    bytes: Uint8Array | null,
  ): Promise<GroundTileData> => {
    const k = tileKey(...data)
    if (bytes && !sent.has(k)) {
      const copy = bytes.slice()
      worker.post('tile', { zoom: data[0], x: data[1], y: data[2], bytes: copy }, [copy.buffer])
    }
    sent.add(k)
    return worker.call('render', { zoom: tile[0], x: tile[1], y: tile[2], dataZoom: data[0] })
  }

  const onMainThread = (
    ground: GroundTiles,
    tile: [number, number, number],
    data: [number, number, number],
    bytes: Uint8Array | null,
  ): GroundTileData => {
    const k = tileKey(...data)
    if (bytes && !sent.has(k)) ground.addTile(...data, bytes)
    sent.add(k)
    return {
      texture: ground.renderTile(...tile, data[0]),
      buildings: ground.buildings(...tile, data[0]),
    }
  }

  return {
    dispose(): void {
      disposed = true
      worker.dispose()
      local?.free()
      local = null
    },
    textureSize: 512,
    setStyle(next): void {
      if (disposed) return
      style = next
      sent.clear()
      if (local) {
        local.free()
        local = null
        local = new GroundTiles(next)
        return
      }
      worker.post('style', next)
    },
    async load(zoom, x, y): Promise<GroundTileData> {
      const dataZoom = Math.min(zoom, source.maxzoom)
      const shift = zoom - dataZoom
      const data: [number, number, number] = [dataZoom, x >> shift, y >> shift]
      const bytes = await source.fetchTile(...data)
      if (disposed) throw new Error('ground loader disposed')
      if (!local) {
        try {
          return await inWorker([zoom, x, y], data, bytes)
        } catch (err) {
          if (!local) throw err
        }
      }
      return onMainThread(local, [zoom, x, y], data, bytes)
    },
  }
}
