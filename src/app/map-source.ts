import * as config from '../config/config'
import { openPmTiles } from '../map/pmtiles'
import { cachedSource, fetchTileJson, tileJsonSource } from '../map/tiles'
import type { TileSource } from '../map/tiles.types'

/**
 * Opens the basemap's vector tiles, cached so the tiles both views share
 * are fetched once: the PMTiles archive at `pmtilesUrl`, or OpenFreeMap's
 * TileJSON without one.
 *
 * @example Open the tiles
 * ```ts
 * import { openMapSource } from "./app/map-source";
 *
 * const source = await openMapSource(null);
 * ```
 */
export async function openMapSource(pmtilesUrl: string | null): Promise<TileSource> {
  const source = pmtilesUrl
    ? await openPmTiles(pmtilesUrl)
    : tileJsonSource(await fetchTileJson(config.TILEJSON_URL))
  return cachedSource(source)
}
