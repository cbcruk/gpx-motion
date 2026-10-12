import * as config from '../../config/config'
import { openPmTiles } from '../../map/pmtiles/pmtiles'
import { cachedSource, fetchTileJson, tileJsonSource } from '../../map/tiles/tiles'
import type { TileSource } from '../../map/tiles/tiles.types'

/**
 * Opens the basemap's vector tiles, cached so the tiles both views share
 * are fetched once: the PMTiles archive at `pmtilesUrl`, or OpenFreeMap's
 * TileJSON without one.
 *
 * @param signal Aborts opening the source and every tile request it makes later.
 *
 * @example Open the tiles
 * ```ts
 * import { openMapSource } from '@/app/map-source/map-source'
 *
 * const source = await openMapSource(null)
 * const bytes = await source.fetchTile(14, 13970, 6344)
 * ```
 */
export async function openMapSource(
  pmtilesUrl: string | null,
  signal?: AbortSignal,
): Promise<TileSource> {
  const source = pmtilesUrl
    ? await openPmTiles(pmtilesUrl, signal)
    : tileJsonSource(await fetchTileJson(config.TILEJSON_URL, signal), signal)
  return cachedSource(source)
}
