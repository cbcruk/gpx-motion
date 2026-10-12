import type { Tile } from '../../crates/wasm/pkg/gpx_motion.js'
import type { TileJson, TileSink, TileSource } from './tiles.types'

/** Zoom assumed for a TileJSON source that doesn't state its `maxzoom`. */
const DEFAULT_MAX_ZOOM = 14

/** Fetches a TileJSON document describing a vector tile source. */
export async function fetchTileJson(url: string): Promise<TileJson> {
  return (await fetch(url).then((r) => r.json())) as TileJson
}

/**
 * Wraps a TileJSON document's first XYZ template as a {@linkcode TileSource}.
 *
 * @example Fetch tiles described by TileJSON
 * ```ts
 * import { fetchTileJson, tileJsonSource } from "./tiles";
 *
 * const source = tileJsonSource(await fetchTileJson("https://tiles.openfreemap.org/planet"));
 * const bytes = await source.fetchTile(14, 13970, 6344);
 * ```
 */
export function tileJsonSource(tileJson: TileJson): TileSource {
  const template = tileJson.tiles[0]
  return {
    maxzoom: tileJson.maxzoom ?? DEFAULT_MAX_ZOOM,
    async fetchTile(zoom, x, y) {
      const url = template
        .replace(/\{(zoom|z)\}/, String(zoom))
        .replace('{x}', String(x))
        .replace('{y}', String(y))
      const response = await fetch(url)
      if (response.status === 404 || response.status === 204) return null
      if (!response.ok) throw new Error(`HTTP ${response.status}: ${url}`)
      return new Uint8Array(await response.arrayBuffer())
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
      const key = `${zoom}/${x}/${y}`
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
 * Fetches every tile in `tiles` from `source` and hands the bytes to `sink`,
 * freeing each `Tile` handle afterwards.
 *
 * Tiles that fail to load or decode are skipped with a console warning, so the
 * map falls back to its style background there.
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
        console.warn(`tile failed: ${tile.zoom}/${tile.x}/${tile.y}`, err)
        return { tile, bytes: null }
      }
    }),
  )

  for (const { tile, bytes } of loaded) {
    if (bytes) {
      try {
        sink.addVectorTile(tile, bytes)
      } catch (err) {
        console.warn(`tile decode failed: ${tile.zoom}/${tile.x}/${tile.y}`, err)
      }
    }
    tile.free()
  }
}
