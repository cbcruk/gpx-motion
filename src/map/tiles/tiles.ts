import type { Tile } from '../../../crates/wasm/pkg/gpx_motion.js'
import { assertOk, fetchOk, isAbort } from '../../fetch/fetch.utils'
import { withFree } from '../../wasm/wasm.utils'
import type { TileJson, TileSink, TileSource } from './tiles.types'
import { tileKey } from './tiles.utils'

/** Zoom assumed for a TileJSON source that doesn't state its `maxzoom`. */
const DEFAULT_MAX_ZOOM = 14

/** Fetches a TileJSON document describing a vector tile source, failing on an HTTP error. */
export async function fetchTileJson(url: string, signal?: AbortSignal): Promise<TileJson> {
  return (await (await fetchOk(url, signal)).json()) as TileJson
}

/**
 * Wraps a TileJSON document's first XYZ template as a {@linkcode TileSource}.
 *
 * @param signal Aborts every tile request of the source, now and later.
 * @throws {Error} When the document lists no tile URL.
 *
 * @example Fetch tiles described by TileJSON
 * ```ts
 * import { fetchTileJson, tileJsonSource } from '@/map/tiles/tiles'
 *
 * const source = tileJsonSource(await fetchTileJson('https://tiles.openfreemap.org/planet'))
 * const bytes = await source.fetchTile(14, 13970, 6344)
 * ```
 */
export function tileJsonSource(tileJson: TileJson, signal?: AbortSignal): TileSource {
  const template = tileJson.tiles[0]
  if (template === undefined) throw new Error('TileJSON lists no tile URLs')
  return {
    maxzoom: tileJson.maxzoom ?? DEFAULT_MAX_ZOOM,
    async fetchTile(zoom, x, y) {
      const url = template
        .replace(/\{(zoom|z)\}/, String(zoom))
        .replace('{x}', String(x))
        .replace('{y}', String(y))
      const response = await fetch(url, { signal: signal ?? null })
      if (response.status === 404 || response.status === 204) return null
      return new Uint8Array(await assertOk(response, url).arrayBuffer())
    },
  }
}

/**
 * Wraps `source` so each tile is fetched once, however often it is asked
 * for (the bytes are kept for the page's lifetime).
 */
export function cachedSource(source: TileSource): TileSource {
  const fetched = new Map<string, Promise<Uint8Array | null>>()
  return {
    maxzoom: source.maxzoom,
    fetchTile(zoom, x, y) {
      const key = tileKey(zoom, x, y)
      let bytes = fetched.get(key)
      if (!bytes) {
        bytes = source.fetchTile(zoom, x, y)
        fetched.set(key, bytes)
        bytes.catch(() => fetched.delete(key))
      }
      return bytes
    },
  }
}

/**
 * Fetches every tile in `tiles` from `source` and hands the bytes to `sink`.
 *
 * Takes ownership of `tiles`: each `Tile` is freed once `sink` has seen it,
 * so `sink` must not keep it.
 *
 * Tiles that fail to load or decode are skipped with a console warning, so the
 * map falls back to its style background there.
 *
 * @example Draw the tiles a 2D map needs
 * ```ts
 * import type { GpxRenderer } from '@wasm'
 * import { cachedSource, loadVectorTiles } from '@/map/tiles/tiles'
 * import type { TileSource } from '@/map/tiles/tiles.types'
 *
 * declare const renderer: GpxRenderer
 * declare const source: TileSource
 *
 * const tiles = renderer.visibleTiles(source.maxzoom, 'dark')
 * await loadVectorTiles(renderer, cachedSource(source), tiles)
 * const rgba = renderer.renderMap('dark')
 * ```
 */
export async function loadVectorTiles(
  sink: TileSink,
  source: TileSource,
  tiles: Tile[],
): Promise<void> {
  const loaded = await Promise.all(
    tiles.map(async (tile) => {
      try {
        return { tile, bytes: await source.fetchTile(tile.zoom, tile.x, tile.y) }
      } catch (err) {
        if (!isAbort(err)) console.warn(`tile failed: ${tileKey(tile.zoom, tile.x, tile.y)}`, err)
        return { tile, bytes: null }
      }
    }),
  )

  for (const { tile, bytes } of loaded) {
    withFree(tile, () => {
      if (!bytes) return
      try {
        sink.addVectorTile(tile, bytes)
      } catch (err) {
        console.warn(`tile decode failed: ${tileKey(tile.zoom, tile.x, tile.y)}`, err)
      }
    })
  }
}
